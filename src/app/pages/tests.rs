//! Integration coverage for the real product Shell mounting the real page hosts.
//!
//! The account controller, Shell, page components and online read services are the production
//! ones. Only external boundaries are injected: a temporary SQLite directory, an in-memory
//! credential store and a deterministic transport.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{AppContext, Entity, TestAppContext, VisualContext, VisualTestContext};

use crate::app::AccountsHandle;
use crate::app::accounts::Accounts;
use crate::app::accounts::runtime::AccountRuntime;
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::domain::{AccountKey, HostId};
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::ui::Shell;
use crate::workspace::Route;

fn setup(cx: &mut TestAppContext) {
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

/// The Shell/account tests route every request by path and body. This is not a one-shot queue:
/// the same Home, repository or profile endpoint can be read again in A→B→A scenarios.
#[derive(Default)]
struct PageTransport {
    reads: Mutex<Vec<String>>,
}

impl PageTransport {
    fn seen(&self, needle: &str) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.contains(needle))
            .count()
    }
}

impl Transport for PageTransport {
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
            let body = request.body.clone().unwrap_or_default();
            let lower = url.to_ascii_lowercase();
            if url.ends_with("/user") {
                return Ok(json(200, serde_json::json!({"id": 1, "login": "alice"})));
            }
            if url.contains("/user/repos") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 10, "name": "demo", "full_name": "alice/demo",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "description": "Synthetic repository",
                        "default_branch": "main"
                    }, {
                        "id": 11, "name": "other", "full_name": "alice/other",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "default_branch": "main"
                    }]),
                ));
            }
            if url.contains("/search/issues") {
                // One open item and one closed item: the confirmed Home rule must
                // return only the open one even if a non-conforming page leaks a
                // closed row.
                return Ok(json(
                    200,
                    serde_json::json!({
                        "total_count": 2, "incomplete_results": false,
                        "items": [{
                            "id": 900, "number": 12, "title": "Synthetic involved issue",
                            "html_url": "https://github.com/alice/demo/issues/12",
                            "state": "open", "updated_at": "2026-10-28T10:00:00Z",
                            "repository_url": "https://api.github.com/repos/alice/demo",
                            "pull_request": null
                        }, {
                            "id": 901, "number": 13, "title": "Synthetic closed issue",
                            "html_url": "https://github.com/alice/demo/issues/13",
                            "state": "closed", "updated_at": "2026-10-28T09:00:00Z",
                            "repository_url": "https://api.github.com/repos/alice/demo",
                            "pull_request": null
                        }]
                    }),
                ));
            }
            if lower.contains("/users/catol") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 22, "login": "carol", "name": "Carol", "avatar_url": null,
                        "bio": "Another user", "followers": 2, "following": 1
                    }),
                ));
            }
            // The active account follows bob and carol, not dave.
            if url.contains("/user/following") {
                return Ok(json(
                    200,
                    serde_json::json!([
                        {"id": 7, "login": "bob", "avatar_url": null},
                        {"id": 8, "login": "carol", "avatar_url": null}
                    ]),
                ));
            }
            // Real detail-endpoint shape for the merged contribution: the author
            // lives here, not in the slim event payload.
            if lower.contains("/repos/alice/demo/pulls/5") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 555, "number": 5, "state": "closed", "merged": true,
                        "title": "Synthetic merged pull request",
                        "html_url": "https://github.com/alice/demo/pull/5",
                        "user": {"id": 8, "login": "carol", "avatar_url": null}
                    }),
                ));
            }
            if url.contains("/received_events") {
                // Live shapes: a star by a followed account, a push (never a card),
                // a star by an unfollowed account, and a live `action=merged` event
                // whose slim pull_request carries no author.
                return Ok(HttpResponse {
                    status: 200,
                    headers: vec![("x-poll-interval".into(), "1".into())],
                    body: serde_json::json!([{
                        "id": "evt-1", "type": "WatchEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T09:00:00Z"
                    }, {
                        "id": "evt-2", "type": "PushEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"ref": "refs/heads/main", "before": "a", "head": "b", "size": 1},
                        "created_at": "2026-10-28T08:00:00Z"
                    }, {
                        "id": "evt-3", "type": "WatchEvent",
                        "actor": {"id": 9, "login": "dave", "avatar_url": null},
                        "repo": {"id": 11, "name": "alice/other", "url": "https://api.github.com/repos/alice/other"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T07:30:00Z"
                    }, {
                        "id": "evt-4", "type": "PullRequestEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {
                            "action": "merged", "number": 5,
                            "pull_request": {
                                "url": "https://api.github.com/repos/alice/demo/pulls/5",
                                "id": 555, "number": 5,
                                "head": {"ref": "feature", "sha": "aaaa"},
                                "base": {"ref": "main", "sha": "bbbb"}
                            }
                        },
                        "created_at": "2026-10-28T07:00:00Z"
                    }])
                    .to_string(),
                });
            }
            if url.ends_with("/graphql") {
                // Home/Profile tests can distinguish the profile query from repository counts by
                // the fixed variables/fields in the body.
                if body.contains("Pinned") || body.contains("itemShowcase") {
                    return Ok(graphql_ok(serde_json::json!({
                        "user": {
                            "pinnedItems": {"totalCount": 0, "pageInfo": {"hasNextPage": false}, "nodes": []},
                            "itemShowcase": {"hasPinnedItems": false, "items": {"totalCount": 0, "pageInfo": {"hasNextPage": false}, "nodes": []}},
                            "contributionsCollection": null
                        }
                    })));
                }
                if body.contains("contributionsCollection") {
                    return Ok(graphql_ok(serde_json::json!({
                        "user": {"contributionsCollection": {"contributionCalendar": {
                            "totalContributions": 3,
                            "weeks": [{"contributionDays": [{"date": "2026-01-01", "contributionCount": 3, "contributionLevel": "FIRST_QUARTILE", "color": "#123456"}]}],
                            "months": []
                        }}}
                    })));
                }
                return Ok(graphql_ok(serde_json::json!({
                    "repository": {"databaseId": 10, "issues": {"totalCount": 4}, "pullRequests": {"totalCount": 2}}
                })));
            }
            if lower.contains("/users/carol") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 22, "login": "carol", "name": "Carol", "avatar_url": null,
                        "bio": "Another user", "followers": 2, "following": 1
                    }),
                ));
            }
            if url.contains("/repos/alice/demo") && url.ends_with("/demo") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10, "name": "demo", "full_name": "alice/demo",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "default_branch": "main",
                        "description": "Synthetic repository", "html_url": "https://github.com/alice/demo"
                    }),
                ));
            }
            Ok(json(599, serde_json::json!({})))
        })
    }
}

