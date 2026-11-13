//! Account-bound picture reads for the read-only reader.
//!
//! This is the production [`ImageProvider`] the reader is given by its host.
//! It never holds a PAT itself: repository pictures are resolved and read
//! through the captured [`RepositorySession`], so every step re-checks the
//! account/authorization context, and external pictures go through the
//! credential-free [`ExternalImageBytes`] path.
//!
//! A repository picture is resolved honestly: the fixed commit is resolved to
//! its root tree, the document's directory chain is walked with the existing
//! directory capability, and the file entry becomes a [`BlobIdentity`] that is
//! read with the existing blob capability. No default branch, contents-API
//! text result or persisted `download_url` is used as picture bytes.
//!
//! State is bounded and cancellable: at most [`ImageLimits::max_concurrent`]
//! loads run at once, decoded snapshots/directories are kept in a small
//! in-memory cache that dies with the service, and every request carries the
//! document generation it belongs to so a stale completion can be dropped.

#[path = "reader_image_flights.rs"]
mod flights;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::content::images::{
    DecodedImage, ImageCompletion, ImageFailure, ImageLimits, ImageMetrics, ImageProvider,
    ImageRefusal, ImageRenderOptions, ImageRequest, ImageScope, ImageSink, ImageStage, ImageTarget,
    ImageUpdate, ImageValidity, RenderUpdate, decode_for_renderer,
    decode_image_with_options_checked,
};
use crate::data::RepositorySession;
use crate::data::reader_image_cache::{ExternalFreshness, ReaderImageCache, now_ms};
use crate::domain::blob::{BlobBody, BlobIdentity, BlobLimits};
use crate::domain::error::GitHubError;
use crate::domain::repository::{
    DirectoryTree, ObjectSha, RepoSnapshot, RepositoryError, RepositoryIdentity, RepositoryPath,
    RevisionSpec, SnapshotDirectory, TreeEntryKind,
};
use crate::github::RepositoryLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::network::executor::BackgroundExecutor;
use crate::network::reader_images::{
    ExternalImageBytes, ExternalImageError, ExternalImageLimits, ExternalImageResponse,
    ExternalRefusal, ValidationHeaders,
};
use crate::storage::cache::{CacheScope, CacheStore};

/// Cache bounds for one service instance. They are per-instance and are never
/// shared between readers or accounts.
const MAX_CACHED_SNAPSHOTS: usize = 4;
const MAX_CACHED_DIRECTORIES: usize = 8;
const MAX_CACHED_DIRECTORY_ENTRIES: usize = 4096;

/// A host-supplied image service backed by one repository session.
///
/// Construct one per reader document generation, hand it to
/// `ReaderView::set_image_provider`, and drop it when the document (or the
/// account context) is replaced. Cloning shares the same service instance.
#[derive(Clone)]
pub struct RepositoryImageService {
    inner: Arc<Inner>,
}

struct Inner {
    metrics: ImageMetrics,
    session: RepositorySession,
    repository: RepositoryIdentity,
    executor: Arc<BackgroundExecutor>,
    external: Arc<dyn ExternalImageBytes>,
    limits: ImageLimits,
    external_limits: ExternalImageLimits,
    repository_limits: RepositoryLimits,
    blob_limits: BlobLimits,
    permits: Arc<tokio::sync::Semaphore>,
    local_permits: Arc<tokio::sync::Semaphore>,
    decode_permits: Arc<tokio::sync::Semaphore>,
    load_permits: Arc<tokio::sync::Semaphore>,
    snapshots: flights::Flights<RepoSnapshot>,
    directories: flights::Flights<DirectoryTree>,
    bytes: flights::Flights<Vec<u8>>,
    blobs: flights::Flights<Vec<u8>>,
    persistent_cache: Option<ReaderImageCache>,
    /// Scope -> (token, source). One source per reader document generation, so
    /// cancelling one document stops only its own loads.
    work: Mutex<HashMap<ImageScope, (CancellationToken, CancellationTokenSource)>>,
    cache: Mutex<ResolutionCache>,
    /// Test-only gate that pauses after decoding and reports that worker's
    /// thread, so a test can change the account at exactly that boundary.
    #[cfg(test)]
    publish_probe: Mutex<Option<PublishProbe>>,
    #[cfg(test)]
    decode_queue_probe: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

/// Per-service test instrumentation for the decode -> publish boundary.
#[cfg(test)]
pub(super) struct PublishProbe {
    entered: tokio::sync::oneshot::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
impl PublishProbe {
    fn finish(self) {
        let _ = self.entered.send(());
        self.resume
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("release the publish probe");
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let work = self.work.get_mut().expect("image work lock");
        for (_, (_, source)) in work.drain() {
            source.cancel();
        }
    }
}

#[derive(Default)]
struct ResolutionCache {
    snapshots: VecDeque<(ObjectSha, Arc<RepoSnapshot>)>,
    directories: VecDeque<(String, Arc<DirectoryTree>)>,
    directory_entries: usize,
}

impl ResolutionCache {
    fn snapshot(&self, commit: &ObjectSha) -> Option<Arc<RepoSnapshot>> {
        self.snapshots
            .iter()
            .find(|(cached, _)| cached == commit)
            .map(|(_, snapshot)| snapshot.clone())
    }

