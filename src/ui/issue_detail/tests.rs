//! Controller and GPUI view tests for `IssueDetailView`.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use gpui::{Entity, TestAppContext, VisualContext, VisualTestContext, px, size};
use serde_json::json;

use super::controller::CommentsRequestMode;
use super::*;
use crate::auth::{InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::github::GitHubClient;
use crate::network::{
    cancel::CancellationToken,
    executor::BackgroundExecutor,
    transport::{HttpRequest, HttpResponse, Transport, TransportError, json_response},
};
use crate::storage::{
    accounts::AccountDatabaseConfig,
    cache::{CacheConfig, CacheStore},
};

struct FixtureTransport {
    routes: Mutex<HashMap<String, Vec<HttpResponse>>>,
}

impl FixtureTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            routes: Mutex::new(HashMap::new()),
        })
    }

    fn set(&self, key: &str, responses: Vec<HttpResponse>) {
        self.routes
            .lock()
            .unwrap()
            .insert(key.to_string(), responses);
    }
}

impl Transport for FixtureTransport {
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
            let key = match url.query() {
                Some(q) if !q.is_empty() => format!("{}?{q}", url.path()),
                _ => url.path().to_string(),
            };
            let mut map = self.routes.lock().unwrap();
            if let Some(queue) = map.get_mut(&key) {
                if queue.len() > 1 {
                    return Ok(queue.remove(0));
                }
                if let Some(last) = queue.first() {
                    return Ok(last.clone());
                }
            }
            if let Some(queue) = map.get_mut(url.path()) {
                if queue.len() > 1 {
                    return Ok(queue.remove(0));
                }
                if let Some(last) = queue.first() {
                    return Ok(last.clone());
                }
            }
            Ok(json_response(404, "{}"))
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

fn response(status: u16, body: &str, headers: &[(&str, &str)]) -> HttpResponse {
    HttpResponse {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.to_string(),
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    service: AccountService,
    executor: Arc<BackgroundExecutor>,
    transport: Arc<FixtureTransport>,
}

impl Harness {
    fn new(with_cache: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let transport = FixtureTransport::new();
        let executor = Arc::new(BackgroundExecutor::new(4));
        let client = Arc::new(GitHubClient::new(Default::default(), transport.clone()));
        let service = if with_cache {
            let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
            runtime
                .block_on(AccountService::open_with_cache(
                    AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                    Arc::new(InMemoryCredentialStore::new()),
                    client,
                    executor.clone(),
                    cache,
                ))
                .unwrap()
        } else {
            runtime
                .block_on(AccountService::open(
                    AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                    Arc::new(InMemoryCredentialStore::new()),
                    client,
                    executor.clone(),
                ))
                .unwrap()
        };
        runtime
            .block_on(service.login(Pat::new("ghp_account-a".to_string())))
            .unwrap();
        Self {
            _temp: temp,
            runtime,
            service,
            executor,
            transport,
        }
    }

    fn mount<'a>(
        &self,
        locator: IssueLocator,
        cx: &'a mut TestAppContext,
    ) -> (Entity<IssueDetailView>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            ely_core::init_for_tests(cx);
            crate::app::theme::install(cx);
        });
        let repos = self.service.repo_service();
        let executor = self.executor.clone();
        let (view, cx) = cx.add_window_view(|window, cx| {
            IssueDetailView::new(repos, executor, locator, PageAvatars::new(None), window, cx)
                .unwrap()
        });
        cx.simulate_resize(size(px(960.0), px(720.0)));
        (view, cx)
    }

    fn settle(&self, cx: &mut VisualTestContext) {
        for _ in 0..12 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            cx.background_executor
                .advance_clock(std::time::Duration::from_millis(20));
            cx.run_until_parked();
        }
    }
}

