//! Standalone preview of the reusable read-only reader.
//!
//! Run with `cargo run --example reader`. It reuses the real reader module and
//! the real fonts through `#[path]` rather than copying any backend logic; the
//! native title bar keeps it a preview, not a second product shell.

// Windows starts it without a console window, like the product binary; the test
// harness keeps its console so assertion output stays visible.
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/platform/fonts.rs"]
mod fonts;

// The module tree lives in one file so every `#[path]` inside it resolves
// against that file on Windows and Linux alike (and `rustfmt` can find it).
#[path = "../src/bin_support/reader_modules.rs"]
mod reader_modules;
#[allow(unused_imports)]
use reader_modules as ui;
#[allow(unused_imports)]
use reader_modules::reader;

use std::rc::Rc;

use ely_gpui_component::{
    Assets,
    buttons::Button,
    theme::{ActiveTheme, Mode, TextSize, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::content::PreparedBody;
use crate::reader::{DocumentKind, ReaderAction, ReaderView, ResourceContext, fixtures};

struct Fixture {
    label: &'static str,
    body: fn() -> PreparedBody,
    kind: fn() -> DocumentKind,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        label: "Markdown",
        body: fixtures::markdown,
        kind: || DocumentKind::Markdown,
    },
    Fixture {
        label: "Source",
        body: fixtures::source,
        kind: || DocumentKind::Source {
            language: Some("Rust".to_string()),
        },
    },
    Fixture {
        label: "Empty",
        body: fixtures::empty,
        kind: || DocumentKind::Markdown,
    },
    Fixture {
        label: "Unsupported",
        body: fixtures::unsupported,
        kind: || DocumentKind::Markdown,
    },
    Fixture {
        label: "TooLarge",
        body: fixtures::too_large,
        kind: || DocumentKind::Markdown,
    },
    Fixture {
        label: "Large source",
        body: fixtures::large_source,
        kind: || DocumentKind::Source {
            language: Some("Rust".to_string()),
        },
    },
    Fixture {
        label: "Large Markdown",
        body: fixtures::large_markdown,
        kind: || DocumentKind::Markdown,
    },
];

struct Preview {
    reader: Entity<ReaderView>,
    active: usize,
    action: Option<String>,
}

impl Preview {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let context = ResourceContext::new(
            fixtures::OWNER,
            fixtures::NAME,
            fixtures::COMMIT,
            fixtures::PATH,
        );
        let this = cx.entity().downgrade();
        let reader = cx.new(|cx| {
            let mut reader = ReaderView::new(
                fixtures::markdown(),
                DocumentKind::Markdown,
                context,
                window,
                cx,
            );
            reader.on_action(Rc::new(move |action, _window, cx| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let shown = describe(&action);
                this.update(cx, |preview, cx| {
                    preview.action = Some(shown);
                    cx.notify();
                });
            }));
            reader
        });
        Self {
            reader,
            active: 0,
            action: None,
        }
    }

    fn switch(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fixture) = FIXTURES.get(index) else {
            return;
        };
        self.active = index;
        let body = (fixture.body)();
        let kind = (fixture.kind)();
        self.action = None;
        self.reader
            .update(cx, |reader, cx| reader.show(body, kind, window, cx));
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let buttons = FIXTURES.iter().enumerate().map(|(index, fixture)| {
            let this = cx.entity();
            let active = index == self.active;
            Button::new(("fixture", index), fixture.label)
                .variant(match active {
                    true => ely_gpui_component::buttons::ButtonVariant::Primary,
                    false => ely_gpui_component::buttons::ButtonVariant::Secondary,
                })
                .on_click(move |_, window, cx| {
                    this.update(cx, |preview, cx| preview.switch(index, window, cx));
                })
                .into_any_element()
        });
        div()
            .debug_selector(|| "reader-toolbar".to_string())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.colors.border)
            .children(buttons)
    }
}

fn describe(action: &ReaderAction) -> String {
    match action {
        ReaderAction::Link(target) => format!("link → {target:?}"),
    }
}

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg, border, subtle) = (
            theme.colors.bg,
            theme.colors.fg,
            theme.colors.border,
            theme.colors.fg_subtle,
        );
        let tiny = theme.text_size(TextSize::Xs);
        let status = self.action.clone().unwrap_or_else(|| {
            "Press a link: the preview shows the resolved target and never opens it.".to_string()
        });
        let toolbar = self.toolbar(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .child(toolbar)
            .child(div().flex_1().min_h_0().w_full().child(self.reader.clone()))
            .child(
                div()
                    .debug_selector(|| "reader-action".to_string())
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .text_size(tiny)
                    .text_color(subtle)
                    .child(status),
            )
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_gpui_component::init(cx).expect("Ely failed to start");
            fonts::register(cx).expect("the bundled fonts failed to register");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = fonts::UI_FAMILY.into();
                theme.mono_family = fonts::CODE_FAMILY.into();
            });
            let bounds = Bounds::centered(None, size(px(1100.0), px(760.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.0), px(480.0))),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| cx.new(|cx| Preview::new(window, cx)))
                .expect("the reader preview window failed to open");
            cx.activate(true);
        });
}
