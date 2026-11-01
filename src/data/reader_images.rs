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

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::content::images::{
    DecodedImage, ImageCompletion, ImageFailure, ImageLimits, ImageProvider, ImageRefusal,
    ImageRequest, ImageScope, ImageSink, ImageTarget, decode_image,
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
    session: RepositorySession,
    repository: RepositoryIdentity,
    executor: Arc<BackgroundExecutor>,
    external: Arc<dyn ExternalImageBytes>,
    limits: ImageLimits,
    external_limits: ExternalImageLimits,
    repository_limits: RepositoryLimits,
    blob_limits: BlobLimits,
    permits: Arc<tokio::sync::Semaphore>,
    persistent_cache: Option<ReaderImageCache>,
    /// Scope -> (token, source). One source per reader document generation, so
    /// cancelling one document stops only its own loads.
    work: Mutex<HashMap<ImageScope, (CancellationToken, CancellationTokenSource)>>,
    cache: Mutex<ResolutionCache>,
    /// Test-only gate that pauses after decoding and reports that worker's
    /// thread, so a test can change the account at exactly that boundary.
    #[cfg(test)]
    publish_probe: Mutex<Option<PublishProbe>>,
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
                session,
                repository,
                executor,
                external,
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
            }),
        }
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

    async fn load(
        &self,
        request: &ImageRequest,
        cancel: CancellationToken,
    ) -> Result<DecodedImage, ImageFailure> {
        // One deadline for the whole load, including decode. A repository
        // picture is several requests (commit, each directory, blob) whose own
        // transport timeouts are per-request; without this, a slow but
        // successful sequence could hold a concurrency permit far past the
        // advertised budget and a host-configured shorter timeout would have no
        // effect on this path at all.
        let deadline = tokio::time::Instant::now() + self.inner.limits.total_timeout;
        let bytes = tokio::select! {
            biased;
            _ = cancel.clone().cancelled() => return Err(ImageFailure::Cancelled),
            _ = tokio::time::sleep_until(deadline) => return Err(ImageFailure::Timeout),
            bytes = async {
                match &request.target {
                    ImageTarget::Repository { owner, name, commit, path } => {
                        self.repository_bytes(owner, name, commit, path, cancel.clone()).await
                    }
                    ImageTarget::External { url } => {
                        self.external_bytes(url, cancel.clone()).await
                    }
                }
            } => bytes?,
        };
        if cancel.is_cancelled() {
            return Err(ImageFailure::Cancelled);
        }
        if !self.inner.session.verify() {
            return Err(ImageFailure::StaleContext);
        }
        let limits = self.inner.limits.clone();
        let decode = tokio::task::spawn_blocking(move || decode_image(bytes, &limits));
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
        if !self.inner.session.verify() {
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
            let resolved_sha_res = tokio::task::spawn_blocking(move || {
                cache_clone.read_resolved_blob(&commit_clone, &path_clone)
            })
            .await
            .map_err(|_| ImageFailure::Network)?;

            match resolved_sha_res {
                Ok(Some(blob_sha)) => {
                    let cache_clone = cache.clone();
                    let sha_for_read = blob_sha.clone();
                    let cached_bytes_res = tokio::task::spawn_blocking(move || {
                        cache_clone.read_repository_blob(&sha_for_read)
                    })
                    .await
                    .map_err(|_| ImageFailure::Network)?;

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

        let snapshot = self.snapshot(&commit, cancel.clone()).await?;
        if snapshot.commit() != &commit {
            return Err(ImageFailure::StaleContext);
        }
        let identity = self.resolve_blob(&snapshot, path, cancel.clone()).await?;

        // Cache resolution mapping in background
        if let Some(ref cache) = self.inner.persistent_cache {
            let cache_clone = cache.clone();
            let commit_clone = commit.clone();
            let path_clone = path.to_string();
            let blob_sha = identity.blob().clone();
            tokio::task::spawn(tokio::task::spawn_blocking(move || {
                let _ = cache_clone.put_resolved_blob(&commit_clone, &path_clone, &blob_sha);
            }));
        }

        // Check persistent cache for blob bytes first
        if let Some(ref cache) = self.inner.persistent_cache {
            let cache_clone = cache.clone();
            let blob_sha = identity.blob().clone();
            let cached_opt =
                tokio::task::spawn_blocking(move || cache_clone.read_repository_blob(&blob_sha))
                    .await
                    .map_err(|_| ImageFailure::Network)?;

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
            tokio::task::spawn_blocking(move || cache_clone.begin_write_repository_blob(&blob_sha))
                .await
                .ok()
                .and_then(|r| r.ok())
        } else {
            None
        };

        let read = self
            .inner
            .session
            .blob(&identity, &self.inner.blob_limits, cancel)
            .await
            .map_err(map_repository_error)?;
        if !self.inner.session.verify() {
            return Err(ImageFailure::StaleContext);
        }
        let (_, body) = read.value.into_parts();
        match body {
            BlobBody::Bytes(bytes) => {
                // Persist to cache in background using pre-acquired ticket
                if let Some(ref cache) = self.inner.persistent_cache
                    && let Some(ticket) = blob_ticket_opt
                {
                    let cache_clone = cache.clone();
                    let bytes_clone = bytes.clone();
                    tokio::task::spawn(tokio::task::spawn_blocking(move || {
                        let _ = cache_clone.put_repository_blob_with_ticket(ticket, &bytes_clone);
                    }));
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
        let scoped = self
            .inner
            .session
            .revision(
                &self.inner.repository,
                RevisionSpec::Commit(commit.clone()),
                &self.inner.repository_limits,
                cancel,
            )
            .await
            .map_err(map_repository_error)?;
        let snapshot = Arc::new(scoped.value);
        self.inner
            .cache
            .lock()
            .expect("image cache")
            .put_snapshot(commit.clone(), snapshot.clone());
        Ok(snapshot)
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
        let scoped = self
            .inner
            .session
            .directory(handle, &self.inner.repository_limits, cancel)
            .await
            .map_err(map_repository_error)?;
        let tree = Arc::new(scoped.value);
        self.inner
            .cache
            .lock()
            .expect("image cache")
            .put_directory(key, tree.clone());
        Ok(tree)
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

    async fn external_bytes(
        &self,
        url: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>, ImageFailure> {
        let limits = self.inner.external_limits.clone();

        if let Some(ref cache) = self.inner.persistent_cache {
            let cache_clone = cache.clone();
            let url_string = url.to_string();
            let cached_snapshot =
                tokio::task::spawn_blocking(move || cache_clone.read_external_image(&url_string))
                    .await
                    .map_err(|_| ImageFailure::Network)?;

            if let Ok(Some(snapshot)) = cached_snapshot {
                let age_secs = snapshot
                    .metadata
                    .cache_control
                    .as_deref()
                    .and_then(|cc| {
                        cc.split(',').find_map(|p| {
                            let p = p.trim();
                            p.strip_prefix("githux-age=")
                                .and_then(|v| v.trim().parse::<u64>().ok())
                        })
                    })
                    .unwrap_or(0);

                let freshness = ExternalFreshness {
                    etag: snapshot.metadata.etag.clone(),
                    last_modified: snapshot.metadata.last_modified.clone(),
                    cache_control: snapshot.metadata.cache_control.clone(),
                    fetched_at_ms: snapshot.metadata.fetched_at_ms,
                    verified_at_ms: snapshot.metadata.verified_at_ms,
                    age_secs,
                };

                let now = now_ms();
                if freshness.is_fresh_at(now) {
                    return Ok(snapshot.bytes);
                }

                // Stale or needs revalidation: capture write ticket BEFORE network await
                let ticket_res = {
                    let cache_clone = cache.clone();
                    let url_string = url.to_string();
                    tokio::task::spawn_blocking(move || {
                        cache_clone.begin_write_external(&url_string)
                    })
                    .await
                    .map_err(|_| ImageFailure::Network)?
                };

                let validation = ValidationHeaders {
                    if_none_match: freshness.etag.as_deref(),
                    if_modified_since: freshness.last_modified.as_deref(),
                };

                let response = self
                    .inner
                    .external
                    .load_with_metadata(url, &limits, validation, cancel.clone())
                    .await;

                match response {
                    Ok(ExternalImageResponse::NotModified { metadata }) => {
                        let cache_clone = cache.clone();
                        let snap = snapshot.clone();
                        if let Ok(ticket) = ticket_res {
                            tokio::task::spawn(tokio::task::spawn_blocking(move || {
                                let _ = cache_clone
                                    .revalidate_external_image_with_ticket(ticket, snap, &metadata);
                            }));
                        }
                        return Ok(snapshot.bytes);
                    }
                    Ok(ExternalImageResponse::Fetched { bytes, metadata }) => {
                        let cache_clone = cache.clone();
                        let bytes_clone = bytes.clone();
                        let meta = metadata.clone();
                        if let Ok(ticket) = ticket_res {
                            tokio::task::spawn(tokio::task::spawn_blocking(move || {
                                let _ = cache_clone.put_external_image_with_ticket(
                                    ticket,
                                    &bytes_clone,
                                    &meta,
                                );
                            }));
                        }
                        return Ok(bytes);
                    }
                    Err(ExternalImageError::Status(404)) => {
                        let cache_clone = cache.clone();
                        if let Ok(ticket) = ticket_res {
                            tokio::task::spawn(tokio::task::spawn_blocking(move || {
                                let _ = cache_clone.invalidate_external_image_with_ticket(ticket);
                            }));
                        }
                        return Err(ImageFailure::NotFound);
                    }
                    Err(e) => {
                        return Err(map_external_error(e));
                    }
                }
            }
        }

        // Cache miss or no cache configured: capture write ticket before network load
        let ticket_opt = if let Some(ref cache) = self.inner.persistent_cache {
            let cache_clone = cache.clone();
            let url_string = url.to_string();
            tokio::task::spawn_blocking(move || cache_clone.begin_write_external(&url_string))
                .await
                .ok()
                .and_then(|r| r.ok())
        } else {
            None
        };

        let response = self
            .inner
            .external
            .load_with_metadata(url, &limits, ValidationHeaders::default(), cancel)
            .await
            .map_err(map_external_error)?;

        match response {
            ExternalImageResponse::Fetched { bytes, metadata } => {
                if let Some(ref cache) = self.inner.persistent_cache
                    && let Some(ticket) = ticket_opt
                {
                    let cache_clone = cache.clone();
                    let bytes_clone = bytes.clone();
                    tokio::task::spawn(tokio::task::spawn_blocking(move || {
                        let _ = cache_clone.put_external_image_with_ticket(
                            ticket,
                            &bytes_clone,
                            &metadata,
                        );
                    }));
                }
                Ok(bytes)
            }
            ExternalImageResponse::NotModified { .. } => {
                // 304 without prior cached snapshot is an invalid condition
                Err(ImageFailure::Network)
            }
        }
    }
}

impl ImageProvider for RepositoryImageService {
    fn start(&self, request: ImageRequest, sink: ImageSink) {
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
        let spawned = self.inner.executor.handle().spawn(async move {
            let _permit = this.inner.permits.clone().acquire_owned().await;
            let result = this.load(&request, token.clone()).await;
            if token.is_cancelled() {
                return;
            }
            sink.push(ImageCompletion { scope, key, result });
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
        ExternalImageError::Cancelled => ImageFailure::Cancelled,
    }
}

#[cfg(test)]
#[path = "reader_images_tests.rs"]
mod tests;
