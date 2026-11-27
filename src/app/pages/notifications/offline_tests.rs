use super::*;
use crate::app::accounts::{Accounts, runtime::AccountRuntime};
use crate::auth::{memory::InMemoryCredentialStore, token::Pat};
use crate::data::{AccountService, notifications::RequestKind};
use crate::github::client::{GitHubClient, GitHubConfig};
use crate::network::{
    cancel::CancellationToken,
    executor::BackgroundExecutor,
    transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError, json_response},
};
use crate::storage::{
    accounts::AccountDatabaseConfig,
    cache::{CacheConfig, CacheStore},
};
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use serde_json::json;
use std::{
    cell::RefCell,
    future::Future,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Reply = Result<HttpResponse, TransportError>;

struct InboxTransport {
    reply: Mutex<Reply>,
    gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl InboxTransport {
    fn new(reply: Reply) -> Self {
        Self {
            reply: Mutex::new(reply),
            gate: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn hold(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.gate.lock().unwrap() = Some(rx);
        tx
    }
}

impl Transport for InboxTransport {
    fn execute<'a>(
        &'a self,
        req: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Reply> + Send + 'a>> {
        self.execute_bounded(req, cancel, usize::MAX)
    }

    fn execute_bounded<'a>(
        &'a self,
        req: HttpRequest,
        cancel: CancellationToken,
        max: usize,
    ) -> Pin<Box<dyn Future<Output = Reply> + Send + 'a>> {
        Box::pin(async move {
            let url = reqwest::Url::parse(&req.url).unwrap();
            if url.path().starts_with("/notifications") {
                self.requests.lock().unwrap().push(req.clone());
            }
            let response = match (req.method, url.path()) {
                (HttpMethod::Get, "/user") => {
                    let bob = req
                        .headers
                        .iter()
                        .any(|(_, value)| value == "Bearer synthetic-bob");
                    let (id, login) = if bob { (2, "bob") } else { (1, "alice") };
                    Ok(json_response(
                        200,
                        &json!({"id":id,"login":login}).to_string(),
                    ))
                }
                (HttpMethod::Get, "/user/repos") => Ok(json_response(200, "[]")),
                (HttpMethod::Get, "/notifications") => {
                    assert_eq!(
                        url.query(),
                        Some("all=false&participating=false&per_page=50&page=1")
                    );
                    let gate = self.gate.lock().unwrap().take();
                    if let Some(gate) = gate {
                        tokio::select! {
                            _ = cancel.cancelled() => return Err(TransportError::Cancelled),
                            result = gate => result.unwrap(),
                        }
                    }
                    self.reply.lock().unwrap().clone()
                }
                (HttpMethod::Delete, "/notifications/threads/1") => Ok(json_response(204, "")),
                (HttpMethod::Put, "/notifications/threads/1/subscription") => {
                    Ok(json_response(200, "{\"ignored\":true}"))
                }
                _ => panic!("unexpected fixture request path {}", url.path()),
            }?;
            assert!(response.body.len() <= max);
            Ok(response)
        })
    }
}

fn offline() -> Reply {
    Err(TransportError::Network {
        message: "synthetic disconnect".into(),
    })
}

fn response(status: u16, body: &str) -> HttpResponse {
    let mut response = json_response(status, body);
    response.headers.extend([
        ("x-poll-interval".into(), "1".into()),
        (
            "last-modified".into(),
            "Thu, 26 Nov 2026 08:00:00 GMT".into(),
        ),
    ]);
    response
}

fn rows() -> HttpResponse {
    let items: Vec<_> = [(1, 10, "alpha", "issues", "Issue"),
        (2, 20, "beta", "pulls", "PullRequest"),
        (3, 20, "beta", "issues", "Issue")]
        .into_iter().map(|(id, repo_id, repo, path, kind)| json!({
            "id": id.to_string(), "unread": true, "reason": "mention", "updated_at": "2026-11-26T08:00:00Z",
            "repository": {"id": repo_id, "name": repo, "owner": {"login": "acme"}},
            "subject": {"title": format!("Thread {id}"), "type": kind,
                "url": format!("https://api.github.com/repos/acme/{repo}/{path}/42")}
        })).collect();
    response(200, &serde_json::to_string(&items).unwrap())
}

struct TestAccounts {
    accounts: AccountService,
    executor: Arc<BackgroundExecutor>,
}

fn open(
    path: &Path,
    credentials: Arc<InMemoryCredentialStore>,
    transport: Arc<InboxTransport>,
) -> TestAccounts {
    let executor = Arc::new(BackgroundExecutor::new(2));
    let store = CacheStore::open(CacheConfig::at(path.join("cache"))).unwrap();
    let accounts = executor
        .raw_handle()
        .block_on(AccountService::open_with_cache(
            AccountDatabaseConfig::at(path.join("accounts.db")),
            credentials,
            Arc::new(GitHubClient::new(GitHubConfig::default(), transport)),
            executor.clone(),
            store,
        ))
        .unwrap();
    TestAccounts { accounts, executor }
}

fn seed(path: &Path, credentials: Arc<InMemoryCredentialStore>) {
    let runtime = open(path, credentials, Arc::new(InboxTransport::new(Ok(rows()))));
    runtime.executor.raw_handle().block_on(async {
        runtime
            .accounts
            .login(Pat::from_str("synthetic-bob"))
            .await
            .unwrap();
        runtime
            .accounts
            .login(Pat::from_str("synthetic-alice"))
            .await
            .unwrap();
        let inbox = runtime.accounts.repo_service().notifications().unwrap();
        inbox
            .execute(inbox.begin(RequestKind::First).unwrap().unwrap())
            .await;
        assert_eq!(inbox.snapshot().items.len(), 3);
        runtime.accounts.close().await.unwrap();
    });
}

fn wait_for(cx: &mut VisualTestContext, mut predicate: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if predicate(cx) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Notifications did not reach the expected state"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
fn notifications_offline_reading_mount_reopens_all_unread_threads_and_retry_replaces_with_empty(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    seed(temp.path(), credentials.clone());
    let transport = Arc::new(InboxTransport::new(offline()));
    let release = transport.hold();
    let opened = open(temp.path(), credentials, transport.clone());
    let runtime = Arc::new(AccountRuntime::new(opened.accounts, opened.executor));
    cx.update(ely_core::init_for_tests);
    let owner = RefCell::new(None);
    let routes = Rc::new(RefCell::new(Vec::new()));
    let (page, cx) = cx.add_window_view(|window, cx| {
        let accounts = cx.new(|cx| Accounts::new(Some(runtime.clone()), window, cx));
        let services = accounts.read(cx).services();
        owner.replace(Some(accounts));
        NotificationsPage::new(
            services,
            Rc::new({
                let routes = routes.clone();
                move |navigation, _, _| {
                    if let PageNavigation::Open(route) = navigation {
                        routes.borrow_mut().push(route);
                    }
                }
            }),
            window,
            cx,
        )
    });

    wait_for(cx, |cx| {
        cx.debug_bounds("notification-row-0").is_some()
            && cx.debug_bounds("notification-row-1").is_some()
    });
    assert!(cx.debug_bounds("notifications-saved").is_none());
    assert!(cx.debug_bounds("notifications-error").is_none());
    assert!(cx.debug_bounds("notification-row-2").is_some());
    for selector in ["notification-title-0", "notification-title-1"] {
        let title = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(title.center(), Modifiers::none());
    }
    let done = cx.debug_bounds("notification-action-done-0").unwrap();
    cx.simulate_click(done.center(), Modifiers::none());
    assert_eq!(
        *routes.borrow(),
        vec![
            Route::Issue {
                namespace: "acme".into(),
                name: "alpha".into(),
                number: 42
            },
            Route::PullRequest {
                namespace: "acme".into(),
                name: "beta".into(),
                number: 42
            }
        ]
    );
    wait_for(cx, |_| !transport.requests.lock().unwrap().is_empty());
    {
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, HttpMethod::Get);
        assert!(
            !requests[0]
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("if-modified-since"))
        );
    }
    release.send(()).unwrap();
    wait_for(cx, |cx| {
        page.read_with(cx, |page, cx| {
            !page.view.as_ref().unwrap().read(cx).is_refreshing()
        })
    });
    assert!(cx.debug_bounds("notifications-saved").is_some());
    assert!(cx.debug_bounds("notification-row-0").is_some());
    *transport.reply.lock().unwrap() = Ok(response(200, "[]"));
    let release = transport.hold();
    let retry = cx.debug_bounds("notifications-error").unwrap();
    cx.simulate_click(retry.center(), Modifiers::none());
    wait_for(cx, |_| transport.requests.lock().unwrap().len() == 2);
    assert!(cx.debug_bounds("notification-row-0").is_some());
    assert!(cx.debug_bounds("notification-row-1").is_some());
    assert!(cx.debug_bounds("notifications-saved").is_none());
    assert!(cx.debug_bounds("notifications-error").is_none());
    release.send(()).unwrap();
    wait_for(cx, |cx| {
        cx.debug_bounds("notifications-saved").is_none()
            && cx.debug_bounds("notification-row-0").is_none()
            && cx.debug_bounds("notification-row-1").is_none()
    });
    assert_eq!(transport.requests.lock().unwrap().len(), 2);
    *transport.reply.lock().unwrap() = offline();
    assert!(
        read_first(&runtime.accounts, &runtime.executor())
            .snapshot()
            .items
            .is_empty()
    );
    drop(owner);
}

