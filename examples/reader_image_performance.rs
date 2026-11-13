//! Offline native preview of original-image viewport scheduling.
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
#[path = "../src/bin_support/reader_images_modules.rs"]
mod reader_images_modules;
#[path = "../src/storage/mod.rs"]
mod storage;
#[allow(unused_imports)]
use reader_images_modules as ui;
#[allow(unused_imports)]
use reader_images_modules::{fixture, reader};
#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/bin_support/reader_images_performance_fixture.rs"]
mod performance_fixture;
use ely_core::{
    Assets,
    buttons::{Button, ButtonVariant},
    theme::{ActiveTheme, ControlSize, Mode, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, ScrollHandle,
    Styled, TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use performance_fixture::PerformanceFixture;
use reader::{
    DocumentKind, ImageLimits, ImageProvider, MarkdownLayout, ReaderView, ResourceContext,
};
use std::{rc::Rc, sync::Arc};
struct Preview {
    fixture: Rc<PerformanceFixture>,
    reader: Entity<ReaderView>,
    table: bool,
    flow: bool,
    pressure: bool,
    scroll: ScrollHandle,
}
impl Preview {
    fn new(fixture: Rc<PerformanceFixture>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.set_window_title("Githux · Originals · Table / Reader");
        let reader = Self::reader(&fixture, true, false, false, window, cx);
        Self {
            fixture,
            reader,
            table: true,
            flow: false,
            pressure: false,
            scroll: ScrollHandle::new(),
        }
    }
    fn reader(
        fixture: &PerformanceFixture,
        table: bool,
        flow: bool,
        pressure: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ReaderView> {
        let weak = cx.entity().downgrade();
        cx.new(|cx| {
            let mut reader = ReaderView::new(
                content::PreparedBody::Utf8 {
                    text: performance_fixture::markdown(table),
                    has_bom: false,
                },
                DocumentKind::Markdown,
                ResourceContext::new(
                    crate::fixture::OWNER,
                    crate::fixture::NAME,
                    crate::fixture::COMMIT,
                    crate::fixture::README_PATH,
                ),
                window,
                cx,
            );
            reader.set_markdown_layout(
                if flow {
                    MarkdownLayout::Flow
                } else {
                    MarkdownLayout::Repository
                },
                cx,
            );
            reader.set_image_limits(
                ImageLimits {
                    max_ready_images: if pressure { 4 } else { 32 },
                    ..Default::default()
                },
                cx,
            );
            reader.set_image_provider(Some(fixture.service.clone() as Arc<dyn ImageProvider>), cx);
            reader.on_layout_changed(Rc::new(move |_, cx| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            }));
            reader
        })
    }
    fn switch(&mut self, table: bool, flow: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.table = table;
        self.flow = flow;
        window.set_window_title(&format!(
            "Githux · Originals · {} / {}{}",
            if table { "Table" } else { "Rows" },
            if flow { "Flow" } else { "Reader" },
            if self.pressure { " / Retain4" } else { "" }
        ));
        self.scroll = ScrollHandle::new();
        self.reader = Self::reader(&self.fixture, table, flow, self.pressure, window, cx);
        cx.notify();
    }
}
impl Render for Preview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let mut toolbar = div().flex().flex_wrap().items_center().gap_2().p_3();
        for (id, label, table, flow) in [
            ("table-reader", "Table · Reader", true, false),
            ("table-flow", "Table · Flow", true, true),
            ("rows-reader", "Rows · Reader", false, false),
            ("rows-flow", "Rows · Flow", false, true),
        ] {
            let owner = cx.entity().downgrade();
            toolbar = toolbar.child(
                Button::new(id, label)
                    .variant(ButtonVariant::Ghost)
                    .size(ControlSize::Sm)
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |view, cx| view.switch(table, flow, window, cx));
                    }),
            );
        }
        let owner = cx.entity().downgrade();
        toolbar = toolbar.child(
            Button::new(
                "retention-pressure",
                if self.pressure {
                    "Retain four: on"
                } else {
                    "Retain four: off"
                },
            )
            .variant(ButtonVariant::Ghost)
            .size(ControlSize::Sm)
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.pressure = !view.pressure;
                    view.switch(view.table, view.flow, window, cx);
                });
            }),
        );
        let body = div()
            .id("performance-preview-scroll")
            .flex_1()
            .min_h_0()
            .w_full()
            .when(self.flow, |element| {
                element.overflow_y_scroll().track_scroll(&self.scroll)
            })
            .child(self.reader.clone());
        div().size_full().flex().flex_col().bg(colors.bg).text_color(colors.fg)
            .child(toolbar)
            .child(div().px_3().pb_2().text_size(px(12.0)).child("Offline generated originals · 9 × 2559×1520 · 64 MiB retained budget · scroll away/back, resize, or move across DPI"))
            .child(body)
    }
}
fn save_report(fixture: &PerformanceFixture, before: &Option<performance_fixture::ProcessMemory>) {
    use content::images::ImageStage;
    let metrics = fixture.service.metrics();
    let stages = [
        ("queue", ImageStage::Queue),
        ("cache", ImageStage::Cache),
        ("metadata", ImageStage::Metadata),
        ("fetch", ImageStage::Fetch),
        ("decode", ImageStage::Decode),
        ("first_ready", ImageStage::FirstReady),
        ("animation", ImageStage::Animation),
    ]
    .map(|(name, stage)| {
        let m = metrics.stage(stage);
        serde_json::json!({"stage": name, "count": m.count, "total_micros": m.micros})
    });
    let unique = fixture
        .source
        .reads
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>()
        .len();
    let report = serde_json::json!({"source_head": option_env!("GITHUX_PREVIEW_HEAD").unwrap_or("development"), "native_window": true, "dedicated_gpu_memory_measured": false, "retained_budget_bytes": 64 * 1024 * 1024, "unique_originals_fetched": unique, "process_before_window": before, "process_at_close": performance_fixture::process_memory(), "stages": stages});
    if let Ok(exe) = std::env::current_exe() {
        let result = serde_json::to_vec_pretty(&report)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(exe.with_extension("metrics.json"), bytes));
        if let Err(error) = result {
            eprintln!("Could not save original-image preview counters: {error}");
        }
    }
}
fn main() {
    let fixture = Rc::new(PerformanceFixture::new(true, ImageLimits::default()));
    let services = fixture.clone();
    let before = performance_fixture::process_memory();
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_core::init(cx).expect("Ely initialization");
            fonts::register(cx).expect("bundled fonts");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = fonts::UI_FAMILY.into();
                theme.mono_family = fonts::CODE_FAMILY.into();
            });
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1180.0), px(820.0)),
                    cx,
                ))),
                window_min_size: Some(size(px(640.0), px(400.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Githux · Original image performance".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let fixture = services.clone();
            cx.open_window(options, move |window, cx| {
                cx.new(|cx| Preview::new(fixture, window, cx))
            })
            .expect("preview window");
            cx.on_window_closed(move |cx, _| {
                if cx.windows().is_empty() {
                    save_report(&services, &before);
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    drop(fixture);
}
