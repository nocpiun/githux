//! Account isolation, cache snapshot, no-store, revocation/missing invalidation and
//! transient-error retention tests for `IssueDetailSession`.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::auth::{InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::domain::{AccountKey, HostId};
use crate::github::GitHubClient;
use crate::network::{
    executor::BackgroundExecutor,
    transport::{HttpRequest, HttpResponse, Transport, TransportError, json_response},
};
use crate::storage::{
    accounts::AccountDatabaseConfig,
    cache::{CacheConfig, CacheStore},
};

struct ScriptedTransport {
    routes: Mutex<HashMap<String, Vec<HttpResponse>>>,
    default_issue_body: Mutex<String>,
    gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

impl ScriptedTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            routes: Mutex::new(HashMap::new()),
            default_issue_body: Mutex::new("Initial body".into()),
            gate: Mutex::new(None),
        })
    }

    fn set_gate(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        resume: tokio::sync::oneshot::Receiver<()>,
    ) {
        *self.gate.lock().unwrap() = Some((entered, resume));
    }

    fn push(&self, path: &str, response: HttpResponse) {
        self.routes
            .lock()
            .unwrap()
            .entry(path.to_string())
            .or_default()
            .push(response);
    }

    fn set_default_body(&self, body: &str) {
        *self.default_issue_body.lock().unwrap() = body.to_string();
    }
}

impl Transport for ScriptedTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let url = reqwest::Url::parse(&request.url).unwrap();
            if url.path() == "/user" {
                let second = request
                    .headers
                    .iter()
                    .any(|(_, value)| value.ends_with("account-b"));
                return Ok(json_response(
                    200,
                    &json!({
                        "id": if second { 2 } else { 1 },
                        "login": if second { "account-b" } else { "account-a" }
                    })
                    .to_string(),
                ));
            }
            if let Some(queue) = self.routes.lock().unwrap().get_mut(url.path())
                && !queue.is_empty()
            {
                return Ok(queue.remove(0));
            }
            let gate = self.gate.lock().unwrap().take();
            if let Some((entered, resume)) = gate {
                let _ = entered.send(());
                let _ = resume.await;
            }
            if url.path().starts_with("/repos/acme/") && !url.path().contains("/issues/") {
                let name = url.path().rsplit('/').next().unwrap();
                let id = match name {
                    "one" => 10,
                    "two" => 20,
                    _ => 30,
                };
                return Ok(json_response(200, &json!({
                    "id": id, "name": name, "owner": {"id": 1, "login": "acme"}, "private": true,
                    "fork": false, "archived": false
                }).to_string()));
            }
            if url.path().ends_with("/comments") {
                return Ok(json_response(200, "[]"));
            }
            let body = self.default_issue_body.lock().unwrap().clone();
            let number = url
                .path()
                .rsplit('/')
                .next()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            Ok(json_response(
                200,
                &json!({
                    "id": 100,
                    "number": number,
                    "title": "Sample issue",
                    "state": "open",
                    "user": { "id": 1, "login": "account-a" },
                    "body": body,
                    "comments": 0,
                    "labels": [],
                    "created_at": "2026-11-01T10:00:00Z",
                    "updated_at": "2026-11-01T10:00:00Z"
                })
                .to_string(),
            ))
        })
    }

    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max {
                return Err(TransportError::ResponseTooLarge { limit: max });
            }
            Ok(response)
        })
    }
}

fn response_with_headers(status: u16, body: &str, headers: &[(&str, &str)]) -> HttpResponse {
    HttpResponse {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.to_string(),
    }
}

async fn open_with_cache(
    temp: &tempfile::TempDir,
    transport: Arc<ScriptedTransport>,
) -> AccountService {
    let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
    let executor = Arc::new(BackgroundExecutor::new(4));
    let client = Arc::new(GitHubClient::new(Default::default(), transport));
    let service = AccountService::open_with_cache(
        AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
        Arc::new(InMemoryCredentialStore::new()),
        client,
        executor,
        cache,
    )
    .await
    .unwrap();
    service
        .login(Pat::new("ghp_account-a".to_string()))
        .await
        .unwrap();
    service
}

