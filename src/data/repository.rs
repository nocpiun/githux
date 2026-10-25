//! Account-bound repository reads. Sessions pin authorization across steps;
//! Git object handles continue to enforce the snapshot relationships from PR19.
use super::RepoService;
use crate::auth::{AuthContext, token::Pat};
use crate::content::{PreparedContent, prepare_content};
use crate::domain::blob::{BlobIdentity, BlobLimits, BlobRead};
use crate::domain::error::GitHubError;
use crate::domain::repository::{
    BranchesOutcome, DirectoryTree, ReadmeDiscovery, RepoSnapshot, RepositoryDetails,
    RepositoryError, RepositoryIdentity, RepositoryLocator, RevisionSpec, SnapshotDirectory,
};
use crate::github::{self, RepositoryLimits};
use crate::network::cancel::CancellationToken;
use std::sync::{Arc, Mutex, Weak, atomic::Ordering};

/// Opaque service-instance and authorization ownership, without credentials.
/// Keep this with successful data and with errors until the UI publication
/// check. AuthContext alone must not be compared across service instances.
#[derive(Clone, Debug)]
pub struct RepositoryReadContext {
    auth: AuthContext,
    registry: Weak<Mutex<crate::auth::AccountRegistry>>,
}

impl RepositoryReadContext {
    pub fn auth(&self) -> &AuthContext {
        &self.auth
    }
}

impl PartialEq for RepositoryReadContext {
    fn eq(&self, other: &Self) -> bool {
        self.registry.ptr_eq(&other.registry) && self.auth == other.auth
    }
}
impl Eq for RepositoryReadContext {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scoped<T> {
    pub context: RepositoryReadContext,
    pub value: T,
}

impl RepoService {
    /// Begin a browse. Every subsequent step uses this original context, even
    /// if the caller switches accounts before starting the next method.
    pub fn session(&self) -> Result<RepositorySession, RepositoryError> {
        let context = self.capture_read_context()?;
        Ok(RepositorySession {
            service: self.clone(),
            context,
        })
    }

    pub(super) fn capture_read_context(&self) -> Result<RepositoryReadContext, GitHubError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(GitHubError::StaleContext);
        }
        let auth = self
            .registry
            .lock()
            .expect("registry lock")
            .active_context()
            .ok_or(GitHubError::StaleContext)?;
        Ok(RepositoryReadContext {
            auth,
            registry: Arc::downgrade(&self.registry),
        })
    }

    /// Recheck on the receiving/UI side immediately before applying either a
    /// success or an error. The UI must additionally verify TabId, selected
    /// resource and its own request generation.
    pub fn is_current(&self, context: &RepositoryReadContext) -> bool {
        context.registry.ptr_eq(&Arc::downgrade(&self.registry))
            && !self.closed.load(Ordering::Acquire)
            && self
                .registry
                .lock()
                .expect("registry lock")
                .active_context()
                .as_ref()
                == Some(&context.auth)
    }

    /// Shared authentication path for list_repos and all snapshot adapters.
    /// Every exit after an await (including cancellation, worker failure and
    /// scheduler failure) passes through the final context check.
    pub(super) async fn read_authenticated<T, E, F, Fut>(
        &self,
        context: &RepositoryReadContext,
        cancel: CancellationToken,
        read: F,
    ) -> Result<T, E>
    where
        E: From<GitHubError>,
        F: FnOnce(Pat, CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        if !self.is_current(context) {
            return Err(GitHubError::StaleContext.into());
        }
        let result = async {
            if cancel.is_cancelled() {
                return Err(GitHubError::Cancelled.into());
            }
            // Capture the slot and check the context under the same short lock.
            let credential = {
                let registry = self.registry.lock().expect("registry lock");
                if registry.active_context().as_ref() != Some(&context.auth) {
                    return Err(GitHubError::StaleContext.into());
                }
                registry
                    .credential_ref(context.auth.account.clone())
                    .ok_or(GitHubError::StaleContext)?
            };
            let store = self.store.clone();
            let worker = tokio::task::spawn_blocking(move || store.load(&credential));
            let loaded = tokio::select! {
                biased;
                _ = cancel.clone().cancelled() => return Err(GitHubError::Cancelled.into()),
                loaded = worker => loaded,
            };
            // Retired credentials can be cleaned while load blocks. Never map
            // missing/load-error/JoinError until ownership is revalidated.
            if !self.is_current(context) {
                return Err(GitHubError::StaleContext.into());
            }
            let token = loaded
                .map_err(|_| GitHubError::CredentialStore {
                    message: "credential worker closed".into(),
                })?
                .map_err(|error| GitHubError::CredentialStore {
                    message: error.to_string(),
                })?
                .ok_or(GitHubError::CredentialStore {
                    message: "credential missing".into(),
                })?;
            let _permit = tokio::select! {
                biased;
                _ = cancel.clone().cancelled() => return Err(GitHubError::Cancelled.into()),
                permit = self.executor.semaphore().acquire_owned() => {
                    permit.map_err(|_| GitHubError::CredentialStore {
                        message: "request scheduler closed".into(),
                    })?
                }
            };
            // A queued task must not send after losing its original context.
            if !self.is_current(context) {
                return Err(GitHubError::StaleContext.into());
            }
            tokio::select! {
                biased;
                _ = cancel.clone().cancelled() => Err(GitHubError::Cancelled.into()),
                result = read(token, cancel.clone()) => result,
            }
        }
        .await;
        // Staleness wins over old success, Unauthorized, missing credentials,
        // cancellation and scheduler/worker errors. No lock crosses an await.
        if !self.is_current(context) {
            return Err(GitHubError::StaleContext.into());
        }
        result
    }
}

