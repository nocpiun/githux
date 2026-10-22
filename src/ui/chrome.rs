//! Custom title bar and the brand strip used where the platform keeps its own frame.
//!
//! The Windows bar draws Githux on the left, then the fixed search entry and the three window
//! buttons. Empty space is the drag area: the brand, the search entry and the buttons sit on their
//! own occluding layers, so a press there never reaches the drag handlers and never starts a window
//! move. Dragging, double-press maximize and edge resizing are left to the platform's non-client
//! hit testing through GPUI's `WindowControlArea` support, so this module adds no Windows API calls.

use std::rc::Rc;

use ely_gpui_component::{
    primitives::{Icon, IconName},
    shell::WindowControls,
    theme::{ActiveTheme, IconSize, Platform, Radius, TextSize},
};
use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    RenderOnce, Role, StatefulInteractiveElement, Styled, Window, WindowControlArea, div, px,
};

use super::geometry::{SEARCH_WIDTH, TITLEBAR_HEIGHT};

/// The fixed search entry. It is an ordinary button, not a text field: pressing it opens the
/// command palette, which owns the only text input in the shell.
type Open = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub(super) struct SearchEntry {
    focus: gpui::FocusHandle,
    on_open: Open,
}

impl SearchEntry {
    pub(super) fn new(
        focus: gpui::FocusHandle,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            focus,
            on_open: Rc::new(on_open),
        }
    }
}

impl RenderOnce for SearchEntry {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let (surface, border, muted, subtle, hover, focus) = (
            colors.surface,
            colors.border,
            colors.fg_muted,
            colors.fg_subtle,
            colors.hover,
            colors.focus,
        );
        let (radius, text, hint) = (
            theme.radius(Radius::Md),
            theme.text_size(TextSize::Sm),
            theme.text_size(TextSize::Xs),
        );
        let open = self.on_open;
        div()
            .id("titlebar-search")
            .debug_selector(|| "search-entry".to_string())
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(TITLEBAR_HEIGHT - 12.0))
            .w(px(SEARCH_WIDTH))
            .px_2()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(surface)
            .text_size(text)
            .text_color(muted)
            .hover(move |style| style.bg(hover))
            .role(Role::Button)
            .aria_label("Search or jump to (Ctrl+K)")
            .cursor_pointer()
            .track_focus(&self.focus)
            .focus(move |style| style.border_color(focus))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(move |_, window, cx| open(window, cx))
            .child(Icon::new(IconName::Search).size(IconSize::Xs).color(subtle))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child("Search or jump to…"),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(hint)
                    .text_color(subtle)
                    .child("Ctrl+K"),
            )
    }
}

/// The Windows title bar: brand, drag space, search entry, window buttons.
pub(super) fn titlebar(
    search_focus: gpui::FocusHandle,
    on_search: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (bg, fg, border) = (colors.bg, colors.fg, colors.border);
    let brand_size = theme.text_size(TextSize::Sm);
    // A press that leaves the drag area before releasing cancels the move, as the platform's own
    // caption does; the state is keyed so it survives a repaint during the press.
    let pressed = window.use_keyed_state("githux-titlebar-pressed", cx, |_, _| false);
    let (down, moved, up, out) = (pressed.clone(), pressed.clone(), pressed.clone(), pressed);
    let controls = WindowControls::new("githux-window-controls").platform(Platform::Windows);
    div()
        .id("githux-titlebar")
        .debug_selector(|| "titlebar".to_string())
        .flex()
        .flex_none()
        .items_center()
        .h(px(TITLEBAR_HEIGHT))
        .w_full()
        .bg(bg)
        .border_b_1()
        .border_color(border)
        .text_color(fg)
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if !window.default_prevented() {
                down.update(cx, |pressed, _| *pressed = true);
            }
        })
        .on_mouse_move(move |_, window, cx| {
            if *moved.read(cx) {
                moved.update(cx, |pressed, _| *pressed = false);
                window.start_window_move();
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            up.update(cx, |pressed, _| *pressed = false)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            out.update(cx, |pressed, _| *pressed = false)
        })
        .child(
            div()
                .debug_selector(|| "titlebar-brand".to_string())
                .flex()
                .flex_none()
                .items_center()
                .h_full()
                .pl_3()
                .pr_2()
                .text_size(brand_size)
                .font_weight(FontWeight::SEMIBOLD)
                .child("Githux")
                .occlude(),
        )
        .child(div().flex_1().h_full())
        .child(
            div()
                .flex_none()
                .pr_2()
                .occlude()
                .child(SearchEntry::new(search_focus, on_search)),
        )
        .child(div().flex_none().h_full().child(controls))
        .into_any_element()
}

/// Brand strip drawn above the shell where the system title bar stays.
pub(super) fn plain_titlebar(cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .debug_selector(|| "titlebar".to_string())
        .flex()
        .flex_none()
        .items_center()
        .h(px(TITLEBAR_HEIGHT))
        .px_3()
        .border_b_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.bg)
        .text_size(theme.text_size(TextSize::Sm))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.colors.fg)
        .child("Githux")
        .into_any_element()
}
