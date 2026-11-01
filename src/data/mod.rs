//! Business/data entry points: login, account management and repository reads.
//!
//! This layer orchestrates the auth, network and github layers:
//! - PAT login verifies identity via `GET /user` **before** saving credentials.
//! - Credential save failure never claims login persisted.
//! - Replace/switch/remove advance the authorization epoch; stale responses are
//!   discarded.
//! - Requests run on the background executor with bounded concurrency and
//!   cancellation.
//!
//! Status: first reusable R2 slice. Not yet wired into UI.

// The GPUI shell does not yet consume this service layer (that is the separate
// UI-integration task). This module-scoped allowance is limited to these
// not-yet-called modules so real warnings stay visible elsewhere; the crate
// root deliberately has no `#![allow]` blanket.
#![allow(dead_code)]

use crate::auth::RegistryError;
use crate::auth::credential::{CredentialError, CredentialRef, CredentialStore};
use crate::auth::token::Pat;
use crate::auth::{AccountRegistry, AuthContext};
use crate::domain::AccountSummary;
use crate::domain::error::GitHubError;
use crate::domain::repo::ReposOutcome;
use crate::github::client::GitHubClient;
use crate::github::repos::fetch_user_repos;
use crate::github::user::{AuthenticatedUser, fetch_authenticated_user};
#[cfg(test)]
mod account_concurrency_tests;
#[cfg(test)]
mod account_tests;
mod accounts;
#[cfg(test)]
mod page_read_tests;
mod page_reads;
pub mod reader_images;
pub mod repository;
#[cfg(test)]
mod repository_content_tests;
#[cfg(test)]
mod repository_tests;
use crate::network::cancel::CancellationToken;
use crate::network::executor::BackgroundExecutor;
use crate::storage::accounts::StorageError;
#[allow(unused_imports)]
pub use accounts::AccountError;
#[allow(unused_imports)]
pub use page_reads::{PageReadSession, ProfileReadme, ProfileReadmeIneligible};
#[allow(unused_imports)]
pub use repository::{RepositoryReadContext, RepositorySession, Scoped};
use std::sync::Arc;

/// Account and credential lifecycle service.
pub struct AccountService {
    registry: Arc<std::sync::Mutex<AccountRegistry>>,
    store: Arc<dyn CredentialStore>,
    client: Arc<GitHubClient>,
    executor: Arc<BackgroundExecutor>,
    /// Serializes "save credential + commit registry record" so two overlapping
    /// `replace` calls cannot interleave: without it, A could save token A, B
    /// could save token B and commit, and then A could commit and report success
    /// while the store actually holds token B. The epoch alone does not protect
    /// this because the store write happens outside the registry lock.
    credential_commit: Arc<std::sync::Mutex<()>>,
    database: accounts::Database,
    closed: Arc<std::sync::atomic::AtomicBool>,
    persistent: bool,
}

/// Repository read service scoped to the active account.
#[derive(Clone)]
pub struct RepoService {
    registry: Arc<std::sync::Mutex<AccountRegistry>>,
    store: Arc<dyn CredentialStore>,
    client: Arc<GitHubClient>,
    executor: Arc<BackgroundExecutor>,
    persistent: bool,
    credential_commit: Arc<std::sync::Mutex<()>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// Newly verified and saved.
    SignedIn(AccountSummary),
    /// PAT replaced for an existing account after identity re-verification.
    Replaced(AccountSummary),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    GitHub(GitHubError),
    CredentialSave(CredentialError),
    IdentityMismatch {
        expected_account: u64,
        actual_account: u64,
    },
    NotFound,
    Storage(StorageError),
    /// Saved and active; retired secret cleanup is still pending.
    CleanupPending(Box<AccountSummary>),
}

impl From<GitHubError> for LoginError {
    fn from(e: GitHubError) -> Self {
        LoginError::GitHub(e)
    }
}

impl From<CredentialError> for LoginError {
    fn from(e: CredentialError) -> Self {
        LoginError::CredentialSave(e)
    }
}

impl AccountService {
    /// Isolated legacy in-memory fixture; production uses open() exclusively.
    #[cfg(test)]
    pub fn new(
        store: Arc<dyn CredentialStore>,
        client: Arc<GitHubClient>,
        executor: Arc<BackgroundExecutor>,
    ) -> Self {
        Self {
            registry: Arc::new(std::sync::Mutex::new(AccountRegistry::default())),
            store,
            client,
            executor,
            credential_commit: Arc::new(std::sync::Mutex::new(())),
            database: Arc::new(std::sync::Mutex::new(None)),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            persistent: false,
        }
    }

    #[cfg(test)]
    pub fn registry_handle(&self) -> Arc<std::sync::Mutex<AccountRegistry>> {
        self.registry.clone()
    }

    pub fn repo_service(&self) -> RepoService {
        RepoService {
            registry: self.registry.clone(),
            store: self.store.clone(),
            client: self.client.clone(),
            executor: self.executor.clone(),
            persistent: self.persistent,
            credential_commit: self.credential_commit.clone(),
            closed: self.closed.clone(),
        }
    }