fn open_shell(
    cx: &mut TestAppContext,
    transport: Arc<PageTransport>,
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

fn pump(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.drain_for_test(cx));
    cx.run_until_parked();
}

fn wait_until(
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&Accounts) -> bool,
) {
    for _ in 0..600 {
        pump(controller, cx);
        std::thread::sleep(Duration::from_millis(2));
        if controller.read_with(cx, |accounts, _| done(accounts)) {
            return;
        }
    }
    panic!("page host did not settle");
}

fn login(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    let input = controller.read_with(cx, |accounts, _| accounts.token_input().clone());
    input.update(cx, |input, cx| input.set_text("synthetic-page-token", cx));
    cx.update_window_entity(controller, |accounts, window, cx| {
        accounts.submit(window, cx)
    });
    wait_until(controller, cx, |accounts| {
        accounts.snapshot().accounts.len() == 1
    });
}

#[gpui::test]
fn home_refresh_and_navigation_actions_do_not_reenter_the_host(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)) {
            break;
        }
    }
    assert!(view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)));
    // A real refresh action goes HomeView -> HomePage through a deferred dispatch. It must not
    // panic, and it must start exactly one new request for that column.
    let before = transport.seen("/search/issues");
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.refresh_home_for_test(crate::ui::home::Column::Home, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(transport.seen("/search/issues"), before + 1);
}