fn read_first(
    accounts: &AccountService,
    executor: &BackgroundExecutor,
) -> crate::data::notifications::Notifications {
    let inbox = accounts.repo_service().notifications().unwrap();
    executor.raw_handle().block_on(async {
        inbox
            .execute(inbox.begin(RequestKind::First).unwrap().unwrap())
            .await;
    });
    inbox
}

#[test]
fn notifications_offline_reading_isolated_and_revoked_mixed_projection_cannot_return() {
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    seed(temp.path(), credentials.clone());
    let transport = Arc::new(InboxTransport::new(offline()));
    let runtime = open(temp.path(), credentials.clone(), transport.clone());
    let alice = runtime.accounts.active_summary().unwrap().identity.key;
    let mut bob = alice.clone();
    bob.id = 2;
    let executor = runtime.executor.clone();
    let handle = executor.raw_handle();
    handle.block_on(runtime.accounts.set_active(bob)).unwrap();
    let other = read_first(&runtime.accounts, &runtime.executor).snapshot();
    assert!(other.items.is_empty() && other.historical_at_ms.is_none());
    handle.block_on(runtime.accounts.set_active(alice)).unwrap();
    let saved = read_first(&runtime.accounts, &runtime.executor);
    let history = saved.snapshot();
    assert_eq!(history.items.len(), 3);
    assert!(history.items.iter().all(|item| item.unread));
    let repos = runtime.accounts.repo_service();
    let cache = repos.cache(saved.context()).unwrap();
    cache
        .store()
        .revoke_repository(cache.scope(), Some(10))
        .unwrap();
    assert!(saved.snapshot().items.is_empty());

    // Even a new response containing the explicitly revoked member must not
    // repopulate a previously invalidated mixed-repository projection.
    *transport.reply.lock().unwrap() = Ok(rows());
    assert_eq!(
        read_first(&runtime.accounts, &runtime.executor)
            .snapshot()
            .items
            .len(),
        3
    );
    handle.block_on(runtime.accounts.close()).unwrap();
    drop(cache);
    drop(saved);
    drop(repos);
    drop(runtime);
    let restarted = open(
        temp.path(),
        credentials,
        Arc::new(InboxTransport::new(offline())),
    );
    assert!(
        read_first(&restarted.accounts, &restarted.executor)
            .snapshot()
            .items
            .is_empty()
    );
}

