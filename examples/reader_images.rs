//! Standalone preview of README picture loading in the reusable reader.
//!
//! Run with `cargo run --example reader_images`.
//!
//! The preview assembles the **real** pipeline through
//! `src/bin_support/reader_images_modules.rs`:
//!
//! ```text
//! AccountService → RepoService::session → RepositorySession
//!   → RepositoryImageService (fixed commit → root tree → directory chain → blob)
//!   → bounded decode (content::images)
//!   → ReaderView picture board → MarkdownView block
//! ```
//!
//! Every external boundary is deterministic and injected:
//!
//! * `src/bin_support/reader_images_fixture.rs` answers the authenticated GitHub
//!   reads by request identity and serves real, generated pictures, so nothing
//!   here depends on a network succeeding;
//! * a temporary SQLite directory plus an in-memory credential store hold one
//!   synthetic PAT, so no real credential is read;
//! * public HTTPS pictures are answered from memory, never from a real host.
//!
//! The toolbar is development scaffolding (scene switch, reset, theme); it is
//! not part of the product Shell. The bottom bar reports what the reader asked
//! for, which is how the owner sees the asynchronous lifecycle.
//!
//! ## What a production host does instead
//!
//! ```ignore
//! // One provider per reader document generation, created once the README's
//! // fixed commit and the page's verified repository session are known.
//! let service = RepositoryImageService::new(
//!     session.clone(),                 // verified RepositorySession (owns auth)
//!     repository_identity.clone(),     // same repository as the document
//!     executor.clone(),                // Arc<BackgroundExecutor>
//!     Arc::new(ReqwestExternalImages::new()?),  // credential-free HTTPS bytes
//!     ImageLimits::default(),
//! );
//! let mut reader = ReaderView::from_prepared(content, kind, window, cx);
//! reader.set_image_provider(Some(Rc::new(service)), cx);
//! // On document/account/selection change, drop the reader instead of reusing
//! // it: a new reader is a new picture generation, so nothing old can appear.
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

#[path = "../src/bin_support/reader_images_modules.rs"]
mod reader_images_modules;
#[allow(unused_imports)]
use reader_images_modules as ui;
#[allow(unused_imports)]
use reader_images_modules::{fixture, reader};

#[path = "../src/platform/fonts.rs"]
mod fonts;

use std::rc::Rc;
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

use crate::auth::InMemoryCredentialStore;
use crate::auth::token::Pat;
use crate::data::AccountService;
use crate::data::reader_images::RepositoryImageService;
use crate::domain::repository::{ObjectSha, RepositoryLocator, RevisionSpec};
use crate::fixture::{ALT_COMMIT, ALT_README_PATH, COMMIT, NAME, OWNER, README_PATH, Scene};
use crate::github::GitHubClient;
use crate::network::executor::BackgroundExecutor;
use crate::network::reader_images::ReqwestExternalImages;
use crate::reader::{DocumentKind, ImageProvider, ReaderView};
use crate::storage::accounts::AccountDatabaseConfig;

