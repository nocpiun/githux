//! Standalone interactive preview for the Global Issues -> Issue Detail flow inside the real Shell.
//!
//! Run the repository flow with `node scripts/verify.mjs -- cargo build --locked --example issue_detail`,
//! then `target/debug/examples/issue_detail.exe --repository`. Select `acme/widgets` in the
//! sidebar, then choose its Issues tab.
//! Open has a full page, a PR-only page, then #42 and #43. Closed fails once and retries to empty.
//! `acme/other` has an empty Closed list and a distinct #42 under Open.
//!
//! Without `--repository`, the preview starts at the Issue detail example.
//!
//! Opens the real product `Shell` backed by an in-memory credential store, a temporary SQLite
//! directory + `CacheStore`, and a deterministic fixture transport so the reviewer can:
//! - Click issue titles in Global Issues to open/reuse `Route::Issue` tabs (while repo links still
//!   open `Route::Repository`).
//! - Inspect rich Markdown body + 2-page paginated comments (`acme/widgets #42`).
//! - Verify same number in another repo (`acme/other #42`) opens a distinct tab with empty body
//!   and confirmed empty comments.
//! - Verify comments failure preserves the issue body and recovers on Retry (`acme/widgets #43`).
//! - Verify 404 missing (`acme/widgets #44`) and 403 permission denied (`acme/widgets #45`).
//! - Verify `tab-refresh` on active issue tabs and closing detail tabs back to the Issues list.

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
use serde_json::json;

use app::AccountsHandle;
use app::accounts::Accounts;
use app::accounts::runtime::AccountRuntime;
use auth::InMemoryCredentialStore;
use data::AccountService;
use github::{GitHubClient, GitHubConfig};
use network::cancel::CancellationToken;
use network::executor::BackgroundExecutor;
use network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use storage::accounts::AccountDatabaseConfig;
use storage::cache::{CacheConfig, CacheStore};
use ui::Shell;
use workspace::Route;

#[derive(Default)]
struct IssueDetailPreviewTransport {
    flaky_comments_attempts: AtomicU64,
    closed_list_attempts: AtomicU64,
}

fn json_ok(body: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.to_string(),
    }
}

fn json_status(status: u16, body: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.to_string(),
    }
}