/// Test-only evidence about the preparation boundary: which thread ran it and
/// a controllable pause between the completed read and publication. The pause
/// is keyed by the exact BlobIdentity so a concurrently running test cannot
/// consume another read's gate.
#[cfg(test)]
pub(super) mod test_hooks {
    use crate::domain::blob::BlobIdentity;
    use std::collections::HashMap;
    use std::sync::{Mutex, mpsc};
    static LAST: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);
    type Gates = HashMap<String, (mpsc::Sender<()>, mpsc::Receiver<()>)>;
    static GATES: std::sync::LazyLock<Mutex<Gates>> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    /// Value key: fetch_blob clones the identity, so a pointer key would miss.
    fn key(identity: &BlobIdentity) -> String {
        format!(
            "{}:{}:{}:{}:{:?}",
            identity.repository().id(),
            identity.commit().as_str(),
            identity.blob().as_str(),
            identity.path().as_str(),
            identity.size()
        )
    }
    pub fn before_prepare(identity: &BlobIdentity) {
        *LAST.lock().unwrap() = Some(std::thread::current().id());
        let gate = GATES.lock().unwrap().remove(&key(identity));
        if let Some((entered, resume)) = gate {
            let _ = entered.send(());
            let _ = resume.recv_timeout(std::time::Duration::from_secs(5));
        }
    }
    pub fn last_prepare_thread() -> Option<std::thread::ThreadId> {
        *LAST.lock().unwrap()
    }
    pub fn pause_prepare_of(identity: &BlobIdentity) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, ready) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        GATES
            .lock()
            .unwrap()
            .insert(key(identity), (entered, resume));
        (ready, release)
    }
}

/// A constrained browse, never a PAT holder. Clone/start separate sessions for
/// independent reads; cancellation belongs to each method call, not a session.
#[derive(Clone)]
pub struct RepositorySession {
    service: RepoService,
    context: RepositoryReadContext,
}

impl std::fmt::Debug for RepositorySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepositorySession")
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

impl RepositorySession {
    pub fn context(&self) -> &RepositoryReadContext {
        &self.context
    }

    pub fn verify(&self) -> bool {
        self.service.is_current(&self.context)
    }

    async fn read<T, F, Fut>(
        &self,
        cancel: CancellationToken,
        read: F,
    ) -> Result<Scoped<T>, RepositoryError>
    where
        F: FnOnce(Pat, CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, RepositoryError>>,
    {
        let value = self
            .service
            .read_authenticated(&self.context, cancel, read)
            .await?;
        Ok(Scoped {
            context: self.context.clone(),
            value,
        })
    }

    pub async fn repository(
        &self,
        locator: RepositoryLocator,
        limits: &RepositoryLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<RepositoryDetails>, RepositoryError> {
        self.read(cancel, async move |pat, cancel| {
            github::fetch_repository(&self.service.client, &pat, &locator, limits, cancel).await
        })
        .await
    }

    pub async fn branches(
        &self,
        repository: &RepositoryIdentity,
        limits: &RepositoryLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<BranchesOutcome>, RepositoryError> {
        self.read(cancel, async move |pat, cancel| {
            github::fetch_branches(&self.service.client, &pat, repository, limits, cancel).await
        })
        .await
    }

    pub async fn revision(
        &self,
        repository: &RepositoryIdentity,
        revision: RevisionSpec,
        limits: &RepositoryLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<RepoSnapshot>, RepositoryError> {
        self.read(cancel, async move |pat, cancel| {
            github::resolve_revision(
                &self.service.client,
                &pat,
                repository,
                revision,
                limits,
                cancel,
            )
            .await
        })
        .await
    }

    pub async fn directory(
        &self,
        directory: &SnapshotDirectory,
        limits: &RepositoryLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<DirectoryTree>, RepositoryError> {
        self.read(cancel, async move |pat, cancel| {
            github::fetch_directory(&self.service.client, &pat, directory, limits, cancel).await
        })
        .await
    }

    pub async fn readme(
        &self,
        snapshot: &RepoSnapshot,
        limits: &RepositoryLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<ReadmeDiscovery>, RepositoryError> {
        self.read(cancel, async move |pat, cancel| {
            github::discover_readme(&self.service.client, &pat, snapshot, limits, cancel).await
        })
        .await
    }

    /// Raw fixed-SHA bytes. An already-known oversized blob is a local
    /// `BlobBody::TooLarge` result, but it still passes every context gate.
    pub async fn blob(
        &self,
        identity: &BlobIdentity,
        limits: &BlobLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<BlobRead>, RepositoryError> {
        self.read(cancel, async move |pat, cancel| {
            github::blob::fetch_blob(&self.service.client, &pat, identity, limits, cancel).await
        })
        .await
    }

    /// Read and prepare in one call. Preparation is CPU work on the bounded
    /// executor's blocking pool, never on the calling (possibly GUI) thread.
    pub async fn content(
        &self,
        identity: &BlobIdentity,
        limits: &BlobLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PreparedContent>, RepositoryError> {
        let budget = limits.decoded_bytes;
        let blob = self.blob(identity, limits, cancel.clone()).await?;
        let service = self.service.clone();
        let context = self.context.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            test_hooks::before_prepare(&blob.value.identity);
            prepare_content(blob.value, budget)
        });
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(GitHubError::Cancelled.into()),
            prepared = prepared => prepared.map_err(|_| RepositoryError::GitHub(
                GitHubError::CredentialStore { message: "content preparation worker closed".into() },
            )),
        };
        // Preparation is another await boundary: staleness outranks the
        // prepared text, cancellation and worker failure, exactly as in
        // read_authenticated. Check before propagating any of them.
        if !service.is_current(&context) {
            return Err(GitHubError::StaleContext.into());
        }
        Ok(Scoped {
            context,
            value: outcome?,
        })
    }
}
