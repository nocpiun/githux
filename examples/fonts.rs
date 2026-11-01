//! Native acceptance for the production Shell's bundled Noto Sans + Noto Sans SC UI glyphs.
//!
//! `cargo run --locked --example fonts` opens the real Shell/account/page/reader composition
//! over a temporary database, in-memory synthetic credential, and deterministic transport.
//! It never opens the user's saved accounts or accesses the network. The README and account
//! label contain the owner's Chinese acceptance samples. Native shaping is asserted against
//! the generated font's cmap (all weights, no system-fallback runs or missing glyphs), and
//! evidence is written under `.agents/output/fonts/sc/`.
//! The optional `--baseline=<directory>` loads locally prepared Latin-only faces with the same
//! private family name for a before/after comparison. It is development-only, never a product flag.

// Keep the preview console-free on Windows, like the product. Tests keep their console output in a
// separate integration target.
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

// The product modules this developer target reuses. They are declared at the top level so
// `crate::app::accounts::Accounts` and `crate::ui::accounts::…` are the very product types, with no
// second, incompatible copy.
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

#[path = "../src/bin_support/repository_fixture.rs"]
#[allow(dead_code)]
mod fixture;

use ely_core::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use network::cancel::CancellationToken;
use network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use std::sync::Arc;

const SAMPLES: &str = "Minecraft 服务器管理面板 简介 功能特性 菜单栏金额 额度显示 中文";
const README: &str = "# Minecraft 服务器管理面板

## 简介

Minecraft 服务器管理面板，中英混排 Latin UI Noto Sans。

## 功能特性

菜单栏金额 · 额度显示 · 中文

普通中文 Regular，**中粗中文与 Bold English**。

```rust
fn main() { println!(\"Minecraft 中文\"); }
```
";

struct FontTransport(fixture::FixtureTransport);
impl Transport for FontTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        limit: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > limit {
                return Err(TransportError::ResponseTooLarge { limit });
            }
            Ok(response)
        })
    }
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            if request.url.contains("/user/repos") {
                return Ok(network::transport::json_response(
                    200,
                    &serde_json::json!([{
                        "id":10,"name":"ledger","full_name":"octo/ledger", "visibility":"public",
                        "owner":{"id":1,"login":"octo","avatar_url":null},"default_branch":"main"
                    }])
                    .to_string(),
                ));
            }
            let url = request.url.clone();
            let mut response = self.0.execute(request, cancel).await?;
            if url.ends_with("/user") {
                let mut body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
                body["name"] = "菜单栏金额 · 额度显示 · 中文".into();
                response.body = body.to_string();
            }
            if url.contains("/readme")
                || url.contains("/git/blobs/b000000000000000000000000000000000000001")
            {
                let mut body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
                body["size"] = README.len().into();
                if url.contains("/git/blobs/") {
                    use base64::Engine;
                    body["content"] = base64::engine::general_purpose::STANDARD
                        .encode(README)
                        .into();
                }
                response.body = body.to_string();
            }
            Ok(response)
        })
    }
}

