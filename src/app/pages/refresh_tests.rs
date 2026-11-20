//! Integration tests for the Shell `tab-refresh` control across all workspace routes.
//!
//! Uses the real production `Shell`, `Accounts`, `AccountService`, and mounted page hosts with a
//! deterministic, controllable transport (gates, scripted failures, poll cooldowns, multi-account
//! responses) to verify request counts, duplicate-click coalescing, spinner completion across
//! multi-section refreshes, content continuity, partial failure + retry, tab switching, closing
//! tabs, account switching, and late async reply isolation.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualContext, VisualTestContext,
};
use tokio::sync::watch;

use crate::app::AccountsHandle;
use crate::app::accounts::Accounts;
use crate::app::accounts::runtime::AccountRuntime;
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::domain::{AccountKey, HostId};
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::ui::Shell;
use crate::ui::shell_refresh::RefreshState;
use crate::workspace::Route;

fn setup(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        crate::platform::fonts::register(cx).expect("test fonts");
        Shell::bind_keys(cx);
    });
}

fn json(status: u16, body: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: body.to_string(),
    }
}

fn graphql_ok(data: serde_json::Value) -> HttpResponse {
    json(200, serde_json::json!({"data": data}))
}

struct Gate {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Gate {
    fn held() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }

    fn release(&self) {
        let _ = self.tx.send(true);
    }
}

/// Controllable request-routed transport supporting multiple accounts, per-endpoint hold gates,
/// scripted failures, and configurable poll-interval headers.
#[derive(Default)]
struct RefreshTransport {
    reads: Mutex<Vec<String>>,
    writes: Mutex<Vec<String>>,
    gates: Mutex<HashMap<String, Arc<Gate>>>,
    failures: Mutex<HashMap<String, VecDeque<u16>>>,
    feed_poll_interval: Mutex<Option<u64>>,
    notifications_poll_interval: Mutex<Option<u64>>,
}

impl RefreshTransport {
    fn seen(&self, needle: &str) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.contains(needle))
            .count()
    }

    fn write_count(&self) -> usize {
        self.writes.lock().unwrap().len()
    }

    fn hold(&self, needle: &str) {
        self.gates
            .lock()
            .unwrap()
            .insert(needle.to_string(), Arc::new(Gate::held()));
    }

    fn release(&self, needle: &str) {
        if let Some(gate) = self.gates.lock().unwrap().remove(needle) {
            gate.release();
        }
    }

    fn fail_once(&self, needle: &str, status: u16) {
        self.failures
            .lock()
            .unwrap()
            .entry(needle.to_string())
            .or_default()
            .push_back(status);
    }

    fn set_feed_poll_interval(&self, seconds: Option<u64>) {
        *self.feed_poll_interval.lock().unwrap() = seconds;
    }

    fn set_notifications_poll_interval(&self, seconds: Option<u64>) {
        *self.notifications_poll_interval.lock().unwrap() = seconds;
    }

    fn matching_gate(&self, key: &str) -> Option<watch::Receiver<bool>> {
        let gates = self.gates.lock().unwrap();
        gates
            .iter()
            .find(|(needle, _)| key.contains(needle.as_str()))
            .map(|(_, gate)| gate.rx.clone())
    }

    fn take_failure(&self, key: &str) -> Option<u16> {
        let mut failures = self.failures.lock().unwrap();
        let needle = failures
            .iter()
            .find(|(needle, queue)| key.contains(needle.as_str()) && !queue.is_empty())
            .map(|(needle, _)| needle.clone())?;
        failures.get_mut(&needle)?.pop_front()
    }
}

