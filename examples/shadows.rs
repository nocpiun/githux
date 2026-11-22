//! Offline native check for the shell's shadow roles.
//!
//! `cargo run --example shadows` draws the two elements that float in the product — the round
//! collapse/expand toggle on the sidebar/island seam and a carried tab ghost — on the real theme
//! grounds, using the production [`ui::elevation`] roles. It never opens account storage or the
//! network.
//!
//! Set `GITHUX_SHADOWS_MODE=light` to draw the light palette instead of the dark one, so the two
//! modes can be compared without clicking through the shell. This is a shadow probe, not a second
//! shell: the owner still verifies the real window, where the same roles are attached to the same
//! elements.

// Keep the preview console-free on Windows, like the product. Tests keep their console output in a
// separate integration target.
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

// The product modules this developer target reuses, declared at the top level so `rustfmt` and the
// module resolver agree on every platform (the avatar probe's assembly).
#[path = "../src/app/mod.rs"]
#[allow(dead_code)]
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
#[allow(dead_code)]
mod platform;
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/ui/mod.rs"]
#[allow(dead_code)]
mod ui;
#[path = "../src/workspace/mod.rs"]
#[allow(dead_code)]
mod workspace;

use ely_core::{
    Assets,
    primitives::{Icon, IconName},
    theme::{ActiveTheme, IconSize, Mode, Radius, TextSize, Theme},
    typography::Ellipsis,
};
use gpui::{
    App, AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, px, size,
};
use ui::elevation;
use ui::geometry;

struct Probe {
    mode: Mode,
}

impl Probe {
    /// The flat tab the ghost is lifted from, so the probe shows what the shadow is *for*.
    fn flat_tab(cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        div()
            .flex()
            .items_center()
            .gap(px(geometry::TAB_ICON_GAP))
            .w(px(geometry::TAB_WIDTH))
            .h(px(geometry::TAB_HEIGHT))
            .pl(px(8.0))
            .pr(px(4.0))
            .rounded(theme.radius(Radius::Md))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .text_color(colors.fg)
            .child(
                Icon::new(IconName::House)
                    .size(IconSize::Xs)
                    .color(colors.fg),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(Ellipsis::new("flat tab")),
            )
            .child(div().flex().flex_none().size(px(geometry::TAB_CLOSE_SIZE)))
    }
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;

        // The seam between the sidebar and the island, with the product's round toggle on it.
        let toggle = elevation::attach(div(), cx)
            .size(px(geometry::SIDEBAR_TOGGLE_SIZE))
            .rounded_full()
            .bg(colors.bg)
            .flex()
            .items_center()
            .justify_center()
            .child(
                Icon::new(IconName::ChevronLeft)
                    .size(IconSize::Xs)
                    .color(colors.fg_muted),
            );
        let seam = div()
            .relative()
            .flex()
            .items_center()
            .h(px(160.0))
            .child(
                // Sidebar ground.
                div()
                    .flex_none()
                    .w(px(geometry::SIDEBAR_WIDTH))
                    .h_full()
                    .bg(colors.bg),
            )
            .child(
                // Island ground with its 1px outline; the toggle straddles the outline.
                div()
                    .flex_1()
                    .h_full()
                    .rounded(px(geometry::ISLAND_RADIUS))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.surface),
            )
            .child(
                div()
                    .absolute()
                    .left(px(geometry::SIDEBAR_WIDTH
                        + geometry::ISLAND_GAP
                        + geometry::ISLAND_BORDER_WIDTH / 2.0
                        - geometry::SIDEBAR_TOGGLE_SIZE / 2.0))
                    .top(gpui::relative(0.5))
                    .mt(px(-geometry::SIDEBAR_TOGGLE_SIZE / 2.0))
                    .child(toggle),
            );

        // The carried tab ghost next to the flat tab it came from.
        let ghost = elevation::attach(div(), cx)
            .flex()
            .items_center()
            .gap(px(geometry::TAB_ICON_GAP))
            .w(px(geometry::TAB_WIDTH))
            .h(px(geometry::TAB_HEIGHT))
            .pl(px(8.0))
            .pr(px(4.0))
            .rounded(theme.radius(Radius::Md))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .text_size(theme.text_size(TextSize::Sm))
            .text_color(colors.fg)
            .child(
                Icon::new(IconName::House)
                    .size(IconSize::Xs)
                    .color(colors.fg),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(Ellipsis::new("nociun/githux")),
            )
            .child(div().flex().flex_none().size(px(geometry::TAB_CLOSE_SIZE)));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.surface)
            .text_color(colors.fg)
            .text_size(theme.text_size(TextSize::Sm))
            .child(
                div().flex_none().px_4().py_3().child(format!(
                    "Shadow roles · {} mode · sidebar/island seam + carried tab",
                    if self.mode == Mode::Dark {
                        "dark"
                    } else {
                        "light"
                    }
                )),
            )
            .child(seam)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_6()
                    .child(div().text_color(colors.fg_muted).child(
                        "The same roles the product attaches to `sidebar-toggle` and `tab-drag-ghost`.",
                    ))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_4()
                            .child(Self::flat_tab(cx))
                            .child(ghost),
                    ),
            )
    }
}

fn main() {
    let mode = match std::env::var("GITHUX_SHADOWS_MODE").as_deref() {
        Ok("light") => Mode::Light,
        _ => Mode::Dark,
    };
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_core::init(cx).expect("Ely failed to start");
            platform::fonts::register(cx).expect("the bundled fonts failed to register");
            // Install the product's dark palette exactly as startup does, then show `mode`. Light
            // stays Ely's own palette, which is what the shell's toggle selects; dark is the
            // owner-selected Geek Neutral, not Ely's warm default dark.
            app::theme::install(cx);
            Theme::set_mode_now(mode, cx);
            Theme::update(cx, |theme| {
                theme.font_family = platform::fonts::UI_FAMILY.into();
                theme.mono_family = platform::fonts::CODE_FAMILY.into();
            });
            let bounds = Bounds::centered(None, size(px(760.0), px(520.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some(format!("Githux · shadows ({mode:?})").into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Probe { mode }),
            )
            .expect("failed to open the shadow probe");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