async fn read_issue(
    session: &IssueDetailSession,
    locator: &IssueLocator,
) -> Result<LiveIssue, IssueDetailError> {
    session
        .refresh_issue(
            locator,
            &IssueDetailLimits::default(),
            CancellationToken::default(),
        )
        .await
        .result
}

async fn cached(session: &IssueDetailSession, locator: &IssueLocator) -> Option<CachedReading> {
    session
        .cached(
            locator,
            &IssueDetailLimits::default(),
            &CancellationToken::default(),
        )
        .await
}

async fn first_comments(
    session: &IssueDetailSession,
    locator: &IssueLocator,
    live: &LiveIssue,
) -> Refresh<LiveComments> {
    session
        .refresh_comments(
            locator,
            live.scope.as_ref(),
            None,
            None,
            &IssueDetailLimits::default(),
            CancellationToken::default(),
            true,
        )
        .await
}

#[tokio::test]
async fn caches_complete_comments_and_isolates_account_repository_and_selection() {
    let temp = tempfile::tempdir().unwrap();
    let transport = ScriptedTransport::new();
    let service = open_with_cache(&temp, transport.clone()).await;
    let repos = service.repo_service();
    let session = repos.issue_detail_session().unwrap();
    let one = IssueLocator::new("acme", "one", 42).unwrap();
    let two = IssueLocator::new("acme", "two", 42).unwrap();
    let alias = IssueLocator::new("ACME", "ONE", 42).unwrap();

    let live = read_issue(&session, &one).await.unwrap();
    let saved = cached(&session, &alias).await.unwrap();
    assert_eq!(saved.scope.repository, 10);
    assert_eq!(saved.issue.body.as_deref(), Some("Initial body"));
    assert!(cached(&session, &two).await.is_none());
    first_comments(&session, &one, &live).await.result.unwrap();
    let empty = cached(&session, &one).await.unwrap().comments.unwrap();
    assert!(empty.comments.is_empty());
    assert!(!empty.partial);

    service
        .login(Pat::from_str("synthetic-account-b"))
        .await
        .unwrap();
    assert!(!session.verify());
    assert!(
        cached(&repos.issue_detail_session().unwrap(), &one)
            .await
            .is_none()
    );
    service
        .set_active(AccountKey::new(1, HostId::github()))
        .await
        .unwrap();
    assert!(
        !session.verify(),
        "switching back must not revive the old session"
    );
    assert!(
        cached(&repos.issue_detail_session().unwrap(), &one)
            .await
            .is_some()
    );
}

#[tokio::test]
async fn issue_and_comments_no_store_and_denial_invalidate_the_corresponding_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let transport = ScriptedTransport::new();
    let service = open_with_cache(&temp, transport.clone()).await;
    let session = service.repo_service().issue_detail_session().unwrap();
    let target = IssueLocator::new("acme", "widgets", 42).unwrap();
    let path = "/repos/acme/widgets/issues/42";
    let comments = "/repos/acme/widgets/issues/42/comments";

    let live = read_issue(&session, &target).await.unwrap();
    first_comments(&session, &target, &live)
        .await
        .result
        .unwrap();
    transport.push(path, json_response(503, "{}"));
    assert!(
        read_issue(&session, &target)
            .await
            .unwrap_err()
            .is_transient()
    );
    assert!(cached(&session, &target).await.unwrap().comments.is_some());

    transport.push(
        comments,
        response_with_headers(503, "{}", &[("cache-control", "no-store")]),
    );
    let attempt = first_comments(&session, &target, &live).await;
    assert!(attempt.no_store && attempt.result.unwrap_err().is_transient());
    assert!(cached(&session, &target).await.unwrap().comments.is_none());

    first_comments(&session, &target, &live)
        .await
        .result
        .unwrap();
    transport.push(
        path,
        response_with_headers(503, "{}", &[("cache-control", "no-store")]),
    );
    assert!(
        read_issue(&session, &target)
            .await
            .unwrap_err()
            .is_transient()
    );
    assert!(cached(&session, &target).await.is_none());

    let live = read_issue(&session, &target).await.unwrap();
    transport.push(comments, json_response(403, "{}"));
    assert!(
        first_comments(&session, &target, &live)
            .await
            .result
            .unwrap_err()
            .is_auth_or_permission()
    );
    assert!(cached(&session, &target).await.is_none());

    read_issue(&session, &target).await.unwrap();
    transport.push(
        path,
        response_with_headers(
            200,
            &json!({
                "id": 100, "number": 42, "title": "Sample issue", "state": "open",
                "body": "Do not retain", "created_at": "", "updated_at": ""
            })
            .to_string(),
            &[("cache-control", "no-store")],
        ),
    );
    let live = read_issue(&session, &target).await.unwrap();
    assert_eq!(live.issue.body.as_deref(), Some("Do not retain"));
    assert!(live.scope.is_none());
    assert!(cached(&session, &target).await.is_none());
}