impl Transport for RefreshTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: crate::network::cancel::CancellationToken,
        max_body_bytes: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
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

    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: crate::network::cancel::CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let is_write = !matches!(request.method, HttpMethod::Get)
                && !(matches!(request.method, HttpMethod::Post)
                    && request.url.ends_with("/graphql"));
            if is_write {
                self.writes
                    .lock()
                    .unwrap()
                    .push(format!("{:?} {}", request.method, request.url));
            }
            let body_str = request.body.as_deref().unwrap_or("");
            let key = if request.url.ends_with("/graphql") {
                format!("{}#{}", request.url, body_str)
            } else {
                request.url.clone()
            };
            self.reads.lock().unwrap().push(key.clone());

            if let Some(mut rx) = self.matching_gate(&key) {
                while !*rx.borrow() {
                    tokio::select! {
                        _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled),
                        changed = rx.changed() => {
                            if changed.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            if let Some(status) = self.take_failure(&key) {
                return Ok(json(
                    status,
                    serde_json::json!({"message": "injected failure"}),
                ));
            }

            let auth = request
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            let is_bob = auth.contains("token-bob");
            let (acct_id, acct_login) = if is_bob {
                (2u64, "bob")
            } else {
                (1u64, "alice")
            };

            let url = request.url.as_str();
            let lower = url.to_ascii_lowercase();

            if url.ends_with("/user") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": acct_id,
                        "login": acct_login,
                        "name": acct_login,
                        "avatar_url": null
                    }),
                ));
            }
            if url.contains("/user/repos") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 10, "name": "demo", "full_name": "alice/demo",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "description": "Demo repo",
                        "default_branch": "main"
                    }]),
                ));
            }
            if url.contains("/user/starred/alice/demo") {
                return Ok(json(404, serde_json::json!({})));
            }
            if url.contains("/notifications") {
                let secs = (*self.notifications_poll_interval.lock().unwrap()).unwrap_or(0);
                let headers = vec![("x-poll-interval".into(), secs.to_string())];
                return Ok(HttpResponse {
                    status: 200,
                    headers,
                    body: serde_json::json!([{
                        "id": "9001",
                        "unread": true,
                        "reason": "mention",
                        "updated_at": "2026-10-28T11:30:00Z",
                        "last_read_at": null,
                        "subject": {
                            "title": "Check active refresh",
                            "type": "Issue",
                            "url": "https://github.com/alice/demo/issues/1",
                            "latest_comment_url": null
                        },
                        "repository": {
                            "id": 10,
                            "name": "demo",
                            "full_name": "alice/demo",
                            "owner": {"login": "alice"},
                            "url": "https://api.github.com/repos/alice/demo"
                        }
                    }])
                    .to_string(),
                });
            }
            if url.contains("/search/issues") {
                let is_pr = url.contains("is%3Apr") || url.contains("is:pr");
                let item = if is_pr {
                    serde_json::json!({
                        "id": 102, "number": 9, "title": "Global PR item",
                        "html_url": "https://github.com/alice/demo/pull/9",
                        "state": "open", "updated_at": "2026-10-28T11:00:00Z",
                        "repository_url": "https://api.github.com/repos/alice/demo",
                        "pull_request": {"merged_at": null}
                    })
                } else {
                    serde_json::json!({
                        "id": 101, "number": 7, "title": "Home or global issue",
                        "html_url": "https://github.com/alice/demo/issues/7",
                        "state": "open", "updated_at": "2026-10-28T11:00:00Z",
                        "repository_url": "https://api.github.com/repos/alice/demo",
                        "pull_request": null
                    })
                };
                return Ok(json(
                    200,
                    serde_json::json!({
                        "total_count": 1,
                        "incomplete_results": false,
                        "items": [item]
                    }),
                ));
            }
            if url.contains("/user/following") {
                return Ok(json(
                    200,
                    serde_json::json!([{"id": 7, "login": "bob", "avatar_url": null}]),
                ));
            }
            if url.contains("/received_events") {
                let mut headers = vec![];
                if let Some(secs) = *self.feed_poll_interval.lock().unwrap() {
                    headers.push(("x-poll-interval".into(), secs.to_string()));
                }
                return Ok(HttpResponse {
                    status: 200,
                    headers,
                    body: serde_json::json!([{
                        "id": "evt-1", "type": "WatchEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T09:00:00Z"
                    }])
                    .to_string(),
                });
            }
            if url.ends_with("/graphql") {
                if body_str.contains("pullRequests(states: OPEN)") {
                    return Ok(graphql_ok(serde_json::json!({
                        "repository": {
                            "databaseId": 10,
                            "issues": {"totalCount": 3},
                            "pullRequests": {"totalCount": 1}
                        }
                    })));
                }
                if body_str.contains("contributionsCollection") {
                    return Ok(graphql_ok(serde_json::json!({
                        "user": {
                            "contributionsCollection": {
                                "contributionCalendar": {
                                    "totalContributions": 12,
                                    "weeks": [{
                                        "contributionDays": [{
                                            "date": "2026-10-28",
                                            "contributionCount": 12,
                                            "contributionLevel": "SECOND_QUARTILE"
                                        }]
                                    }]
                                }
                            }
                        }
                    })));
                }
                return Ok(graphql_ok(serde_json::json!({
                    "user": {
                        "pinnedItems": {"totalCount": 0, "nodes": []},
                        "repositories": {
                            "nodes": [{
                                "databaseId": 10, "name": "demo", "nameWithOwner": "alice/demo",
                                "description": "Demo repo", "isPrivate": false,
                                "stargazerCount": 12, "forkCount": 2,
                                "primaryLanguage": {"name": "Rust"}
                            }]
                        }
                    }
                })));
            }
            if lower.ends_with("/users/alice") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 1, "login": "alice", "name": "Alice",
                        "bio": "Builder", "followers": 10, "following": 2,
                        "public_repos": 1, "public_gists": 0
                    }),
                ));
            }
            if lower.ends_with("/users/bob") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 2, "login": "bob", "name": "Bob",
                        "bio": "Reviewer", "followers": 4, "following": 1,
                        "public_repos": 1, "public_gists": 0
                    }),
                ));
            }
            if lower.ends_with("/repos/alice/alice") || lower.ends_with("/repos/bob/bob") {
                return Ok(json(404, serde_json::json!({"message": "Not Found"})));
            }
            if lower.contains("/repos/alice/demo/branches") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "name": "main",
                        "commit": {"sha": "1111111111111111111111111111111111111111"},
                        "protected": true
                    }]),
                ));
            }
            if lower.contains("/repos/alice/demo/commits/main") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "sha": "1111111111111111111111111111111111111111",
                        "commit": {
                            "tree": {"sha": "2222222222222222222222222222222222222222"}
                        }
                    }),
                ));
            }
            if lower
                .contains("/repos/alice/demo/git/trees/2222222222222222222222222222222222222222")
            {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "sha": "2222222222222222222222222222222222222222",
                        "truncated": false,
                        "tree": [
                            {
                                "path": "README.md", "mode": "100644", "type": "blob",
                                "sha": "3333333333333333333333333333333333333333", "size": 14
                            },
                            {
                                "path": "unopened", "mode": "040000", "type": "tree",
                                "sha": "4444444444444444444444444444444444444444"
                            }
                        ]
                    }),
                ));
            }
            if lower
                .contains("/repos/alice/demo/git/blobs/3333333333333333333333333333333333333333")
            {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "sha": "3333333333333333333333333333333333333333",
                        "size": 14,
                        "encoding": "base64",
                        "content": "IyBEZW1vIFJFQURN"
                    }),
                ));
            }
            if lower.contains("/repos/alice/demo/releases/latest") {
                return Ok(json(404, serde_json::json!({"message": "Not Found"})));
            }
            if lower.contains("/repos/alice/demo/releases") {
                return Ok(json(200, serde_json::json!([])));
            }
            if lower.contains("/repos/alice/demo/contributors") {
                return Ok(json(
                    200,
                    serde_json::json!([{"id": 1, "login": "alice", "contributions": 5}]),
                ));
            }
            if lower.contains("/repos/alice/demo/languages") {
                return Ok(json(200, serde_json::json!({"Rust": 1000})));
            }
            if lower.ends_with("/repos/alice/demo") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10, "name": "demo", "full_name": "alice/demo",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "private": false,
                        "archived": false, "fork": false, "default_branch": "main",
                        "stargazers_count": 12, "forks_count": 2,
                        "open_issues_count": 4, "size": 1,
                        "description": "Demo repo", "html_url": "https://github.com/alice/demo"
                    }),
                ));
            }
            Ok(json(599, serde_json::json!({"unmodelled": url})))
        })
    }
}

