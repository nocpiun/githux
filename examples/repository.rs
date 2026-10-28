//! Standalone preview of the reusable repository browsing page.
//!
//! Run with `cargo run --example repository`. The example assembles the real
//! page, reader and scroll modules through `src/bin_support/repository_modules.rs`
//! and drives the **real** data pipeline (`AccountService` → `RepoService` →
//! snapshot/tree/blob → `PreparedContent` → `ReaderView`).
//!
//! Everything external is deterministic and injected:
//!
//! * a scripted in-memory [`Transport`] that answers the exact REST calls the
//!   page makes, so no request leaves the machine;
//! * a temporary SQLite directory and an in-memory credential store holding one
//!   synthetic PAT, so no real credential is read.
//!
//! The product binary never declares these modules or this entry point, and the
//! release artifact contains no `examples/repository.exe`.
//!
//! ## What a production host does instead
//!
//! A real host owns the service and passes the already authenticated
//! capabilities; nothing in this file is needed for production:
//!
//! ```ignore
//! let services = RepositoryPageServices::new(accounts.clone(), executor.clone());
//! let page = cx.new(|cx| {
//!     let mut page = RepositoryPage::new(services, locator, saved_intent, cx);
//!     page.activate(cx);            // opens a fresh, verified repository session
//!     page
//! });
//! // Teardown: page.deactivate(cx) cancels reads and keeps navigation intent;
//! // rebind(services, cx) restores that intent under new authorization.
//! ```

// Keep the preview console-free on Windows, like the product binary. The test
// harness stays separate and keeps its console.
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

#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/bin_support/repository_modules.rs"]
mod repository_modules;
#[allow(unused_imports)]
use repository_modules as ui;
#[allow(unused_imports)]
use repository_modules::{reader, repository};

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ely_core::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use crate::auth::token::Pat;
use crate::data::AccountService;
use crate::domain::repository::RepositoryLocator;
use crate::github::GitHubClient;
use crate::network::cancel::CancellationToken;
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use crate::repository::navigation::RepositoryNavigation;
use crate::repository::{PageAction, RepositoryPage, RepositoryPageServices};
use crate::storage::accounts::AccountDatabaseConfig;

fn main() {
    // Every external boundary is injected before the window opens: a scripted
    // transport, a temporary account directory and one synthetic PAT.
    let fixture = FixtureRuntime::new();
    let services = RepositoryPageServices::new(fixture.accounts.clone(), fixture.executor.clone());
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_core::init(cx).expect("Ely failed to start");
            fonts::register(cx).expect("the bundled fonts failed to register");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = fonts::UI_FAMILY.into();
                theme.mono_family = fonts::CODE_FAMILY.into();
            });
            let bounds = Bounds::centered(None, size(px(1180.0), px(820.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(820.0), px(520.0))),
                // The platform title bar keeps this a preview, not a second Shell.
                titlebar: Some(TitlebarOptions {
                    title: Some("Githux · Repository page preview".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    let mut page = RepositoryPage::new(
                        services.clone(),
                        RepositoryLocator::new("octo", "demo").expect("fixture locator"),
                        RepositoryNavigation::default(),
                        window,
                        cx,
                    );
                    page.on_action(std::rc::Rc::new(|action, _window, _cx| match action {
                        // The preview reports actions instead of opening a
                        // browser or navigating a different repository.
                        PageAction::OpenExternal(url) => {
                            eprintln!("repository preview: external link not opened: {url}");
                        }
                        PageAction::LinkNotFollowed { path, reason } => {
                            eprintln!("repository preview: link {path} not followed: {reason}");
                        }
                    }));
                    page.activate(cx);
                    page
                })
            })
            .expect("the repository preview window failed to open");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    // Dropping the fixture closes the temporary account database and lets the
    // existing executor drain on its own thread.
    drop(fixture);
}

/// Owns the fixture service stack for the whole preview.
struct FixtureRuntime {
    accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
    _temp: tempfile::TempDir,
    _runtime: tokio::runtime::Runtime,
}

impl FixtureRuntime {
    /// Builds the real service stack over a scripted transport and a temporary
    /// account directory. Nothing here reads a real credential or the network.
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary account directory");
        let transport = Arc::new(ScriptedTransport::demo());
        let executor = Arc::new(BackgroundExecutor::new(2));
        let store = Arc::new(auth::InMemoryCredentialStore::new());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime");
        let accounts = runtime.block_on(async {
            let accounts = AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                store,
                Arc::new(GitHubClient::new(Default::default(), transport)),
                executor.clone(),
            )
            .await
            .expect("open fixture account service");
            accounts
                .login(Pat::from_str("synthetic-preview-token"))
                .await
                .expect("synthetic login");
            Arc::new(accounts)
        });
        Self {
            accounts,
            executor,
            _temp: temp,
            _runtime: runtime,
        }
    }
}