    fn put_snapshot(&mut self, commit: ObjectSha, snapshot: Arc<RepoSnapshot>) {
        self.snapshots.push_front((commit, snapshot));
        self.snapshots.truncate(MAX_CACHED_SNAPSHOTS);
    }

    fn directory(&self, key: &str) -> Option<Arc<DirectoryTree>> {
        self.directories
            .iter()
            .find(|(cached, _)| cached == key)
            .map(|(_, tree)| tree.clone())
    }

    fn put_directory(&mut self, key: String, tree: Arc<DirectoryTree>) {
        let entries = tree.entries.len();
        if entries > MAX_CACHED_DIRECTORY_ENTRIES {
            // A huge directory is not worth retaining; resolution still works.
            return;
        }
        self.directories.push_front((key, tree));
        self.directory_entries += entries;
        while self.directory_entries > MAX_CACHED_DIRECTORY_ENTRIES
            || self.directories.len() > MAX_CACHED_DIRECTORIES
        {
            let Some((_, evicted)) = self.directories.pop_back() else {
                break;
            };
            self.directory_entries = self.directory_entries.saturating_sub(evicted.entries.len());
        }
    }
}

impl RepositoryImageService {
    /// Builds the service over an already-captured repository session.
    pub fn new(
        session: RepositorySession,
        repository: RepositoryIdentity,
        executor: Arc<BackgroundExecutor>,
        external: Arc<dyn ExternalImageBytes>,
        limits: ImageLimits,
    ) -> Self {
        Self::with_cache(session, repository, executor, external, limits, None)
    }

    /// Builds the service with an optional persistent cache store and scope.
    ///
    /// The injected cache scope must match the session's authenticated account,
    /// and the caller explicitly passes the repository visibility.
    pub fn with_cache(
        session: RepositorySession,
        repository: RepositoryIdentity,
        executor: Arc<BackgroundExecutor>,
        external: Arc<dyn ExternalImageBytes>,
        limits: ImageLimits,
        cache: Option<(CacheStore, CacheScope, bool)>,
    ) -> Self {
        let max_concurrent = limits.max_concurrent.max(1);
        let blob_limits = BlobLimits {
            decoded_bytes: limits.max_response_bytes,
            response_bytes: limits.max_response_bytes.saturating_mul(2),
        };
        let external_limits = ExternalImageLimits {
            max_bytes: limits.max_response_bytes,
            timeout: limits.total_timeout,
            max_redirects: limits.max_redirects,
        };
        let persistent_cache = cache.and_then(|(store, scope, is_private)| {
            // Verify that scope account matches session context account
            if scope.account() != &session.context().auth().account {
                return None;
            }
            Some(ReaderImageCache::new(
                store,
                scope,
                Some(repository.id),
                is_private,
            ))
        });
        Self {
            inner: Arc::new(Inner {
                metrics: ImageMetrics::default(),
                session,
                repository,
                executor,
                external,
                local_permits: Arc::new(tokio::sync::Semaphore::new(2)),
                decode_permits: Arc::new(tokio::sync::Semaphore::new(2)),
                load_permits: Arc::new(tokio::sync::Semaphore::new(limits.max_in_flight.max(1))),
                snapshots: flights::Flights::default(),
                directories: flights::Flights::default(),
                bytes: flights::Flights::default(),
                blobs: flights::Flights::default(),
                limits,
                external_limits,
                repository_limits: RepositoryLimits::default(),
                blob_limits,
                permits: Arc::new(tokio::sync::Semaphore::new(max_concurrent)),
                persistent_cache,
                work: Mutex::new(HashMap::new()),
                cache: Mutex::new(ResolutionCache::default()),
                #[cfg(test)]
                publish_probe: Mutex::new(None),
                #[cfg(test)]
                decode_queue_probe: Mutex::new(None),
            }),
        }
    }

    /// Aggregate timing/counters contain no resource identifiers.
    pub fn metrics(&self) -> ImageMetrics {
        self.inner.metrics.clone()
    }

    /// The repository identity this service reads.
    pub fn repository(&self) -> &RepositoryIdentity {
        &self.inner.repository
    }

    /// The session this service is bound to; `verify` reports whether it is
    /// still the active authorization.
    pub fn verify(&self) -> bool {
        self.inner.session.verify()
    }

    /// Pauses the next load after decoding and before publishing, so a test can
    /// change the account at exactly that boundary.
    #[cfg(test)]
    pub fn pause_before_publish(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        *self.inner.publish_probe.lock().expect("publish probe lock") =
            Some(PublishProbe { entered, resume });
        (ready, release)
    }

