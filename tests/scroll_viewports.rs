//! Geometry through the production Shell and page hosts, with offline HTTP fixtures.
// The test mounts the production host directly; app::run is an unused startup entry here.
#[allow(dead_code)]
#[path = "../src/app/mod.rs"]
mod app;
#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/platform/mod.rs"]
pub mod platform;
#[path = "../src/bin_support/test_repository_modules.rs"]
mod repository_modules;
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/ui/mod.rs"]
mod ui;
#[path = "../src/workspace/mod.rs"]
mod workspace;

use gpui::{AppContext, TestAppContext, VisualTestContext, px, size};
use network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use std::{sync::Arc, time::Duration};
type TransportFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
>;
use repository_modules::fixture::{FixtureTransport, NAME, OWNER, Scene};

struct GeometryTransport(FixtureTransport, std::sync::Mutex<Vec<String>>);
impl Transport for GeometryTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: network::cancel::CancellationToken,
    ) -> TransportFuture<'a> {
        let url = reqwest::Url::parse(&request.url).unwrap();
        let path = url.path();
        self.1.lock().unwrap().push(path.to_string());
        let value = if path == "/user/repos" {
            Some(serde_json::json!([]))
        } else if path == "/user/following" {
            Some(serde_json::json!([{ "id": 7, "login": "actor" }]))
        } else if path.ends_with("/received_events") {
            Some(serde_json::Value::Array((0..30).map(|i| serde_json::json!({
                "id": format!("event-{i}"), "type": "WatchEvent", "actor": { "id": 7, "login": "actor" },
                "repo": { "id": 1, "name": format!("{OWNER}/{NAME}") }, "public": true,
                "created_at": "2026-11-01T00:00:00Z", "payload": { "action": "started" }
            })).collect()))
        } else if path == "/search/issues" {
            Some(
                serde_json::json!({"total_count": 30, "incomplete_results": false, "items": (0..30).map(|i| serde_json::json!({
                "id": i+1, "number": i+1, "title": format!("Work item {i}"), "state": "open",
                "html_url": format!("https://github.com/{OWNER}/{NAME}/issues/{}", i+1),
                "repository_url": format!("https://api.github.com/repos/{OWNER}/{NAME}"),
                "updated_at": "2026-11-01T00:00:00Z", "pull_request": null
            })).collect::<Vec<_>>() }),
            )
        } else {
            None
        };
        if let Some(value) = value {
            return Box::pin(async move {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: value.to_string(),
                })
            });
        }
        self.0.execute(request, cancel)
    }
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: network::cancel::CancellationToken,
        limit: usize,
    ) -> TransportFuture<'a> {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > limit {
                return Err(TransportError::ResponseTooLarge { limit });
            }
            Ok(response)
        })
    }
}
fn frames(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|w, _| w.refresh());
        cx.run_until_parked();
    }
}
fn bounds(cx: &mut VisualTestContext, name: &str) -> gpui::Bounds<gpui::Pixels> {
    cx.debug_bounds(Box::leak(name.to_string().into_boxed_str()))
        .unwrap_or_else(|| panic!("missing {name}"))
}