#[test]
fn notifications_offline_reading_forgets_no_store_and_explicit_authorization_failures() {
    for result in [
        {
            let mut failure = response(503, "{}");
            failure.headers.extend([
                ("cache-control".into(), "private".into()),
                ("cache-control".into(), "No-Store".into()),
            ]);
            failure
        },
        response(401, "{}"),
        {
            let mut failure = response(403, "{}");
            failure.headers.push((
                "x-github-sso".into(),
                "required; url=https://github.com/orgs/acme/sso".into(),
            ));
            failure
        },
    ] {
        let temp = tempfile::tempdir().unwrap();
        let credentials = Arc::new(InMemoryCredentialStore::new());
        seed(temp.path(), credentials.clone());
        let runtime = open(
            temp.path(),
            credentials.clone(),
            Arc::new(InboxTransport::new(Ok(result))),
        );
        assert!(
            read_first(&runtime.accounts, &runtime.executor)
                .snapshot()
                .items
                .is_empty()
        );
        runtime
            .executor
            .raw_handle()
            .block_on(runtime.accounts.close())
            .unwrap();
        drop(runtime);
        let restarted = open(
            temp.path(),
            credentials,
            Arc::new(InboxTransport::new(offline())),
        );
        assert!(
            read_first(&restarted.accounts, &restarted.executor)
                .snapshot()
                .items
                .is_empty()
        );
    }
}

#[test]
fn notifications_offline_reading_completed_actions_invalidate_the_disk_projection() {
    use crate::domain::notification::ThreadAction;
    for action in [ThreadAction::Done, ThreadAction::Unsubscribe] {
        let temp = tempfile::tempdir().unwrap();
        let credentials = Arc::new(InMemoryCredentialStore::new());
        seed(temp.path(), credentials.clone());
        let online = open(
            temp.path(),
            credentials.clone(),
            Arc::new(InboxTransport::new(Ok(rows()))),
        );
        let inbox = read_first(&online.accounts, &online.executor);
        let request = inbox.begin_thread_action("1", action).unwrap().unwrap();
        let executor = online.executor.clone();
        executor
            .raw_handle()
            .block_on(inbox.execute_thread_action(request));
        assert!(inbox.snapshot().items.iter().all(|item| item.id != "1"));
        executor
            .raw_handle()
            .block_on(online.accounts.close())
            .unwrap();
        drop(inbox);
        drop(online);
        let restarted = open(
            temp.path(),
            credentials,
            Arc::new(InboxTransport::new(offline())),
        );
        assert!(
            read_first(&restarted.accounts, &restarted.executor)
                .snapshot()
                .items
                .is_empty()
        );
    }
}