fn main() {
    // Development-only scene selection: `cargo run --example reader_images --
    // extremes`. The toolbar's Select does the same thing interactively.
    let initial = std::env::args()
        .nth(1)
        .and_then(|name| {
            Scene::ALL
                .iter()
                .copied()
                .find(|scene| scene.label().eq_ignore_ascii_case(&name))
        })
        .unwrap_or(Scene::Healthy);
    let fixture = FixtureRuntime::new(initial);
    let services = fixture.services.clone();
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
                window_min_size: Some(size(px(760.0), px(520.0))),
                // The platform title bar keeps this a preview, not a second Shell.
                titlebar: Some(TitlebarOptions {
                    title: Some("Githux · README images preview".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let services = services.clone();
            cx.open_window(options, move |window, cx| {
                cx.new(|cx| Preview::new(services.clone(), initial, window, cx))
            })
            .expect("the README images preview window failed to open");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    drop(fixture);
}

/// Owns the fixture service stack for the whole preview.
struct FixtureRuntime {
    services: Services,
    _temp: tempfile::TempDir,
}

#[derive(Clone)]
struct Services {
    accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
    transport: Arc<crate::fixture::FixtureTransport>,
    external: Arc<crate::fixture::MemoryExternalImages>,
}

impl FixtureRuntime {
    fn new(scene: Scene) -> Self {
        let temp = tempfile::tempdir().expect("temporary account directory");
        let transport = Arc::new(crate::fixture::FixtureTransport::new(scene));
        let external = Arc::new(crate::fixture::MemoryExternalImages::new());
        let executor = Arc::new(BackgroundExecutor::new(4));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime");
        let accounts = runtime.block_on(async {
            let accounts = AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(Default::default(), transport.clone())),
                executor.clone(),
            )
            .await
            .expect("open the fixture account service");
            accounts
                .login(Pat::from_str("synthetic-reader-images-token"))
                .await
                .expect("synthetic login");
            Arc::new(accounts)
        });
        Self {
            services: Services {
                accounts,
                executor,
                transport,
                external,
            },
            _temp: temp,
        }
    }
}

/// One preview scene: the reader, its provider and what happened.
struct Preview {
    services: Services,
    reader: Entity<ReaderView>,
    scene: Scene,
    /// The document painted into the current reader.
    document: Option<String>,
    log: Vec<String>,
    mismatches: usize,
    provider: Option<Arc<RepositoryImageService>>,
}

/// One document the preview can open: its fixed commit and path.
struct Document {
    commit: &'static str,
    path: &'static str,
}

impl Preview {
    fn new(services: Services, scene: Scene, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut preview = Self {
            services,
            reader: cx.new(|cx| {
                ReaderView::new(
                    crate::content::PreparedBody::Empty,
                    DocumentKind::Markdown,
                    crate::reader::ResourceContext::new(OWNER, NAME, COMMIT, README_PATH),
                    window,
                    cx,
                )
            }),
            scene,
            document: None,
            log: Vec::new(),
            mismatches: 0,
            provider: None,
        };
        preview.open(scene, window, cx);
        preview
    }

    /// Opens the scene's README at its fixed commit through the real pipeline.
    ///
    /// The block is the shape a production host uses: capture a verified
    /// session, build one provider for this document generation, attach it, and
    /// drop the previous reader so no old picture can survive.
    fn open(&mut self, scene: Scene, window: &mut Window, cx: &mut Context<Self>) {
        self.scene = scene;
        self.services.transport.set_scene(scene);
        self.log.clear();
        self.mismatches = 0;

        let document = match scene {
            Scene::Switch => Document {
                commit: ALT_COMMIT,
                path: ALT_README_PATH,
            },
            _ => Document {
                commit: COMMIT,
                path: README_PATH,
            },
        };
        let executor = self.services.executor.clone();
        let accounts = self.services.accounts.clone();
        let external = self.services.external.clone();
        let transport = self.services.transport.clone();

        // The real read path: a verified session, then the fixed README blob.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("document runtime");
        let commit = ObjectSha::new(document.commit).expect("fixture commit");
        let prepared = runtime.block_on(async {
            let session = accounts.repo_service().session().expect("verified session");
            let repository = accounts
                .repo_service()
                .session()
                .expect("verified session")
                .repository(
                    RepositoryLocator::new(OWNER, NAME).expect("locator"),
                    &crate::github::RepositoryLimits::default(),
                    Default::default(),
                )
                .await
                .map(|scoped| scoped.value.identity)
                .expect("repository identity");
            (repository, session)
        });
        let (repository, session) = prepared;
        let content = runtime.block_on(async {
            // Walk the fixed tree to the README blob, exactly as the picture
            // provider walks to a picture: root → docs → file.
            let identity = {
                use crate::domain::repository::{SnapshotDirectory, TreeEntryKind};
                let limits = crate::github::RepositoryLimits::default();
                let snapshot = session
                    .revision(
                        &repository,
                        RevisionSpec::Commit(commit.clone()),
                        &limits,
                        Default::default(),
                    )
                    .await
                    .expect("fixture snapshot")
                    .value;
                let mut directory = SnapshotDirectory::root(&snapshot);
                let mut found = None;
                let segments: Vec<&str> = document.path.split('/').collect();
                for (index, segment) in segments.iter().enumerate() {
                    let tree = session
                        .directory(&directory, &limits, Default::default())
                        .await
                        .expect("fixture tree")
                        .value;
                    let at = tree
                        .entries
                        .iter()
                        .position(|entry| entry.path.as_str() == *segment)
                        .expect("fixture path segment");
                    if index + 1 == segments.len() {
                        assert!(matches!(tree.entries[at].kind, TreeEntryKind::File { .. }));
                        found = Some(
                            crate::domain::blob::BlobIdentity::from_file(&tree, at)
                                .expect("blob identity"),
                        );
                    } else {
                        directory = tree.child_directory(at).expect("child directory");
                    }
                }
                found.expect("README blob")
            };
            session
                .content(
                    &identity,
                    &crate::domain::blob::BlobLimits::default(),
                    Default::default(),
                )
                .await
                .expect("fixture README")
                .value
        });

        let provider = Arc::new(RepositoryImageService::new(
            session,
            repository,
            executor,
            external,
            Scene::limits(scene),
        ));
        let kind = DocumentKind::for_path(document.path);
        let prepared_context = crate::reader::ResourceContext::from_prepared(&content);
        let body = content.body;
        let log = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let log_for_reader = log.clone();

        self.reader = cx.new(|cx| {
            let mut reader = ReaderView::new(body, kind, prepared_context, window, cx);
            reader.set_markdown_layout(crate::reader::MarkdownLayout::Repository, cx);
            reader.on_action(Rc::new(move |action, _window, _cx| {
                log_for_reader
                    .borrow_mut()
                    .push(format!("link reported, not opened: {action:?}"));
            }));
            reader.set_image_provider(Some(provider.clone() as Arc<dyn ImageProvider>), cx);
            reader
        });
        self.provider = Some(provider);
        self.document = Some(format!("{}@{}", document.path, document.commit));
        self.log.push(format!(
            "opened {} at fixed commit {} ({})",
            document.path,
            document.commit,
            if scene.is_flow() {
                "natural height, page scroll"
            } else {
                "bounded viewport"
            }
        ));
        self.log.extend(log.borrow().iter().cloned());
        self.mismatches = transport.mismatches().len();
        // Reading the external source list keeps the message honest: the preview
        // only ever answers these addresses from memory.
        let external_reads = self.services.external.reads().len();
        if external_reads > 0 {
            self.log.push(format!(
                "{external_reads} public picture(s) answered locally"
            ));
        }
        cx.notify();
    }

    fn choose(&mut self, scene: Scene, window: &mut Window, cx: &mut Context<Self>) {
        self.open(scene, window, cx);
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(self.scene, window, cx);
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let small = theme.text_size(TextSize::Xs);
        let owner = cx.weak_entity();
        let picker = Select::new(
            "reader-images-scene",
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
                let _ = owner.update(cx, |preview, cx| preview.choose(*scene, window, cx));
            }
        });
        let reset_owner = cx.weak_entity();
        let reset = Button::new("reader-images-reset", "Reset")
            .variant(ely_core::buttons::ButtonVariant::Ghost)
            .size(ely_core::theme::ControlSize::Sm)
            .on_click(move |_, window, cx| {
                let _ = reset_owner.update(cx, |preview, cx| preview.reset(window, cx));
            });
        let theme_owner = cx.weak_entity();
        let theme_toggle = Button::new("reader-images-theme", "Light / Dark")
            .size(ely_core::theme::ControlSize::Sm)
            .on_click(move |_, _, cx| {
                let mode = if cx.theme().is_dark() {
                    Mode::Light
                } else {
                    Mode::Dark
                };
                Theme::set_mode_now(mode, cx);
                let _ = theme_owner.update(cx, |_, cx| cx.notify());
            });
        let document = self
            .document
            .clone()
            .unwrap_or_else(|| "no document".into());
        let status = self
            .log
            .iter()
            .rev()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join("  ·  ");
        let mismatch = (self.mismatches > 0).then(|| {
            div()
                .debug_selector(|| "reader-images-fixture-mismatch".to_string())
                .text_size(small)
                .text_color(colors.danger)
                .child(format!(
                    "{} fixture request(s) were not modelled — see the console.",
                    self.mismatches
                ))
        });
        div()
            .debug_selector(|| "reader-images-toolbar".to_string())
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
                div()
                    .id("reader-images-scenes")
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .w_full()
                    .min_w_0()
                    .child(picker)
                    .child(reset)
                    .child(theme_toggle),
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
                    .debug_selector(|| "reader-images-document".to_string())
                    .text_size(small)
                    .text_color(colors.fg_subtle)
                    .child(document),
            )
            .child(
                div()
                    .debug_selector(|| "reader-images-action".to_string())
                    .text_size(small)
                    .text_color(colors.fg_subtle)
                    .child(if status.is_empty() {
                        "Waiting for the first picture result.".to_string()
                    } else {
                        status
                    }),
            )
            .children(mismatch)
            .child(div().text_size(small).text_color(colors.fg_subtle).child(
                "Development preview · synthetic token, generated pictures, no real network. \
                 The toolbar is not part of the product Shell.",
            ))
    }
}

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let body = if self.scene.is_flow() {
            // The natural-height case: the reader has no viewport of its own, so
            // this outer element owns the page scroll and its bar. A host adds
            // its sibling sections around the reader; here a short footer shows
            // that the page keeps scrolling past the README.
            let footer = div()
                .debug_selector(|| "reader-images-page-footer".to_string())
                .p_4()
                .text_size(cx.theme().text_size(TextSize::Sm))
                .text_color(colors.fg_subtle)
                .child("Sibling page content after the README (a profile's other sections).");
            div()
                .id("reader-images-page")
                .debug_selector(|| "reader-images-page".to_string())
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_y_scroll()
                .child(self.reader.clone())
                .child(footer)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(self.reader.clone())
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .child(self.toolbar(cx))
            .child(body)
    }
}

/// Keeps the unused-import surface honest: the preview documents the real
/// credential-free fetcher a production host injects.
#[allow(dead_code)]
fn production_external_source() -> Option<Arc<dyn crate::network::reader_images::ExternalImageBytes>>
{
    ReqwestExternalImages::new().ok().map(|source| {
        Arc::new(source) as Arc<dyn crate::network::reader_images::ExternalImageBytes>
    })
}
