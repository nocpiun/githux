//! Standalone preview of the reusable repository browsing page.
//!
//! Run with `cargo run --example repository`. The example assembles the real
//! page, reader and scroll modules through `src/bin_support/repository_modules.rs`
//! and drives the **real** data pipeline (`AccountService` → `RepoService` →
//! snapshot/tree/blob → `PreparedContent` → `ReaderView`).
//!
//! Everything external is deterministic and injected:
//!
//! * `src/bin_support/repository_fixture.rs` answers by request identity, so the
//!   same file, directory or branch can be opened again and again, in any order;
//! * a temporary SQLite directory and an in-memory credential store hold one
//!   synthetic PAT, so no real credential is read.
//!
//! The preview toolbar above the page is development scaffolding: it switches
//! scenes, resets the page and reports the actions the host would take. It is
//! not part of the product page and never appears in the release binary.
//!
//! ## What a production host does instead
//!
//! ```ignore
//! let services = RepositoryPageServices::new(accounts.clone(), executor.clone());
//! let page = cx.new(|cx| {
//!     let mut page = RepositoryPage::new(services, locator, saved_intent, window, cx);
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
use repository_modules::repository;

use std::sync::Arc;

use ely_core::{
    Assets,
    buttons::Button,
    forms::{Choice, Select},
    theme::{ActiveTheme, Mode, TextSize, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::auth::token::Pat;
use crate::data::AccountService;
use crate::domain::repository::RepositoryLocator;
use crate::github::GitHubClient;
use crate::network::executor::BackgroundExecutor;
use crate::repository::{PageAction, RepositoryNavigation, RepositoryPage, RepositoryPageServices};
use crate::repository_modules::fixture::{FixtureTransport, NAME, OWNER, Scene};
use crate::storage::accounts::AccountDatabaseConfig;

fn main() {
    // Every external boundary is injected before the window opens: the shared
    // fixture transport, a temporary account directory and one synthetic PAT.
    let fixture = FixtureRuntime::new();
    let services = RepositoryPageServices::new(fixture.accounts.clone(), fixture.executor.clone());
    let transport = fixture.transport.clone();
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
            let bounds = Bounds::centered(None, size(px(1240.0), px(820.0)), cx);
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
            let transport = transport.clone();
            cx.open_window(options, move |window, cx| {
                cx.new(|cx| Preview::new(services.clone(), transport.clone(), window, cx))
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
    transport: Arc<FixtureTransport>,
    _temp: tempfile::TempDir,
}

impl FixtureRuntime {
    /// Builds the real service stack over the shared fixture transport and a
    /// temporary account directory. Nothing here reads a real credential or the
    /// network.
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary account directory");
        let transport = Arc::new(FixtureTransport::new(Scene::Normal));
        let executor = Arc::new(BackgroundExecutor::new(4));
        let store = Arc::new(auth::InMemoryCredentialStore::new());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime");
        // Opened and signed in on the throwaway runtime; every later service call
        // runs on the `BackgroundExecutor`'s own runtime thread.
        let accounts = runtime.block_on(async {
            let accounts = AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                store,
                Arc::new(GitHubClient::new(Default::default(), transport.clone())),
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
            transport,
            _temp: temp,
        }
    }
}

/// The development shell around the page: a scene toolbar and an action log.
struct Preview {
    services: RepositoryPageServices,
    transport: Arc<FixtureTransport>,
    page: Entity<RepositoryPage>,
    scene: Scene,
    actions: Vec<String>,
    mismatches: usize,
}

impl Preview {
    fn new(
        services: RepositoryPageServices,
        transport: Arc<FixtureTransport>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let page = Preview::open_page(&services, window, cx);
        let this = Self {
            services,
            transport,
            page,
            scene: Scene::Normal,
            actions: Vec::new(),
            mismatches: 0,
        };
        this.wire_actions(cx);
        this
    }

    fn open_page(
        services: &RepositoryPageServices,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<RepositoryPage> {
        let services = services.clone();
        let locator = RepositoryLocator::new(OWNER, NAME).expect("fixture locator");
        cx.new(|cx| {
            let mut page = RepositoryPage::new(
                services,
                locator,
                RepositoryNavigation::default(),
                window,
                cx,
            );
            page.activate(cx);
            page
        })
    }

    /// Reports actions instead of opening a browser or navigating elsewhere.
    fn wire_actions(&self, cx: &mut Context<Self>) {
        let log = cx.entity().downgrade();
        let page = self.page.clone();
        page.update(cx, |page, _| {
            page.on_action(std::rc::Rc::new(move |action, _window, cx| {
                let text = match action {
                    PageAction::StarRequested { binding } => format!(
                        "StarRequested for {} (host entry only, no mutation)",
                        binding.repository.locator().name()
                    ),
                    PageAction::ForkRequested { binding } => format!(
                        "ForkRequested for {} (host entry only, no mutation)",
                        binding.repository.locator().name()
                    ),
                    PageAction::Overview(action) => {
                        format!("overview action (not opened): {action:?}")
                    }
                    PageAction::OpenExternal(url) => format!("external link (not opened): {url}"),
                    PageAction::LinkNotFollowed { path, reason } => {
                        format!("link not followed: {path} — {reason}")
                    }
                };
                let _ = log.update(cx, |preview, cx| {
                    preview.actions.push(text);
                    preview.mismatches = preview.transport.mismatches().len();
                    cx.notify();
                });
            }));
        });
    }

    /// Restarts the page under the current scene, exactly as a host would after
    /// an account/authorization change.
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transport.set_scene(self.scene);
        let fresh = self.page.clone();
        fresh.update(cx, |page, cx| page.deactivate(cx));
        self.page = Preview::open_page(&self.services, window, cx);
        self.wire_actions(cx);
        self.actions.clear();
        self.mismatches = 0;
        cx.notify();
    }

    fn choose(&mut self, scene: Scene, window: &mut Window, cx: &mut Context<Self>) {
        self.scene = scene;
        self.reset(window, cx);
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let small = theme.text_size(TextSize::Xs);
        let scene_owner = cx.weak_entity();
        let scene_picker = Select::new(
            "preview-scene",
            Scene::ALL
                .iter()
                .map(|scene| Choice::new(scene.label(), scene.label())),
        )
        .selected(self.scene.label())
        .size(ely_core::theme::ControlSize::Sm)
        .on_change(move |value, window, cx| {
            if let Some(scene) = Scene::ALL
                .iter()
                .find(|scene| scene.label() == value.as_ref())
            {
                let _ = scene_owner.update(cx, |preview, cx| preview.choose(*scene, window, cx));
            }
        });
        let reset = {
            let entity = cx.weak_entity();
            Button::new("scene-reset", "Reset")
                .variant(ely_core::buttons::ButtonVariant::Ghost)
                .size(ely_core::theme::ControlSize::Sm)
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |preview, cx| preview.reset(window, cx));
                })
        };
        let log = if self.actions.is_empty() {
            "No page actions reported yet.".to_string()
        } else {
            self.actions
                .iter()
                .rev()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("  ·  ")
        };
        let mismatch = (self.mismatches > 0).then(|| {
            div()
                .debug_selector(|| "preview-fixture-mismatch".to_string())
                .text_size(small)
                .text_color(colors.danger)
                .child(format!(
                    "{} fixture request(s) were not modelled — see the console.",
                    self.mismatches
                ))
        });
        div()
            .debug_selector(|| "preview-toolbar".to_string())
            .flex()
            .flex_col()
            .gap_1()
            .flex_none()
            .min_w_0()
            .w_full()
            .px_3()
            .py_2()
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            .child(
                // A bounded picker keeps every scene and Reset reachable at narrow widths.
                div()
                    .id("preview-scenes")
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .w_full()
                    .min_w_0()
                    .child(scene_picker)
                    .child(reset),
            )
            .child(
                div()
                    .text_size(small)
                    .text_color(colors.fg_muted)
                    .min_w_0()
                    .child(self.scene.description()),
            )
            .child(
                div()
                    .debug_selector(|| "preview-action".to_string())
                    .text_size(small)
                    .text_color(colors.fg_subtle)
                    .child(log),
            )
            .children(mismatch)
            .child(div().text_size(small).text_color(colors.fg_subtle).child(
                "Development preview · deterministic local fixture, synthetic token, no network. \
                         The toolbar is not part of the product page.",
            ))
    }
}

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .child(self.toolbar(cx))
            .child(div().flex_1().min_h_0().w_full().child(self.page.clone()))
    }
}