fn open_shell(
    cx: &mut TestAppContext,
    transport: Arc<RefreshTransport>,
) -> (Entity<Shell>, Entity<Accounts>, &mut VisualTestContext) {
    setup(cx);
    let temp = tempfile::tempdir().unwrap();
    let executor = Arc::new(BackgroundExecutor::new(4));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let accounts = runtime
        .block_on(async {
            AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    transport as Arc<dyn Transport>,
                )),
                executor.clone(),
            )
            .await
        })
        .expect("open account service");
    std::mem::forget(temp);
    let runtime = AccountRuntime::new(accounts, executor);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let controller = cx.new(|cx| Accounts::new(Some(Arc::new(runtime)), window, cx));
        cx.set_global(AccountsHandle(controller));
        Shell::new(window, cx)
    });
    let controller = cx.read(AccountsHandle::get).expect("installed controller");
    (view, controller, cx)
}

fn pump_all(view: &Entity<Shell>, controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.drain_for_test(cx));
    cx.update_window_entity(view, |shell, window, cx| {
        shell.drain_all_pages_for_test(window, cx);
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn wait_for(
    view: &Entity<Shell>,
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&Shell, &Accounts, &gpui::App) -> bool,
) {
    for _ in 0..800 {
        pump_all(view, controller, cx);
        let ok = view.read_with(cx, |shell, cx| {
            let accounts = controller.read(cx);
            done(shell, accounts, cx)
        });
        if ok {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("condition did not settle in time");
}

fn sign_in_token(
    view: &Entity<Shell>,
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    token: &str,
    expected_count: usize,
) {
    let input = controller.read_with(cx, |accounts, _| accounts.token_input().clone());
    input.update(cx, |input, cx| input.set_text(token, cx));
    cx.update_window_entity(controller, |accounts, window, cx| {
        accounts.submit(window, cx)
    });
    wait_for(view, controller, cx, |_, accounts, _| {
        accounts.snapshot().accounts.len() >= expected_count
            && matches!(
                accounts.snapshot().repositories.preview,
                crate::ui::accounts::RepositoryPreview::Ready(_)
            )
    });
}

fn click_tab_refresh(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let bounds = cx
        .debug_bounds("tab-refresh")
        .expect("tab-refresh slot should exist");
    let center = bounds.center();
    cx.simulate_mouse_move(center, None, Modifiers::default());
    cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn signed_out_and_local_routes_keep_tab_refresh_disabled_with_zero_requests(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(RefreshTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    pump_all(&view, &controller, cx);

    // Signed-out Home is disabled and clicking does not issue requests.
    assert!(view.read_with(cx, |shell, cx| shell.active_refresh_state(cx).is_disabled()));
    click_tab_refresh(cx);
    assert_eq!(transport.seen("/search/issues"), 0);
    assert_eq!(transport.seen("/received_events"), 0);

    // Sign in and verify Settings and PullRequest detail routes are disabled with zero requests.
    sign_in_token(&view, &controller, cx, "token-alice", 1);
    for route in [
        Route::Settings,
        Route::PullRequest {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 5,
        },
    ] {
        cx.update_window_entity(&view, |shell, window, cx| {
            shell.navigate(route.clone(), window, cx);
        });
        pump_all(&view, &controller, cx);
        assert!(
            view.read_with(cx, |shell, cx| shell.active_refresh_state(cx).is_disabled()),
            "route {route:?} must disable tab-refresh"
        );
        let reads_before = transport.reads.lock().unwrap().len();
        click_tab_refresh(cx);
        pump_all(&view, &controller, cx);
        assert_eq!(
            transport.reads.lock().unwrap().len(),
            reads_before,
            "clicking disabled tab-refresh on {route:?} must not issue requests"
        );
    }
}

#[gpui::test]
fn home_tab_refresh_coalesces_waits_for_both_columns_and_recovers_from_partial_failure(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(RefreshTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    sign_in_token(&view, &controller, cx, "token-alice", 1);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx);
    });
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready() && shell.home_is_ready_for_test(cx)
    });
    assert!(cx.debug_bounds("tab-refresh-spinner").is_none());

    let home_before = transport.seen("/search/issues");
    let feed_before = transport.seen("/received_events");

    // Hold Feed so Home finishes first; spinner must stay active until both settle.
    transport.hold("/received_events");
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);
    assert!(view.read_with(cx, |shell, cx| {
        shell.active_refresh_state(cx).is_refreshing()
    }));
    assert!(
        cx.debug_bounds("tab-refresh-spinner").is_some(),
        "spinner must render while Home refresh is in flight"
    );

    // Duplicate click while refreshing must be coalesced/ignored.
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);

    // Wait for Home column to settle while Feed is still held.
    wait_for(&view, &controller, cx, |_, _, _| {
        transport.seen("/search/issues") == home_before + 1
    });
    pump_all(&view, &controller, cx);
    assert!(
        view.read_with(cx, |shell, cx| shell
            .active_refresh_state(cx)
            .is_refreshing()),
        "spinner must remain active while Feed is still in flight"
    );
    assert_eq!(transport.seen("/search/issues"), home_before + 1);
    assert_eq!(transport.seen("/received_events"), feed_before + 1);

    // Release Feed; both columns settle and spinner stops.
    transport.release("/received_events");
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
    });
    assert!(cx.debug_bounds("tab-refresh-spinner").is_none());

    // Partial failure: Home fails 503 while Feed succeeds -> spinner still stops and retry works.
    transport.fail_once("/search/issues", 503);
    click_tab_refresh(cx);
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
            && transport.seen("/search/issues") == home_before + 2
            && transport.seen("/received_events") == feed_before + 2
    });
    assert!(cx.debug_bounds("tab-refresh-spinner").is_none());

    // Retry via tab-refresh recovers Home.
    click_tab_refresh(cx);
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready() && shell.home_is_ready_for_test(cx)
    });
    assert_eq!(transport.seen("/search/issues"), home_before + 3);

    // Respect Feed poll cooldown while still refreshing Home.
    transport.set_feed_poll_interval(Some(60));
    click_tab_refresh(cx);
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
            && transport.seen("/received_events") == feed_before + 4
    });
    let home_cd = transport.seen("/search/issues");
    let feed_cd = transport.seen("/received_events");
    click_tab_refresh(cx);
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready() && transport.seen("/search/issues") == home_cd + 1
    });
    assert_eq!(
        transport.seen("/received_events"),
        feed_cd,
        "tab-refresh must respect Feed X-Poll-Interval cooldown"
    );
}