impl Transport for IssueDetailPreviewTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let url = reqwest::Url::parse(&request.url).expect("valid url");
            let path = url.path();
            if path != "/user" {
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
            match path {
                "/user" => Ok(json_ok(json!({
                    "id": 1,
                    "login": "octocat",
                    "name": "The Octocat",
                    "avatar_url": null
                }))),
                "/user/repos" => Ok(json_ok(json!([
                    {
                        "id": 10, "name": "widgets", "full_name": "acme/widgets",
                        "owner": { "id": 2, "login": "acme", "avatar_url": null },
                        "visibility": "public", "description": "Core widget toolkit",
                        "default_branch": "main"
                    },
                    {
                        "id": 11, "name": "other", "full_name": "acme/other",
                        "owner": { "id": 2, "login": "acme", "avatar_url": null },
                        "visibility": "public", "description": "Companion repository",
                        "default_branch": "main"
                    }
                ]))),
                "/search/issues" => {
                    let query = url
                        .query_pairs()
                        .find(|(k, _)| k == "q")
                        .map(|(_, v)| v.into_owned())
                        .unwrap_or_default();
                    if query.contains("is:issue") {
                        Ok(json_ok(json!({
                            "total_count": 5,
                            "incomplete_results": false,
                            "items": [
                                {
                                    "id": 4201, "number": 42,
                                    "title": "Render read-only Issue detail with Markdown and paginated comments",
                                    "state": "open", "updated_at": "2026-11-18T14:20:00Z",
                                    "repository_url": "https://api.github.com/repos/acme/widgets",
                                    "pull_request": null
                                },
                                {
                                    "id": 4202, "number": 42,
                                    "title": "Same issue number (#42) in another repository (no body, no comments)",
                                    "state": "open", "updated_at": "2026-11-17T09:15:00Z",
                                    "repository_url": "https://api.github.com/repos/acme/other",
                                    "pull_request": null
                                },
                                {
                                    "id": 4203, "number": 43,
                                    "title": "Comments fail once with 503 while body stays visible (click Retry)",
                                    "state": "open", "updated_at": "2026-11-16T18:00:00Z",
                                    "repository_url": "https://api.github.com/repos/acme/widgets",
                                    "pull_request": null
                                },
                                {
                                    "id": 4204, "number": 44,
                                    "title": "Missing / deleted issue (returns HTTP 404)",
                                    "state": "open", "updated_at": "2026-11-15T11:30:00Z",
                                    "repository_url": "https://api.github.com/repos/acme/widgets",
                                    "pull_request": null
                                },
                                {
                                    "id": 4205, "number": 45,
                                    "title": "Permission denied issue (returns HTTP 403)",
                                    "state": "open", "updated_at": "2026-11-14T08:00:00Z",
                                    "repository_url": "https://api.github.com/repos/acme/widgets",
                                    "pull_request": null
                                }
                            ]
                        })))
                    } else {
                        Ok(json_ok(json!({
                            "total_count": 0,
                            "incomplete_results": false,
                            "items": []
                        })))
                    }
                }
                "/graphql"
                    if request
                        .body
                        .as_deref()
                        .is_some_and(|body| body.contains("RepositoryOpenCounts")) =>
                {
                    let body: serde_json::Value =
                        serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                    let widgets = body["variables"]["name"] == "widgets";
                    Ok(json_ok(json!({"data":{"repository":{
                        "databaseId": if widgets { 10 } else { 11 },
                        "issues":{"totalCount": if widgets { 32 } else { 1 }},
                        "pullRequests":{"totalCount": if widgets { 1 } else { 0 }}
                    }}})))
                }
                "/repos/acme/widgets/releases"
                | "/repos/acme/other/releases"
                | "/repos/acme/widgets/contributors"
                | "/repos/acme/other/contributors"
                | "/repos/acme/widgets/branches"
                | "/repos/acme/other/branches" => Ok(json_ok(json!([]))),
                "/repos/acme/widgets/languages" | "/repos/acme/other/languages" => {
                    Ok(json_ok(json!({})))
                }
                "/repos/acme/widgets" | "/repos/acme/other" => {
                    let name = path.rsplit('/').next().unwrap();
                    Ok(json_ok(json!({
                        "id": if name == "widgets" { 10 } else { 11 },
                        "name": name, "owner": {"id":2,"login":"acme"},
                        "private":false,"default_branch":null,"fork":false,"archived":false,
                        "stargazers_count":0,"forks_count":0,"open_issues_count":32,"size":1,
                        "html_url": format!("https://github.com/acme/{name}")
                    })))
                }
                "/repos/acme/widgets/issues"
                | "/repositories/10/issues"
                | "/repos/acme/other/issues" => Ok(self.repository_issues(&url)),
                "/repos/acme/widgets/issues/42" => Ok(json_ok(json!({
                    "id": 4201,
                    "number": 42,
                    "title": "Render read-only Issue detail with Markdown and paginated comments",
                    "state": "open",
                    "state_reason": null,
                    "user": { "id": 1, "login": "octocat", "avatar_url": null },
                    "body": "## Overview\n\nClicking an issue title in **Global Issues** opens or reuses an independent `Route::Issue` tab.\n\n### Acceptance checks\n- Shows `#42`, state badge, title, author, Markdown body, and paginated comments\n- Clicking `acme/widgets` still opens the repository tab\n- Closing this tab returns to the Issues list with its state intact\n\n```rust\nRoute::Issue {\n    namespace: \"acme\".into(),\n    name: \"widgets\".into(),\n    number: 42,\n}\n```\n\nSee [GitHub Docs](https://docs.github.com/en/rest/issues/issues).",
                    "comments": 3,
                    "labels": [
                        { "name": "enhancement", "color": "a2eeef" },
                        { "name": "first-release", "color": "0e8a16" }
                    ],
                    "created_at": "2026-11-15T09:00:00Z",
                    "updated_at": "2026-11-18T14:20:00Z"
                }))),
                "/repos/acme/widgets/issues/42/comments" => {
                    let page = url
                        .query_pairs()
                        .find(|(k, _)| k == "page")
                        .and_then(|(_, v)| v.parse::<u32>().ok())
                        .unwrap_or(1);
                    if page == 1 {
                        Ok(HttpResponse {
                            status: 200,
                            headers: vec![
                                ("content-type".into(), "application/json".into()),
                                (
                                    "link".into(),
                                    r#"<https://api.github.com/repos/acme/widgets/issues/42/comments?per_page=30&page=2>; rel="next""#.into(),
                                ),
                            ],
                            body: json!([
                                {
                                    "id": 9001,
                                    "user": { "id": 12, "login": "alice", "avatar_url": null },
                                    "body": "Confirmed that **Markdown Flow** lets the whole page scroll smoothly with one scrollbar.",
                                    "created_at": "2026-11-16T10:00:00Z",
                                    "updated_at": "2026-11-16T10:00:00Z",
                                    "author_association": "MEMBER"
                                },
                                {
                                    "id": 9002,
                                    "user": { "id": 13, "login": "bob", "avatar_url": null },
                                    "body": "Click **Load more comments** below to fetch page 2 without disturbing the issue body above.",
                                    "created_at": "2026-11-17T15:30:00Z",
                                    "updated_at": "2026-11-17T15:30:00Z",
                                    "author_association": "CONTRIBUTOR"
                                }
                            ])
                            .to_string(),
                        })
                    } else {
                        Ok(json_ok(json!([
                            {
                                "id": 9003,
                                "user": { "id": 14, "login": "carol", "avatar_url": null },
                                "body": "Page 2 loaded cleanly! Closing this tab will bring you back to the Global Issues list.",
                                "created_at": "2026-11-18T14:20:00Z",
                                "updated_at": "2026-11-18T14:20:00Z",
                                "author_association": "OWNER"
                            }
                        ])))
                    }
                }
                "/repos/acme/other/issues/42" => Ok(json_ok(json!({
                    "id": 4202,
                    "number": 42,
                    "title": "Same issue number (#42) in another repository (no body, no comments)",
                    "state": "open",
                    "state_reason": null,
                    "user": { "id": 12, "login": "alice", "avatar_url": null },
                    "body": null,
                    "comments": 0,
                    "labels": [],
                    "created_at": "2026-11-17T08:00:00Z",
                    "updated_at": "2026-11-17T09:15:00Z"
                }))),
                "/repos/acme/other/issues/42/comments" => Ok(json_ok(json!([]))),
                "/repos/acme/widgets/issues/43" => Ok(json_ok(json!({
                    "id": 4203,
                    "number": 43,
                    "title": "Comments fail once with 503 while body stays visible (click Retry)",
                    "state": "open",
                    "user": { "id": 1, "login": "octocat", "avatar_url": null },
                    "body": "Even when the comments endpoint returns `503 Service Unavailable`, this issue body remains readable and the Comments section offers an independent **Retry** button.",
                    "comments": 1,
                    "labels": [{ "name": "resilience" }],
                    "created_at": "2026-11-16T17:00:00Z",
                    "updated_at": "2026-11-16T18:00:00Z"
                }))),
                "/repos/acme/widgets/issues/43/comments" => {
                    let attempt = self.flaky_comments_attempts.fetch_add(1, Ordering::AcqRel);
                    if attempt == 0 {
                        Ok(json_status(
                            503,
                            json!({ "message": "Service unavailable" }),
                        ))
                    } else {
                        Ok(json_ok(json!([{
                            "id": 9101,
                            "user": { "id": 12, "login": "alice", "avatar_url": null },
                            "body": "Comments recovered on retry!",
                            "created_at": "2026-11-16T18:00:00Z",
                            "updated_at": "2026-11-16T18:00:00Z",
                            "author_association": "MEMBER"
                        }])))
                    }
                }
                "/repos/acme/widgets/issues/44" => {
                    Ok(json_status(404, json!({ "message": "Not Found" })))
                }
                "/repos/acme/widgets/issues/45" => {
                    Ok(json_status(403, json!({ "message": "Forbidden" })))
                }
                _ if sample_number(path).is_some() => {
                    let number = sample_number(path).unwrap();
                    Ok(json_ok(json!({
                        "id": number + 10000, "number":number, "title":format!("Sample issue #{number}"),
                        "state":"open","user":{"id":1,"login":"octocat"},
                        "body":format!("This is the Markdown body for **widgets #{number}**."),
                        "comments":0,"created_at":"2026-11-19T00:00:00Z","updated_at":"2026-11-19T00:00:00Z"
                    })))
                }
                _ if path
                    .strip_suffix("/comments")
                    .and_then(sample_number)
                    .is_some() =>
                {
                    Ok(json_ok(json!([])))
                }
                _ => Ok(json_status(404, json!({ "message": "Not Found" }))),
            }
        })
    }

    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_bytes: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max_bytes {
                return Err(TransportError::ResponseTooLarge { limit: max_bytes });
            }
            Ok(response)
        })
    }
}

