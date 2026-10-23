//! The application-level session tab strip drawn at the top of the island.
//!
//! Every tab is one workspace session. The strip is a single row that clips: each tab shrinks
//! between [`TAB_MIN_WIDTH`] and [`TAB_MAX_WIDTH`], and its label is cut with an ellipsis while
//! the tooltip keeps the full name. That keeps the island's width independent of how many tabs
//! are open.
//!
//! A tab is a tab stop with its own focus handle, so keyboard navigation can reach it and
//! Enter/Space activates it. The close affordance is mouse-only; `Ctrl+W` is the keyboard path to
//! close the current tab. Closing a tab stops the click from bubbling into the tab's own
//! activation.

use std::rc::Rc;

use ely_gpui_component::{
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, FocusHandle, FontWeight, IntoElement, ParentElement, Role,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px, transparent_black,
};

use crate::workspace::{Route, TabId};

use super::geometry::{TAB_CLOSE_SIZE, TAB_HEIGHT, TAB_MAX_WIDTH, TAB_MIN_WIDTH, TAB_STRIP_HEIGHT};

pub(super) type Run = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// One session as the shell has resolved it for this frame.
pub(super) struct Entry {
    pub id: TabId,
    pub index: usize,
    pub title: String,
    pub icon: IconName,
    pub active: bool,
    pub focus: FocusHandle,
    pub select: Run,
    pub close: Run,
}

/// The icon a session tab shows for its page kind.
pub(super) fn route_icon(route: &Route) -> IconName {
    match route {
        Route::Home => IconName::House,
        Route::Settings => IconName::Settings,
        Route::Profile => IconName::User,
        Route::Notifications => IconName::Bell,
        Route::Issues => IconName::CircleDot,
        Route::PullRequests => IconName::GitPullRequest,
        Route::Repository { .. } => IconName::Folder,
    }
}

/// The strip: every session tab in order, then the optional limit hint.
pub(super) fn strip(entries: Vec<Entry>, hint: Option<String>, cx: &App) -> AnyElement {
    let tabs = entries
        .into_iter()
        .map(|entry| tab(entry, cx))
        .collect::<Vec<_>>();
    div()
        .debug_selector(|| "tab-strip".to_string())
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .h(px(TAB_STRIP_HEIGHT))
        .w_full()
        .min_w_0()
        .px_2()
        .overflow_hidden()
        .children(tabs)
        .when_some(hint, |strip, hint| strip.child(hint_pill(hint, cx)))
        .into_any_element()
}

fn tab(entry: Entry, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (fg, muted, hover, active_bg, focus_color) = (
        colors.fg,
        colors.fg_muted,
        colors.hover,
        colors.active,
        colors.focus,
    );
    let (radius, text) = (theme.radius(Radius::Md), theme.text_size(TextSize::Sm));
    let active = entry.active;
    let title = entry.title.clone();
    let tooltip = entry.title.clone();
    let (select, close) = (entry.select, entry.close);
    let index = entry.index;
    let close_label = format!("Close {title}");
    let icon_color = if active { fg } else { muted };

    div()
        .id(("workspace-tab", entry.id.get()))
        .debug_selector(move || format!("tab-{index}"))
        .role(Role::Tab)
        .aria_selected(active)
        .aria_label(title.clone())
        .flex()
        .flex_1()
        .items_center()
        .gap_1()
        .h(px(TAB_HEIGHT))
        .min_w(px(TAB_MIN_WIDTH))
        .max_w(px(TAB_MAX_WIDTH))
        .px_2()
        .rounded(radius)
        .border_1()
        .border_color(transparent_black())
        .text_size(text)
        .text_color(if active { fg } else { muted })
        .when(active, |tab| {
            tab.bg(active_bg).font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |tab| tab.hover(move |style| style.bg(hover)))
        .cursor_pointer()
        .track_focus(&entry.focus)
        .focus(move |style| style.border_color(focus_color))
        .on_click(move |_, window, cx| select(window, cx))
        .tooltip(Tooltip::text(tooltip))
        .child(Icon::new(entry.icon).size(IconSize::Xs).color(icon_color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(Ellipsis::new(title)),
        )
        .child(
            div()
                .id(("workspace-tab-close", entry.id.get()))
                .debug_selector(move || format!("tab-close-{index}"))
                .role(Role::Button)
                .aria_label(close_label)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(TAB_CLOSE_SIZE))
                .rounded(theme.radius(Radius::Sm))
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    close(window, cx);
                })
                .tooltip(Tooltip::text("Close tab"))
                .child(Icon::new(IconName::X).size(IconSize::Xs).color(muted)),
        )
        .into_any_element()
}

/// The short notice shown when a new page cannot open because [`crate::workspace::MAX_OPEN_TABS`]
/// sessions are already open. It is cleared by the next successful tab action.
fn hint_pill(hint: String, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .debug_selector(|| "tab-limit-hint".to_string())
        .flex_none()
        .flex_shrink(1.0)
        .min_w_0()
        .max_w(px(200.0))
        .truncate()
        .text_size(theme.text_size(TextSize::Xs))
        .text_color(theme.colors.fg_subtle)
        .child(hint)
        .into_any_element()
}