    #[allow(clippy::too_many_arguments)] // One bounded job's publication/cancellation context.
    async fn load(
        &self,
        request: &ImageRequest,
        cancel: CancellationToken,
        render_options: ImageRenderOptions,
        deadline: tokio::time::Instant,
        sink: ImageSink,
        phase: Arc<std::sync::atomic::AtomicU32>,
        validity: ImageValidity,
    ) -> Result<Option<DecodedImage>, ImageFailure> {
        // One deadline for the whole load, including decode. A repository
        // picture is several requests (commit, each directory, blob) whose own
        // transport timeouts are per-request; without this, a slow but
        // successful sequence could hold a concurrency permit far past the
        // advertised budget and a host-configured shorter timeout would have no
        // effect on this path at all.
        let mut stale_bytes = None;
        if sink.supports_updates()
            && let ImageTarget::External { url } = &request.target
            && let Some(snapshot) = self.external_cached(url).await?
            && let Some(remaining_ms) =
                ExternalFreshness::from_snapshot(&snapshot).stale_window_remaining_ms(now_ms())
        {
            let mut stale_validity = validity.clone();
            stale_validity.expires =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(remaining_ms));
            let original = Arc::new(snapshot.bytes);
            let decoded = self
                .decode_bytes(
                    request,
                    cancel.clone(),
                    render_options,
                    deadline,
                    sink.clone(),
                    phase.clone(),
                    stale_validity.clone(),
                    original.clone(),
                )
                .await;
            let may_revalidate = validity.is_current()
                && !cancel.is_cancelled()
                && !sink.is_cancelled()
                && tokio::time::Instant::now() < deadline;
            match decoded {
                Ok(decoded) => {
                    if let Some(decoded) = decoded {
                        sink.publish_next(
                            request.scope,
                            request.key.clone(),
                            &phase,
                            false,
                            Some(stale_validity),
                            Ok(ImageUpdate::Ready(decoded)),
                        );
                    }
                    stale_bytes = Some(original);
                }
                // Optional cache pixels may expire or be unusable without
                // preventing revalidation while the original controls hold.
                Err(ImageFailure::StaleContext)
                    if may_revalidate && !stale_validity.is_current() => {}
                Err(
                    ImageFailure::Malformed
                    | ImageFailure::Unsupported
                    | ImageFailure::TooLarge { .. }
                    | ImageFailure::TooManyPixels { .. },
                ) if may_revalidate => {}
                Err(error) => return Err(error),
            }
        }
        let service = self.clone();
        let target = request.target.clone();
        let bytes = tokio::select! {
            biased;
            _ = cancel.clone().cancelled() => return Err(ImageFailure::Cancelled),
            _ = tokio::time::sleep_until(deadline) => return Err(ImageFailure::Timeout),
            bytes = self.inner.bytes.read(request.target.key().as_str().to_string(), &self.inner.executor, cancel.clone(), self.inner.limits.total_timeout, move |shared_cancel| async move {
                match &target {
                    ImageTarget::Repository { owner, name, commit, path } => {
                        service.repository_bytes(owner, name, commit, path, shared_cancel).await
                    }
                    ImageTarget::External { url } => {
                        service.external_bytes(url, shared_cancel).await
                    }
                }
            }) => bytes?,
        };
        if cancel.is_cancelled() {
            return Err(ImageFailure::Cancelled);
        }
        if !self.inner.session.verify() {
            return Err(ImageFailure::StaleContext);
        }
        if stale_bytes
            .as_ref()
            .is_some_and(|original| **original == *bytes)
        {
            sink.publish_next(
                request.scope,
                request.key.clone(),
                &phase,
                false,
                Some(validity),
                Ok(ImageUpdate::Validated),
            );
            return Ok(None);
        }
        if stale_bytes.is_some()
            && let ImageTarget::External { url } = &request.target
            && self.external_cached(url).await?.is_none()
        {
            sink.publish_next(
                request.scope,
                request.key.clone(),
                &phase,
                false,
                None,
                Ok(ImageUpdate::Discard),
            );
        }
        self.decode_bytes(
            request,
            cancel,
            render_options,
            deadline,
            sink,
            phase,
            validity,
            bytes,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)] // One bounded job's publication/cancellation context.
    async fn decode_bytes(
        &self,
        request: &ImageRequest,
        cancel: CancellationToken,
        render_options: ImageRenderOptions,
        deadline: tokio::time::Instant,
        sink: ImageSink,
        phase: Arc<std::sync::atomic::AtomicU32>,
        validity: ImageValidity,
        bytes: Arc<Vec<u8>>,
    ) -> Result<Option<DecodedImage>, ImageFailure> {
        #[cfg(test)]
        if let Some(entered) = self
            .inner
            .decode_queue_probe
            .lock()
            .expect("decode queue probe lock")
            .take()
        {
            let _ = entered.send(());
        }
        // Move the permit into the blocking worker: timing out its await must
        // not release CPU admission while an uninterruptible raster decode runs.
        let permit = tokio::select! {
            biased;
            _ = cancel.clone().cancelled() => return Err(ImageFailure::Cancelled),
            _ = tokio::time::sleep_until(deadline) => return Err(ImageFailure::Timeout),
            permit = self.inner.decode_permits.clone().acquire_owned() => permit.map_err(|_| ImageFailure::Cancelled)?,
        };
        let limits = self.inner.limits.clone();
        let decode_cancel = cancel.clone();
        let decode_deadline = deadline.into_std();
        let decode_validity = validity.clone();
        let scope = request.scope;
        let key = request.key.clone();
        let metrics = self.inner.metrics.clone();
        let started = deadline.into_std() - self.inner.limits.total_timeout;
        let decode = tokio::task::spawn_blocking(move || {
            let _decode_timer = metrics.begin(ImageStage::Decode);
            let sink = sink.with_admission(Arc::new(permit));
            let bytes = Arc::try_unwrap(bytes).unwrap_or_else(|bytes| (*bytes).clone());
            let check = || {
                if !decode_validity.is_current() {
                    return Err(ImageFailure::StaleContext);
                }
                if decode_cancel.is_cancelled() || sink.is_cancelled() {
                    Err(ImageFailure::Cancelled)
                } else if std::time::Instant::now() >= decode_deadline {
                    Err(ImageFailure::Timeout)
                } else {
                    Ok(())
                }
            };
            if sink.supports_updates() {
                let mut animation_started = None;
                let decoded =
                    decode_for_renderer(bytes, &limits, render_options, &check, &mut |update| {
                        check()?;
                        let update = match update {
                            RenderUpdate::First(decoded) => {
                                metrics.record(ImageStage::FirstReady, started);
                                animation_started = Some(std::time::Instant::now());
                                ImageUpdate::Ready(decoded)
                            }
                            RenderUpdate::Frames(frames) => ImageUpdate::Frames(frames),
                        };
                        sink.publish_next(
                            scope,
                            key.clone(),
                            &phase,
                            false,
                            Some(decode_validity.clone()),
                            Ok(update),
                        );
                        Ok(())
                    });
                if let Some(started) = animation_started {
                    metrics.record(ImageStage::Animation, started);
                }
                if let Some(decoded) = decoded? {
                    check()?;
                    metrics.record(ImageStage::FirstReady, started);
                    sink.publish_next(
                        scope,
                        key,
                        &phase,
                        false,
                        Some(decode_validity),
                        Ok(ImageUpdate::Ready(decoded)),
                    );
                }
                Ok(None)
            } else {
                decode_image_with_options_checked(bytes, &limits, render_options, &check).map(Some)
            }
        });
        let decoded = tokio::select! {
            biased;
            _ = cancel.clone().cancelled() => return Err(ImageFailure::Cancelled),
            _ = tokio::time::sleep_until(deadline) => return Err(ImageFailure::Timeout),
            decoded = decode => decoded.map_err(|_| ImageFailure::Network)?,
        };
        #[cfg(test)]
        {
            let probe = self
                .inner
                .publish_probe
                .lock()
                .expect("publish probe lock")
                .take();
            if let Some(probe) = probe {
                let _ = tokio::task::spawn_blocking(move || probe.finish()).await;
            }
        }
        // Decoding is another await boundary: the account may have been
        // switched, removed or reauthorized while the blocking decode ran.
        // Retired authorization must never publish repository bytes, so this is
        // re-checked after the await — staleness outranks the decoded picture,
        // cancellation and worker errors, exactly as `RepositorySession::content`
        // does after its own preparation step.
        if !validity.is_current() {
            return Err(ImageFailure::StaleContext);
        }
        if cancel.is_cancelled() {
            return Err(ImageFailure::Cancelled);
        }
        decoded
    }

