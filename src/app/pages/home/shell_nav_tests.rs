//! Shell composition for Home Issue/PR row navigation.
//!
//! The page-level suite proves the host maps a row identity onto a workspace destination and that
//! keyboard activation emits exactly one typed action. This file drives the real `Shell` +
//! `Accounts` + `HomePage` + `Workspace` together, because the user-visible promise is "the row's
//! own tab opens, the same resource reuses it, and an Issue row never borrows the Pull request tab".
//!
//! Activation here is a pointer click on the visible title control; the keyboard path through the
//! focused list is covered by `src/app/pages/home/tests.rs` and `src/ui/home/tests.rs`. Native
//! Windows input dispatch stays out of scope (LESSONS L002).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualContext, VisualTestContext};

use crate::app::AccountsHandle;
use crate::app::accounts::Accounts;
use crate::app::accounts::runtime::AccountRuntime;
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::ui::Shell;
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

/// Routes by request identity (never a FIFO), so the concurrent Home and Feed reads cannot consume
/// each other's response. Four Home rows: Issue #7 and Pull request #7 in the same repository, Issue
/// #7 in another repository, and Issue #8 — all sharing one title on purpose.
#[derive(Default)]
struct NavTransport {
    reads: Mutex<Vec<String>>,
}

impl NavTransport {
    fn seen(&self, needle: &str) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.contains(needle))
            .count()
    }
}

impl Transport for NavTransport {
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
        _cancel: crate::network::cancel::CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.reads.lock().unwrap().push(request.url.clone());
            assert_eq!(request.method, HttpMethod::Get, "{}", request.url);
            let url = request.url.as_str();
            if url.ends_with("/user") {
                return Ok(json(200, serde_json::json!({"id": 1, "login": "alice"})));
            }
            if url.contains("/user/repos") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 10, "name": "demo", "full_name": "alice/demo",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "default_branch": "main",
                        "fork": false, "archived": false
                    }]),
                ));
            }
            if url.contains("/search/issues") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "total_count": 4, "incomplete_results": false,
                        "items": [
                            {
                                "id": 9001, "number": 7, "title": "Shared row title",
                                "html_url": "https://github.com/alice/demo/issues/7",
                                "state": "open", "updated_at": "2026-10-28T10:00:00Z",
                                "repository_url": "https://api.github.com/repos/alice/demo",
                                "pull_request": null
                            },
                            {
                                "id": 9002, "number": 7, "title": "Shared row title",
                                "html_url": "https://github.com/alice/demo/pull/7",
                                "state": "open", "updated_at": "2026-10-28T09:00:00Z",
                                "repository_url": "https://api.github.com/repos/alice/demo",
                                "pull_request": {"merged_at": null}
                            },
                            {
                                "id": 9003, "number": 7, "title": "Shared row title",
                                "html_url": "https://github.com/alice/other/issues/7",
                                "state": "open", "updated_at": "2026-10-28T08:00:00Z",
                                "repository_url": "https://api.github.com/repos/alice/other",
                                "pull_request": null
                            },
                            {
                                "id": 9004, "number": 8, "title": "Shared row title",
                                "html_url": "https://github.com/alice/demo/issues/8",
                                "state": "open", "updated_at": "2026-10-28T07:00:00Z",
                                "repository_url": "https://api.github.com/repos/alice/demo",
                                "pull_request": null
                            }
                        ]
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
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": "evt-1", "type": "WatchEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo",
                                 "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T09:00:00Z"
                    }]),
                ));
            }
            if url.contains("/repos/alice/demo") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10, "name": "demo", "owner": {"id": 1, "login": "alice"},
                        "visibility": "public", "private": false, "default_branch": "main",
                        "fork": false, "archived": false, "stargazers_count": 0,
                        "forks_count": 0, "open_issues_count": 0, "size": 1,
                        "description": "Synthetic repository",
                        "html_url": "https://github.com/alice/demo"
                    }),
                ));
            }
            panic!("unmodelled Home navigation request: {}", request.url);
        })
    }
}

fn open_shell(
    cx: &mut TestAppContext,
    transport: Arc<NavTransport>,
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
    // Keep the temporary directory alive for the window's lifetime.
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

fn pump_home(shell: &Entity<Shell>, controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.drain_for_test(cx));
    shell.update(cx, |shell, cx| shell.drain_home_for_test(cx));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn wait_for_home_ready(
    shell: &Entity<Shell>,
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
) {
    for _ in 0..800 {
        pump_home(shell, controller, cx);
        if shell.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("Home did not settle");
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let target = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} was not drawn"));
    cx.simulate_click(target.center(), Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn home_rows_open_their_own_tab_and_reuse_it(cx: &mut TestAppContext) {
    let transport = Arc::new(NavTransport::default());
    let (shell, controller, cx) = open_shell(cx, transport.clone());
    // Sign in through the real account controller.
    let input = controller.read_with(cx, |accounts, _| accounts.token_input().clone());
    input.update(cx, |input, cx| {
        input.set_text("synthetic-home-nav-token", cx)
    });
    cx.update_window_entity(&controller, |accounts, window, cx| {
        accounts.submit(window, cx)
    });
    for _ in 0..800 {
        controller.update(cx, |accounts, cx| accounts.drain_for_test(cx));
        cx.run_until_parked();
        if controller.read_with(cx, |accounts, _| accounts.snapshot().accounts.len() == 1) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.snapshot().accounts.len()),
        1
    );
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    wait_for_home_ready(&shell, &controller, cx);
    assert!(transport.seen("/search/issues") >= 1);
    assert!(
        cx.debug_bounds("home-item-row-3").is_some(),
        "all four fixture rows are visible"
    );
    let home_tab = shell.read_with(cx, |shell, _| shell.active_tab_id());

    // Row 1 is the Pull request. The Issue above it shares title, repository and number, so a
    // title- or number-only destination would land on the same tab instead of opening this one.
    click("home-title-1", cx);
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.route()),
        Route::PullRequest {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 7,
        },
        "the Pull request row opens the pull-request destination"
    );
    let pr_tab = shell.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(pr_tab, home_tab);
    assert_eq!(
        shell.update(cx, |shell, _| shell.workspace().sessions().len()),
        2,
        "Home plus one Pull request tab"
    );

    // Leaving Home and returning reuses the retained Home page rather than rebuilding it.
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.active_tab_id()),
        home_tab
    );
    assert!(cx.debug_bounds("home-item-row-3").is_some());

    // Activating the same Pull request row again reuses its tab; it does not open a second page.
    click("home-title-1", cx);
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.active_tab_id()),
        pr_tab,
        "the same resource reuses its tab"
    );
    assert_eq!(
        shell.update(cx, |shell, _| shell.workspace().sessions().len()),
        2
    );

    // The Issue rows have no destination in this baseline: activating one must neither open a
    // stand-in tab nor fall through to the Pull request tab that happens to share the number.
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    click("home-title-0", cx);
    assert_eq!(
        shell.update(cx, |shell, _| shell.workspace().sessions().len()),
        2,
        "an Issue row must not open a stand-in tab"
    );
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.route()),
        Route::Home,
        "an Issue row leaves the workspace where it was"
    );
}