    /// Sign in: verify a PAT via `GET /user`, then persist the credential. If
    /// credential storage fails, login is **not** reported as persisted.
    ///
    /// The account is keyed by the identity the token actually belongs to. This
    /// is the *add account* path. To change the PAT of an existing account, use
    /// [`AccountService::replace`] so a token for a different account cannot
    /// silently swap identities.
    pub async fn login(&self, pat: Pat) -> Result<LoginOutcome, LoginError> {
        self.login_cancellable(pat, CancellationToken::default())
            .await
    }

    /// Cancellation before submission leaves no writes. After submission the
    /// durable commit completes even if this future is dropped.
    pub async fn login_cancellable(
        &self,
        pat: Pat,
        cancel: CancellationToken,
    ) -> Result<LoginOutcome, LoginError> {
        let verified = self.verify_cancellable(&pat, cancel.clone()).await?;
        if cancel.is_cancelled() {
            return Err(LoginError::GitHub(GitHubError::Cancelled));
        }
        self.commit_login(verified.identity, pat, None).await
    }

    /// Replace the PAT of an **existing** account.
    ///
    /// The token is verified first, then its identity is compared with
    /// `expected`. A token that belongs to a different account yields
    /// [`LoginError::IdentityMismatch`] and neither account is saved or mutated,
    /// so "replace account A" can never silently become "add/replace B".
    pub async fn replace(
        &self,
        expected: crate::domain::AccountKey,
        pat: Pat,
    ) -> Result<LoginOutcome, LoginError> {
        let epoch = self
            .registry
            .lock()
            .expect("registry lock")
            .incarnation(&expected)
            .ok_or(LoginError::NotFound)?;
        let verified = self.verify(&pat).await?;
        if verified.identity.key != expected {
            return Err(LoginError::IdentityMismatch {
                expected_account: expected.id,
                actual_account: verified.identity.key.id,
            });
        }

        self.commit_login(verified.identity, pat, Some(epoch)).await
    }

    async fn verify(&self, pat: &Pat) -> Result<AuthenticatedUser, LoginError> {
        self.verify_cancellable(pat, CancellationToken::default())
            .await
    }

    async fn verify_cancellable(
        &self,
        pat: &Pat,
        cancel: CancellationToken,
    ) -> Result<AuthenticatedUser, LoginError> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(LoginError::Storage(StorageError::Closed));
        }
        let _permit = tokio::select! {
            permit = self.executor.semaphore().acquire_owned() => permit.map_err(|_|LoginError::Storage(StorageError::Closed))?,
            _ = cancel.clone().cancelled() => return Err(LoginError::GitHub(GitHubError::Cancelled)),
        };
        Ok(fetch_authenticated_user(&self.client, pat, cancel).await?)
    }

    /// Remove an account and delete its credential.
    ///
    /// A credential deletion failure is reported instead of silently claiming a
    /// successful removal. The registry entry is still dropped so no stale
    /// reference remains active, and the returned error tells the caller that a
    /// stored secret may linger and needs retrying. Persistent services retain
    /// a durable cleanup reference; remove_saved exposes the typed partial result.
    ///
    /// Removal participates in the same `credential_commit` serialization as
    /// login and replacement. Without it, an overlapping `replace` could pass its
    /// existence recheck, removal could delete the old credential and drop the
    /// account, and the replacement could then save a new PAT that no account
    /// refers to — leaving an orphaned secret with no cleanup path.
    pub async fn remove(&self, account: crate::domain::AccountKey) -> Result<(), GitHubError> {
        self.remove_saved(account)
            .await
            .map_err(|e| GitHubError::CredentialStore {
                message: format!("account removal: {e:?}"),
            })
    }

    pub fn summaries(&self) -> Vec<AccountSummary> {
        self.registry.lock().expect("registry lock").summaries()
    }

    pub fn active_context(&self) -> Option<AuthContext> {
        self.registry
            .lock()
            .expect("registry lock")
            .active_context()
    }

    /// Current authoritative display identity. Used only to classify own-profile navigation;
    /// authorization still comes from a separately captured read session.
    pub fn active_summary(&self) -> Option<AccountSummary> {
        self.registry
            .lock()
            .expect("registry lock")
            .active_summary()
    }
}

impl RepoService {
    /// Fetch repositories for the active account with an ignorable-by-default
    /// cancellation token. Returns `StaleContext` if the authorization context
    /// changed during the request.
    pub async fn list_repos(&self) -> Result<ReposOutcome, GitHubError> {
        self.list_repos_cancellable(CancellationToken::default())
            .await
    }

    /// Fetch repositories for the active account, honouring an explicit
    /// cancellation token. Every response is validated against the
    /// authorization context captured before the request; a switch, PAT
    /// replacement or removal during flight yields `StaleContext` and the data
    /// is discarded.
    pub async fn list_repos_cancellable(
        &self,
        cancel: CancellationToken,
    ) -> Result<ReposOutcome, GitHubError> {
        let context = self.capture_read_context()?;
        self.read_authenticated(&context, cancel, async move |token, cancel| {
            fetch_user_repos(&self.client, &token, cancel).await
        })
        .await
    }