#[gpui::test]
fn repository_contributor_navigation_is_deferred_and_opens_a_profile(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    // Emit the exact typed action a real contributor control produces while the repository host
    // is on the update stack. It must be dispatched after that update, then reach Shell's shared
    // open-or-activate path without panicking.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate_contributor_for_test("carol", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::UserProfile {
            host: HostId::github(),
            login: "carol".to_string(),
        }
    );
}

#[gpui::test]
fn repository_star_and_fork_remain_disabled_in_production(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    // In production Shell the buttons must stay disabled because no remote write is implemented.
    assert_eq!(
        view.read_with(cx, |shell, cx| shell
            .active_repository_mutations_enabled(cx)),
        Some(false),
        "Star and Fork must remain disabled when only navigation is handled"
    );
}

#[gpui::test]
fn own_contributor_link_activates_the_existing_profile_tab(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    // Open the signed-in user's own profile from the sidebar first.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Profile, window, cx);
    });
    cx.run_until_parked();
    let own = view.read_with(cx, |shell, _| shell.active_tab_id());
    let tabs = view.read_with(cx, |shell, _| shell.tab_count());
    // Then follow the same user's contributor link from a repository page.
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate_contributor_for_test("alice", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Profile,
        "the signed-in user's contributor link resolves to the own-profile route"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        own,
        "it activates the already-open profile tab"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        tabs + 1,
        "only the repository tab was added; no second profile page"
    );
}

#[gpui::test]
fn feed_refresh_is_reenabled_when_its_server_poll_interval_expires(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)) {
            break;
        }
    }
    // The synthetic Feed response requests a one-second poll interval. Refresh stays disabled
    // while that interval runs, then the host restores the control without issuing a read.
    assert!(!view.read_with(cx, |shell, cx| shell.feed_refresh_available_for_test(cx)));
    let feed_reads = transport.seen("/received_events");
    // Drive the same expiry boundary the real timer reaches, without depending on the test
    // scheduler's wall clock or introducing a real sleep.
    view.update(cx, |shell, cx| shell.expire_feed_cooldown_for_test(cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |shell, cx| shell.feed_refresh_available_for_test(cx)));
    assert_eq!(
        transport.seen("/received_events"),
        feed_reads,
        "cooldown expiry only restores refresh; it must not poll"
    );
}

#[gpui::test]
fn the_shell_mounts_real_home_and_reads_its_two_columns(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    let _ = (&view, &controller);
    // Home/Feed use the real page-data service under the settled account. A synthetic transport
    // answers both; mounting uses the next frame, so pump until the real Home view is present.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if cx.debug_bounds("home-view").is_some() {
            break;
        }
        if let Some(page_unavailable) = cx.debug_bounds("page-service-unavailable") {
            panic!(
                "Shell kept the fallback page after login: {page_unavailable:?}; route={:?}",
                view.read_with(cx, |shell, _| shell.route())
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let dumped = controller.read_with(cx, |accounts, _| {
        (
            accounts.services().is_some(),
            accounts.snapshot().active.clone(),
            format!("{:?}", accounts.snapshot().repositories.preview),
        )
    });
    if cx.debug_bounds("home-view").is_none() {
        panic!("home not mounted: {dumped:?}");
    }
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if transport.seen("/search/issues") >= 1
            && transport.seen("/received_events") >= 1
            && view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if transport.seen("/search/issues") == 0 && transport.seen("/search/issues?") == 0 {
        panic!(
            "no search read; reads={:?} requests={:?}",
            transport.reads.lock().unwrap(),
            view.read_with(cx, |shell, cx| shell.home_debug_reads(cx))
        );
    }
    assert!(
        transport.seen("/search/issues") >= 1 || transport.seen("/search/issues?") >= 1,
        "Home requests its account-scoped involves query"
    );
    assert!(
        transport.seen("/received_events") >= 1,
        "Feed requests the received-events endpoint"
    );
    assert!(
        transport.seen("/user/following") >= 1,
        "Feed reads the followed-account set for the membership constraint"
    );
    assert!(view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)));
    // Through the real adapters: Home keeps only the open item; Feed keeps the
    // followed star and the merged contribution (author resolved from the detail
    // endpoint), dropping the push and the unfollowed account's star.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("home-item-row-0").is_some(),
        "the open Home item is visible"
    );
    assert!(
        cx.debug_bounds("home-item-row-1").is_none(),
        "the closed Home item must not have a row"
    );
    assert!(
        cx.debug_bounds("feed-event-row-0").is_some(),
        "the followed star is visible"
    );
    assert!(
        cx.debug_bounds("feed-event-row-1").is_some(),
        "the merged contribution is visible"
    );
    assert!(
        cx.debug_bounds("feed-event-row-2").is_none(),
        "the push event and the unfollowed star must not have rows"
    );
}

