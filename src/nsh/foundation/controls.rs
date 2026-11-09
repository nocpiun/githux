//! Presentation only: callbacks carry no account credentials or data services.
use super::theme as t;
use nsh_core::{element::ImageSrc, prelude::*};

#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Home,
    Settings,
    Close,
    Refresh,
    Warning,
    Issue,
    PullRequest,
}

pub fn icon(kind: Icon) -> impl IntoElement {
    // Lucide paths, under the repository's existing ISC notice.
    let path = match kind {
        Icon::Home => "<path d='m3 10 9-7 9 7v10a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1z'/>",
        Icon::Settings => {
            "<circle cx='12' cy='12' r='3'/><path d='m9 3-1 3-3 1-2 3 2 2-1 3 3 2 2-1 3 2 3-2 3 1 2-3-1-3 2-2-2-3-3-1-1-3z'/>"
        }
        Icon::Close => "<path d='m6 6 12 12M6 18 18 6'/>",
        Icon::Refresh => {
            "<path d='M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-2l2 3M4 16l2 3a7 7 0 0 0 12-2'/>"
        }
        Icon::Warning => "<path d='m12 3 10 18H2zM12 9v4M12 17h.01'/>",
        Icon::Issue => "<circle cx='12' cy='12' r='9'/><circle cx='12' cy='12' r='1'/>",
        Icon::PullRequest => {
            "<circle cx='6' cy='5' r='3'/><circle cx='6' cy='19' r='3'/><circle cx='18' cy='19' r='3'/><path d='M6 8v8M18 16V9a4 4 0 0 0-4-4h-1m3-3-3 3 3 3'/>"
        }
    };
    svg(ImageSrc::svg_markup(&format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'>{path}</svg>")))
        .size(px(16.0)).text_color(t::MUTED)
}

pub fn tooltip(message: impl Into<SharedString>) -> impl IntoTooltip {
    nsh_base::tooltip()
        .bg(t::TOOLTIP_BG)
        .rounded(px(4.0))
        .px_2()
        .py_1()
        .child(t::label(message.into()).text_size(px(12.0)))
}

pub fn button(title: &'static str, action: impl Fn() + 'static) -> View {
    view()
        .h(px(28.0))
        .px_2()
        .rounded(px(4.0))
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .role(AccessibilityRole::Button)
        .label(title)
        .focusable(true)
        .cursor_pointer()
        .hover(|s| s.bg(t::HOVER))
        .focus(|s| s.bg(t::ACTIVE))
        .on_press(move |_: &()| action())
        .child(t::label(title))
}

pub fn icon_button(title: &'static str, glyph: Icon, action: impl Fn() + 'static) -> View {
    view()
        .size(px(28.0))
        .rounded(px(4.0))
        .items_center()
        .justify_center()
        .role(AccessibilityRole::Button)
        .label(title)
        .focusable(true)
        .cursor_pointer()
        .hover(|s| s.bg(t::HOVER))
        .focus(|s| s.bg(t::ACTIVE))
        .tooltip(tooltip(title))
        .on_press(move |_: &()| action())
        .child(icon(glyph))
}

pub fn badge(value: impl IntoReactive<SharedString>) -> View {
    view()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(t::ACTIVE)
        .child(t::label(value).text_size(px(11.0)).text_color(t::MUTED))
}

/// A real failure gets a small warning and its explanation. Retry is present only with an action.
/// Confirmed optional absence is handled by the host by omitting this element entirely.
pub fn failed(message: impl Into<SharedString>, retry: Option<std::rc::Rc<dyn Fn()>>) -> View {
    let message = message.into();
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .label(message.clone())
        .tooltip(tooltip(message))
        .child(view().text_color(t::WARNING).child(icon(Icon::Warning)))
        .children(retry.map(|retry| button("Retry", move || retry())))
}

/// The same two-line 72px presentation block for Home and global lists. Navigation belongs to
/// the caller; it supplies links as elements, so missing actions never become fake links.
pub fn work_item_row(title: impl IntoElement, metadata: impl IntoElement, selected: bool) -> View {
    view()
        .h(px(t::WORK_ITEM_ROW_HEIGHT))
        .px_2()
        .py_2()
        .gap_1()
        .justify_center()
        .rounded(px(4.0))
        .overflow_hidden()
        .bg(if selected { t::ACTIVE } else { t::CLEAR })
        .child(title)
        .child(metadata)
}