    async fn repository_bytes(
        &self,
        owner: &str,
        name: &str,
        commit: &str,
        path: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>, ImageFailure> {
        if !self.inner.session.verify() {
            return Err(ImageFailure::StaleContext);
        }
        let locator = self.inner.repository.locator();
        if !locator.owner().eq_ignore_ascii_case(owner)
            || !locator.name().eq_ignore_ascii_case(name)
        {
            return Err(ImageFailure::StaleContext);
        }
        let commit = ObjectSha::new(commit).map_err(|_| ImageFailure::NotFound)?;

        // Try offline resolution and blob retrieval first
        if let Some(ref cache) = self.inner.persistent_cache {
            let cache_clone = cache.clone();
            let commit_clone = commit.clone();
            let path_clone = path.to_string();
            let resolved_sha_res = self
                .local(move || cache_clone.read_resolved_blob(&commit_clone, &path_clone))
                .await?;

            match resolved_sha_res {
                Ok(Some(blob_sha)) => {
                    let cache_clone = cache.clone();
                    let sha_for_read = blob_sha.clone();
                    let cached_bytes_res = self
                        .local(move || cache_clone.read_repository_blob(&sha_for_read))
                        .await?;

                    match cached_bytes_res {
                        Ok(Some(bytes)) => {
                            if !self.inner.session.verify() {
                                return Err(ImageFailure::StaleContext);
                            }
                            if bytes.len() > self.inner.blob_limits.decoded_bytes {
                                return Err(ImageFailure::TooLarge {
                                    limit: self.inner.blob_limits.decoded_bytes,
                                });
                            }
                            return Ok(bytes);
                        }
                        Err(crate::storage::cache::CacheError::AccessDenied) => {
                            return Err(ImageFailure::StaleContext);
                        }
                        _ => {}
                    }
                }
                Err(crate::storage::cache::CacheError::AccessDenied) => {
                    return Err(ImageFailure::StaleContext);
                }
                _ => {}
            }
        }

        let resolution_ticket = if let Some(cache) = self.inner.persistent_cache.clone() {
            let commit = commit.clone();
            let path = path.to_string();
            self.local(move || cache.begin_resolution(&commit, &path))
                .await?
                .ok()
        } else {
            None
        };
        let snapshot = self.snapshot(&commit, cancel.clone()).await?;
        if snapshot.commit() != &commit {
            return Err(ImageFailure::StaleContext);
        }
        let identity = self.resolve_blob(&snapshot, path, cancel.clone()).await?;

        // Resolution uses the ticket captured before metadata I/O too.
        if let (Some(cache), Some(ticket)) =
            (self.inner.persistent_cache.clone(), resolution_ticket)
        {
            let blob_sha = identity.blob().clone();
            let _ = self
                .local(move || cache.put_resolved_blob_with_ticket(ticket, &blob_sha))
                .await;
        }

        let service = self.clone();
        let key = identity.blob().as_str().to_string();
        let bytes = self
            .inner
            .blobs
            .read(
                key,
                &self.inner.executor,
                cancel,
                self.inner.limits.total_timeout,
                move |cancel| async move { service.resolved_blob_bytes(identity, cancel).await },
            )
            .await?;
        Ok(Arc::try_unwrap(bytes).unwrap_or_else(|bytes| (*bytes).clone()))
    }

    async fn resolved_blob_bytes(
        &self,
        identity: BlobIdentity,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>, ImageFailure> {
        // Check persistent cache for blob bytes first
        if let Some(ref cache) = self.inner.persistent_cache {
            let cache_clone = cache.clone();
            let blob_sha = identity.blob().clone();
            let cached_opt = self
                .local(move || cache_clone.read_repository_blob(&blob_sha))
                .await?;

            match cached_opt {
                Ok(Some(bytes)) => {
                    if !self.inner.session.verify() {
                        return Err(ImageFailure::StaleContext);
                    }
                    if bytes.len() > self.inner.blob_limits.decoded_bytes {
                        return Err(ImageFailure::TooLarge {
                            limit: self.inner.blob_limits.decoded_bytes,
                        });
                    }
                    return Ok(bytes);
                }
                Err(crate::storage::cache::CacheError::AccessDenied) => {
                    return Err(ImageFailure::StaleContext);
                }
                _ => {}
            }
        }

        let blob_ticket_opt = if let Some(ref cache) = self.inner.persistent_cache {
            let cache_clone = cache.clone();
            let blob_sha = identity.blob().clone();
            self.local(move || cache_clone.begin_write_repository_blob(&blob_sha))
                .await
                .ok()
                .and_then(|r| r.ok())
        } else {
            None
        };

        let network = self.network_permit(cancel.clone()).await?;
        let fetch_started = std::time::Instant::now();
        let read = self
            .inner
            .session
            .image_blob(&identity, &self.inner.blob_limits, cancel)
            .await
            .map_err(map_repository_error)?;
        if !self.inner.session.verify() {
            return Err(ImageFailure::StaleContext);
        }
        drop(network);
        self.inner.metrics.record(ImageStage::Fetch, fetch_started);
        let (blob, metadata) = read.value;
        let (_, body) = blob.into_parts();
        match body {
            BlobBody::Bytes(bytes) => {
                // Persist to cache in background using pre-acquired ticket
                if let Some(ref cache) = self.inner.persistent_cache
                    && let Some(ticket) = blob_ticket_opt
                {
                    let cache_clone = cache.clone();
                    let bytes_clone = bytes.clone();
                    let outcome = self
                        .local(move || {
                            cache_clone.put_repository_blob_response(
                                ticket,
                                &bytes_clone,
                                metadata.cache_control,
                            )
                        })
                        .await?;
                    if matches!(
                        outcome,
                        Err(crate::storage::cache::CacheError::Stale
                            | crate::storage::cache::CacheError::AccessDenied)
                    ) {
                        return Err(ImageFailure::StaleContext);
                    }
                }
                Ok(bytes)
            }
            BlobBody::TooLarge { limit, .. } => Err(ImageFailure::TooLarge { limit }),
        }
    }

    async fn snapshot(
        &self,
        commit: &ObjectSha,
        cancel: CancellationToken,
    ) -> Result<Arc<RepoSnapshot>, ImageFailure> {
        if let Some(snapshot) = self
            .inner
            .cache
            .lock()
            .expect("image cache")
            .snapshot(commit)
        {
            return Ok(snapshot);
        }
        let service = self.clone();
        let requested = commit.clone();
        self.inner
            .snapshots
            .read(
                commit.as_str().into(),
                &self.inner.executor,
                cancel,
                self.inner.limits.total_timeout,
                move |cancel| async move {
                    let _network = service.network_permit(cancel.clone()).await?;
                    let _metadata = service.inner.metrics.begin(ImageStage::Metadata);
                    let scoped = service
                        .inner
                        .session
                        .revision(
                            &service.inner.repository,
                            RevisionSpec::Commit(requested.clone()),
                            &service.inner.repository_limits,
                            cancel,
                        )
                        .await
                        .map_err(map_repository_error)?;
                    service
                        .inner
                        .cache
                        .lock()
                        .expect("image cache")
                        .put_snapshot(requested, Arc::new(scoped.value.clone()));
                    Ok(scoped.value)
                },
            )
            .await
    }

    async fn directory(
        &self,
        handle: &SnapshotDirectory,
        cancel: CancellationToken,
    ) -> Result<Arc<DirectoryTree>, ImageFailure> {
        let key = format!(
            "{}:{}",
            handle.commit().as_str(),
            handle.path().map(RepositoryPath::as_str).unwrap_or("")
        );
        if let Some(tree) = self
            .inner
            .cache
            .lock()
            .expect("image cache")
            .directory(&key)
        {
            return Ok(tree);
        }
        let service = self.clone();
        let handle = handle.clone();
        self.inner
            .directories
            .read(
                key.clone(),
                &self.inner.executor,
                cancel,
                self.inner.limits.total_timeout,
                move |cancel| async move {
                    let _network = service.network_permit(cancel.clone()).await?;
                    let _metadata = service.inner.metrics.begin(ImageStage::Metadata);
                    let scoped = service
                        .inner
                        .session
                        .directory(&handle, &service.inner.repository_limits, cancel)
                        .await
                        .map_err(map_repository_error)?;
                    service
                        .inner
                        .cache
                        .lock()
                        .expect("image cache")
                        .put_directory(key, Arc::new(scoped.value.clone()));
                    Ok(scoped.value)
                },
            )
            .await
    }

    /// Resolves a repository-relative picture path at the snapshot's commit.
    ///
    /// Symlinks and submodules are not followed: a picture must be an ordinary
    /// file entry in the fixed tree, so a link cannot turn a README into a
    /// filesystem read.
    async fn resolve_blob(
        &self,
        snapshot: &RepoSnapshot,
        path: &str,
        cancel: CancellationToken,
    ) -> Result<BlobIdentity, ImageFailure> {
        let path = RepositoryPath::new(path).map_err(|_| ImageFailure::NotFound)?;
        let segments: Vec<&str> = path.as_str().split('/').collect();
        let mut directory = SnapshotDirectory::root(snapshot);
        for (index, segment) in segments.iter().enumerate() {
            let tree = self.directory(&directory, cancel.clone()).await?;
            let found = tree
                .entries
                .iter()
                .enumerate()
                .find(|(_, entry)| entry.path.as_str() == *segment);
            let Some((entry_index, entry)) = found else {
                return Err(ImageFailure::NotFound);
            };
            if index + 1 == segments.len() {
                if !matches!(entry.kind, TreeEntryKind::File { .. }) {
                    return Err(ImageFailure::NotFound);
                }
                return BlobIdentity::from_file(&tree, entry_index)
                    .map_err(|_| ImageFailure::NotFound);
            }
            match entry.kind {
                TreeEntryKind::Directory => {
                    directory = tree
                        .child_directory(entry_index)
                        .map_err(|_| ImageFailure::NotFound)?;
                }
                _ => return Err(ImageFailure::NotFound),
            }
        }
        Err(ImageFailure::NotFound)
    }

    fn validity(&self, target: &ImageTarget) -> Result<ImageValidity, ImageFailure> {
        let authorization = self
            .inner
            .persistent_cache
            .as_ref()
            .map(|cache| {
                cache
                    .store()
                    .authorization(
                        cache.scope(),
                        match target {
                            ImageTarget::Repository { .. } => cache.repo_numeric_id(),
                            ImageTarget::External { .. } => None,
                        },
                    )
                    .ok_or(ImageFailure::StaleContext)
            })
            .transpose()?;
        let session = self.inner.session.clone();
        Ok(ImageValidity::new(
            move || {
                session.verify()
                    && authorization
                        .as_ref()
                        .is_none_or(|token| token.is_current())
            },
            None,
        ))
    }

    async fn network_permit(
        &self,
        cancel: CancellationToken,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, ImageFailure> {
        let _queue = self.inner.metrics.begin(ImageStage::Queue);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ImageFailure::Cancelled),
            permit = self.inner.permits.clone().acquire_owned() => permit.map_err(|_| ImageFailure::Cancelled),
        }
    }

    async fn external_bytes(
        &self,
        url: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>, ImageFailure> {
        self.external_bytes_attempt(url, cancel, true).await
    }
    fn external_bytes_attempt<'a>(
        &'a self,
        url: &'a str,
        cancel: CancellationToken,
        allow_cached: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<u8>, ImageFailure>> + Send + 'a>> {
        Box::pin(async move {
            let cached = if allow_cached {
                self.external_cached(url).await?
            } else {
                None
            };
            if let Some(snapshot) = &cached
                && ExternalFreshness::from_snapshot(snapshot).is_fresh_at(now_ms())
            {
                return Ok(snapshot.bytes.clone());
            }
            let ticket = if let Some(cache) = self.inner.persistent_cache.clone() {
                let url = url.to_string();
                self.local(move || cache.begin_write_external(&url))
                    .await?
                    .ok()
            } else {
                None
            };
            let freshness = cached.as_ref().map(ExternalFreshness::from_snapshot);
            let validation = ValidationHeaders {
                if_none_match: freshness.as_ref().and_then(|f| f.etag.as_deref()),
                if_modified_since: freshness.as_ref().and_then(|f| f.last_modified.as_deref()),
            };
            let network = self.network_permit(cancel.clone()).await?;
            let fetch_started = std::time::Instant::now();
            let response = self
                .inner
                .external
                .load_with_metadata(url, &self.inner.external_limits, validation, cancel.clone())
                .await;
            drop(network);
            self.inner.metrics.record(ImageStage::Fetch, fetch_started);
            match response {
                Ok(ExternalImageResponse::Fetched { bytes, metadata }) => {
                    if let (Some(cache), Some(ticket)) =
                        (self.inner.persistent_cache.clone(), ticket)
                    {
                        let saved = bytes.clone();
                        let outcome = self
                            .local(move || {
                                cache.put_external_image_with_ticket(ticket, &saved, &metadata)
                            })
                            .await?;
                        if matches!(
                            outcome,
                            Err(crate::storage::cache::CacheError::Stale
                                | crate::storage::cache::CacheError::AccessDenied)
                        ) {
                            return Err(ImageFailure::StaleContext);
                        }
                    }
                    Ok(bytes)
                }
                Ok(ExternalImageResponse::NotModified { metadata }) => {
                    let Some(snapshot) = cached else {
                        if allow_cached {
                            return Box::pin(self.external_bytes_attempt(url, cancel, false)).await;
                        }
                        return Err(ImageFailure::Network);
                    };
                    let current = self.external_cached(url).await?;
                    if current.as_ref().is_none_or(|current| {
                        current.bytes != snapshot.bytes
                            || current.metadata.etag != snapshot.metadata.etag
                            || current.metadata.last_modified != snapshot.metadata.last_modified
                            || current.metadata.cache_control != snapshot.metadata.cache_control
                    }) {
                        if allow_cached {
                            return Box::pin(self.external_bytes_attempt(url, cancel, false)).await;
                        }
                        return Err(ImageFailure::Network);
                    }
                    let bytes = snapshot.bytes.clone();
                    if let (Some(cache), Some(ticket)) =
                        (self.inner.persistent_cache.clone(), ticket)
                    {
                        let url = url.to_string();
                        let outcome = self
                            .local(move || {
                                cache.revalidate_external_image_with_ticket(
                                    ticket, &url, snapshot, &metadata,
                                )
                            })
                            .await?;
                        if matches!(
                            outcome,
                            Err(crate::storage::cache::CacheError::Stale
                                | crate::storage::cache::CacheError::AccessDenied)
                        ) {
                            return Err(ImageFailure::StaleContext);
                        }
                    }
                    Ok(bytes)
                }
                Err(ExternalImageError::Status(404)) => {
                    if let (Some(cache), Some(ticket)) =
                        (self.inner.persistent_cache.clone(), ticket)
                    {
                        let _ = self
                            .local(move || cache.invalidate_external_image_with_ticket(ticket))
                            .await;
                    }
                    Err(ImageFailure::NotFound)
                }
                Err(ExternalImageError::NoStore(_)) => {
                    if let (Some(cache), Some(ticket)) =
                        (self.inner.persistent_cache.clone(), ticket)
                    {
                        let _ = self
                            .local(move || cache.invalidate_external_image_with_ticket(ticket))
                            .await;
                    }
                    Err(ImageFailure::Invalidated)
                }
                Err(error) => Err(map_external_error(error)),
            }
        })
    }
    async fn external_cached(
        &self,
        url: &str,
    ) -> Result<Option<crate::domain::cache::CacheSnapshot>, ImageFailure> {
        let Some(cache) = self.inner.persistent_cache.clone() else {
            return Ok(None);
        };
        let url = url.to_string();
        match self.local(move || cache.read_external_image(&url)).await? {
            Ok(snapshot) => Ok(snapshot),
            Err(crate::storage::cache::CacheError::AccessDenied) => Err(ImageFailure::StaleContext),
            Err(_) => Ok(None),
        }
    }
    async fn local<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, ImageFailure> {
        let _cache_timer = self.inner.metrics.begin(ImageStage::Cache);
        let permit = self
            .inner
            .local_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ImageFailure::Cancelled)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .map_err(|_| ImageFailure::Network)
    }
}

impl ImageProvider for RepositoryImageService {
    fn start(&self, request: ImageRequest, sink: ImageSink) {
        self.start_with_options(request, sink, ImageRenderOptions::default());
    }

