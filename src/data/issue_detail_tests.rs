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
            if url.path().ends_with("/comments") {
                return Ok(json_response(200, "[]"));
            }
            let body = self.default_issue_body.lock().unwrap().clone();
            Ok(json_response(
                200,
                &json!({
                    "id": 100,
                    "number": 42,
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

#[tokio::test]
async fn caches_issue_and_comments_and_isolates_across_accounts_and_repos() {
    let temp = tempfile::tempdir().unwrap();
    let transport = ScriptedTransport::new();
    let service = open_with_cache(&temp, transport.clone()).await;
    let repos = service.repo_service();
    let session_a = repos.issue_detail_session().unwrap();
    let locator_one = IssueLocator::new("acme", "one", 42).unwrap();
    let locator_two = IssueLocator::new("acme", "two", 42).unwrap();
    let limits = IssueDetailLimits::default();

    // Online fetch populates cache for acme/one #42 under account-a.
    let fetched = session_a
        .fetch_issue(&locator_one, &limits, CancellationToken::default())
        .await
        .unwrap();
    assert_eq!(fetched.value.body.as_deref(), Some("Initial body"));

    // Cached read hits for acme/one #42 (case-insensitively), misses for acme/two #42.
    let alias = IssueLocator::new("ACME", "ONE", 42).unwrap();
    let hit = session_a
        .read_cached_issue(&alias, CancellationToken::default())
        .await
        .unwrap()
        .expect("cached hit for same issue");
    assert_eq!(hit.value.issue.body.as_deref(), Some("Initial body"));
    assert!(hit.value.authorization.is_current());

    let other_repo = session_a
        .read_cached_issue(&locator_two, CancellationToken::default())
        .await
        .unwrap();
    assert!(
        other_repo.is_none(),
        "same issue number in another repo must not hit"
    );

    // Single-page comments (no next page) and confirmed empty comments both hit cache.
    transport.push(
        "/repos/acme/one/issues/42/comments",
        json_response(
            200,
            &json!([{
                "id": 701,
                "user": { "id": 1, "login": "account-a" },
                "body": "Single page cached comment",
                "created_at": "2026-11-01T12:00:00Z",
                "updated_at": "2026-11-01T12:00:00Z"
            }])
            .to_string(),
        ),
    );
    let live_comments = session_a
        .fetch_comments_page(&locator_one, None, &limits, CancellationToken::default())
        .await
        .unwrap();
    assert_eq!(live_comments.value.comments.len(), 1);
    assert!(live_comments.value.next.is_none());
    let cached_comments = session_a
        .read_cached_comments_first_page(&alias, &limits, CancellationToken::default())
        .await
        .unwrap()
        .expect("single-page comments with has_next=false must hit cache");
    assert_eq!(cached_comments.value.page.comments.len(), 1);
    assert!(cached_comments.value.page.next.is_none());

    // Confirmed empty comments page also hits cache.
    let empty_comments = session_a
        .fetch_comments_page(&locator_two, None, &limits, CancellationToken::default())
        .await
        .unwrap();
    assert!(empty_comments.value.comments.is_empty());
    let cached_empty = session_a
        .read_cached_comments_first_page(&locator_two, &limits, CancellationToken::default())
        .await
        .unwrap()
        .expect("confirmed empty comments page must hit cache");
    assert!(cached_empty.value.page.comments.is_empty());
    assert!(cached_empty.value.page.next.is_none());

    // Switch to account-b: account-a's cached snapshot must never appear.
    service
        .login(Pat::new("ghp_account-b".to_string()))
        .await
        .unwrap();
    let session_b = repos.issue_detail_session().unwrap();
    assert!(!session_a.verify());
    let b_cached = session_b
        .read_cached_issue(&locator_one, CancellationToken::default())
        .await
        .unwrap();
    assert!(
        b_cached.is_none(),
        "account-b must not read account-a's cached issue"
    );
}

#[tokio::test]
async fn no_store_and_missing_or_forbidden_invalidate_cache_while_transient_preserves_it() {
    let temp = tempfile::tempdir().unwrap();
    let transport = ScriptedTransport::new();
    let service = open_with_cache(&temp, transport.clone()).await;
    let repos = service.repo_service();
    let session = repos.issue_detail_session().unwrap();
    let locator = IssueLocator::new("acme", "widgets", 42).unwrap();
    let limits = IssueDetailLimits::default();

    // Populate cache.
    session
        .fetch_issue(&locator, &limits, CancellationToken::default())
        .await
        .unwrap();
    assert!(
        session
            .read_cached_issue(&locator, CancellationToken::default())
            .await
            .unwrap()
            .is_some()
    );

    // Transient 503 preserves the cached snapshot.
    transport.push(
        "/repos/acme/widgets/issues/42",
        response_with_headers(503, "{}", &[]),
    );
    let err = session
        .fetch_issue(&locator, &limits, CancellationToken::default())
        .await
        .unwrap_err();
    assert!(err.is_transient());
    assert!(
        session
            .read_cached_issue(&locator, CancellationToken::default())
            .await
            .unwrap()
            .is_some(),
        "transient 503 must preserve cached snapshot"
    );

    // 404 invalidates the cached snapshot.
    transport.push(
        "/repos/acme/widgets/issues/42",
        response_with_headers(404, "{}", &[]),
    );
    let err = session
        .fetch_issue(&locator, &limits, CancellationToken::default())
        .await
        .unwrap_err();
    assert!(err.is_missing());
    assert!(
        session
            .read_cached_issue(&locator, CancellationToken::default())
            .await
            .unwrap()
            .is_none(),
        "404 must invalidate cached snapshot"
    );

    // Re-populate cache, then verify Cache-Control: no-store invalidates it (L020).
    transport.set_default_body("Fresh body");
    session
        .fetch_issue(&locator, &limits, CancellationToken::default())
        .await
        .unwrap();
    assert!(
        session
            .read_cached_issue(&locator, CancellationToken::default())
            .await
            .unwrap()
            .is_some()
    );

    transport.push(
        "/repos/acme/widgets/issues/42",
        response_with_headers(
            200,
            &json!({
                "id": 100,
                "number": 42,
                "title": "Sample issue",
                "state": "open",
                "user": { "id": 1, "login": "account-a" },
                "body": "Secret body",
                "comments": 0,
                "labels": [],
                "created_at": "2026-11-01T10:00:00Z",
                "updated_at": "2026-11-01T10:00:00Z"
            })
            .to_string(),
            &[("cache-control", "no-store")],
        ),
    );
    let live = session
        .fetch_issue(&locator, &limits, CancellationToken::default())
        .await
        .unwrap();
    assert_eq!(live.value.body.as_deref(), Some("Secret body"));
    assert!(
        session
            .read_cached_issue(&locator, CancellationToken::default())
            .await
            .unwrap()
            .is_none(),
        "no-store must invalidate any prior cached snapshot"
    );

    // Switching back and forth advances epoch so stale session is rejected.
    service
        .login(Pat::new("ghp_account-b".to_string()))
        .await
        .unwrap();
    service
        .set_active(AccountKey::new(1, HostId::github()))
        .await
        .unwrap();
    let stale = session
        .fetch_issue(&locator, &limits, CancellationToken::default())
        .await
        .unwrap_err();
    assert_eq!(stale, IssueDetailError::GitHub(GitHubError::StaleContext));
}

#[tokio::test]
async fn in_flight_read_does_not_repopulate_snapshot_invalidated_before_completion() {
    let temp = tempfile::tempdir().unwrap();
    let transport = ScriptedTransport::new();
    let service = open_with_cache(&temp, transport.clone()).await;
    let repos = service.repo_service();
    let session = repos.issue_detail_session().unwrap();
    let locator = IssueLocator::new("acme", "widgets", 99).unwrap();
    let limits = IssueDetailLimits::default();

    // Gate the first online fetch so we can invalidate the cache while it is in flight.
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    transport.set_gate(entered_tx, resume_rx);

    let session_in_flight = session.clone();
    let locator_in_flight = locator.clone();
    let limits_in_flight = limits.clone();
    let in_flight = tokio::spawn(async move {
        session_in_flight
            .fetch_issue(
                &locator_in_flight,
                &limits_in_flight,
                CancellationToken::default(),
            )
            .await
    });

    // Wait until the first fetch has acquired its write ticket and entered transport.
    entered_rx.await.unwrap();

    // Concurrent 404 invalidates the slot while the first fetch is still in flight.
    transport.push(
        "/repos/acme/widgets/issues/99",
        response_with_headers(404, "{}", &[]),
    );
    let err = session
        .fetch_issue(&locator, &limits, CancellationToken::default())
        .await
        .unwrap_err();
    assert!(err.is_missing());

    // Release the older in-flight fetch; its stale write ticket must NOT repopulate the cache.
    let _ = resume_tx.send(());
    let _ = in_flight.await.unwrap().unwrap();

    assert!(
        session
            .read_cached_issue(&locator, CancellationToken::default())
            .await
            .unwrap()
            .is_none(),
        "stale in-flight fetch must not repopulate a cache slot invalidated while it was running"
    );
}

#[tokio::test]
async fn unauthorized_response_quarantines_entire_account_cache() {
    let temp = tempfile::tempdir().unwrap();
    let transport = ScriptedTransport::new();
    let service = open_with_cache(&temp, transport.clone()).await;
    let repos = service.repo_service();
    let session = repos.issue_detail_session().unwrap();
    let issue_one = IssueLocator::new("acme", "one", 1).unwrap();
    let issue_two = IssueLocator::new("acme", "two", 2).unwrap();
    let limits = IssueDetailLimits::default();

    // Populate cache for issue_two.
    session
        .fetch_issue(&issue_two, &limits, CancellationToken::default())
        .await
        .unwrap();
    let cached_before = session
        .read_cached_issue(&issue_two, CancellationToken::default())
        .await
        .unwrap()
        .expect("issue_two should be cached before 401");
    assert!(cached_before.value.authorization.is_current());

    // A 401 on issue_one must quarantine the whole account cache, revoking issue_two as well.
    transport.push(
        "/repos/acme/one/issues/1",
        response_with_headers(401, "{}", &[]),
    );
    let err = session
        .fetch_issue(&issue_one, &limits, CancellationToken::default())
        .await
        .unwrap_err();
    assert_eq!(err, IssueDetailError::GitHub(GitHubError::Unauthorized));
    assert!(
        !cached_before.value.authorization.is_current(),
        "existing authorization token must be invalidated by 401 quarantine"
    );
    assert!(
        session
            .read_cached_issue(&issue_two, CancellationToken::default())
            .await
            .unwrap()
            .is_none(),
        "quarantined account cache must deny reads for other issues"
    );
}