#[gpui::test]
fn profile_tab_refresh_revalidates_all_sections_and_handles_partial_failure(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(RefreshTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    sign_in_token(&view, &controller, cx, "token-alice", 1);

    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Profile, window, cx);
    });
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready() && shell.profile_target_for_test(cx).is_some()
    });

    let basic_before = transport.seen("/users/alice");
    let contrib_before = transport.seen("contributionsCollection");
    let pinned_before = transport.seen("pinnedItems");
    let readme_before = transport.seen("/repos/alice/alice");

    // Hold Contributions so spinner stays up until all 4 sections settle.
    transport.hold("contributionsCollection");
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);
    assert!(view.read_with(cx, |shell, cx| {
        shell.active_refresh_state(cx).is_refreshing()
    }));
    assert!(cx.debug_bounds("tab-refresh-spinner").is_some());

    // Duplicate click is coalesced.
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);

    wait_for(&view, &controller, cx, |_, _, _| {
        transport.seen("/users/alice") == basic_before + 1
            && transport.seen("pinnedItems") == pinned_before + 1
            && transport.seen("/repos/alice/alice") == readme_before + 1
    });
    assert!(
        view.read_with(cx, |shell, cx| shell
            .active_refresh_state(cx)
            .is_refreshing()),
        "spinner must stay active while Contributions is still in flight"
    );

    transport.release("contributionsCollection");
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
    });
    assert_eq!(
        transport.seen("contributionsCollection"),
        contrib_before + 1
    );
    assert!(cx.debug_bounds("tab-refresh-spinner").is_none());

    // Partial failure on Basic preserves mounted ProfileView and allows retry.
    transport.fail_once("/users/alice", 503);
    click_tab_refresh(cx);
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
            && transport.seen("/users/alice") == basic_before + 2
    });
    assert!(
        view.read_with(cx, |shell, cx| shell.profile_target_for_test(cx).is_some()),
        "transient Basic refresh error must not tear down the mounted ProfileView"
    );
}