#[gpui::test]
fn renders_issue_body_and_paginates_comments_and_recovers_from_append_error(
    cx: &mut TestAppContext,
) {
    let harness = Harness::new(false);
    harness.transport.set(
        "/repos/acme/widgets/issues/42",
        vec![json_response(
            200,
            &json!({
                "id": 9001,
                "number": 42,
                "title": "Add read-only issue detail",
                "state": "open",
                "user": { "id": 7, "login": "octocat" },
                "body": "# Summary\n\nThis is **Markdown** content.",
                "comments": 2,
                "labels": [{ "name": "enhancement" }],
                "created_at": "2026-11-01T10:00:00Z",
                "updated_at": "2026-11-02T10:00:00Z"
            })
            .to_string(),
        )],
    );
    harness.transport.set(
        "/repos/acme/widgets/issues/42/comments?per_page=30&page=1",
        vec![response(
            200,
            &json!([{
                "id": 501,
                "user": { "id": 8, "login": "alice" },
                "body": "First comment body",
                "created_at": "2026-11-01T12:00:00Z",
                "updated_at": "2026-11-01T12:00:00Z",
                "author_association": "MEMBER"
            }])
            .to_string(),
            &[(
                "link",
                r#"<https://api.github.com/repos/acme/widgets/issues/42/comments?per_page=30&page=2>; rel="next""#,
            )],
        )],
    );
    // Page 2 fails once with 503, then succeeds on retry.
    harness.transport.set(
        "/repos/acme/widgets/issues/42/comments?per_page=30&page=2",
        vec![
            json_response(503, "{}"),
            json_response(
                200,
                &json!([{
                    "id": 502,
                    "user": { "id": 9, "login": "bob" },
                    "body": "Second comment body",
                    "created_at": "2026-11-01T13:00:00Z",
                    "updated_at": "2026-11-01T13:00:00Z"
                }])
                .to_string(),
            ),
        ],
    );

    let locator = IssueLocator::new("acme", "widgets", 42).unwrap();
    let (view, cx) = harness.mount(locator, cx);
    harness.settle(cx);

    assert!(cx.debug_bounds("issue-detail-title").is_some());
    assert!(cx.debug_bounds("issue-detail-body").is_some());
    assert!(cx.debug_bounds("issue-comment-501").is_some());
    assert!(cx.debug_bounds("issue-detail-comments-footer").is_some());

    // Trigger Load more (page 2 fails with 503): issue body and comment 501 MUST remain visible.
    view.update(cx, |view, cx| {
        assert!(view.request_comments(CommentsRequestMode::Append, false, cx));
    });
    harness.settle(cx);

    assert!(
        cx.debug_bounds("issue-detail-body").is_some(),
        "comments append failure must never erase issue body"
    );
    assert!(
        cx.debug_bounds("issue-comment-501").is_some(),
        "comments append failure must preserve previously loaded comments"
    );
    assert!(
        cx.debug_bounds("issue-detail-comments-append-error")
            .is_some()
    );

    // Retry page 2: succeeds and appends comment 502.
    view.update(cx, |view, cx| {
        assert!(view.request_comments(CommentsRequestMode::Append, false, cx));
    });
    harness.settle(cx);

    assert!(cx.debug_bounds("issue-comment-501").is_some());
    assert!(cx.debug_bounds("issue-comment-502").is_some());
    assert!(
        cx.debug_bounds("issue-detail-comments-append-error")
            .is_none()
    );
    assert!(cx.debug_bounds("issue-detail-comments-footer").is_none());
}

#[gpui::test]
fn distinguishes_empty_body_empty_comments_and_comments_error(cx: &mut TestAppContext) {
    let harness = Harness::new(false);
    harness.transport.set(
        "/repos/acme/widgets/issues/7",
        vec![json_response(
            200,
            &json!({
                "id": 9007,
                "number": 7,
                "title": "Empty issue",
                "state": "closed",
                "state_reason": "completed",
                "user": { "id": 7, "login": "octocat" },
                "body": null,
                "comments": 0,
                "labels": [],
                "created_at": "2026-11-01T10:00:00Z",
                "updated_at": "2026-11-01T11:00:00Z"
            })
            .to_string(),
        )],
    );
    // First page of comments fails once, then returns truly empty [].
    harness.transport.set(
        "/repos/acme/widgets/issues/7/comments?per_page=30&page=1",
        vec![json_response(503, "{}"), json_response(200, "[]")],
    );

    let locator = IssueLocator::new("acme", "widgets", 7).unwrap();
    let (view, cx) = harness.mount(locator, cx);
    harness.settle(cx);

    assert!(cx.debug_bounds("issue-detail-empty-body").is_some());
    assert!(
        cx.debug_bounds("issue-detail-comments-error").is_some(),
        "failed comments read must not appear as 'No comments yet.'"
    );
    assert!(cx.debug_bounds("issue-detail-empty-comments").is_none());

    // Retry comments page 1 -> now truly empty.
    view.update(cx, |view, cx| {
        assert!(view.request_comments(CommentsRequestMode::Replace, false, cx));
    });
    harness.settle(cx);

    assert!(cx.debug_bounds("issue-detail-empty-body").is_some());
    assert!(cx.debug_bounds("issue-detail-comments-error").is_none());
    assert!(cx.debug_bounds("issue-detail-empty-comments").is_some());
}

