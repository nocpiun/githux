//! Fixed global sidebar: top navigation, a bounded repository group, the account row and the four
//! quick entries.
//!
//! Only the repository group scrolls. Home, Settings, the group heading, the account row and the
//! quick entries are flex-none children of a full-height column, so resizing the window moves the
//! boundary of the scroll group and never the fixed parts. Rows expose accessible names and full
//! identifiers; long names are cut inside the column instead of widening it.

use ely_core::{
    data_display::Avatar,
    navigation::NavItem,
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, AvatarSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, FocusHandle, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Role, ScrollHandle, Styled, Window, div, prelude::*, px, transparent_black,
};

use super::{
    fixtures::{AccountFixture, RepoFixture},
    geometry::{ACCOUNT_ROW_HEIGHT, REPO_ROW_HEIGHT, SIDEBAR_PADDING, SIDEBAR_WIDTH},
};

type Run = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// Home or Settings; walks and clicks as every other sidebar row does.
pub(super) fn nav_row(
    selector: &'static str,
    icon: IconName,
    label: &'static str,
    active: bool,
    run: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let run: Run = Box::new(run);
    div()
        .debug_selector(move || selector.to_string())
        .w_full()
        .child(
            NavItem::new(selector, icon, label)
                .active(active)
                .on_click(move |window, cx| run(window, cx)),
        )
        .into_any_element()
}

/// One `namespace/repo` row: the namespace's avatar, then the full identifier.
///
/// `focus` is the row's own tab stop. The shell subscribes to it so a row reached with Tab is
/// scrolled into view; a row that is merely clipped would otherwise take focus invisibly.
pub(super) fn repo_row(
    index: usize,
    repo: &RepoFixture,
    focus: &FocusHandle,
    active: bool,
    run: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (radius, text) = (theme.radius(Radius::Md), theme.text_size(TextSize::Sm));
    let (fg, muted, hover, active_bg, focus_color) = (
        colors.fg,
        colors.fg_muted,
        colors.hover,
        colors.active,
        colors.focus,
    );
    let full_name = repo.full_name();
    let avatar_name = repo.namespace.clone();
    let run: Run = Box::new(run);
    div()
        .id(("repo-row", index))
        .debug_selector(move || format!("repo-row-{index}"))
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(REPO_ROW_HEIGHT))
        .px_2()
        .rounded(radius)
        .border_1()
        .border_color(transparent_black())
        .text_size(text)
        .text_color(if active { fg } else { muted })
        .when(active, |row| {
            row.bg(active_bg).font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |row| {
            row.hover(move |style| style.bg(hover).text_color(fg))
        })
        .role(Role::Button)
        .aria_label(full_name.clone())
        .cursor_pointer()
        .track_focus(focus)
        .focus(move |style| style.border_color(focus_color))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| run(window, cx))
        .child(Avatar::new(("repo-avatar", index), avatar_name).size(AvatarSize::Xs))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Ellipsis::new(full_name.clone())),
        )
        .tooltip(Tooltip::text(full_name))
        .into_any_element()
}

/// The signed-in account; pressing it opens the profile.
pub(super) fn account_row(
    account: AccountFixture,
    active: bool,
    run: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (radius, name_size, login_size) = (
        theme.radius(Radius::Md),
        theme.text_size(TextSize::Sm),
        theme.text_size(TextSize::Xs),
    );
    let (hover, fg, subtle, focus) = (colors.hover, colors.fg, colors.fg_subtle, colors.focus);
    let run: Run = Box::new(run);
    div()
        .id("account-row")
        .debug_selector(|| "account-row".to_string())
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(ACCOUNT_ROW_HEIGHT))
        .px_2()
        .rounded(radius)
        .border_1()
        .border_color(transparent_black())
        .text_color(fg)
        .when(active, |row| row.bg(hover))
        .hover(move |style| style.bg(hover))
        .role(Role::Button)
        .aria_label(format!("Profile: {} ({})", account.name, account.login))
        .cursor_pointer()
        .tab_index(0)
        .focus(move |style| style.border_color(focus))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| run(window, cx))
        .child(Avatar::new("account-avatar", account.name).size(AvatarSize::Sm))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(name_size)
                        .text_color(fg)
                        .child(Ellipsis::new(account.name)),
                )
                .child(
                    div()
                        .text_size(login_size)
                        .text_color(subtle)
                        .child(Ellipsis::new(format!("@{}", account.login))),
                ),
        )
        .into_any_element()
}