impl IssueDetailPreviewTransport {
    fn repository_issues(&self, url: &reqwest::Url) -> HttpResponse {
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
        let state = params.get("state").map(|value| value.as_ref());
        let page = params.get("page").map(|value| value.as_ref());
        let row = |number, title: String, state: &str| {
            json!({
                "number":number,"title":title,"state":state,"updated_at":"2026-11-19T00:00:00Z"
            })
        };
        let other = url.path() == "/repos/acme/other/issues";
        let (rows, next) = match (other, state, page) {
            (true, Some("closed"), Some("1")) => (vec![], None),
            (true, Some("open"), Some("1")) => (
                vec![row(
                    42,
                    "Same issue number (#42) in another repository (no body, no comments)".into(),
                    "open",
                )],
                None,
            ),
            (false, Some("closed"), Some("1")) => {
                if self.closed_list_attempts.fetch_add(1, Ordering::AcqRel) == 0 {
                    return json_status(503, json!({"message":"Synthetic list failure"}));
                }
                (vec![], None)
            }
            (false, Some("open"), Some("1")) => (
                (101..=130)
                    .rev()
                    .map(|number| row(number, format!("Sample issue #{number}"), "open"))
                    .collect(),
                Some(2),
            ),
            (false, Some("open"), Some("2")) => {
                (vec![json!({"number":100,"pull_request":{}})], Some(3))
            }
            (false, Some("open"), Some("3")) => (
                vec![
                    row(
                        42,
                        "Render read-only Issue detail with Markdown and paginated comments".into(),
                        "open",
                    ),
                    row(
                        43,
                        "Comments fail once with 503 while body stays visible (click Retry)".into(),
                        "open",
                    ),
                ],
                None,
            ),
            _ => {
                return json_status(
                    599,
                    json!({"message":"Unmodelled repository Issues request"}),
                );
            }
        };
        let mut response = json_ok(json!(rows));
        if let Some(next) = next {
            response.headers.push(("link".into(), format!(
                "<https://api.github.com/repositories/10/issues?state=open&sort=created&direction=desc&per_page=30&page={next}&after=preview-{next}>; rel=\"next\""
            )));
        }
        response
    }
}