#[gpui::test]
fn distinguishes_missing_not_an_issue_and_permission_errors(cx: &mut TestAppContext) {
    let cases = [
        (404, "{}", "issue-detail-error-missing"),
        (410, "{}", "issue-detail-error-missing"),
        (403, "{}", "issue-detail-error-permission"),
        (
            200,
            r#"{"id":1,"number":9,"title":"PR","state":"open","created_at":"2026-11-01T10:00:00Z","updated_at":"2026-11-01T10:00:00Z","pull_request":{"url":"https://api.github.com/repos/acme/widgets/pulls/9"}}"#,
            "issue-detail-error-not-an-issue",
        ),
    ];

    for (status, body, expected_selector) in cases {
        let harness = Harness::new(false);
        harness.transport.set(
            "/repos/acme/widgets/issues/9",
            vec![json_response(status, body)],
        );
        let locator = IssueLocator::new("acme", "widgets", 9).unwrap();
        let (_view, cx) = harness.mount(locator, cx);
        harness.settle(cx);
        assert!(
            cx.debug_bounds(expected_selector).is_some(),
            "status {status} should render {expected_selector}"
        );
    }
}

#[gpui::test]
fn cached_snapshot_survives_transient_error_but_clears_on_missing_or_account_switch(
    cx: &mut TestAppContext,
) {
    let harness = Harness::new(true);
    let locator = IssueLocator::new("acme", "widgets", 42).unwrap();
    harness.transport.set(
        "/repos/acme/widgets/issues/42",
        vec![
            json_response(
                200,
                &json!({
                    "id": 9042,
                    "number": 42,
                    "title": "Cached issue",
                    "state": "open",
                    "user": { "id": 7, "login": "octocat" },
                    "body": "Persisted body",
                    "comments": 0,
                    "labels": [],
                    "created_at": "2026-11-01T10:00:00Z",
                    "updated_at": "2026-11-01T10:00:00Z"
                })
                .to_string(),
            ),
            json_response(503, "{}"),
            json_response(404, "{}"),
        ],
    );
    harness.transport.set(
        "/repos/acme/widgets/issues/42/comments?per_page=30&page=1",
        vec![json_response(200, "[]")],
    );

    // Seed cache via first live read.
    let session = harness
        .service
        .repo_service()
        .issue_detail_session()
        .unwrap();
    harness
        .runtime
        .block_on(session.fetch_issue(
            &locator,
            &IssueDetailLimits::default(),
            CancellationToken::default(),
        ))
        .unwrap();

    // Open view when online refresh fails with 503: cached body stays visible with Cached badge!
    let (view, cx) = harness.mount(locator.clone(), cx);
    harness.settle(cx);
    assert!(
        cx.debug_bounds("issue-detail-body").is_some(),
        "transient 503 must retain cached issue body"
    );
    assert!(cx.debug_bounds("issue-detail-cached-badge").is_some());

    // Next refresh returns 404: stale cached body MUST be cleared and missing error shown.
    view.update(cx, |view, cx| {
        assert!(view.refresh(cx));
    });
    harness.settle(cx);
    assert!(cx.debug_bounds("issue-detail-body").is_none());
    assert!(cx.debug_bounds("issue-detail-error-missing").is_some());

    // Switch account: view immediately renders Account changed.
    harness
        .runtime
        .block_on(harness.service.login(Pat::new("ghp_account-b".to_string())))
        .unwrap();
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-detail-account-changed").is_some());
}

