//! Interactive real-controller preview over a repeatable mock transport.
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
#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/bin_support/work_items_modules.rs"]
mod ui;
use domain::work_item::WorkItemKind;
use ely_core::{
    Assets,
    forms::{Choice, Select},
    theme::{ActiveTheme, ControlSize, Mode, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};
use std::{rc::Rc, sync::Arc};
use ui::{
    fixture::{FixtureRuntime, FixtureTransport, SCENES, Scene},
    global_work_items::WorkItemsView,
};
struct Preview {
    page: Entity<WorkItemsView>,
    transport: Arc<FixtureTransport>,
    scene: Scene,
    action: String,
}
impl Preview {
    fn new(runtime: Arc<FixtureRuntime>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let kind = if std::env::args().any(|arg| arg == "--pr") {
            WorkItemKind::PullRequest
        } else {
            WorkItemKind::Issue
        };
        let page = cx.new(|cx| {
            WorkItemsView::new(
                runtime.accounts.repo_service(),
                runtime.executor.clone(),
                kind,
                window,
                cx,
            )
            .unwrap()
        });
        let weak = cx.entity().downgrade();
        page.update(cx, |page, _| {
            page.on_repository(Rc::new(move |navigation, _, cx| {
                let _ = weak.update(cx, |preview, cx| {
                    preview.action = format!(
                        "Repository: {} ({})",
                        navigation.repository.full_name(),
                        navigation.repository.host
                    );
                    cx.notify();
                });
            }))
        });
        // A stable wall clock keeps the preview's relative labels meaningful
        // and reproducible instead of racing the real time while the window
        // is open. The page itself owns the low-frequency refresh timer.
        let preview_now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_793_620_800);
        page.update(cx, |page, cx| page.set_now(Some(preview_now), cx));
        Self {
            page,
            transport: runtime.transport.clone(),
            scene: runtime.transport.scene(),
            action: "Repository callbacks appear here".into(),
        }
    }
}
impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let scene = Select::new(
            "preview-scene",
            SCENES
                .iter()
                .enumerate()
                .map(|(index, (_, label))| Choice::new(index.to_string(), *label)),
        )
        .selected(
            SCENES
                .iter()
                .position(|(scene, _)| *scene == self.scene)
                .unwrap_or(0)
                .to_string(),
        )
        .size(ControlSize::Sm)
        .on_change(move |value, _, cx| {
            if let Ok(index) = value.parse::<usize>()
                && let Some((scene, _)) = SCENES.get(index)
            {
                let _ = weak.update(cx, |preview, cx| {
                    preview.scene = *scene;
                    preview.transport.set_scene(*scene);
                    let _ = preview.page.update(cx, |page, cx| page.rebind(cx));
                    cx.notify();
                });
            }
        });
        let weak = cx.entity().downgrade();
        let kind = self.page.read(cx).controller.query.kind;
        let mode = Select::new(
            "preview-mode",
            [
                Choice::new("issue", "Issues"),
                Choice::new("pr", "Pull Requests"),
            ],
        )
        .selected(if kind == WorkItemKind::Issue {
            "issue"
        } else {
            "pr"
        })
        .size(ControlSize::Sm)
        .on_change(move |value, _, cx| {
            let _ = weak.update(cx, |preview, cx| {
                preview.page.update(cx, |page, cx| {
                    let query = page.controller.query.clone();
                    page.filter(
                        if value == "pr" {
                            WorkItemKind::PullRequest
                        } else {
                            WorkItemKind::Issue
                        },
                        query.relation,
                        query.state,
                        cx,
                    );
                });
            });
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors.bg)
            .text_color(cx.theme().colors.fg)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap_2()
                    .p_2()
                    .child(div().w(px(180.0)).child(mode))
                    .child(div().w(px(230.0)).child(scene)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(self.page.clone()),
            )
            .child(div().flex_none().p_2().child(self.action.clone()))
    }
}
fn main() {
    let scene = if std::env::args().any(|arg| arg == "--long") {
        Scene::Long
    } else if std::env::args().any(|arg| arg == "--partial") {
        Scene::Partial
    } else if std::env::args().any(|arg| arg == "--empty") {
        Scene::Empty
    } else if std::env::args().any(|arg| arg == "--delayed") {
        Scene::Delayed
    } else if std::env::args().any(|arg| arg == "--append-failure") {
        Scene::AppendFailure
    } else if std::env::args().any(|arg| arg == "--initial-failure") {
        Scene::InitialFailure
    } else {
        Scene::Mixed
    };
    let runtime = Arc::new(FixtureRuntime::new(scene));
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_core::init(cx).unwrap();
            fonts::register(cx).unwrap();
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = fonts::UI_FAMILY.into();
                theme.mono_family = fonts::CODE_FAMILY.into();
            });
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(960.0), px(720.0)),
                    cx,
                ))),
                window_min_size: Some(size(px(400.0), px(360.0))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Githux · Global work items preview".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let window_runtime = runtime.clone();
            cx.open_window(options, move |window, cx| {
                cx.new(|cx| Preview::new(window_runtime.clone(), window, cx))
            })
            .unwrap();
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
            // Keep accounts/temp directory alive while the native window is open.
            let mut runtime = Some(runtime);
            cx.on_app_quit(move |_| {
                drop(runtime.take());
                async {}
            })
            .detach();
        });
}
