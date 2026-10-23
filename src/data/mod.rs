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

use crate::auth::credential::{CredentialError, CredentialRef, CredentialStore};
use crate::auth::token::Pat;
use crate::auth::{AccountRegistry, AuthContext, RegistryError};
use crate::domain::AccountSummary;
use crate::domain::error::GitHubError;
use crate::domain::repo::ReposOutcome;
use crate::github::client::GitHubClient;
use crate::github::repos::fetch_user_repos;
use crate::github::user::{AuthenticatedUser, fetch_authenticated_user};
use crate::network::cancel::CancellationToken;
use crate::network::executor::BackgroundExecutor;
use std::sync::Arc;

/// Account and credential lifecycle service.
pub struct AccountService {
    registry: Arc<std::sync::Mutex<AccountRegistry>>,
    store: Arc<dyn CredentialStore>,
    client: Arc<GitHubClient>,
    executor: Arc<BackgroundExecutor>,
}

/// Repository read service scoped to the active account.
pub struct RepoService {
    registry: Arc<std::sync::Mutex<AccountRegistry>>,
    store: Arc<dyn CredentialStore>,
    client: Arc<GitHubClient>,
    executor: Arc<BackgroundExecutor>,
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
        }
    }

    pub fn registry_handle(&self) -> Arc<std::sync::Mutex<AccountRegistry>> {
        self.registry.clone()
    }

    pub fn repo_service(&self) -> RepoService {
        RepoService {
            registry: self.registry.clone(),
            store: self.store.clone(),
            client: self.client.clone(),
            executor: self.executor.clone(),
        }
    }

    /// Verify a PAT via `GET /user`, then persist the credential. If credential
    /// storage fails, login is **not** reported as persisted.
    pub async fn login(&self, pat: Pat) -> Result<LoginOutcome, LoginError> {
        let cancel = CancellationToken::default();
        let _permit = self
            .executor
            .semaphore()
            .acquire_owned()
            .await
            .map_err(|_| {
                LoginError::GitHub(GitHubError::CredentialStore {
                    message: "request scheduler closed".to_string(),
                })
            })?;
        let verified: AuthenticatedUser =
            fetch_authenticated_user(&self.client, &pat, cancel).await?;
        let credential =
            CredentialRef::new(verified.identity.key.host.clone(), verified.identity.key.id);

        // Save credential before registering the account so a storage failure
        // never leaves a registry entry claiming persistence.
        self.store
            .save(&credential, &pat)
            .map_err(LoginError::from)?;

        let outcome = {
            let mut reg = self.registry.lock().expect("registry lock");
            let existing = reg.credential_ref(verified.identity.key.clone()).is_some();
            if existing {
                let summary = reg
                    .replace(
                        verified.identity.key.clone(),
                        verified.identity.clone(),
                        credential,
                    )
                    .map_err(|_| LoginError::NotFound)?;
                LoginOutcome::Replaced(summary)
            } else {
                let summary = reg.upsert_verified(verified.identity.clone(), credential);
                LoginOutcome::SignedIn(summary)
            }
        };
        Ok(outcome)
    }

    pub async fn remove(&self, account: crate::domain::AccountKey) -> Result<(), GitHubError> {
        let credential = {
            let reg = self.registry.lock().expect("registry lock");
            reg.credential_ref(account.clone())
        };
        if let Some(cred) = credential {
            let _ = self.store.delete(&cred);
        }
        let result = {
            let mut reg = self.registry.lock().expect("registry lock");
            reg.remove(account)
        };
        match result {
            Ok(()) => Ok(()),
            Err(RegistryError::NotFound) => Err(GitHubError::CredentialStore {
                message: "account not found".to_string(),
            }),
            Err(_) => Err(GitHubError::CredentialStore {
                message: "registry error".to_string(),
            }),
        }
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
        let (context, credential) = {
            let reg = self.registry.lock().expect("registry lock");
            let context = reg.active_context().ok_or(GitHubError::StaleContext)?;
            let credential = reg
                .credential_ref(context.account.clone())
                .ok_or(GitHubError::StaleContext)?;
            (context, credential)
        };

        let token = self
            .store
            .load(&credential)
            .map_err(|e| GitHubError::CredentialStore {
                message: e.to_string(),
            })?
            .ok_or(GitHubError::CredentialStore {
                message: "credential missing".to_string(),
            })?;

        // Bound simultaneous GitHub reads through the executor's shared
        // semaphore so a burst of pages/accounts cannot open unbounded sockets.
        let _permit = self
            .executor
            .semaphore()
            .acquire_owned()
            .await
            .map_err(|_| GitHubError::CredentialStore {
                message: "request scheduler closed".to_string(),
            })?;

        let outcome = fetch_user_repos(&self.client, &token, cancel).await?;

        // Re-check the context: if the account was switched/replaced/removed
        // during the request, discard the result.
        let current = {
            let reg = self.registry.lock().expect("registry lock");
            reg.active_context()
        };
        if current.as_ref() != Some(&context) {
            return Err(GitHubError::StaleContext);
        }

        Ok(outcome)
    }

    /// Explicitly select the active account (future multi-account UI hook).
    pub fn set_active(&self, account: crate::domain::AccountKey) -> Result<(), RegistryError> {
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
        // First repos read (will be cancelled before its response is consumed).
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: repos_body(&[1]),
        }))
        .await;
        // Second repos read (independent, must succeed).
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