    fn start_with_options(
        &self,
        request: ImageRequest,
        sink: ImageSink,
        options: ImageRenderOptions,
    ) {
        let (token, _source) = {
            let mut work = self.inner.work.lock().expect("image work lock");
            work.entry(request.scope)
                .or_insert_with(CancellationToken::new)
                .clone()
        };
        let this = self.clone();
        let scope = request.scope;
        let key = request.key.clone();
        let failure_sink = sink.clone();
        let failure_key = key.clone();
        let deadline = tokio::time::Instant::now() + self.inner.limits.total_timeout;
        let admission = self.inner.load_permits.clone();
        let validity = self.validity(&request.target);
        let spawned = self.inner.executor.handle().spawn(async move {
            let phase = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let valid = validity.as_ref().ok().cloned();
            let result = match validity {
                Ok(validity) => tokio::select! {
                    biased;
                    _ = token.clone().cancelled() => Err(ImageFailure::Cancelled),
                    _ = sink.cancelled() => Err(ImageFailure::Cancelled),
                    result = tokio::time::timeout_at(deadline, async {
                        let _admission = admission.acquire_owned().await.map_err(|_| ImageFailure::Cancelled)?;
                        this.load(&request, token.clone(), options, deadline, sink.clone(), phase.clone(), validity).await
                    }) => result.unwrap_or(Err(ImageFailure::Timeout)),
                },
                Err(error) => Err(error),
            };
            if token.is_cancelled() {
                return;
            }
            if sink.supports_updates() {
                if matches!(result, Ok(Some(_))) { this.inner.metrics.record(ImageStage::FirstReady, deadline.into_std() - this.inner.limits.total_timeout); }
                let result =
                    result.map(|decoded| decoded.map_or(ImageUpdate::Finished, ImageUpdate::Ready));
                let terminal_validity = result.is_ok().then_some(valid).flatten();
                sink.publish_next(
                    scope,
                    key,
                    &phase,
                    true,
                    terminal_validity,
                    result,
                );
            } else {
                sink.push(ImageCompletion {
                    scope,
                    key,
                    result: result.and_then(|decoded| decoded.ok_or(ImageFailure::Malformed)),
                });
            }
        });
        if spawned.is_err() {
            failure_sink.push(ImageCompletion {
                scope,
                key: failure_key,
                result: Err(ImageFailure::Network),
            });
        }
    }