#[gpui::test]
fn attached_image_provider_receives_requests_from_issue_body_and_comments(cx: &mut TestAppContext) {
    struct RecordingProvider {
        requested: Mutex<Vec<String>>,
    }
    impl crate::ui::reader::ImageProvider for RecordingProvider {
        fn start(
            &self,
            request: crate::ui::reader::ImageRequest,
            _sink: crate::ui::reader::ImageSink,
        ) {
            if let crate::ui::reader::ImageTarget::External { url } = request.target {
                self.requested.lock().unwrap().push(url);
            }
        }
        fn cancel(&self, _scope: crate::ui::reader::ImageScope) {}
    }

    let harness = Harness::new(false);
    harness.transport.set(
        "/repos/acme/widgets/issues/77",
        vec![json_response(
            200,
            &json!({
                "id": 9077,
                "number": 77,
                "title": "Issue with screenshots",
                "state": "open",
                "user": { "id": 7, "login": "octocat" },
                "body": "Body screenshot:\n\n![body](https://example.com/body.png)",
                "comments": 1,
                "labels": [],
                "created_at": "2026-11-01T10:00:00Z",
                "updated_at": "2026-11-01T11:00:00Z"
            })
            .to_string(),
        )],
    );
    harness.transport.set(
        "/repos/acme/widgets/issues/77/comments?per_page=30&page=1",
        vec![json_response(
            200,
            &json!([{
                "id": 7701,
                "user": { "id": 8, "login": "alice" },
                "body": "Comment screenshot:\n\n![comment](https://example.com/comment.png)",
                "created_at": "2026-11-01T12:00:00Z",
                "updated_at": "2026-11-01T12:00:00Z"
            }])
            .to_string(),
        )],
    );

    let provider = Arc::new(RecordingProvider {
        requested: Mutex::new(Vec::new()),
    });
    let locator = IssueLocator::new("acme", "widgets", 77).unwrap();
    let (view, cx) = harness.mount(locator, cx);
    let provider_trait: Arc<dyn crate::ui::reader::ImageProvider> = provider.clone();
    view.update(cx, |view, cx| {
        view.set_image_provider(Some(provider_trait), cx);
    });
    harness.settle(cx);

    let urls = provider.requested.lock().unwrap().clone();
    assert!(
        urls.iter().any(|u| u == "https://example.com/body.png"),
        "issue body reader must request images through the attached provider: {urls:?}"
    );
    assert!(
        urls.iter().any(|u| u == "https://example.com/comment.png"),
        "comment reader must request images through the attached provider: {urls:?}"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.pending_cache_reads),
        0,
        "all cache read tasks must settle cleanly"
    );
}

#[gpui::test]
fn relative_markdown_link_in_issue_body_resolves_to_github_head_url(cx: &mut TestAppContext) {
    let harness = Harness::new(false);
    harness.transport.set(
        "/repos/acme/widgets/issues/78",
        vec![json_response(
            200,
            &json!({
                "id": 9078,
                "number": 78,
                "title": "Issue with relative link",
                "state": "open",
                "user": { "id": 7, "login": "octocat" },
                "body": "See [details](docs/details.md#sec).",
                "comments": 0,
                "labels": [],
                "created_at": "2026-11-01T10:00:00Z",
                "updated_at": "2026-11-01T11:00:00Z"
            })
            .to_string(),
        )],
    );
    harness.transport.set(
        "/repos/acme/widgets/issues/78/comments?per_page=30&page=1",
        vec![json_response(200, "[]")],
    );

    let locator = IssueLocator::new("acme", "widgets", 78).unwrap();
    let (view, cx) = harness.mount(locator, cx);
    let emitted = Arc::new(Mutex::new(Vec::<String>::new()));
    let emitted_sink = emitted.clone();
    view.update(cx, |view, _| {
        view.on_action(Rc::new(move |action, _, _| {
            if let IssueDetailAction::External(url) = action {
                emitted_sink.lock().unwrap().push(url);
            }
        }));
    });
    harness.settle(cx);

    let body_reader = view
        .read_with(cx, |view, _| {
            view.body_reader.as_ref().map(|r| r.view.clone())
        })
        .expect("body reader");
    cx.update_window_entity(&body_reader, |reader, window, cx| {
        reader.handle_link("docs/details.md#sec", window, cx);
    });
    cx.run_until_parked();

    assert_eq!(
        emitted.lock().unwrap().as_slice(),
        &["https://github.com/acme/widgets/blob/HEAD/docs/details.md#sec"]
    );
}
