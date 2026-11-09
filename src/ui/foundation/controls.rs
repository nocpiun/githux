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
    Search,
    Bell,
    Plus,
    More,
    Star,
    Fork,
    Code,
    Repo,
    ChevronDown,
    ChevronRight,
    ExternalLink,
    Check,
    Clock,
    GitMerge,
    Dot,
}

pub fn icon(kind: Icon) -> impl IntoElement {
    colored_icon(kind, t::theme().muted_foreground)
}

fn colored_icon(kind: Icon, color: AdaptiveColor) -> impl IntoElement {
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
        Icon::Search => "<circle cx='11' cy='11' r='7'/><path d='m20 20-3.5-3.5'/>",
        Icon::Bell => {
            "<path d='M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9'/><path d='M13.7 21a2 2 0 0 1-3.4 0'/>"
        }
        Icon::Plus => "<path d='M12 5v14M5 12h14'/>",
        Icon::More => {
            "<circle cx='5' cy='12' r='1.4'/><circle cx='12' cy='12' r='1.4'/><circle cx='19' cy='12' r='1.4'/>"
        }
        Icon::Star => {
            "<path d='m12 3 2.7 5.6 6.3.9-4.5 4.4 1 6.1-5.5-2.9-5.5 2.9 1-6.1L3 9.5l6.3-.9z'/>"
        }
        Icon::Fork => {
            "<circle cx='6' cy='5' r='2.5'/><circle cx='18' cy='5' r='2.5'/><circle cx='12' cy='19' r='2.5'/><path d='M6 7.5V10a3 3 0 0 0 3 3h6a3 3 0 0 0 3-3V7.5M12 13v3.5'/>"
        }
        Icon::Code => "<path d='m9 18-6-6 6-6M15 6l6 6-6 6'/>",
        Icon::Repo => {
            "<path d='M4 4a2 2 0 0 1 2-2h12v18H6a2 2 0 0 1-2-2z'/><path d='M8 2v18M4 18h14'/>"
        }
        Icon::ChevronDown => "<path d='m6 9 6 6 6-6'/>",
        Icon::ChevronRight => "<path d='m9 6 6 6-6 6'/>",
        Icon::ExternalLink => {
            "<path d='M14 4h6v6M20 4l-9 9M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5'/>"
        }
        Icon::Check => "<path d='m5 13 4 4L19 7'/>",
        Icon::Clock => "<circle cx='12' cy='12' r='9'/><path d='M12 7v5l3 2'/>",
        Icon::GitMerge => {
            "<circle cx='6' cy='6' r='2.5'/><circle cx='6' cy='18' r='2.5'/><circle cx='18' cy='12' r='2.5'/><path d='M6 8.5v7M8.5 6H13a3 3 0 0 1 3 3v1'/>"
        }
        Icon::Dot => "<circle cx='12' cy='12' r='3.5'/>",
    };
    svg(ImageSrc::svg_markup(&format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'>{path}</svg>")))
        .size(px(16.0)).text_color(color)
}

pub fn tooltip(message: impl Into<SharedString>) -> impl IntoTooltip {
    nsh_base::tooltip()
        .bg(t::theme().popover)
        .rounded(t::theme().radius)
        .px_2()
        .py_1()
        .child(
            t::label(message.into())
                .text_size(px(12.0))
                .text_color(t::theme().popover_foreground),
        )
}

pub fn button(title: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .h(px(28.0))
        .px_2()
        .rounded(t::theme().radius)
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .accessibility_label(title)
        .cursor_pointer()
        .hover(|s| s.bg(t::theme().accent))
        .focus(|s| s.bg(t::theme().accent))
        .on_press(move |_: &()| action())
        .child(t::label(title))
}

pub fn icon_button(
    title: &'static str,
    glyph: Icon,
    action: impl Fn() + 'static,
) -> impl IntoElement {
    view().tooltip(tooltip(title)).child(
        nsh_base::button()
            .size(px(28.0))
            .rounded(t::theme().radius)
            .items_center()
            .justify_center()
            .accessibility_label(title)
            .cursor_pointer()
            .hover(|s| s.bg(t::theme().accent))
            .focus(|s| s.bg(t::theme().accent))
            .on_press(move |_: &()| action())
            .child(icon(glyph)),
    )
}

pub fn badge(value: impl IntoReactive<SharedString>) -> View {
    view()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(t::theme().radius)
        .bg(t::theme().accent)
        .child(
            t::label(value)
                .text_size(px(11.0))
                .text_color(t::theme().muted_foreground),
        )
}

/// A real failure gets a small warning and its explanation. Retry is present only with an action.
/// Confirmed optional absence is handled by the host by omitting this element entirely.
pub fn failed(message: impl Into<SharedString>, retry: Option<std::rc::Rc<dyn Fn()>>) -> View {
    let message = message.into();
    let can_retry = retry.is_some();
    view()
        .size(px(28.0))
        .items_center()
        .justify_center()
        .label(message.clone())
        .tooltip(tooltip(message))
        .role(if can_retry {
            AccessibilityRole::Button
        } else {
            AccessibilityRole::Image
        })
        .focusable(can_retry)
        .on_press(move |_: &()| {
            if let Some(retry) = &retry {
                retry();
            }
        })
        .child(colored_icon(Icon::Warning, t::WARNING))
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
        .rounded(t::theme().radius)
        .overflow_hidden()
        .bg(if selected {
            t::theme().accent
        } else {
            AdaptiveColor::TRANSPARENT
        })
        .child(title)
        .child(metadata)
}