fn main() {
    // External boundaries are explicit. No user credential, database or live network is read.
    let dir = tempfile::tempdir().unwrap();
    let executor = Arc::new(network::executor::BackgroundExecutor::new(4));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let accounts = rt.block_on(async {
        let service = data::AccountService::open(
            storage::accounts::AccountDatabaseConfig::at(dir.path().join("fonts.sqlite3")),
            Arc::new(auth::InMemoryCredentialStore::new()),
            Arc::new(github::GitHubClient::new(
                github::GitHubConfig::default(),
                Arc::new(FontTransport(fixture::FixtureTransport::new(
                    fixture::Scene::Normal,
                ))),
            )),
            executor.clone(),
        )
        .await
        .unwrap();
        service
            .login(auth::token::Pat::from_str("synthetic-font-preview"))
            .await
            .unwrap();
        service
    });
    let runtime = Arc::new(app::accounts::runtime::AccountRuntime::new(
        accounts, executor,
    ));
    let baseline = std::env::args().find_map(|arg| {
        arg.strip_prefix("--baseline=")
            .map(std::path::PathBuf::from)
    });
    gpui_platform::application().with_assets(Assets).run(move |cx: &mut App| {
        ely_core::init(cx).unwrap();
        if let Some(ref directory) = baseline {
            let faces = ["Regular", "Medium", "SemiBold", "Bold"].into_iter().map(|style| {
                std::borrow::Cow::Owned(std::fs::read(directory.join(format!("GithuxUI-{style}.ttf"))).unwrap())
            }).chain(std::iter::once(std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/google-sans-code/GoogleSansCode.ttf").as_slice()))).collect();
            cx.text_system().add_fonts(faces).unwrap();
        } else {
            platform::fonts::register(cx).unwrap();
        }
        Theme::set_mode_now(Mode::Dark, cx);
        Theme::update(cx, |t| {
            t.font_family = platform::fonts::UI_FAMILY.into();
            t.mono_family = platform::fonts::CODE_FAMILY.into();
        });
        ui::Shell::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        cx.open_window(WindowOptions {window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default()}, move |window,cx| {
            let mut evidence = Vec::new();
            for weight in [400.,500.,600.,700.] {
                for (family,text) in [(platform::fonts::UI_FAMILY,SAMPLES), (platform::fonts::CODE_FAMILY,"fn main()") ] {
                    let font = gpui::Font {family: family.into(),weight:gpui::FontWeight(weight),..Default::default()};
                    let id = cx.text_system().resolve_font(&font);
                    let run = gpui::TextRun {len:text.len(),font,..Default::default()};
                    let line = window.text_system().layout_line(text,px(16.),&[run],None);
                    evidence.push(serde_json::json!({"family":family,"weight":weight,"primary":id.0,"text":text,
                        "runs":line.runs.iter().map(|run| serde_json::json!({"font_id":run.font_id.0,"glyphs":run.glyphs.iter().map(|g|serde_json::json!({"id":g.id.0,"index":g.index})).collect::<Vec<_>>() })).collect::<Vec<_>>() }));
                }
            }
            if baseline.is_none() {
                let expected: serde_json::Value = serde_json::from_str(include_str!("../assets/fonts/githux-ui/sample-glyphs.json")).unwrap();
                for row in &evidence {
                    let primary = row["primary"].as_u64().unwrap();
                    for run in row["runs"].as_array().unwrap() {
                        assert_eq!(run["font_id"].as_u64().unwrap(), primary, "unexpected system fallback");
                        for glyph in run["glyphs"].as_array().unwrap() {
                            assert_ne!(glyph["id"].as_u64().unwrap(), 0, "missing glyph");
                            if row["family"] == platform::fonts::UI_FAMILY {
                                let text = row["text"].as_str().unwrap();
                                let index = glyph["index"].as_u64().unwrap() as usize;
                                let cp = text[index..].chars().next().unwrap() as u32;
                                let weight = row["weight"].as_f64().unwrap() as u32;
                                assert_eq!(glyph["id"], expected[weight.to_string()][cp.to_string()]);
                            }
                        }
                    }
                }
            }
            std::fs::create_dir_all(".agents/output/fonts/sc").unwrap();
            std::fs::write(".agents/output/fonts/sc/shaping.json",serde_json::to_string_pretty(&evidence).unwrap()).unwrap();
            let controller = cx.new(|cx| app::accounts::Accounts::new(Some(runtime),window,cx));
            cx.set_global(app::AccountsHandle(controller));
            let shell = cx.new(|cx|ui::Shell::new(window,cx));
            let target = shell.downgrade();
            window.spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                let _ = cx.update(|window,cx| {
                    let _ = target.update(cx, |shell,cx| {shell.navigate(workspace::Route::Repository{namespace:"octo".into(),name:"ledger".into()},window,cx);});
                });
            }).detach();
            shell
        }).unwrap();
        cx.on_window_closed(|cx,_|{if cx.windows().is_empty(){cx.quit();}}).detach();
        cx.activate(true);
    });
    drop(dir);
}