    fn cancel(&self, scope: ImageScope) {
        let source = self
            .inner
            .work
            .lock()
            .expect("image work lock")
            .remove(&scope);
        if let Some((_, source)) = source {
            source.cancel();
        }
    }
}

fn map_repository_error(error: RepositoryError) -> ImageFailure {
    match error {
        RepositoryError::GitHub(GitHubError::Cancelled) => ImageFailure::Cancelled,
        RepositoryError::GitHub(GitHubError::StaleContext) => ImageFailure::StaleContext,
        RepositoryError::GitHub(GitHubError::Timeout { .. }) => ImageFailure::Timeout,
        RepositoryError::GitHub(GitHubError::NotFound) => ImageFailure::NotFound,
        RepositoryError::GitHub(GitHubError::Unauthorized | GitHubError::Forbidden) => {
            ImageFailure::StaleContext
        }
        RepositoryError::LimitExceeded { limit, .. } => ImageFailure::TooLarge { limit },
        RepositoryError::GitHub(GitHubError::ResponseTooLarge { limit }) => {
            ImageFailure::TooLarge { limit }
        }
        RepositoryError::InvalidInput { .. } => ImageFailure::NotFound,
        RepositoryError::InvalidResponse { .. }
        | RepositoryError::GitHub(_)
        | RepositoryError::EmptyOrUnavailable
        | RepositoryError::SsoPartial { .. } => ImageFailure::Network,
    }
}

fn map_external_error(error: ExternalImageError) -> ImageFailure {
    match error {
        ExternalImageError::Refused(ExternalRefusal::Insecure) => {
            ImageFailure::Refused(ImageRefusal::InsecureExternal)
        }
        ExternalImageError::Refused(ExternalRefusal::UnsupportedScheme) => {
            ImageFailure::Refused(ImageRefusal::LocalOrUnsupportedScheme)
        }
        ExternalImageError::Refused(ExternalRefusal::LocalSource) => {
            ImageFailure::Refused(ImageRefusal::LocalOrUnsupportedScheme)
        }
        ExternalImageError::Refused(ExternalRefusal::InvalidUrl) => {
            ImageFailure::Refused(ImageRefusal::InvalidPath)
        }
        ExternalImageError::Refused(ExternalRefusal::TooManyRedirects) => ImageFailure::Network,
        ExternalImageError::Status(404) => ImageFailure::NotFound,
        ExternalImageError::Status(_) => ImageFailure::Network,
        ExternalImageError::Timeout => ImageFailure::Timeout,
        ExternalImageError::Network => ImageFailure::Network,
        ExternalImageError::TooLarge { limit } => ImageFailure::TooLarge { limit },
        ExternalImageError::NoStore(_) => ImageFailure::Invalidated,
        ExternalImageError::Cancelled => ImageFailure::Cancelled,
    }
}

#[cfg(test)]
#[path = "reader_images_tests.rs"]
mod tests;