#[gpui::test]
fn repository_tab_refresh_revalidates_mounted_sources_without_writes_or_full_traversal(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(RefreshTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    sign_in_token(&view, &controller, cx, "token-alice", 1);

    let repo_route = Route::Repository {
        namespace: "alice".into(),
        name: "demo".into(),
    };
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(repo_route, window, cx);
    });
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
            && transport.seen("/repos/alice/demo/languages") >= 1
            && transport.seen("/repos/alice/demo/branches") >= 1
    });

    let repo_before = transport.seen("/repos/alice/demo");
    let branches_before = transport.seen("/repos/alice/demo/branches");
    let languages_before = transport.seen("/repos/alice/demo/languages");
    let counts_before = transport.seen("pullRequests(states: OPEN)");

    // Hold Languages so spinner stays active across chained stages until overview finishes.
    transport.hold("/repos/alice/demo/languages");
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);
    assert!(view.read_with(cx, |shell, cx| {
        shell.active_refresh_state(cx).is_refreshing()
    }));
    assert!(cx.debug_bounds("tab-refresh-spinner").is_some());

    // Duplicate click while refreshing is ignored.
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);

    wait_for(&view, &controller, cx, |_, _, _| {
        transport.seen("/repos/alice/demo/branches") == branches_before + 1
            && transport.seen("pullRequests(states: OPEN)") == counts_before + 1
    });
    assert!(
        view.read_with(cx, |shell, cx| shell
            .active_refresh_state(cx)
            .is_refreshing()),
        "spinner must stay active while overview Languages is still in flight"
    );

    transport.release("/repos/alice/demo/languages");
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
    });
    assert!(transport.seen("/repos/alice/demo") > repo_before);
    assert_eq!(
        transport.seen("/repos/alice/demo/languages"),
        languages_before + 1
    );
    assert_eq!(
        transport.seen("4444444444444444444444444444444444444444"),
        0,
        "refresh must never recursively traverse unopened directories"
    );
    assert_eq!(
        transport.write_count(),
        0,
        "refresh must never trigger write operations"
    );

    // Partial failure on branches settles spinner and allows retry.
    transport.fail_once("/repos/alice/demo/branches", 503);
    click_tab_refresh(cx);
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
            && transport.seen("/repos/alice/demo/branches") == branches_before + 2
    });
    assert!(cx.debug_bounds("tab-refresh-spinner").is_none());
}

