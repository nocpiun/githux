//! The application-level session tab strip drawn at the top of the island.
//!
//! Every tab is one workspace session. Tabs keep one readable width and never compress: once they
//! no longer fit, the strip scrolls horizontally instead of shrinking labels, wrapping or widening
//! the island. The scrolling is GPUI's own horizontal overflow on a [`ScrollHandle`], so the wheel
//! path is the framework's: a vertical wheel maps onto the only scrollable axis, Windows already
//! turns Shift+wheel into a horizontal delta, and a device's native horizontal delta arrives as
//! one, so nothing is converted twice.
//!
//! The scrollbar is Ely's overlay [`Scrollbar`] bound to the same handle, so it is draggable. It
//! hides itself while there is nothing to scroll, and because it is an overlay inside a reserved
//! band the tabs keep their vertical position whether or not it is shown.
//!
//! A tab is a tab stop with its own focus handle, so keyboard navigation reaches it and
//! Enter/Space activates it. Focus is shown with neutral text, never a coloured border, and an
//! inactive tab never looks active. Middle-clicking anywhere in a tab — icon, label, blank space
//! or the close button — closes it exactly once and does not activate it. Only the tab registers
//! that handler, so the nested close button cannot run a second close.

use std::rc::Rc;

use ely_gpui_component::{
    layout::Scrollbar,
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Axis, FocusHandle, FontWeight, IntoElement, ParentElement, Role, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px, transparent_black,
};

use crate::workspace::{Route, TabId};

use super::geometry::{TAB_CLOSE_SIZE, TAB_HEIGHT, TAB_SCROLLBAR_BAND, TAB_STRIP_INSET, TAB_WIDTH};

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

/// The strip: one inset, a horizontally scrolling row of tabs, then the overlay scrollbar band.
pub(super) fn strip(entries: Vec<Entry>, scroll: &ScrollHandle, cx: &App) -> AnyElement {
    let row = div()
        .id("tab-scroll")
        .debug_selector(|| "tab-scroll".to_string())
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(super::geometry::TAB_GAP))
        .h(px(TAB_HEIGHT))
        .w_full()
        .min_w_0()
        // The only scrollable axis: a vertical wheel therefore moves this row sideways, and a
        // native horizontal delta (or Windows' Shift+wheel) moves it by itself.
        .overflow_x_scroll()
        .track_scroll(scroll)
        .children(entries.into_iter().map(|entry| tab(entry, cx)))
        .into_any_element();

    div()
        .debug_selector(|| "tab-strip".to_string())
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .h(px(super::geometry::TAB_STRIP_HEIGHT))
        .w_full()
        .min_w_0()
        // One inset on top and both sides; the band below is the scrollbar's own space.
        .pt(px(TAB_STRIP_INSET))
        .pl(px(TAB_STRIP_INSET))
        .pr(px(TAB_STRIP_INSET))
        .overflow_hidden()
        .child(row)
        .child(
            // Ely's overlay scrollbar paints nothing while the row fits, so this band stays empty
            // until there is something to scroll and the tabs never shift either way.
            div()
                .debug_selector(|| "tab-scrollbar-band".to_string())
                .relative()
                .flex_none()
                .w_full()
                .h(px(TAB_SCROLLBAR_BAND))
                .child(Scrollbar::new("tab-scrollbar", scroll, Axis::Horizontal)),
        )
        .into_any_element()
}

fn tab(entry: Entry, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (fg, muted, hover, active_bg) = (colors.fg, colors.fg_muted, colors.hover, colors.active);
    let (radius, text) = (theme.radius(Radius::Md), theme.text_size(TextSize::Sm));
    let active = entry.active;
    let title = entry.title.clone();
    let tooltip = entry.title.clone();
    let (select, close, middle_close) = (entry.select, entry.close.clone(), entry.close);
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
        .flex_none()
        .items_center()
        .gap_1()
        .w(px(TAB_WIDTH))
        .h(px(TAB_HEIGHT))
        .px_2()
        .rounded(radius)
        // A transparent border keeps the box the same size in every state; it never takes the
        // theme's focus colour.
        .border_1()
        .border_color(transparent_black())
        .text_size(text)
        .text_color(if active { fg } else { muted })
        .when(active, |tab| {
            tab.bg(active_bg).font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |tab| tab.hover(move |style| style.bg(hover)))
        // Keyboard focus is a neutral foreground change on top of whatever background the tab
        // already has, so a focused inactive tab still does not look active.
        .focus(move |style| style.text_color(fg))
        .cursor_pointer()
        .track_focus(&entry.focus)
        .on_click(move |_, window, cx| select(window, cx))
        // Middle click closes; `on_aux_click` only receives non-primary buttons, so it never runs
        // the selection, and this is the only aux handler in the tab subtree.
        .on_aux_click(move |event, window, cx| {
            if event.is_middle_click() {
                middle_close(window, cx);
            }
        })
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
