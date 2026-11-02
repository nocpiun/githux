//! Deterministic service-chain tests for Star/Unstar. The real AccountService,
//! RepositorySession and request gates run over a controlled transport; no real
//! GitHub repository or credential is touched.

use super::repository_actions::RepositoryActionService;
use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::domain::repository::{RepositoryError, RepositoryIdentity, RepositoryLocator};
use crate::domain::repository_actions::*;
use crate::network::transport::{
    HttpMethod, HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use serde_json::json;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;

#[derive(Default)]
struct ActionTransport {
    responses: Mutex<VecDeque<Result<HttpResponse, MockFailure>>>,
    requests: Mutex<Vec<HttpRequest>>,
}

#[derive(Debug, Clone)]
enum MockFailure {
    Timeout,
    Network,
}

impl ActionTransport {
    fn push(&self, status: u16, body: serde_json::Value) {
        self.responses
            .lock()
            .unwrap()
            .push_back(Ok(json_response(status, &body.to_string())));
    }

    fn push_empty(&self, status: u16) {
        self.push(status, json!({}));
    }

    fn fail(&self, failure: MockFailure) {
        self.responses.lock().unwrap().push_back(Err(failure));
    }

    fn methods(&self) -> Vec<HttpMethod> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.method)
            .collect()
    }

    fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.url.clone())
            .collect()
    }
}

impl Transport for ActionTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            self.requests.lock().unwrap().push(request);
            match self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted action response")
            {
                Ok(response) => Ok(response),
                Err(MockFailure::Timeout) => Err(TransportError::Timeout { seconds: 1 }),
                Err(MockFailure::Network) => Err(TransportError::Network {
                    message: "injected disconnect".into(),
                }),
            }
        })
    }

    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max_body_bytes {
                return Err(TransportError::ResponseTooLarge {
                    limit: max_body_bytes,
                });
            }
            Ok(response)
        })
    }
}

fn details(stars: u64) -> serde_json::Value {
    json!({
        "id": 42, "name": "ledger", "full_name": "octo/ledger",
        "owner": {"id": 7, "login": "octo", "avatar_url": null},
        "description": "Fixture", "visibility": "public", "private": false,
        "default_branch": "main", "fork": false, "archived": false,
        "stargazers_count": stars, "forks_count": 2, "open_issues_count": 0,
        "subscribers_count": 1, "size": 1, "html_url": "https://github.com/octo/ledger",
        "clone_url": "https://github.com/octo/ledger.git", "pushed_at": "2026-11-01T00:00:00Z"
    })
}

fn repository() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 42,
        locator: RepositoryLocator::new("octo", "ledger").unwrap(),
    }
}

async fn service() -> (
    tempfile::TempDir,
    Arc<ActionTransport>,
    Arc<AccountService>,
    Arc<BackgroundExecutor>,
) {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(ActionTransport::default());
    transport.push(200, json!({"id": 1, "login": "alice"}));
    let executor = Arc::new(BackgroundExecutor::new(4));
    let accounts = Arc::new(
        AccountService::open(
            crate::storage::accounts::AccountDatabaseConfig::at(temp.path().join("accounts.db")),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(Default::default(), transport.clone())),
            executor.clone(),
        )
        .await
        .unwrap(),
    );
    accounts
        .login(Pat::from_str("synthetic-action-token"))
        .await
        .unwrap();
    (temp, transport, accounts, executor)
}

fn actions(accounts: &AccountService) -> RepositoryActionService {
    let session = accounts.repo_service().session().unwrap();
    RepositoryActionService::new(&session, accounts).unwrap()
}

#[tokio::test]
async fn initial_starred_and_unstarred_use_the_real_status_endpoint() {
    let (_temp, transport, accounts, _executor) = service().await;
    let actions = actions(&accounts);
    for (status, expected) in [(204, StarredState::Starred), (404, StarredState::Unstarred)] {
        transport.push_empty(status);
        let result = actions
            .starred(&repository(), Default::default())
            .await
            .unwrap();
        assert_eq!(result.value.state, expected);
        assert_eq!(result.value.repository.id(), 42);
        assert_eq!(result.context, *actions.context());
    }
    assert!(
        transport
            .paths()
            .iter()
            .skip(1)
            .all(|url| { url == "https://api.github.com/user/starred/octo/ledger" })
    );
}

