//! Shell composition and offline acceptance for Home and Feed work-item navigation.
//!
//! The page-level suites prove the host maps a row identity onto a workspace destination and that
//! each surface emits exactly one typed action. This file drives the real `Shell` + `Accounts` +
//! `HomePage`/`IssueDetailPage` + `Workspace` together with one representative fixture (mixed Home
//! list plus a merged Feed pull request) and clicks the real UI regions: the Home Issue title, the
//! Home Pull request title, the Home icon and `#number`, the Feed pull-request icon, the Feed title
//! and "Read more".
//!
//! What "success" means here:
//! - a Home Issue click opens `Route::Issue` and the real body from the issue detail host renders;
//! - a Home Pull request click opens `Route::PullRequest` (its body is owned by pr-conversation);
//! - the three Feed entry points and the Home rows reuse one tab per resource;
//! - owner/repository links keep their own destinations.
//!
//! In-place selection, a blank tab or a title-derived stand-in never counts as success. Native
//! Windows input dispatch is still out of scope (LESSONS L002).

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

/// Routes by request identity (never a FIFO), so the concurrent Home/Feed reads cannot consume each
/// other's response.
///
/// Home rows, all sharing one displayed title on purpose:
/// - Issue #21 in `alice/demo`
/// - Pull request #7 in `alice/demo`
/// - Issue #21 in `alice/other` (same number, different repository)
///
/// Feed: one merged contribution to a pull request in `alice/demo` with a truncated body, so the
/// card shows the PR icon, the title with `#number`, and "Read more".
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
            let url = request.url.as_str();
            if url.ends_with("/graphql") {
                // The pull-request conversation reads its body through GraphQL; comments stay on
                // the REST issue-comments endpoint.
                let body = request.body.clone().unwrap_or_default();
                let payload: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                assert!(
                    payload["query"]
                        .as_str()
                        .is_some_and(|query| query.starts_with("query PRConversation")),
                    "unmodelled graphql query: {body}"
                );
                let vars = &payload["variables"];
                let number = vars["number"].as_u64().unwrap();
                return Ok(json(
                    200,
                    serde_json::json!({"data": {"repository": {
                        "nameWithOwner": format!(
                            "{}/{}",
                            vars["owner"].as_str().unwrap(),
                            vars["name"].as_str().unwrap()
                        ),
                        "pullRequest": {
                            "number": number,
                            "title": "Shared Home row",
                            "state": "OPEN",
                            "isDraft": false,
                            "author": {"login": "alice"},
                            "body": format!(
                                "Real pull request body for #{number}, rendered by the conversation host."
                            ),
                            "baseRefName": "main",
                            "headRefName": "feature",
                            "headRef": null,
                            "headRepository": {"nameWithOwner": "alice/demo"}
                        }
                    }}}),
                ));
            }
            assert_eq!(request.method, HttpMethod::Get, "{}", request.url);
            if url.ends_with("/user") {
                return Ok(json(200, serde_json::json!({"id": 1, "login": "alice"})));
            }
            if url.ends_with("/users/alice") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 1, "login": "alice", "name": "Alice", "avatar_url": null,
                        "bio": null, "followers": 0, "following": 0
                    }),
                ));
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
                        "total_count": 3, "incomplete_results": false,
                        "items": [
                            {
                                "id": 9001, "number": 21, "title": "Shared Home row",
                                "html_url": "https://github.com/alice/demo/issues/21",
                                "state": "open", "updated_at": "2026-10-28T10:00:00Z",
                                "repository_url": "https://api.github.com/repos/alice/demo",
                                "pull_request": null
                            },
                            {
                                "id": 9002, "number": 7, "title": "Shared Home row",
                                "html_url": "https://github.com/alice/demo/pull/7",
                                "state": "open", "updated_at": "2026-10-28T09:00:00Z",
                                "repository_url": "https://api.github.com/repos/alice/demo",
                                "pull_request": {"merged_at": null}
                            },
                            {
                                "id": 9003, "number": 21, "title": "Shared Home row",
                                "html_url": "https://github.com/alice/other/issues/21",
                                "state": "open", "updated_at": "2026-10-28T08:00:00Z",
                                "repository_url": "https://api.github.com/repos/alice/other",
                                "pull_request": null
                            }
                        ]
                    }),
                ));
            }
            if url.contains("/user/following") {
                return Ok(json(
                    200,
                    serde_json::json!([
                        {"id": 7, "login": "bob", "avatar_url": null},
                        {"id": 8, "login": "carol", "avatar_url": null}
                    ]),
                ));
            }
            if url.contains("/received_events") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": "evt-pr", "type": "PullRequestEvent",
                        "actor": {"id": 8, "login": "carol", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo",
                                 "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {
                            "action": "merged", "number": 5,
                            "pull_request": {
                                "url": "https://api.github.com/repos/alice/demo/pulls/5",
                                "id": 505, "number": 5,
                                "head": {"ref": "feature", "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                                "base": {"ref": "main", "sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
                            }
                        },
                        "created_at": "2026-10-28T09:00:00Z"
                    }]),
                ));
            }
            if url.contains("/repos/alice/demo/pulls/5") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 505, "number": 5, "state": "closed", "merged": true,
                        "title": "Merge the navigation work",
                        "html_url": "https://github.com/alice/demo/pull/5",
                        "user": {"id": 7, "login": "bob", "avatar_url": null},
                        "body": "A detailed pull request description that exceeds the Feed preview limit. ".repeat(8)
                    }),
                ));
            }
            if url.contains("/repos/alice/demo/issues/21/comments") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 301,
                        "user": {"id": 2, "login": "bob", "avatar_url": null},
                        "body": "Confirmed while reviewing the navigation change.",
                        "created_at": "2026-10-28T11:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z",
                        "author_association": "CONTRIBUTOR"
                    }]),
                ));
            }
            if url.contains("/repos/alice/other/issues/21/comments") {
                return Ok(json(200, serde_json::json!([])));
            }
            if url.ends_with("/repos/alice/demo/issues/21") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10001, "number": 21, "title": "Shared Home row",
                        "state": "open",
                        "user": {"id": 1, "login": "alice", "avatar_url": null},
                        "body": "Real issue body rendered by the issue detail host.",
                        "comments": 1,
                        "labels": [{"name": "bug"}],
                        "created_at": "2026-10-28T10:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z"
                    }),
                ));
            }
            if url.contains("/repos/alice/demo/issues/7/comments") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 501,
                        "user": {"id": 2, "login": "bob", "avatar_url": null},
                        "body": "Pull request conversation comment.",
                        "created_at": "2026-10-28T11:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z",
                        "author_association": "CONTRIBUTOR"
                    }]),
                ));
            }
            if url.contains("/repos/alice/demo/issues/5/comments") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 601,
                        "user": {"id": 2, "login": "bob", "avatar_url": null},
                        "body": "Merged pull request conversation comment.",
                        "created_at": "2026-10-28T11:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z",
                        "author_association": "CONTRIBUTOR"
                    }]),
                ));
            }
            if url.ends_with("/repos/alice/demo/issues/7") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10007, "number": 7, "title": "Shared Home row",
                        "state": "open",
                        "user": {"id": 1, "login": "alice", "avatar_url": null},
                        "body": null, "comments": 1, "labels": [],
                        "created_at": "2026-10-28T10:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z"
                    }),
                ));
            }
            if url.ends_with("/repos/alice/demo/issues/5") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10005, "number": 5, "title": "Merge the navigation work",
                        "state": "closed",
                        "user": {"id": 7, "login": "bob", "avatar_url": null},
                        "body": null, "comments": 1, "labels": [],
                        "created_at": "2026-10-28T09:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z"
                    }),
                ));
            }
            if url.ends_with("/repos/alice/other/issues/21") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10002, "number": 21, "title": "Other repository issue",
                        "state": "open",
                        "user": {"id": 1, "login": "alice", "avatar_url": null},
                        "body": null, "comments": 0, "labels": [],
                        "created_at": "2026-10-28T09:00:00Z",
                        "updated_at": "2026-10-28T09:30:00Z"
                    }),
                ));
            }
            if url.contains("/repos/alice/demo") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10, "name": "demo", "owner": {"id": 1, "login": "alice"},
                        "visibility": "public", "private": false, "default_branch": "main",
                        "fork": false, "archived": false, "stargazers_count": 0,
                        "forks_count": 0, "open_issues_count": 1, "size": 1,
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