fn sample_number(path: &str) -> Option<u64> {
    path.strip_prefix("/repos/acme/widgets/issues/")?
        .parse()
        .ok()
        .filter(|number| (101..=130).contains(number))
}

/// Wait for the real signed-in account workspace to become active (handling async account
/// directory restoration without fixed sleeps or altering product default Home), then open
/// `Route::Issues` followed by `acme/widgets #42` exactly once.
fn install_preview_bootstrap(
    controller: &gpui::Entity<Accounts>,
    shell: &gpui::Entity<Shell>,
    window: &mut gpui::Window,
    cx: &mut App,
) {
    let shell_weak = shell.downgrade();
    let window_handle = window.window_handle();
    let mut opened = false;
    let mut maybe_open = move |accounts: gpui::Entity<Accounts>, cx: &mut App| {
        if opened || accounts.read(cx).snapshot().active.is_none() {
            return;
        }
        opened = true;
        let shell_weak = shell_weak.clone();
        let _ = window_handle.update(cx, move |_, window, cx| {
            // Defer one turn so Shell's own Accounts observer has already run `sync_accounts`
            // and switched to the signed-in account's workspace.
            window.defer(cx, move |window, cx| {
                let _ = shell_weak.update(cx, |shell, cx| {
                    // Open Global Issues first so closing the detail tab returns to it as the
                    // left neighbour, then open `acme/widgets #42` directly for out-of-box review.
                    shell.navigate(Route::Issues, window, cx);
                    shell.navigate(
                        Route::Issue {
                            namespace: "acme".into(),
                            name: "widgets".into(),
                            number: 42,
                        },
                        window,
                        cx,
                    );
                });
            });
        });
    };

    maybe_open(controller.clone(), cx);
    cx.observe(controller, move |accounts, cx| {
        maybe_open(accounts, cx);
    })
    .detach();
}

fn main() {
    let repository_preview = std::env::args().any(|arg| arg == "--repository");
    let temp = tempfile::tempdir().expect("temp dir");
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let executor = Arc::new(BackgroundExecutor::new(4));
    let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).expect("cache store");
    let service = tokio_rt.block_on(async {
        let service = AccountService::open_with_cache(
            AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(
                GitHubConfig::default(),
                Arc::new(IssueDetailPreviewTransport::default()),
            )),
            executor.clone(),
            cache,
        )
        .await
        .expect("preview account service");
        service
            .login(auth::token::Pat::new("ghp_preview_token".to_string()))
            .await
            .expect("preview login");
        service
    });
    let runtime = Arc::new(AccountRuntime::new(service, executor));

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
                        title: Some("Githux — Issue Detail Preview".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let controller = cx.new(|cx| Accounts::new(Some(runtime.clone()), window, cx));
                    cx.set_global(AccountsHandle(controller.clone()));
                    let shell = cx.new(|cx| Shell::new(window, cx));
                    if !repository_preview {
                        install_preview_bootstrap(&controller, &shell, window, cx);
                    }
                    shell
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