#[gpui::test]
fn notifications_and_global_work_items_support_tab_refresh_and_cooldown(cx: &mut TestAppContext) {
    let transport = Arc::new(RefreshTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    sign_in_token(&view, &controller, cx, "token-alice", 1);

    // Notifications
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Notifications, window, cx);
    });
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready() && transport.seen("/notifications") >= 1
    });
    let notif_before = transport.seen("/notifications");
    transport.hold("/notifications");
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);
    assert!(view.read_with(cx, |shell, cx| {
        shell.active_refresh_state(cx).is_refreshing()
    }));
    click_tab_refresh(cx);
    transport.release("/notifications");
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
    });
    assert_eq!(transport.seen("/notifications"), notif_before + 1);

    // Notifications server cooldown disables tab-refresh.
    transport.set_notifications_poll_interval(Some(60));
    click_tab_refresh(cx);
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_disabled()
            && transport.seen("/notifications") == notif_before + 2
    });

    // Global Issues & Pull Requests
    for route in [Route::Issues, Route::PullRequests] {
        cx.update_window_entity(&view, |shell, window, cx| {
            shell.navigate(route.clone(), window, cx);
        });
        wait_for(&view, &controller, cx, |shell, _, cx| {
            shell.active_refresh_state(cx).is_ready()
        });
        let search_before = transport.seen("/search/issues");
        transport.hold("/search/issues");
        click_tab_refresh(cx);
        pump_all(&view, &controller, cx);
        assert!(
            view.read_with(cx, |shell, cx| shell
                .active_refresh_state(cx)
                .is_refreshing()),
            "{route:?} must show refreshing while in flight"
        );
        click_tab_refresh(cx);
        transport.release("/search/issues");
        wait_for(&view, &controller, cx, |shell, _, cx| {
            shell.active_refresh_state(cx).is_ready()
        });
        assert_eq!(transport.seen("/search/issues"), search_before + 1);
    }
}