#[tokio::test]
async fn in_flight_issue_cannot_restore_a_snapshot_after_concurrent_denial() {
    let temp = tempfile::tempdir().unwrap();
    let transport = ScriptedTransport::new();
    let service = open_with_cache(&temp, transport.clone()).await;
    let session = service.repo_service().issue_detail_session().unwrap();
    let locator = IssueLocator::new("acme", "widgets", 99).unwrap();
    read_issue(&session, &locator).await.unwrap();

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    transport.set_gate(entered_tx, resume_rx);
    let pending = tokio::spawn({
        let session = session.clone();
        let locator = locator.clone();
        async move { read_issue(&session, &locator).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), entered_rx)
        .await
        .unwrap()
        .unwrap();
    transport.push("/repos/acme/widgets/issues/99", json_response(404, "{}"));
    assert!(
        read_issue(&session, &locator)
            .await
            .unwrap_err()
            .is_missing()
    );
    resume_tx.send(()).unwrap();
    pending.await.unwrap().unwrap();
    assert!(cached(&session, &locator).await.is_none());
}

#[tokio::test]
async fn repository_revocation_and_credential_rotation_require_fresh_authorized_reads() {
    let temp = tempfile::tempdir().unwrap();
    let transport = ScriptedTransport::new();
    let service = open_with_cache(&temp, transport.clone()).await;
    let repos = service.repo_service();
    let session = repos.issue_detail_session().unwrap();
    let target = IssueLocator::new("acme", "two", 2).unwrap();
    read_issue(&session, &target).await.unwrap();
    let old = cached(&session, &target).await.unwrap();
    let cache = repos.cache(session.context()).unwrap();
    cache
        .store()
        .revoke_repository(cache.scope(), Some(20))
        .unwrap();
    assert!(!old.authorization.is_current());
    assert!(cached(&session, &target).await.is_none());
    read_issue(&session, &target).await.unwrap();
    let before_rotation = cached(&session, &target).await.unwrap();

    service
        .replace(
            AccountKey::new(1, HostId::github()),
            Pat::from_str("synthetic-issue-replacement"),
        )
        .await
        .unwrap();
    let replacement = repos.issue_detail_session().unwrap();
    assert!(!session.verify());
    assert!(!before_rotation.authorization.is_current());
    assert!(cached(&replacement, &target).await.is_none());
    transport.push("/repos/acme/two", json_response(503, "{}"));
    assert!(
        read_issue(&replacement, &target)
            .await
            .unwrap()
            .scope
            .is_none()
    );
    assert!(cached(&replacement, &target).await.is_none());
    transport.set_default_body("Read under replacement credentials");
    let live = read_issue(&replacement, &target).await.unwrap();
    assert!(
        live.scope.is_some(),
        "fresh metadata must unlock the numeric repository gate"
    );
    first_comments(&replacement, &target, &live)
        .await
        .result
        .unwrap();
    let saved = cached(&replacement, &target).await.unwrap();
    assert_eq!(
        saved.issue.body.as_deref(),
        Some("Read under replacement credentials")
    );
    assert!(saved.comments.is_some());

    transport.push("/repos/acme/one/issues/1", json_response(401, "{}"));
    let other = IssueLocator::new("acme", "one", 1).unwrap();
    assert_eq!(
        read_issue(&replacement, &other).await.unwrap_err(),
        GitHubError::Unauthorized.into()
    );
    assert!(!saved.authorization.is_current());
    assert!(cached(&replacement, &target).await.is_none());
}
