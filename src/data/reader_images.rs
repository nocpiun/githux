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
    ExternalImageBytes, ExternalImageError, ExternalImageLimits, ExternalRefusal,
};

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
    /// Scope -> (token, source). One source per reader document generation, so
    /// cancelling one document stops only its own loads.
    work: Mutex<HashMap<ImageScope, (CancellationToken, CancellationTokenSource)>>,
    cache: Mutex<ResolutionCache>,
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
                work: Mutex::new(HashMap::new()),
                cache: Mutex::new(ResolutionCache::default()),
            }),
        }
    }

    /// Overrides the JSON read limits used for commit/tree/blob requests.
    pub fn with_repository_limits(mut self, limits: RepositoryLimits) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("service not yet shared")
            .repository_limits = limits;
        self
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

    async fn load(
        &self,
        request: &ImageRequest,
        cancel: CancellationToken,
    ) -> Result<DecodedImage, ImageFailure> {
        let bytes = match &request.target {
            ImageTarget::Repository {
                owner,
                name,
                commit,
                path,
            } => {
                self.repository_bytes(owner, name, commit, path, cancel.clone())
                    .await?
            }
            ImageTarget::External { url } => self.external_bytes(url, cancel.clone()).await?,
        };
        if cancel.is_cancelled() {
            return Err(ImageFailure::Cancelled);
        }
        if !self.inner.session.verify() {
            return Err(ImageFailure::StaleContext);
        }
        let limits = self.inner.limits.clone();
        tokio::task::spawn_blocking(move || decode_image(bytes, &limits))
            .await
            .map_err(|_| ImageFailure::Network)?
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
        let snapshot = self.snapshot(&commit, cancel.clone()).await?;
        if snapshot.commit() != &commit {
            return Err(ImageFailure::StaleContext);
        }
        let identity = self.resolve_blob(&snapshot, path, cancel.clone()).await?;
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
            BlobBody::Bytes(bytes) => Ok(bytes),
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
        self.inner
            .external
            .load(url, &limits, cancel)
            .await
            .map_err(map_external_error)
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