#[gpui::test]
fn tab_switching_closing_and_account_switching_isolate_refresh_state_and_late_replies(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(RefreshTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    sign_in_token(&view, &controller, cx, "token-alice", 1);

    // Mount Home (Tab A) and Repository (Tab B), plus Settings (Tab C).
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx);
    });
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
    });
    let home_tab = view.read_with(cx, |shell, _| shell.active_tab_id_for_test());

    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(
            Route::Repository {
                namespace: "alice".into(),
                name: "demo".into(),
            },
            window,
            cx,
        );
    });
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
    });
    let repo_tab = view.read_with(cx, |shell, _| shell.active_tab_id_for_test());

    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Settings, window, cx);
    });
    let settings_tab = view.read_with(cx, |shell, _| shell.active_tab_id_for_test());

    // Start a delayed refresh on Home (Tab A).
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab_for_test(home_tab, window, cx);
    });
    transport.hold("/received_events");
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);
    assert!(view.read_with(cx, |shell, cx| {
        shell.active_refresh_state(cx).is_refreshing()
    }));

    // Rapidly switch across Repo -> Settings -> Home -> Repo and verify each shows its real state.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab_for_test(repo_tab, window, cx);
    });
    pump_all(&view, &controller, cx);
    assert!(
        matches!(
            view.read_with(cx, |shell, cx| shell.active_refresh_state(cx)),
            RefreshState::Ready { .. }
        ),
        "adjacent Repo tab must stay Ready while Home refreshes in the background"
    );
    assert!(cx.debug_bounds("tab-refresh-spinner").is_none());

    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab_for_test(settings_tab, window, cx);
    });
    pump_all(&view, &controller, cx);
    assert!(view.read_with(cx, |shell, cx| shell.active_refresh_state(cx).is_disabled()));

    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab_for_test(home_tab, window, cx);
    });
    pump_all(&view, &controller, cx);
    assert!(
        view.read_with(cx, |shell, cx| shell
            .active_refresh_state(cx)
            .is_refreshing()),
        "returning to still-refreshing Home tab must show spinner again"
    );

    // Close the refreshing Home tab, then release its gate: Repo tab must remain unaffected.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.close_tab(home_tab, window, cx);
    });
    transport.release("/received_events");
    pump_all(&view, &controller, cx);
    assert_ne!(
        view.read_with(cx, |shell, _| shell.active_tab_id_for_test()),
        home_tab
    );

    // Now start a delayed refresh on alice's Repo tab, then switch to a second account (bob).
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab_for_test(repo_tab, window, cx);
    });
    pump_all(&view, &controller, cx);
    transport.hold("/repos/alice/demo/languages");
    click_tab_refresh(cx);
    pump_all(&view, &controller, cx);
    assert!(view.read_with(cx, |shell, cx| {
        shell.active_refresh_state(cx).is_refreshing()
    }));

    // Sign in bob (switches active account).
    sign_in_token(&view, &controller, cx, "token-bob", 2);
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.snapshot().active.clone()),
        Some(AccountKey::new(2, HostId::github()))
    );
    // Release alice's delayed gate and mount bob's Home; bob's active tab must settle cleanly
    // without alice's state or late replies interfering.
    transport.release("/repos/alice/demo/languages");
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx);
    });
    wait_for(&view, &controller, cx, |shell, _, cx| {
        shell.active_refresh_state(cx).is_ready()
    });
}