fn pump_all(shell: &Entity<Shell>, controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.drain_for_test(cx));
    cx.update_window_entity(shell, |shell, window, cx| {
        shell.drain_all_pages_for_test(window, cx);
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn wait_for_home_ready(
    shell: &Entity<Shell>,
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
) {
    for _ in 0..800 {
        pump_all(shell, controller, cx);
        if shell.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("Home did not settle");
}

fn wait_for_bounds(
    shell: &Entity<Shell>,
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    selector: &'static str,
) {
    for _ in 0..800 {
        pump_all(shell, controller, cx);
        if cx.debug_bounds(selector).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("{selector} did not appear");
}

fn click(
    shell: &Entity<Shell>,
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    selector: &'static str,
) {
    wait_for_bounds(shell, controller, cx, selector);
    let target = cx.debug_bounds(selector).expect("bounds were just checked");
    cx.simulate_click(target.center(), Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn home_and_feed_work_item_surfaces_open_their_own_workspace_tab(cx: &mut TestAppContext) {
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
    let home_tab = shell.read_with(cx, |shell, _| shell.active_tab_id());
    assert!(
        cx.debug_bounds("home-item-row-2").is_some(),
        "the mixed Home list shows all three rows"
    );
    wait_for_bounds(&shell, &controller, cx, "feed-pr-read-more-0");

    // 1. Home Issue title -> Route::Issue, with the real body from the issue detail host.
    click(&shell, &controller, cx, "home-title-0");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.route()),
        Route::Issue {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 21,
        }
    );
    wait_for_bounds(&shell, &controller, cx, "issue-detail-body");
    wait_for_bounds(&shell, &controller, cx, "issue-comment-301");
    let demo_issue_tab = shell.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(demo_issue_tab, home_tab);

    // 2. Home Pull request title -> Route::PullRequest (its body is owned by pr-conversation).
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.active_tab_id()),
        home_tab,
        "returning to Home reuses the retained Home tab"
    );
    click(&shell, &controller, cx, "home-title-1");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.route()),
        Route::PullRequest {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 7,
        }
    );
    let home_pr_tab = shell.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(home_pr_tab, home_tab);
    assert_ne!(home_pr_tab, demo_issue_tab);
    // The pull-request target is a real page now: the conversation body and comments render, so a
    // blank tab is not mistaken for success.
    wait_for_bounds(&shell, &controller, cx, "pr-conversation-body");
    wait_for_bounds(&shell, &controller, cx, "pr-comment-501");

    // 3. Home `#number` and icon address the same Issue as its title (reuse, no duplicate tab).
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    click(&shell, &controller, cx, "home-0-number");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.active_tab_id()),
        demo_issue_tab,
        "the #number opens the same issue tab"
    );
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    click(&shell, &controller, cx, "home-icon-0");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.active_tab_id()),
        demo_issue_tab,
        "the icon opens the same issue tab"
    );

    // 4. Feed pull-request icon, title and Read more are one destination.
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    click(&shell, &controller, cx, "feed-pr-icon-0");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.route()),
        Route::PullRequest {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 5,
        },
        "the Feed pull-request icon opens the merged pull request"
    );
    let feed_pr_tab = shell.read_with(cx, |shell, _| shell.active_tab_id());
    wait_for_bounds(&shell, &controller, cx, "pr-conversation-body");
    wait_for_bounds(&shell, &controller, cx, "pr-comment-601");
    for selector in ["feed-title-0", "feed-pr-read-more-0"] {
        cx.update_window_entity(&shell, |shell, window, cx| {
            shell.navigate(Route::Home, window, cx);
        });
        cx.run_until_parked();
        click(&shell, &controller, cx, selector);
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.active_tab_id()),
            feed_pr_tab,
            "{selector} reuses the same pull-request tab"
        );
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.route()),
            Route::PullRequest {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 5,
            }
        );
        wait_for_bounds(&shell, &controller, cx, "pr-conversation-body");
        wait_for_bounds(&shell, &controller, cx, "pr-comment-601");
    }
    assert_ne!(feed_pr_tab, home_pr_tab, "the Feed merge is a different PR");

    // 5. The owner and repository links beside the title keep their own destinations.
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    click(&shell, &controller, cx, "home-0-owner");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.route()),
        Route::Profile,
        "the owner link still opens a profile, not the Issue"
    );
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    click(&shell, &controller, cx, "home-0-repository");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.route()),
        Route::Repository {
            namespace: "alice".into(),
            name: "demo".into(),
        },
        "the repository link still opens the repository"
    );

    // 6. Returning Home keeps the retained rows (position/selection are covered page-locally by
    // `returning_to_home_keeps_the_selected_row_navigable`).
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.active_tab_id()),
        home_tab
    );
    assert!(cx.debug_bounds("home-item-row-2").is_some());
}