#[tokio::test]
async fn both_204_writes_refresh_the_authoritative_count_without_local_arithmetic() {
    let (_temp, transport, accounts, _executor) = service().await;
    let actions = actions(&accounts);

    transport.push_empty(204); // PUT
    transport.push(200, details(129));
    let starred = actions
        .set_starred(
            &repository(),
            StarredState::Starred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(starred.observation.state, StarredState::Starred);
    assert_eq!(starred.confirmation, StarConfirmation::Written);
    assert_eq!(starred.summary.unwrap().stars, 129);

    transport.push_empty(204); // DELETE
    transport.push(200, details(128));
    let unstarred = actions
        .set_starred(
            &repository(),
            StarredState::Unstarred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(unstarred.observation.state, StarredState::Unstarred);
    assert_eq!(unstarred.summary.unwrap().stars, 128);
    assert_eq!(
        transport.methods(),
        vec![
            HttpMethod::Get, // login
            HttpMethod::Put,
            HttpMethod::Get, // summary
            HttpMethod::Delete,
            HttpMethod::Get, // summary
        ]
    );
}

#[tokio::test]
async fn rejected_and_rate_limited_writes_are_not_confirmed_or_refreshed() {
    let (_temp, transport, accounts, _executor) = service().await;
    let actions = actions(&accounts);
    transport.push_empty(403);
    let error = actions
        .set_starred(
            &repository(),
            StarredState::Starred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error, RepositoryActionError::GitHub(GitHubError::Forbidden));

    transport
        .responses
        .lock()
        .unwrap()
        .push_back(Ok(HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "12".into())],
            body: String::new(),
        }));
    let error = actions
        .set_starred(
            &repository(),
            StarredState::Starred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RepositoryActionError::GitHub(GitHubError::RateLimited { retry })
            if retry.seconds == Some(12)
    ));
    assert_eq!(
        transport.methods(),
        vec![HttpMethod::Get, HttpMethod::Put, HttpMethod::Put]
    );
}

#[tokio::test]
async fn timeout_that_actually_applied_is_reconciled_without_replaying_put() {
    let (_temp, transport, accounts, _executor) = service().await;
    let actions = actions(&accounts);
    transport.fail(MockFailure::Timeout);
    transport.push_empty(204); // reconciliation proves starred
    transport.push(200, details(130));
    let outcome = actions
        .set_starred(
            &repository(),
            StarredState::Starred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(outcome.confirmation, StarConfirmation::Reconciled);
    assert_eq!(outcome.observation.state, StarredState::Starred);
    assert_eq!(outcome.summary.unwrap().stars, 130);
    assert_eq!(
        transport.methods(),
        vec![
            HttpMethod::Get,
            HttpMethod::Put,
            HttpMethod::Get,
            HttpMethod::Get
        ]
    );
}

#[tokio::test]
async fn disconnect_that_did_not_apply_is_observed_honestly_without_replaying_delete() {
    let (_temp, transport, accounts, _executor) = service().await;
    let actions = actions(&accounts);
    transport.fail(MockFailure::Network);
    transport.push_empty(404); // reconciliation proves still unstarred
    transport.push(200, details(128));
    let outcome = actions
        .set_starred(
            &repository(),
            StarredState::Unstarred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(outcome.confirmation, StarConfirmation::Reconciled);
    assert_eq!(outcome.observation.state, StarredState::Unstarred);
    assert_eq!(outcome.summary.unwrap().stars, 128);
    assert_eq!(
        transport.methods(),
        vec![
            HttpMethod::Get,
            HttpMethod::Delete,
            HttpMethod::Get,
            HttpMethod::Get
        ]
    );
}

#[tokio::test]
async fn unknown_outcome_is_explicit_and_never_replays_the_write() {
    let (_temp, transport, accounts, _executor) = service().await;
    let actions = actions(&accounts);
    transport.fail(MockFailure::Timeout);
    transport.fail(MockFailure::Timeout);
    let error = actions
        .set_starred(
            &repository(),
            StarredState::Starred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RepositoryActionError::UnknownOutcome {
            write: GitHubError::Timeout { .. },
            reconciliation: GitHubError::Timeout { .. }
        }
    ));
    assert_eq!(
        transport.methods(),
        vec![HttpMethod::Get, HttpMethod::Put, HttpMethod::Get]
    );
}

#[tokio::test]
async fn count_failure_does_not_undo_a_server_confirmed_star() {
    let (_temp, transport, accounts, _executor) = service().await;
    let actions = actions(&accounts);
    transport.push_empty(204);
    transport.push_empty(503);
    let outcome = actions
        .set_starred(
            &repository(),
            StarredState::Starred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(outcome.observation.state, StarredState::Starred);
    assert_eq!(outcome.confirmation, StarConfirmation::Written);
    assert!(matches!(
        outcome.summary,
        Err(RepositoryError::GitHub(GitHubError::UnexpectedStatus {
            status: 503
        }))
    ));
}

#[tokio::test]
async fn account_a_b_a_invalidates_the_old_action_session_before_any_write() {
    let (_temp, transport, accounts, _executor) = service().await;
    let old = actions(&accounts);
    transport.push(200, json!({"id": 2, "login": "bob"}));
    accounts.login(Pat::from_str("synthetic-b")).await.unwrap();
    transport.push(200, json!({"id": 1, "login": "alice"}));
    accounts
        .login(Pat::from_str("synthetic-a-new"))
        .await
        .unwrap();

    let error = old
        .set_starred(
            &repository(),
            StarredState::Starred,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error,
        RepositoryActionError::GitHub(GitHubError::StaleContext)
    );
    assert_eq!(transport.methods(), vec![HttpMethod::Get; 3]);
}