    /// Legacy synchronous selection for in-memory services. Persistent services
    /// return PersistenceRequired: use AccountService::set_active().await.
    pub fn set_active(&self, account: crate::domain::AccountKey) -> Result<(), RegistryError> {
        // Legacy in-memory selection remains synchronous. Persistent callers
        // must await AccountService::set_active to observe durable completion.
        if self.persistent {
            return Err(RegistryError::PersistenceRequired);
        }
        let _gate = self
            .credential_commit
            .try_lock()
            .map_err(|_| RegistryError::Busy)?;
        let mut reg = self.registry.lock().expect("registry lock");
        reg.set_active(account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::credential::UnsupportedStore;
    use crate::auth::memory::InMemoryCredentialStore;
    use crate::github::client::GitHubConfig;
    use crate::network::transport::{HttpResponse, MockTransport};

    fn user_body(id: u64, login: &str) -> String {
        serde_json::json!({
            "id": id, "login": login,
            "avatar_url": format!("https://github.com/{login}.png"),
            "html_url": format!("https://github.com/{login}")
        })
        .to_string()
    }

    fn repos_body(ids: &[u64]) -> String {
        let arr: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": id, "name": format!("r{id}"), "full_name": format!("u/r{id}"),
                    "owner": {"id": 1, "login": "u"}, "visibility": "public"
                })
            })
            .collect();
        serde_json::json!(arr).to_string()
    }

    fn build_service(
        store: Arc<dyn CredentialStore>,
        mock: Arc<MockTransport>,
    ) -> (AccountService, Arc<BackgroundExecutor>) {
        let client = Arc::new(GitHubClient::new(GitHubConfig::default(), mock));
        let exec = Arc::new(BackgroundExecutor::new(4));
        (AccountService::new(store, client, exec.clone()), exec)
    }

    #[tokio::test]
    async fn login_verifies_then_persists() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(42, "octocat"),
        }))
        .await;

        let (svc, _exec) = build_service(store.clone(), mock);
        let pat = Pat::from_str("ghp_real_token");
        let outcome = svc.login(pat).await.expect("login ok");
        match outcome {
            LoginOutcome::SignedIn(summary) => assert_eq!(summary.identity.key.id, 42),
            _ => panic!("expected SignedIn"),
        }
    }

    #[tokio::test]
    async fn credential_save_failure_does_not_persist() {
        let store = Arc::new(InMemoryCredentialStore::failing_saves()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(42, "octocat"),
        }))
        .await;

        let (svc, _exec) = build_service(store, mock);
        let result = svc.login(Pat::from_str("ghp_token")).await;
        assert!(matches!(result, Err(LoginError::CredentialSave(_))));
        assert!(svc.summaries().is_empty());
    }

    #[tokio::test]
    async fn replace_advances_epoch() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(99, "dev"),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(99, "dev"),
        }))
        .await;

        let (svc, _exec) = build_service(store, mock);
        let first = svc.login(Pat::from_str("ghp_old")).await.expect("first");
        let first_epoch = match first {
            LoginOutcome::SignedIn(s) => s.epoch,
            _ => panic!(),
        };
        let second = svc.login(Pat::from_str("ghp_new")).await.expect("replace");
        let second_epoch = match second {
            LoginOutcome::Replaced(s) => s.epoch,
            _ => panic!("expected Replaced"),
        };
        assert!(second_epoch.value() > first_epoch.value());
    }

    #[tokio::test]
    async fn list_repos_returns_data() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(7, "dev"),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: repos_body(&[1, 2, 3]),
        }))
        .await;

        let (svc, _exec) = build_service(store, mock);
        svc.login(Pat::from_str("ghp_t")).await.expect("login");
        let repo_svc = svc.repo_service();
        let outcome = repo_svc.list_repos().await.expect("repos");
        assert_eq!(outcome.repositories.len(), 3);
    }

    /// The dedicated replacement entry point rejects a PAT that belongs to a
    /// different account instead of silently swapping identities.
    #[tokio::test]
    async fn replace_rejects_a_token_for_a_different_account() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        // login as account 1
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(1, "account_one"),
        }))
        .await;
        // the "replacement" token verifies as account 2
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(2, "account_two"),
        }))
        .await;

        let (svc, _exec) = build_service(store.clone(), mock);
        svc.login(Pat::from_str("ghp_one")).await.expect("login");

        let result = svc
            .replace(
                crate::domain::AccountKey::new(1, crate::domain::HostId::github()),
                Pat::from_str("ghp_two"),
            )
            .await;
        assert!(
            matches!(
                result,
                Err(LoginError::IdentityMismatch {
                    expected_account: 1,
                    actual_account: 2
                })
            ),
            "expected IdentityMismatch, got {result:?}"
        );

        // Neither account was added or mutated.
        let summaries = svc.summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].identity.key.id, 1);
        // The stale account-2 credential was never saved.
        let cred_two = crate::auth::CredentialRef::new(crate::domain::HostId::github(), 2);
        assert!(store.load(&cred_two).expect("load").is_none());
    }

    /// Regression: two replacements running on **different worker threads** must
    /// not interleave their credential write with the registry commit.
    ///
    /// Without the commit lock, A could save token A, B could save token B and
    /// commit, and then A could commit — leaving the store holding B while the
    /// registry reports A's epoch. It uses a real multi-thread runtime with two
    /// spawned tasks so the critical section is genuinely concurrent, and an
    /// instrumented store that flags any overlap of two `save` calls.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn overlapping_replacements_are_serialized() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        #[derive(Default)]
        struct InstrumentedStore {
            inner: InMemoryCredentialStore,
            saves_in_flight: AtomicUsize,
            overlap_detected: AtomicBool,
        }
        impl CredentialStore for InstrumentedStore {
            fn save(
                &self,
                r#ref: &crate::auth::CredentialRef,
                token: &Pat,
            ) -> Result<(), CredentialError> {
                let now = self.saves_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                if now > 1 {
                    self.overlap_detected.store(true, Ordering::SeqCst);
                }
                // Widen the window: a real credential write is not instantaneous.
                std::thread::sleep(std::time::Duration::from_millis(60));
                let result = self.inner.save(r#ref, token);
                self.saves_in_flight.fetch_sub(1, Ordering::SeqCst);
                result
            }
            fn load(
                &self,
                r#ref: &crate::auth::CredentialRef,
            ) -> Result<Option<Pat>, CredentialError> {
                self.inner.load(r#ref)
            }
            fn delete(&self, r#ref: &crate::auth::CredentialRef) -> Result<(), CredentialError> {
                self.inner.delete(r#ref)
            }
            fn describe(&self) -> &'static str {
                "instrumented (test)"
            }
        }

        // The transport uses a short-lived current-thread runtime internally, so
        // the mock responses are scripted up front and consumed in order.
        let store = Arc::new(InstrumentedStore::default());
        let store_trait = store.clone() as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        for _ in 0..3 {
            mock.enqueue(Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: user_body(61, "dev"),
            }))
            .await;
        }

        let (svc, _exec) = build_service(store_trait, mock);
        let svc = Arc::new(svc);
        svc.login(Pat::from_str("ghp_initial"))
            .await
            .expect("login");
        let key = svc.active_context().expect("ctx").account;

        // Two truly concurrent replacements on different worker threads.
        let (tx_a, rx_a) = tokio::sync::oneshot::channel();
        let (tx_b, rx_b) = tokio::sync::oneshot::channel();
        let a = {
            let svc = svc.clone();
            let key = key.clone();
            tokio::spawn(async move {
                let _ = tx_a.send(());
                svc.replace(key, Pat::from_str("ghp_alpha")).await
            })
        };
        let b = {
            let svc = svc.clone();
            let key = key.clone();
            tokio::spawn(async move {
                let _ = tx_b.send(());
                svc.replace(key, Pat::from_str("ghp_beta")).await
            })
        };
        // Reduce start skew so both enter the critical section as close as
        // possible; the mutex is what must keep them apart.
        let _ = rx_a.await;
        let _ = rx_b.await;
        let ra = a.await.expect("join a");
        let rb = b.await.expect("join b");

        assert!(
            !store.overlap_detected.load(Ordering::SeqCst),
            "two credential saves overlapped: the commit critical section is not serialized"
        );
        assert!(
            ra.is_ok() && rb.is_ok(),
            "both serialized replacements should succeed: {ra:?} {rb:?}"
        );

        let a_epoch = match &ra {
            Ok(LoginOutcome::Replaced(s)) => s.epoch.value(),
            _ => 0,
        };
        let b_epoch = match &rb {
            Ok(LoginOutcome::Replaced(s)) => s.epoch.value(),
            _ => 0,
        };
        let expected = if a_epoch > b_epoch {
            "ghp_alpha"
        } else {
            "ghp_beta"
        };
        let current = svc.active_context().expect("ctx");
        assert_eq!(current.epoch.value(), a_epoch.max(b_epoch));
        let stored = store
            .load(&crate::auth::CredentialRef::new(
                crate::domain::HostId::github(),
                61,
            ))
            .expect("load")
            .expect("credential present");
        assert_eq!(
            stored.expose(),
            expected,
            "store and registry must agree on the winning token"
        );
    }

    /// Regression: `remove` must be serialized with `replace`. Without it, a
    /// replacement could pass its existence recheck, removal could delete the old
    /// credential and drop the account, and the replacement could then save a new
    /// PAT that no account references — an orphaned secret with no cleanup path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn removal_is_serialized_with_replacement() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        /// Store that detects overlapping credential operations, and records
        /// whether a save ever landed with no account referring to it.
        #[derive(Default)]
        struct InstrumentedStore {
            inner: InMemoryCredentialStore,
            ops_in_flight: AtomicUsize,
            overlap_detected: AtomicBool,
        }
        impl CredentialStore for InstrumentedStore {
            fn save(
                &self,
                r#ref: &crate::auth::CredentialRef,
                token: &Pat,
            ) -> Result<(), CredentialError> {
                let now = self.ops_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                if now > 1 {
                    self.overlap_detected.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
                let result = self.inner.save(r#ref, token);
                self.ops_in_flight.fetch_sub(1, Ordering::SeqCst);
                result
            }
            fn load(
                &self,
                r#ref: &crate::auth::CredentialRef,
            ) -> Result<Option<Pat>, CredentialError> {
                self.inner.load(r#ref)
            }
            fn delete(&self, r#ref: &crate::auth::CredentialRef) -> Result<(), CredentialError> {
                let now = self.ops_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                if now > 1 {
                    self.overlap_detected.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
                let result = self.inner.delete(r#ref);
                self.ops_in_flight.fetch_sub(1, Ordering::SeqCst);
                result
            }
            fn describe(&self) -> &'static str {
                "instrumented (test)"
            }
        }

        let store = Arc::new(InstrumentedStore::default());
        let store_trait = store.clone() as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        // login + one replacement verification.
        for _ in 0..2 {
            mock.enqueue(Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: user_body(71, "dev"),
            }))
            .await;
        }

        let (svc, _exec) = build_service(store_trait, mock);
        let svc = Arc::new(svc);
        svc.login(Pat::from_str("ghp_initial"))
            .await
            .expect("login");
        let key = svc.active_context().expect("ctx").account;

        let (replace_go, replace_wait) = tokio::sync::oneshot::channel::<()>();
        let (remove_go, remove_wait) = tokio::sync::oneshot::channel::<()>();

        let replace_task = {
            let svc = svc.clone();
            let key = key.clone();
            tokio::spawn(async move {
                let _ = replace_go.send(());
                svc.replace(key, Pat::from_str("ghp_new")).await
            })
        };
        let remove_task = {
            let svc = svc.clone();
            let key = key.clone();
            tokio::spawn(async move {
                let _ = remove_go.send(());
                svc.remove(key).await
            })
        };
        let _ = replace_wait.await;
        let _ = remove_wait.await;

        let replace_result = replace_task.await.expect("join replace");
        let remove_result = remove_task.await.expect("join remove");

        assert!(
            !store.overlap_detected.load(Ordering::SeqCst),
            "remove and replace overlapped a credential operation: not serialized"
        );

        // Whichever order they serialized in, the end state must be consistent:
        // either the account exists with the new credential, or it is gone with
        // no credential left behind. Never "no account but a stored PAT".
        let account_exists = svc.summaries().iter().any(|s| s.identity.key == key);
        let credential = store
            .load(&crate::auth::CredentialRef::new(
                crate::domain::HostId::github(),
                71,
            ))
            .expect("load");
        match (
            account_exists,
            replace_result.is_ok(),
            remove_result.is_ok(),
        ) {
            (false, _, true) => assert!(
                credential.is_none(),
                "account removed by a successful remove must not leave a stored PAT"
            ),
            (true, true, _) => assert!(
                credential.is_some(),
                "account present after a successful replace must have a stored PAT"
            ),
            _ => { /* both failed in an allowed way; nothing to assert on state */ }
        }
    }

    /// Replacing an unknown account is not an implicit add.
    /// Replacing an unknown account is not an implicit add.
    #[tokio::test]
    async fn replace_unknown_account_is_not_found() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(77, "ghost"),
        }))
        .await;

        let (svc, _exec) = build_service(store.clone(), mock);
        let result = svc
            .replace(
                crate::domain::AccountKey::new(77, crate::domain::HostId::github()),
                Pat::from_str("ghp_ghost"),
            )
            .await;
        assert!(matches!(result, Err(LoginError::NotFound)));
        assert!(svc.summaries().is_empty());
    }

    /// The explicit replacement path advances the epoch on success.
    #[tokio::test]
    async fn replace_advances_the_epoch() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(11, "dev"),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(11, "dev"),
        }))
        .await;

        let (svc, _exec) = build_service(store, mock);
        let first = svc.login(Pat::from_str("ghp_a")).await.expect("login");
        let first_epoch = match first {
            LoginOutcome::SignedIn(s) => s.epoch,
            _ => panic!("expected SignedIn"),
        };

        let replaced = svc
            .replace(
                crate::domain::AccountKey::new(11, crate::domain::HostId::github()),
                Pat::from_str("ghp_b"),
            )
            .await
            .expect("replace");
        let second_epoch = match replaced {
            LoginOutcome::Replaced(s) => s.epoch,
            _ => panic!("expected Replaced"),
        };
        assert!(second_epoch.value() > first_epoch.value());
    }

    /// A stale `Unauthorized` from an old PAT must be converted to
    /// `StaleContext`, not surfaced as a failure of the new credential.
    #[tokio::test]
    async fn stale_error_after_replacement_becomes_stale_context() {
        use crate::network::transport::{HttpRequest, Transport, TransportError};
        use std::pin::Pin;

        struct ReplaceDuringReposThen401 {
            mock: Arc<MockTransport>,
            registry: Arc<std::sync::Mutex<crate::auth::AccountRegistry>>,
            store: Arc<dyn CredentialStore>,
        }

        impl Transport for ReplaceDuringReposThen401 {
            fn execute<'a>(
                &'a self,
                request: HttpRequest,
                cancel: CancellationToken,
            ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
            {
                Box::pin(async move {
                    let is_repos = request.url.contains("/user/repos");
                    let response = self.mock.execute(request, cancel).await;
                    if is_repos {
                        let key =
                            crate::domain::AccountKey::new(321, crate::domain::HostId::github());
                        let identity = crate::domain::AccountIdentity {
                            key: key.clone(),
                            login: "dev".to_string(),
                            display_name: None,
                            avatar_url: None,
                            html_url: None,
                        };
                        let credential =
                            crate::auth::CredentialRef::new(crate::domain::HostId::github(), 321);
                        let fresh = Pat::from_str("ghp_fresh");
                        self.store.save(&credential, &fresh).expect("save");
                        self.registry
                            .lock()
                            .expect("registry")
                            .replace(key, identity, credential)
                            .expect("replace");
                    }
                    response
                })
            }
        }

        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(321, "dev"),
        }))
        .await;
        // The in-flight read fails with 401 under the *old* PAT.
        mock.enqueue(Ok(HttpResponse {
            status: 401,
            headers: vec![],
            body: r#"{"message":"Bad credentials"}"#.to_string(),
        }))
        .await;

        let registry = Arc::new(std::sync::Mutex::new(
            crate::auth::AccountRegistry::default(),
        ));
        let transport = Arc::new(ReplaceDuringReposThen401 {
            mock: mock.clone(),
            registry: registry.clone(),
            store: store.clone(),
        });
        let client = Arc::new(GitHubClient::new(GitHubConfig::default(), transport));
        let exec = Arc::new(BackgroundExecutor::new(2));
        let svc = AccountService {
            registry: registry.clone(),
            store: store.clone(),
            client,
            executor: exec.clone(),
            credential_commit: Arc::new(std::sync::Mutex::new(())),
            database: Arc::new(std::sync::Mutex::new(None)),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            persistent: false,
        };

        svc.login(Pat::from_str("ghp_old")).await.expect("login");
        let result = svc.repo_service().list_repos().await;
        assert!(
            matches!(result, Err(GitHubError::StaleContext)),
            "stale 401 must become StaleContext, got {result:?}"
        );
        // The account is still present (the 401 did not remove it).
        assert_eq!(svc.summaries().len(), 1);
    }

    /// Removing an account whose credential deletion fails reports the failure
    /// instead of silently claiming a clean removal.
    #[tokio::test]
    async fn remove_reports_credential_deletion_failure() {
        #[derive(Default)]
        struct FailingDeleteStore {
            inner: InMemoryCredentialStore,
        }
        impl CredentialStore for FailingDeleteStore {
            fn save(
                &self,
                r#ref: &crate::auth::CredentialRef,
                token: &Pat,
            ) -> Result<(), CredentialError> {
                self.inner.save(r#ref, token)
            }
            fn load(
                &self,
                r#ref: &crate::auth::CredentialRef,
            ) -> Result<Option<Pat>, CredentialError> {
                self.inner.load(r#ref)
            }
            fn delete(&self, _: &crate::auth::CredentialRef) -> Result<(), CredentialError> {
                Err(CredentialError::Platform {
                    message: "injected delete failure".to_string(),
                })
            }
            fn describe(&self) -> &'static str {
                "failing-delete (test)"
            }
        }

        let store = Arc::new(FailingDeleteStore::default()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(55, "dev"),
        }))
        .await;

        let (svc, _exec) = build_service(store, mock);
        svc.login(Pat::from_str("ghp_x")).await.expect("login");
        let account = svc.active_context().expect("ctx").account;

        let result = svc.remove(account).await;
        assert!(
            matches!(result, Err(GitHubError::CredentialStore { .. })),
            "expected a reported deletion failure, got {result:?}"
        );
        // The registry entry is still dropped so nothing stale stays active.
        assert!(svc.summaries().is_empty());
    }

    #[tokio::test]
    async fn unsupported_store_prevents_login() {
        let store = Arc::new(UnsupportedStore) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(1, "u"),
        }))
        .await;

        let (svc, _exec) = build_service(store, mock);
        let result = svc.login(Pat::from_str("ghp_t")).await;
        assert!(matches!(
            result,
            Err(LoginError::CredentialSave(CredentialError::Unsupported))
        ));
    }

    /// Regression: cancelling a read that is *waiting for a concurrency permit*
    /// must return promptly instead of waiting for the in-flight request to
    /// release the semaphore.
    #[tokio::test]
    async fn cancelling_a_queued_read_returns_without_waiting_for_a_permit() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(401, "dev"),
        }))
        .await;

        // A 1-permit executor: the first read holds the only permit.
        let client = Arc::new(GitHubClient::new(GitHubConfig::default(), mock.clone()));
        let exec = Arc::new(BackgroundExecutor::new(1));
        let svc = AccountService::new(store, client, exec.clone());
        svc.login(Pat::from_str("ghp_q")).await.expect("login");
        let repo_svc = Arc::new(svc.repo_service());

        // Occupy the only permit for a while.
        let hold = exec.semaphore().acquire_owned().await.expect("permit");

        let (cancel, source) = CancellationToken::new();
        let queued = {
            let repo_svc = repo_svc.clone();
            tokio::spawn(async move { repo_svc.list_repos_cancellable(cancel).await })
        };

        // Give the queued read time to reach (and block on) the semaphore.
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert!(
            !queued.is_finished(),
            "read should be queued on the semaphore"
        );

        source.cancel();

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), queued)
            .await
            .expect("a cancelled queued read must not wait for the permit")
            .expect("join");
        assert!(
            matches!(outcome, Err(GitHubError::Cancelled)),
            "expected Cancelled, got {outcome:?}"
        );
        drop(hold);
    }

    /// A response that arrives after the active account was replaced must be
    /// discarded, not written into the new account's data.
    ///
    /// A scripted transport performs the PAT replacement *while* the repos
    /// request is being answered, which is deterministic and models the real
    /// "user replaced the token while a read was in flight" ordering.
    #[tokio::test]
    async fn stale_response_after_account_replacement_is_discarded() {
        use crate::network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
        use std::pin::Pin;

        struct ReplaceDuringRepos {
            mock: Arc<MockTransport>,
            registry: Arc<std::sync::Mutex<crate::auth::AccountRegistry>>,
            store: Arc<dyn CredentialStore>,
        }

        impl Transport for ReplaceDuringRepos {
            fn execute<'a>(
                &'a self,
                request: HttpRequest,
                cancel: CancellationToken,
            ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
            {
                Box::pin(async move {
                    let is_repos = request.url.contains("/user/repos");
                    let response = self.mock.execute(request, cancel).await;
                    if is_repos {
                        // Replace the PAT before this response is returned to
                        // the caller: the epoch advances mid-flight.
                        let identity = crate::domain::AccountIdentity {
                            key: crate::domain::AccountKey::new(
                                500,
                                crate::domain::HostId::github(),
                            ),
                            login: "account_a".to_string(),
                            display_name: None,
                            avatar_url: None,
                            html_url: None,
                        };
                        let credential =
                            crate::auth::CredentialRef::new(crate::domain::HostId::github(), 500);
                        let fresh = Pat::from_str("ghp_a2");
                        self.store.save(&credential, &fresh).expect("save");
                        let mut reg = self.registry.lock().expect("registry");
                        reg.replace(
                            crate::domain::AccountKey::new(500, crate::domain::HostId::github()),
                            identity,
                            credential,
                        )
                        .expect("replace");
                    }
                    response
                })
            }
        }

        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(500, "account_a"),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: repos_body(&[1, 2]),
        }))
        .await;

        let registry = Arc::new(std::sync::Mutex::new(
            crate::auth::AccountRegistry::default(),
        ));
        let transport = Arc::new(ReplaceDuringRepos {
            mock: mock.clone(),
            registry: registry.clone(),
            store: store.clone(),
        });
        let client = Arc::new(GitHubClient::new(GitHubConfig::default(), transport));
        let exec = Arc::new(BackgroundExecutor::new(2));
        let svc = AccountService {
            registry: registry.clone(),
            store: store.clone(),
            client,
            executor: exec.clone(),
            credential_commit: Arc::new(std::sync::Mutex::new(())),
            database: Arc::new(std::sync::Mutex::new(None)),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            persistent: false,
        };

        svc.login(Pat::from_str("ghp_a1")).await.expect("login A");
        let repo_svc = svc.repo_service();
        let outcome = repo_svc.list_repos().await;

        assert!(
            matches!(outcome, Err(GitHubError::StaleContext)),
            "expected StaleContext, got {outcome:?}"
        );
        // The returned error carries no repository data from the old epoch.
        assert!(outcome.is_err());
    }

    /// Removing the active account while a read is in flight must also discard
    /// the result.
    #[tokio::test]
    async fn stale_response_after_removal_is_discarded() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(700, "gone"),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: repos_body(&[9]),
        }))
        .await;

        let (svc, _exec) = build_service(store, mock);
        svc.login(Pat::from_str("ghp_rm")).await.expect("login");
        let account = svc.active_context().expect("ctx").account;
        let repo_svc = svc.repo_service();

        let read = tokio::spawn(async move { repo_svc.list_repos().await });
        tokio::task::yield_now().await;
        svc.remove(account).await.expect("remove");

        let outcome = read.await.expect("join");
        assert!(
            matches!(outcome, Err(GitHubError::StaleContext)),
            "expected StaleContext, got {outcome:?}"
        );
    }

    /// Cancelling one in-flight read must not cancel an independent read that
    /// targets the same service.
    #[tokio::test]
    async fn cancelling_one_read_does_not_affect_another() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(800, "dev"),
        }))
        .await;
        // The cancelled read returns before it reaches the transport, so it
        // consumes no scripted response. Only the independent read is served.
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: repos_body(&[2, 3, 4]),
        }))
        .await;

        let (svc, _exec) = build_service(store, mock);
        svc.login(Pat::from_str("ghp_c")).await.expect("login");
        let repo_svc = svc.repo_service();

        let (cancel, source) = CancellationToken::new();
        source.cancel();
        let cancelled = repo_svc.list_repos_cancellable(cancel).await;
        assert!(matches!(cancelled, Err(GitHubError::Cancelled)));

        let ok = repo_svc.list_repos().await.expect("independent read");
        assert_eq!(ok.repositories.len(), 3);
    }

    /// A rate-limited response must not remove the account or destroy the token.
    #[tokio::test]
    async fn rate_limited_read_keeps_account_and_credential() {
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(900, "dev"),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 403,
            headers: vec![
                ("x-ratelimit-remaining".to_string(), "0".to_string()),
                ("retry-after".to_string(), "60".to_string()),
            ],
            body: r#"{"message":"API rate limit exceeded"}"#.to_string(),
        }))
        .await;

        let (svc, _exec) = build_service(store.clone(), mock);
        svc.login(Pat::from_str("ghp_rl")).await.expect("login");
        let repo_svc = svc.repo_service();

        let outcome = repo_svc.list_repos().await;
        assert!(matches!(outcome, Err(GitHubError::RateLimited { .. })));
        // Account survives and its credential is still present.
        let summaries = svc.summaries();
        assert_eq!(summaries.len(), 1);
        let credential_ref = crate::auth::CredentialRef::new(crate::domain::HostId::github(), 900);
        assert!(
            store.load(&credential_ref).expect("load").is_some(),
            "token must not be deleted on rate limit"
        );
    }

    /// Full end-to-end against real localhost sockets: login via `GET /user`,
    /// then read two pages of `/user/repos` following a real `Link` header. No
    /// GitHub network, no real credentials.
    #[tokio::test]
    async fn localhost_end_to_end_login_and_paginated_repos() {
        use crate::network::test_server::{LocalHttpServer, ScriptedResponse};

        let server = Arc::new(LocalHttpServer::start(vec![]));
        let page2 = format!("{}user/repos?page=2", server.base_url());
        server.script(vec![
            ScriptedResponse::json(
                200,
                &serde_json::json!({
                    "id": 31337, "login": "localmock", "name": "Local Mock",
                    "avatar_url": "https://example.invalid/a.png",
                    "html_url": "https://example.invalid/localmock"
                })
                .to_string(),
            ),
            ScriptedResponse::json(
                200,
                &serde_json::json!([{
                    "id": 1, "name": "alpha", "full_name": "localmock/alpha",
                    "owner": {"id": 31337, "login": "localmock"},
                    "visibility": "public", "fork": false, "archived": false,
                    "default_branch": "main"
                }])
                .to_string(),
            )
            .with_header("Link", &format!("<{page2}>; rel=\"next\"")),
            ScriptedResponse::json(
                200,
                &serde_json::json!([{
                    "id": 2, "name": "beta", "full_name": "localmock/beta",
                    "owner": {"id": 31337, "login": "localmock"},
                    "visibility": "private", "fork": true, "archived": false
                }])
                .to_string(),
            ),
        ]);

        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let transport = Arc::new(
            crate::network::transport::ReqwestTransport::new(std::time::Duration::from_secs(5))
                .expect("client"),
        );
        let config = GitHubConfig {
            base_url: server.base_url().to_string(),
            ..Default::default()
        };
        let client = Arc::new(GitHubClient::new(config, transport));
        let exec = Arc::new(BackgroundExecutor::new(2));
        let svc = AccountService::new(store, client, exec);

        svc.login(Pat::from_str("ghp_local_e2e"))
            .await
            .expect("login");
        let summaries = svc.summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].identity.key.id, 31337);
        assert_eq!(summaries[0].identity.login, "localmock");

        let outcome = svc.repo_service().list_repos().await.expect("repos");
        assert_eq!(outcome.repositories.len(), 2);
        assert_eq!(outcome.readiness, crate::domain::ReposReadiness::Complete);
        assert_eq!(outcome.repositories[0].full_name, "localmock/alpha");
        assert_eq!(
            outcome.repositories[1].visibility,
            crate::domain::RepoVisibility::Private
        );
        assert!(outcome.repositories[1].fork);

        // Every request carried the Authorization header, and affiliation was
        // requested on the first page.
        let recorded = server.recorded();
        assert_eq!(recorded.len(), 3);
        for request in &recorded {
            assert_eq!(
                request.header("authorization"),
                Some("Bearer ghp_local_e2e"),
                "request {:?} must carry Authorization",
                request.request_line
            );
        }
        assert!(
            recorded[1]
                .request_line
                .contains("affiliation=owner%2Ccollaborator%2Corganization_member")
                || recorded[1]
                    .request_line
                    .contains("affiliation=owner,collaborator,organization_member"),
            "first repos page must request the three affiliations, got {}",
            recorded[1].request_line
        );
    }

    /// A mid-pagination failure over real sockets yields a partial outcome, not
    /// a fake complete empty list.
    #[tokio::test]
    async fn localhost_partial_page_failure_is_partial() {
        use crate::network::test_server::{LocalHttpServer, ScriptedResponse};

        let server = Arc::new(LocalHttpServer::start(vec![]));
        let page2 = format!("{}user/repos?page=2", server.base_url());
        server.script(vec![
            ScriptedResponse::json(
                200,
                &serde_json::json!({
                    "id": 4242, "login": "localmock",
                    "avatar_url": "https://example.invalid/a.png",
                    "html_url": "https://example.invalid/localmock"
                })
                .to_string(),
            ),
            ScriptedResponse::json(
                200,
                &serde_json::json!([{
                    "id": 1, "name": "alpha", "full_name": "localmock/alpha",
                    "owner": {"id": 4242, "login": "localmock"},
                    "visibility": "public"
                }])
                .to_string(),
            )
            .with_header("Link", &format!("<{page2}>; rel=\"next\"")),
            ScriptedResponse::json(503, r#"{"message":"unavailable"}"#),
        ]);

        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let transport = Arc::new(
            crate::network::transport::ReqwestTransport::new(std::time::Duration::from_secs(5))
                .expect("client"),
        );
        let config = GitHubConfig {
            base_url: server.base_url().to_string(),
            ..Default::default()
        };
        let client = Arc::new(GitHubClient::new(config, transport));
        let exec = Arc::new(BackgroundExecutor::new(2));
        let svc = AccountService::new(store, client, exec);

        svc.login(Pat::from_str("ghp_local_partial"))
            .await
            .expect("login");
        let outcome = svc.repo_service().list_repos().await.expect("outcome");
        assert_eq!(outcome.repositories.len(), 1);
        assert!(matches!(
            outcome.readiness,
            crate::domain::ReposReadiness::Partial { .. }
        ));
    }
}