#[gpui::test]
fn sidebar_repository_opens_the_real_repository_page_and_keeps_it_on_activation(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let repository = view.read_with(cx, |shell, _| shell.repositories()[0].clone());
    assert!(controller.read_with(cx, |accounts, _| accounts.services().is_some()));
    let before = transport.seen("/repos/alice/demo") + transport.seen("/branches");
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&repository), window, cx);
    });
    cx.run_until_parked();
    // A real repository page was mounted for the route: the tab and page exist even when the
    // synthetic transport only models the summary and not every git endpoint.
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::repository(&repository)
    );
    assert!(cx.debug_bounds("repository-page").is_some());
    let _ = before;
}

#[gpui::test]
fn a_profile_route_canonicalizes_only_after_server_identity_arrives(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(
            Route::UserProfile {
                host: HostId::github(),
                login: "CATOL".to_string(),
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    // The route spelling is only a request target; the assertion below proves no stable numeric
    // ProfileTarget was invented before the server answered.
    assert_eq!(
        cx.update(|_, cx| { view.read_with(cx, |shell, cx| shell.profile_target_for_test(cx)) }),
        None
    );
    // The route keeps the requested (unverified) spelling; only a server response may
    // canonicalize it. No hash/login/0 is treated as a stable numeric ProfileTarget.
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::UserProfile {
            host: HostId::github(),
            login: "CATOL".to_string()
        }
    );

    // Drive the real Profile host: the synthetic transport identifies id 22 and canonical login
    // `carol`, so pins/contributions/README can then be requested for that real target.
    for _ in 0..400 {
        view.update(cx, |shell, cx| {
            shell.mount_active_page_for_test_windowless(cx)
        });
        cx.update_window_entity(&view, |shell, window, cx| {
            shell.drain_profile_for_test(window, cx)
        });
        cx.run_until_parked();
        if transport.seen("/users/") > 0 {
            cx.update_window_entity(&view, |shell, window, cx| {
                shell.drain_profile_for_test(window, cx)
            });
        }
        if view.read_with(cx, |shell, _| shell.route())
            == (Route::UserProfile {
                host: HostId::github(),
                login: "carol".to_string(),
            })
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(30));
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drain_profile_for_test(window, cx)
    });
    assert!(transport.seen("/users/") >= 1);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::UserProfile {
            host: HostId::github(),
            login: "carol".to_string()
        },
        "canonical server login replaces the requested spelling"
    );
}