/// One of the four equal-width quick entries. New stays unavailable this round: its resource list
/// is not approved, so it keeps an accessible name, takes no focus and runs nothing.
pub(super) fn quarter_button(
    name: &'static str,
    icon: IconName,
    label: &'static str,
    active: bool,
    disabled: bool,
    run: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (radius, hover, muted, focus) = (
        theme.radius(Radius::Md),
        colors.hover,
        if active { colors.fg } else { colors.fg_muted },
        colors.focus,
    );
    let run: Run = Box::new(run);
    div()
        .id(name)
        .debug_selector(move || format!("quarter-{name}"))
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .h(px(REPO_ROW_HEIGHT))
        .rounded(radius)
        .border_1()
        .border_color(transparent_black())
        .hover(move |style| style.bg(hover))
        .role(Role::Button)
        .aria_label(label)
        .when(active, |button| {
            button.bg(hover).font_weight(FontWeight::MEDIUM)
        })
        .map(|button| {
            if disabled {
                return button
                    .aria_description("Creating resources is not available in this round.")
                    .opacity(0.45)
                    .cursor_not_allowed();
            }
            button
                .cursor_pointer()
                .tab_index(0)
                .focus(move |style| style.border_color(focus))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(move |_, window, cx| run(window, cx))
        })
        .child(Icon::new(icon).size(IconSize::Sm).color(muted))
        .tooltip(Tooltip::text(label))
        .into_any_element()
}

/// The four equal-width quick entries under the account row.
pub(super) fn quarter_row(buttons: [AnyElement; 4]) -> AnyElement {
    div()
        .debug_selector(|| "quarter-row".to_string())
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .w_full()
        .children(buttons)
        .into_any_element()
}

/// A quiet group heading above the repository list.
pub(super) fn group_heading(text: String, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .debug_selector(|| "repo-heading".to_string())
        .flex_none()
        .px_2()
        .pt_1()
        .text_size(theme.text_size(TextSize::Xs))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.fg_subtle)
        .child(text)
        .into_any_element()
}

/// Scrolls `row` into the list's viewport with the least movement, leaving a row that already fits
/// alone. The row's painted bounds and the list's viewport are in the same space, so this needs no
/// second layout pass.
pub(super) fn reveal_row(scroll: &ScrollHandle, row: gpui::Bounds<gpui::Pixels>) {
    let view = scroll.bounds();
    if view.size.height <= gpui::Pixels::ZERO {
        return;
    }
    let offset = scroll.offset();
    let top = row.top() + offset.y;
    let bottom = row.bottom() + offset.y;
    let shift = if top < view.top() {
        view.top() - top
    } else if bottom > view.bottom() {
        (view.bottom() - bottom).max(view.top() - top)
    } else {
        gpui::Pixels::ZERO
    };
    if shift != gpui::Pixels::ZERO {
        scroll.set_offset(gpui::Point::new(offset.x, offset.y + shift));
    }
}

/// The repository scroll group: the only part of the sidebar that scrolls.
pub(super) fn repository_group(
    rows: Vec<AnyElement>,
    scroll: &ScrollHandle,
    cx: &App,
) -> AnyElement {
    let gap = cx.theme().control_height(ely_core::theme::ControlSize::Sm) / 12.0;
    div()
        .id("repo-scroll-list")
        .debug_selector(|| "repo-scroll".to_string())
        .flex()
        .flex_col()
        .gap(gap)
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .children(rows)
        .into_any_element()
}

/// The whole fixed column, from top navigation to the quick entries.
pub(super) fn sidebar(
    nav: AnyElement,
    heading: AnyElement,
    repositories: AnyElement,
    account: AnyElement,
    quarters: AnyElement,
) -> AnyElement {
    div()
        .debug_selector(|| "sidebar".to_string())
        .flex()
        .flex_col()
        .flex_none()
        .w(px(SIDEBAR_WIDTH))
        .h_full()
        .min_h_0()
        .gap_1()
        .px(px(SIDEBAR_PADDING))
        .pt_2()
        .pb_2()
        .child(nav)
        .child(heading)
        .child(repositories)
        .child(account)
        .child(quarters)
        .into_any_element()
}