#[gpui::test]
fn production_scroll_viewports_keep_headers_first_rows_and_keyboard_reveal_inside_page(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        platform::fonts::register(cx).unwrap();
        ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = true);
        ui::Shell::bind_keys(cx);
    });
    let executor = Arc::new(network::executor::BackgroundExecutor::new(4));
    let transport = Arc::new(GeometryTransport(
        FixtureTransport::new(Scene::LongTree),
        Default::default(),
    ));
    let accounts = data::AccountService::new(
        Arc::new(auth::InMemoryCredentialStore::new()),
        Arc::new(github::GitHubClient::new(
            github::GitHubConfig::default(),
            transport.clone(),
        )),
        executor.clone(),
    );
    executor
        .raw_handle()
        .block_on(accounts.login(auth::token::Pat::from_str("synthetic-geometry")))
        .unwrap();
    let runtime = Arc::new(app::accounts::runtime::AccountRuntime::new(
        accounts, executor,
    ));
    let (shell, cx) = cx.add_window_view(|window, cx| {
        let accounts = cx.new(|cx| app::accounts::Accounts::new(Some(runtime), window, cx));
        cx.set_global(app::AccountsHandle(accounts));
        ui::Shell::new(window, cx)
    });
    cx.update(|w, _| w.activate_window());
    let accounts = cx.read(app::AccountsHandle::get).unwrap();
    for _ in 0..400 {
        accounts.update(cx, |a, cx| a.drain_for_test(cx));
        frames(cx);
        if accounts.read_with(cx, |a, _| {
            matches!(
                a.snapshot().repositories.preview,
                ui::accounts::RepositoryPreview::Ready(_)
            )
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    cx.update(|window, cx| shell.update(cx, |s, cx| s.home_mount_for_test(window, cx)));
    for _ in 0..400 {
        accounts.update(cx, |a, cx| a.drain_for_test(cx));
        shell.update(cx, |s, cx| s.drain_home_for_test(cx));
        frames(cx);
        if cx.debug_bounds("feed-event-row-0").is_some()
            && cx.debug_bounds("home-item-row-0").is_some()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        cx.debug_bounds("home-item-row-0").is_some()
            && cx.debug_bounds("feed-event-row-0").is_some(),
        "host={:?}; home status={:?}, empty={:?}; requests={:?}",
        shell.read_with(cx, |s, cx| s.home_debug_reads(cx)),
        cx.debug_bounds("home-status"),
        cx.debug_bounds("home-empty"),
        transport.1.lock().unwrap()
    );
    for (dimensions, font_scale) in [
        (size(px(1180.0), px(800.0)), 1.0),
        (size(px(880.0), px(1000.0)), 1.0),
        (size(px(1180.0), px(1200.0)), 1.25),
        (size(px(880.0), px(1400.0)), 1.25),
    ] {
        cx.update(|_, cx| {
            ely_core::theme::Theme::update(cx, |theme| theme.font_scale = font_scale)
        });
        cx.simulate_resize(dimensions);
        frames(cx);
        let page = bounds(cx, "island-page");
        for name in ["home", "feed"] {
            let header = bounds(cx, &format!("{name}-header"));
            let rows = bounds(cx, &format!("{name}-rows"));
            let first = bounds(
                cx,
                if name == "home" {
                    "home-item-row-0"
                } else {
                    "feed-event-row-0"
                },
            );
            assert!(header.top() >= page.top() && header.bottom() <= page.bottom());
            assert!(
                first.top() >= rows.top() && first.bottom() <= rows.bottom(),
                "{name}: first={first:?} rows={rows:?}"
            );
            assert!((rows.top() - header.bottom()).abs() < px(0.01));
            assert!((rows.bottom() - page.bottom()).abs() < px(0.01));
            let focus_point = gpui::point(rows.right() - px(0.5), rows.top() + px(2.0));
            cx.simulate_click(focus_point, gpui::Modifiers::none());
            cx.simulate_keystrokes("end");
            frames(cx);
            let last = bounds(
                cx,
                if name == "home" {
                    "home-item-row-29"
                } else {
                    "feed-event-row-29"
                },
            );
            assert!(
                last.top() >= rows.top() - px(1.0) && last.bottom() <= rows.bottom() + px(1.0),
                "{name}: last={last:?} rows={rows:?}"
            );
            cx.simulate_keystrokes("home");
            frames(cx);
            // Real mounted host: select a measured row, then partially scroll it across
            // the fixed header boundary. Its actual paint must not lose another band.
            cx.simulate_keystrokes("down");
            cx.simulate_keystrokes("down");
            frames(cx);
            cx.simulate_mouse_move(rows.center(), None, gpui::Modifiers::none());
            cx.simulate_event(gpui::ScrollEvent {
                position: rows.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-17.0))),
                modifiers: gpui::Modifiers::none(),
                touch_phase: Default::default(),
                source: gpui::ScrollSource::Pointer,
            });
            frames(cx);
            let row = bounds(
                cx,
                if name == "home" {
                    "home-item-row-2"
                } else {
                    "feed-card-2"
                },
            );
            assert!(row.top() < header.bottom() && row.bottom() > header.bottom());
            cx.update(|window, _| {
                let scale = window.scale_factor();
                let quad = window
                    .painted_quads()
                    .into_iter()
                    .find(|quad| {
                        quad.bounds == row.scale(scale) && !quad.background.is_transparent()
                    })
                    .expect("mounted edge row is painted");
                let visible = quad.bounds.intersect(&quad.content_mask.bounds);
                let probe = gpui::point(row.center().x, header.bottom() + px(0.5));
                let probe = gpui::Bounds::new(probe, size(px(0.0), px(0.0)))
                    .scale(scale)
                    .origin;
                assert!(
                    visible.contains(&probe),
                    "mounted page clip leaves an invisible band: {visible:?} {probe:?}"
                );
            });
            cx.simulate_keystrokes("home");
            frames(cx);
            cx.update(|window, cx| window.focus_next(cx));
            frames(cx);
            let focused_header = bounds(cx, &format!("{name}-header"));
            assert_eq!(
                focused_header.top(),
                header.top(),
                "focusing a content link must not scroll the whole mounted page"
            );
        }
    }
    cx.update(|w, cx| {
        shell.update(cx, |s, cx| {
            s.navigate(
                workspace::Route::Repository {
                    namespace: OWNER.into(),
                    name: NAME.into(),
                },
                w,
                cx,
            );
        })
    });
    for _ in 0..400 {
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| s.drain_repository_star_for_test(window, cx))
        });
        frames(cx);
        if cx.debug_bounds("repository-row-file-0000.txt").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        cx.debug_bounds("repository-row-file-0000.txt").is_some(),
        "repo root={:?}, error={:?}, tree={:?}; requests={:?}; mismatches={:?}",
        cx.debug_bounds("repository-page-root"),
        cx.debug_bounds("repository-error"),
        cx.debug_bounds("repository-tree-viewport"),
        transport.1.lock().unwrap(),
        transport.0.mismatches()
    );
    let first = bounds(cx, "repository-row-file-0000.txt");
    let viewport = bounds(cx, "repository-tree-scroll-viewport");
    let header = bounds(cx, "repository-tree-header");
    assert!(viewport.top() >= header.bottom());
    assert_eq!(first.top(), viewport.top());
    assert!(first.bottom() <= viewport.bottom());
    cx.simulate_click(first.center(), gpui::Modifiers::none());
    cx.simulate_keystrokes("end");
    frames(cx);
    let last = bounds(cx, "repository-row-file-0399.txt");
    assert!(last.top() >= viewport.top() && last.bottom() <= viewport.bottom());
    cx.simulate_keystrokes("home");
    frames(cx);
    assert_eq!(
        bounds(cx, "repository-row-file-0000.txt").top(),
        viewport.top()
    );
    // The fixed sidebar has a separate scroll handle and must reveal both edges
    // without moving its heading or footer, including the collapsed rail.
    shell.update(cx, |shell, cx| {
        shell.set_repositories(
            (0..80)
                .map(|i| domain::RepoSummary {
                    id: i + 1,
                    full_name: format!("owner/repo-{i}"),
                    name: format!("repo-{i}"),
                    owner: domain::RepoOwner {
                        id: 1,
                        login: "owner".into(),
                        avatar_url: None,
                    },
                    visibility: domain::RepoVisibility::Public,
                    fork: false,
                    archived: false,
                    description: None,
                    default_branch: Some("main".into()),
                })
                .collect(),
            cx,
        );
    });
    frames(cx);
    for collapsed in [false, true] {
        if collapsed {
            let toggle = bounds(cx, "sidebar-toggle");
            cx.simulate_click(toggle.center(), gpui::Modifiers::none());
            frames(cx);
        }
        for index in [79, 0] {
            let focus = shell.read_with(cx, |shell, _| shell.repo_focus()[index].clone());
            cx.update(|w, cx| w.focus(&focus, cx));
            frames(cx);
            let row = bounds(cx, &format!("repo-row-{index}"));
            let list = bounds(cx, "repo-scroll");
            assert!(row.top() >= list.top() && row.bottom() <= list.bottom());
        }
    }
}
