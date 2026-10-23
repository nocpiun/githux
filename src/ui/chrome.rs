//! The custom title bar.
//!
//! Every platform draws the same bar: Githux on the left, the fixed search entry on the right and
//! empty space between them. Only Windows adds its three window buttons, because the other
//! platforms keep their system frame this round and would otherwise show two sets of controls.
//!
//! Empty space is the drag area: the brand, the search entry and the buttons sit on their own
//! occluding layers, so a press there never reaches the drag handlers and never starts a window
//! move. Dragging, double-press maximize and edge resizing are left to the platform's non-client
//! hit testing through GPUI's `WindowControlArea` support, so this module adds no Windows API calls.

use std::rc::Rc;

use ely_gpui_component::{
    primitives::{Icon, IconName},
    theme::{ActiveTheme, IconSize, Radius, TextSize},
};
use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    RenderOnce, Role, StatefulInteractiveElement, Styled, Window, WindowControlArea, div, px,
};

use super::geometry::{SEARCH_WIDTH, TITLEBAR_CONTENT_HEIGHT, TITLEBAR_HEIGHT, TITLEBAR_PADDING};

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
            .h(px(TITLEBAR_CONTENT_HEIGHT))
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

/// The title bar: brand, drag space, search entry and, on Windows, the window buttons.
///
/// Whether the bar draws caption buttons is a compile-time choice, not a theme value: the system
/// frame is dropped on Windows only, so that is the only platform where the bar owns them.
pub(super) fn titlebar(
    search_focus: gpui::FocusHandle,
    on_search: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (bg, fg) = (colors.bg, colors.fg);
    let brand_size = theme.text_size(TextSize::Sm);
    // A press that leaves the drag area before releasing cancels the move, as the platform's own
    // caption does; the state is keyed so it survives a repaint during the press.
    let pressed = window.use_keyed_state("githux-titlebar-pressed", cx, |_, _| false);
    let (down, moved, up, out) = (pressed.clone(), pressed.clone(), pressed.clone(), pressed);
    #[cfg(target_os = "windows")]
    let native_area = crate::platform::caption::area(window);
    #[cfg(target_os = "windows")]
    let native_width = native_area.map(|area| area.width);
    #[cfg(not(target_os = "windows"))]
    let native_width: Option<f32> = None;
    let caption = cfg!(target_os = "windows");
    let controls = caption.then(|| {
        #[cfg(target_os = "windows")]
        if let Some(width) = native_width {
            return div()
                .debug_selector(|| "native-caption-space".to_string())
                .flex_none()
                .w(px(width))
                .h_full()
                .relative()
                .occlude()
                .children(native_area.map(|area| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(area.top))
                        .bg(bg)
                }))
                .children(native_area.map(|area| {
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(TITLEBAR_HEIGHT - area.bottom))
                        .bg(bg)
                }))
                .children(native_area.map(|area| {
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(px(area.right))
                        .bg(bg)
                }))
                .into_any_element();
        }
        caption_buttons(window, cx)
    });
    div()
        .id("githux-titlebar")
        .debug_selector(|| "titlebar".to_string())
        .relative()
        .flex()
        .flex_none()
        .h(px(TITLEBAR_HEIGHT))
        .items_start()
        .w_full()
        .text_color(fg)
        // Empty space drags the window; the platform turns this into the caption hit test.
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
        // Leave DWM's caption area transparent in the DirectComposition surface.
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right(px(native_width.unwrap_or(0.0)))
                .bg(bg),
        )
        .child(
            div()
                .debug_selector(|| "titlebar-brand".to_string())
                .flex()
                .flex_none()
                .items_center()
                .h(px(TITLEBAR_CONTENT_HEIGHT))
                .mt(px(TITLEBAR_PADDING))
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
                .flex()
                .items_center()
                .h(px(TITLEBAR_CONTENT_HEIGHT))
                .mt(px(TITLEBAR_PADDING))
                .pr_2()
                .occlude()
                .child(SearchEntry::new(search_focus, on_search)),
        )
        .children(controls.map(|controls| div().flex_none().h_full().child(controls)))
        .into_any_element()
}

/// Windows caption controls with a geometric maximize/restore glyph rather than a rounded
/// square or a copy icon. Keep GPUI's native hit regions for system caption interactions.
fn caption_buttons(window: &Window, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let maximized = window.is_maximized();
    let buttons = [
        ("caption-Minimize", "Minimize", WindowControlArea::Min),
        (
            "caption-Maximize",
            if maximized { "Restore" } else { "Maximize" },
            WindowControlArea::Max,
        ),
        ("caption-Close", "Close", WindowControlArea::Close),
    ];
    div()
        .id("githux-window-controls")
        .flex()
        .flex_none()
        .occlude()
        .children(buttons.map(|(id, label, area)| {
            let close = matches!(area, WindowControlArea::Close);
            let hover = if close { colors.danger } else { colors.hover };
            let foreground = if close { colors.on_accent } else { colors.fg };
            let icon = if matches!(area, WindowControlArea::Max) {
                maximize_glyph(maximized, colors.fg_muted, foreground, id)
            } else {
                Icon::new(if close { IconName::X } else { IconName::Minus })
                    .size(IconSize::Sm)
                    .color(colors.fg_muted)
                    .group_hover_color(id, foreground)
                    .into_any_element()
            };
            div()
                .id(id)
                .group(id)
                .debug_selector(move || id.to_string())
                .flex()
                .items_center()
                .justify_center()
                .w(theme.caption_button_width())
                .h(theme.caption_button_height(window.rem_size()))
                .role(Role::Button)
                .aria_label(label)
                .window_control_area(area)
                .hover(move |style| style.bg(hover))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(move |_, window, _| match area {
                    WindowControlArea::Min => window.minimize_window(),
                    WindowControlArea::Max => window.zoom_window(),
                    WindowControlArea::Close => window.remove_window(),
                    _ => unreachable!("only caption buttons are constructed"),
                })
                .child(icon)
        }))
        .into_any_element()
}

fn maximize_glyph(
    maximized: bool,
    color: gpui::Hsla,
    hover: gpui::Hsla,
    group: &'static str,
) -> AnyElement {
    let outline = || {
        div()
            .absolute()
            .border_color(color)
            .group_hover(group, move |style| style.border_color(hover))
    };
    let front = outline()
        .left_0()
        .top(px(if maximized { 2.0 } else { 0.0 }))
        .size(px(if maximized { 8.0 } else { 10.0 }))
        .border_1();
    div()
        .relative()
        .size(px(10.0))
        .child(front)
        .children(maximized.then(|| {
            // Only the exposed rear edges are painted; no fill can cover the hover background.
            div()
                .size_full()
                .child(
                    outline()
                        .left(px(2.0))
                        .top_0()
                        .w(px(8.0))
                        .h(px(8.0))
                        .border_t_1()
                        .border_r_1(),
                )
                .child(outline().left(px(2.0)).top_0().h(px(2.0)).border_l_1())
                .child(outline().left(px(8.0)).top(px(7.0)).w(px(2.0)).border_t_1())
        }))
        .into_any_element()
}