// No explicit shutdown: the executor's own `Drop` (when the last reference is
// released) stops accepting work, drains tracked tasks and joins its thread.

/// A deterministic transport: every scripted request is answered in order.
///
/// It is fixture material for the preview only; the production transport is the
/// existing `ReqwestTransport` and is untouched.
struct ScriptedTransport {
    responses: Mutex<VecDeque<HttpResponse>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl ScriptedTransport {
    fn demo() -> Self {
        let mut responses = VecDeque::new();
        let commit = "c0ffee0000000000000000000000000000000000";
        let root = "1111111111111111111111111111111111111111";
        let docs = "2222222222222222222222222222222222222222";
        let readme_blob = "3333333333333333333333333333333333333333";
        // GET /user (synthetic login)
        responses.push_back(json_response(
            200,
            &serde_json::json!({"id": 7, "login": "octo"}).to_string(),
        ));
        // GET /repos/octo/demo
        responses.push_back(json_response(
            200,
            &serde_json::json!({
                "id": 42,
                "name": "demo",
                "owner": {"id": 7, "login": "octo"},
                "description": "Deterministic repository preview with no real network.",
                "visibility": "public",
                "private": false,
                "default_branch": "main",
                "fork": false,
                "archived": false,
                "stargazers_count": 128,
                "forks_count": 9,
                "open_issues_count": 4,
                "subscribers_count": 12,
                "size": 64,
                "html_url": "https://example.invalid/octo/demo",
                "clone_url": "https://example.invalid/octo/demo.git",
                "pushed_at": "2026-10-20T10:00:00Z"
            })
            .to_string(),
        ));
        // GET /repos/octo/demo/branches
        responses.push_back(json_response(
            200,
            &serde_json::json!([
                {"name": "main", "commit": {"sha": commit}, "protected": true},
                {"name": "docs", "commit": {"sha": "beef000000000000000000000000000000000000"}, "protected": false}
            ])
            .to_string(),
        ));
        // GET /repos/octo/demo/commits/main
        responses.push_back(json_response(
            200,
            &serde_json::json!({
                "sha": commit,
                "commit": {
                    "message": "Fixture commit",
                    "tree": {"sha": root},
                    "author": {"name": "octo", "email": null, "date": "2026-10-20T10:00:00Z"},
                    "committer": {"name": "octo", "email": null, "date": "2026-10-20T10:00:00Z"}
                }
            })
            .to_string(),
        ));
        // GET /repos/octo/demo/git/trees/{root}
        responses.push_back(json_response(
            200,
            &serde_json::json!({
                "sha": root,
                "truncated": false,
                "tree": [
                    {"path": "README.md", "mode": "100644", "type": "blob", "sha": readme_blob, "size": README.len()},
                    {"path": "docs", "mode": "040000", "type": "tree", "sha": docs},
                    {"path": "src", "mode": "040000", "type": "tree", "sha": "5555555555555555555555555555555555555555"}
                ]
            })
            .to_string(),
        ));
        // GET /repos/octo/demo/readme?ref=...
        responses.push_back(json_response(
            200,
            &serde_json::json!({
                "type": "file",
                "path": "README.md",
                "sha": readme_blob,
                "size": README.len()
            })
            .to_string(),
        ));
        // GET /repos/octo/demo/git/blobs/{readme_blob}
        responses.push_back(blob_response(readme_blob, README.as_bytes().to_vec()));
        Self {
            responses: Mutex::new(responses),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl Transport for ScriptedTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            self.requests.lock().unwrap().push(request);
            let response = self.responses.lock().unwrap().pop_front();
            match response {
                Some(response) => Ok(response),
                // The demo only scripts the first screen; a further request is a
                // typed 404 rather than a silent empty result.
                None => Ok(json_response(
                    404,
                    &serde_json::json!({"message": "not scripted in the preview"}).to_string(),
                )),
            }
        })
    }

    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
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
}

fn blob_response(sha: &str, bytes: Vec<u8>) -> HttpResponse {
    use base64::Engine;
    let content = base64::engine::general_purpose::STANDARD.encode(&bytes);
    json_response(
        200,
        &serde_json::json!({
            "sha": sha,
            "encoding": "base64",
            "size": bytes.len(),
            "content": content
        })
        .to_string(),
    )
}

const README: &str = r#"# Repository preview

This README comes from the **real** `RepositorySession` → `fetch_blob` →
`PreparedContent` → `ReaderView` pipeline. The only fakes are the transport
responses, the temporary account database and the synthetic PAT.

- expanding `docs` loads exactly one more tree
- selecting a file shows the read-only source view
- [a relative link](docs/guide.md) is followed at this commit
- [an external link](https://example.com) is reported, never opened

```rust
fn greeting() -> &'static str {
    "你好，repository preview"
}
```
"#;
