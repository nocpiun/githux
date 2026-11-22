//! Standalone interactive preview for the Shell `tab-refresh` control across all routes.
//!
//! Run with `cargo run --locked --example shell_refresh`.
//!
//! Opens the real product `Shell` backed by an in-memory credential store, a temporary SQLite
//! directory, and a deterministic delayed fixture transport (650ms per request) so the reviewer
//! can click `tab-refresh` on Home, Profile, Repository, Notifications, Issues, Pull Requests,
//! and Settings to inspect the loading spinner animation, duplicate-click coalescing, tab
//! switching, and disabled-route semantics without a real PAT.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

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
#[path = "../src/storage/mod.rs"]
mod storage;

#[path = "../src/app/mod.rs"]
#[allow(dead_code)]
mod app;
#[path = "../src/platform/mod.rs"]
#[allow(dead_code)]
mod platform;
#[path = "../src/ui/mod.rs"]
#[allow(dead_code)]
mod ui;
#[path = "../src/workspace/mod.rs"]
#[allow(dead_code)]
mod workspace;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ely_core::{Assets, theme::Theme};
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

use app::AccountsHandle;
use app::accounts::Accounts;
use app::accounts::runtime::AccountRuntime;
use auth::InMemoryCredentialStore;
use data::AccountService;
use github::{GitHubClient, GitHubConfig};
use network::executor::BackgroundExecutor;
use network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use storage::accounts::AccountDatabaseConfig;
use ui::Shell;

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

#[derive(Default)]
struct DelayedPreviewTransport {
    revision: AtomicU64,
}

impl Transport for DelayedPreviewTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: network::cancel::CancellationToken,
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
        cancel: network::cancel::CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let delay_ms = if request.url.ends_with("/user") {
                60
            } else {
                650
            };
            tokio::select! {
                _ = cancel.cancelled() => return Err(TransportError::Cancelled),
                _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
            }
            let rev = self.revision.fetch_add(1, Ordering::Relaxed) + 1;
            let url = request.url.as_str();
            let lower = url.to_ascii_lowercase();
            let body_str = request.body.as_deref().unwrap_or("");

            if url.ends_with("/user") {
                return Ok(json(
                    200,
                    serde_json::json!({"id": 1, "login": "alice", "name": "Alice", "avatar_url": null}),
                ));
            }
            if url.contains("/user/repos") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 10, "name": "demo", "full_name": "alice/demo",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "description": "Active-page refresh preview repo",
                        "default_branch": "main"
                    }]),
                ));
            }
            if url.contains("/user/starred/alice/demo") {
                return Ok(json(404, serde_json::json!({})));
            }
            if url.contains("/notifications") {
                return Ok(HttpResponse {
                    status: 200,
                    headers: vec![("x-poll-interval".into(), "0".into())],
                    body: serde_json::json!([{
                        "id": "9001", "unread": true, "reason": "mention",
                        "updated_at": "2026-11-19T10:00:00Z", "last_read_at": null,
                        "subject": {
                            "title": format!("Refreshable notification (sync #{rev})"),
                            "type": "Issue",
                            "url": "https://github.com/alice/demo/issues/1",
                            "latest_comment_url": null
                        },
                        "repository": {
                            "id": 10, "name": "demo", "full_name": "alice/demo",
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
                        "id": 102, "number": 9,
                        "title": format!("Wire active-page refresh (sync #{rev})"),
                        "html_url": "https://github.com/alice/demo/pull/9",
                        "state": "open", "updated_at": "2026-11-19T10:00:00Z",
                        "repository_url": "https://api.github.com/repos/alice/demo",
                        "pull_request": {"merged_at": null}
                    })
                } else {
                    serde_json::json!({
                        "id": 101, "number": 7,
                        "title": format!("Verify tab-refresh spinner (sync #{rev})"),
                        "html_url": "https://github.com/alice/demo/issues/7",
                        "state": "open", "updated_at": "2026-11-19T10:00:00Z",
                        "repository_url": "https://api.github.com/repos/alice/demo",
                        "pull_request": null
                    })
                };
                return Ok(json(
                    200,
                    serde_json::json!({"total_count": 1, "incomplete_results": false, "items": [item]}),
                ));
            }
            if url.contains("/user/following") {
                return Ok(json(
                    200,
                    serde_json::json!([{"id": 7, "login": "bob", "avatar_url": null}]),
                ));
            }
            if url.contains("/received_events") {
                return Ok(HttpResponse {
                    status: 200,
                    headers: vec![("x-poll-interval".into(), "0".into())],
                    body: serde_json::json!([{
                        "id": format!("evt-{rev}"), "type": "WatchEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"action": "started"},
                        "created_at": "2026-11-19T09:30:00Z"
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
                                    "totalContributions": 12 + rev,
                                    "weeks": [{
                                        "contributionDays": [{
                                            "date": "2026-11-19",
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
                                "description": "Active-page refresh preview repo",
                                "isPrivate": false, "stargazerCount": 12 + rev, "forkCount": 2,
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
                        "bio": format!("Preview profile (sync #{rev})"),
                        "followers": 10, "following": 2, "public_repos": 1, "public_gists": 0
                    }),
                ));
            }
            if lower.ends_with("/repos/alice/alice") {
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
                        "commit": {"tree": {"sha": "2222222222222222222222222222222222222222"}}
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
                        "tree": [{
                            "path": "README.md", "mode": "100644", "type": "blob",
                            "sha": "3333333333333333333333333333333333333333", "size": 24
                        }]
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
                        "size": 24, "encoding": "base64",
                        "content": "IyBEZW1vIFJFQURNRSBwcmV2aWV3Cg=="
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
                        "stargazers_count": 12 + rev, "forks_count": 2,
                        "open_issues_count": 4, "size": 1,
                        "description": format!("Active-page refresh preview repo (sync #{rev})"),
                        "html_url": "https://github.com/alice/demo"
                    }),
                ));
            }
            Ok(json(599, serde_json::json!({"unmodelled": url})))
        })
    }
}

fn main() {
    let temp = tempfile::tempdir().expect("preview temp dir");
    let executor = Arc::new(BackgroundExecutor::new(4));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let accounts = rt.block_on(async {
        let service = AccountService::open(
            AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(
                GitHubConfig::default(),
                Arc::new(DelayedPreviewTransport::default()),
            )),
            executor.clone(),
        )
        .await
        .expect("preview account service");
        service
            .login(auth::token::Pat::new("preview-token".to_string()))
            .await
            .expect("preview login");
        service
    });
    let runtime = Arc::new(AccountRuntime::new(accounts, executor));

    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_core::init(cx).expect("Ely failed to start");
            platform::fonts::register(cx).expect("bundled fonts failed to register");
            app::theme::install(cx);
            Theme::update(cx, |theme| {
                theme.font_family = platform::fonts::UI_FAMILY.into();
                theme.mono_family = platform::fonts::CODE_FAMILY.into();
            });
            Shell::bind_keys(cx);
            let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
            let runtime = runtime.clone();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("Githux — Active Page Refresh Preview".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let controller = cx.new(|cx| Accounts::new(Some(runtime.clone()), window, cx));
                    cx.set_global(AccountsHandle(controller));
                    cx.new(|cx| Shell::new(window, cx))
                },
            )
            .expect("open preview window");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    app::accounts::wait_for_shutdown();
    drop(temp);
}