#[gpui::test]
fn closing_one_repository_tab_does_not_stop_or_clear_the_other(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    let first = view.read_with(cx, |shell, _| shell.active_tab_id());
    view.update(cx, |shell, cx| {
        shell.select_repository(first, "main", Some("README.md"));
        cx.notify();
    });
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[1]), window, cx);
    });
    cx.run_until_parked();
    let second = view.read_with(cx, |shell, _| shell.active_tab_id());
    view.update(cx, |shell, cx| {
        shell.select_repository(second, "release", Some("src/main.rs"));
        cx.notify();
    });
    assert_ne!(first, second);

    // Activation keeps each mounted page entity; it does not rebuild content for ordinary tab
    // switches. Only the active page owns current interaction state.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.close_tab(first, window, cx)
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |shell, _| shell.tab_ids().contains(&first)));
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), second);
    assert_eq!(
        view.read_with(cx, |shell, _| shell
            .repository_state(second)
            .and_then(|state| state.file.clone())),
        Some("src/main.rs".to_string()),
        "closing a different tab leaves this page's navigation intact"
    );
}

#[gpui::test]
fn same_resource_activation_and_reorder_keep_one_page_instance(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    let id = view.read_with(cx, |shell, _| shell.active_tab_id());
    view.update(cx, |shell, cx| {
        shell.select_repository(id, "main", Some("README.md"));
        cx.notify();
    });
    let before = view.read_with(cx, |shell, _| shell.repository_state(id).cloned());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), id);
    view.update(cx, |shell, cx| {
        assert!(shell.workspace().reorder(id, 0));
        cx.notify();
    });
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), id);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repository_state(id).cloned()),
        before
    );
}

#[gpui::test]
fn authorization_change_drops_page_hosts_without_reusing_their_old_context(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    let tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert!(view.read_with(cx, |shell, cx| {
        shell.mounted_repository_context_for_test(tab, cx).is_some()
    }));
    // A real revalidation advances the account epoch even though the same account stays active.
    controller.update(cx, |accounts, cx| {
        accounts.revalidate(AccountKey::new(1, HostId::github()), cx);
    });
    wait_until(&controller, cx, |accounts| {
        !accounts.snapshot().accounts.is_empty()
    });
    for _ in 0..100 {
        pump(&controller, cx);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if view.read_with(cx, |shell, cx| {
            shell.mounted_repository_context_for_test(tab, cx).is_none()
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        view.read_with(cx, |shell, cx| shell
            .mounted_repository_context_for_test(tab, cx)
            .is_none()),
        "the old authorization-bound page host must be dropped"
    );
}

#[gpui::test]
fn compact_retry_state_is_readable_and_retries_once(cx: &mut TestAppContext) {
    use crate::ui::compact_status;

    cx.update(ely_core::init_for_tests);
    let (view, cx) = cx.add_window_view(|_window, _cx| {
        use gpui::{App, Context, IntoElement, Render, Window, div, prelude::*};
        struct Probe {
            retries: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let retries = self.retries.clone();
                let failed = compact_status::failed(
                    "probe-failed",
                    "Failed to load docs",
                    "rate limited; try again later",
                    Some(move |_: &mut Window, _: &mut App| retries.set(retries.get() + 1)),
                    None,
                    cx,
                );
                let loading = compact_status::loading("probe-loading", "Loading docs…", cx);
                let empty = compact_status::empty("No files", cx);
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(failed)
                    .child(loading)
                    .child(empty)
            }
        }
        Probe {
            retries: std::rc::Rc::new(std::cell::Cell::new(0)),
        }
    });
    cx.run_until_parked();
    let failed = cx
        .debug_bounds("compact-failed-Failed to load docs")
        .unwrap();
    let loading = cx.debug_bounds("compact-loading-Loading docs…").unwrap();
    assert!(failed.size.height > gpui::px(0.0));
    assert!(loading.size.height > gpui::px(0.0));
    assert!(cx.debug_bounds("compact-empty-No files").is_some());
    // One real click on the warning mark is one retry. The test entity owns the count; the
    // production pages route the same callback through their existing single-resource retry.
    cx.simulate_click(failed.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |probe, _| probe.retries.get()),
        1,
        "a warning mark carries exactly one retry"
    );
}
