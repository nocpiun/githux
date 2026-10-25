//! Fixed global sidebar: top navigation, a bounded repository group, the account row and the four
//! quick entries.
//!
//! Only the repository group scrolls. Home, Settings, the group heading, the account row and the
//! quick entries are flex-none children of a full-height column, so resizing the window moves the
//! boundary of the scroll group and never the fixed parts. Rows expose accessible names and full
//! identifiers; long names are cut inside the column instead of widening it.

use ely_gpui_component::{
    data_display::Avatar,
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
    geometry::{
        ACCOUNT_ROW_HEIGHT, REPO_ACCOUNT_GAP, REPO_LEFT_PADDING, REPO_ROW_HEIGHT, SIDEBAR_PADDING,
    },
};

type Run = Box<dyn Fn(&mut Window, &mut App) + 'static>;

impl super::Shell {
    pub(super) fn sidebar_divider(
        &self,
        window: &Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        use super::{geometry::*, sidebar_state::SidebarDrag};
        let width = self
            .sidebar_state
            .width(window.viewport_size().width.into());
        let collapsed = self.sidebar_state.collapsed;
        let (bg, hover, fg) = (
            cx.theme().colors.bg,
            cx.theme().colors.hover,
            cx.theme().colors.fg_muted,
        );
        let this = cx.entity();
        let handle = div()
            .id("sidebar-resize")
            .debug_selector(|| "sidebar-resize".to_string())
            .absolute()
            .top_0()
            .bottom(px(SIDEBAR_PADDING))
            .left(px(width))
            .w(px(SIDEBAR_RESIZE_HIT_WIDTH))
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .when(!collapsed, |handle| {
                handle.on_drag(
                    SidebarDrag {
                        owner: cx.entity_id(),
                        start_width: width,
                        start_x: std::cell::Cell::new(window.mouse_position().x),
                    },
                    move |drag, _, window, cx| {
                        drag.start_x.set(window.mouse_position().x);
                        this.update(cx, |shell, cx| {
                            shell.sidebar_state.dragging = true;
                            cx.notify();
                        });
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::ResizeLeftRight, window);
                        // A drag uses GPUI capture and an empty drag image; no page can receive a click.
                        window.prevent_default();
                        cx.new(|_| gpui::EmptyView)
                    },
                )
            });
        let button = div()
            .id("sidebar-toggle")
            .debug_selector(|| "sidebar-toggle".to_string())
            .absolute()
            .left(px(width + ISLAND_GAP / 2.0 - SIDEBAR_TOGGLE_SIZE / 2.0))
            .top(gpui::relative(0.5))
            .mt(px(-SIDEBAR_TOGGLE_SIZE / 2.0))
            .size(px(SIDEBAR_TOGGLE_SIZE))
            .rounded_full()
            .bg(bg)
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .role(Role::Button)
            .aria_label(if collapsed {
                "Expand sidebar"
            } else {
                "Collapse sidebar"
            })
            .tab_index(0)
            .cursor_pointer()
            .border_1()
            .border_color(transparent_black())
            .focus(move |style| style.border_color(fg))
            .hover(move |style| style.bg(hover))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(|shell, _, window, cx| {
                window.focus(&shell.root_focus, cx);
                shell.sidebar_state.dragging = false;
                shell.sidebar_state.collapsed = !shell.sidebar_state.collapsed;
                shell.sidebar_state.menu_open.set(false);
                cx.stop_active_drag(window);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronLeft
                })
                .size(IconSize::Xs)
                .color(fg),
            );
        div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .child(handle)
            .child(button)
            .into_any_element()
    }
}

/// Home or Settings; walks and clicks as every other sidebar row does.
pub(super) fn nav_row(
    selector: &'static str,
    icon: IconName,
    label: &'static str,
    active: bool,
    collapsed: bool,
    run: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (fg, hover, muted) = (theme.colors.fg, theme.colors.hover, theme.colors.fg_muted);
    let height = theme.control_height(ely_gpui_component::theme::ControlSize::Md);
    let row = div()
        .id(selector)
        .debug_selector(move || selector.to_string())
        .flex()
        .items_center()
        .gap_2()
        .h(height)
        .w_full()
        .min_w_0()
        .rounded(theme.radius(Radius::Md))
        .border_1()
        .border_color(transparent_black())
        .text_size(theme.text_size(TextSize::Sm))
        .text_color(if active { fg } else { muted })
        .when(active, |row| row.bg(hover).font_weight(FontWeight::MEDIUM))
        .hover(move |style| style.bg(hover).text_color(fg))
        .role(Role::Button)
        .aria_label(label)
        .cursor_pointer()
        .tab_index(0)
        .focus(move |style| style.border_color(muted))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| run(window, cx))
        .child(Icon::new(icon).size(IconSize::Sm))
        .map(|row| {
            if collapsed {
                row.justify_center()
            } else {
                row.px_2()
                    .child(div().flex_1().min_w_0().child(Ellipsis::new(label)))
            }
        })
        .tooltip(Tooltip::text(label));
    marked(row.into_any_element(), active, height.into(), cx)
}

/// Full-width lane: clickable background starts at the inset; marker has no hitbox.
/// Repository lanes and their markers are clipped together by the repository viewport.
fn marked(row: AnyElement, active: bool, height: gpui::Length, cx: &App) -> AnyElement {
    div()
        .relative()
        .flex_none()
        .w_full()
        .min_w_0()
        .h(height)
        .pl(px(SIDEBAR_PADDING))
        .child(row)
        .when(active, |lane| {
            lane.child(
                div()
                    .debug_selector(|| "sidebar-indicator".to_string())
                    .absolute()
                    .left(px(1.0))
                    .top(gpui::relative(0.5))
                    .mt(px(-8.0))
                    .w(px(2.0))
                    .h(px(16.0))
                    .rounded_full()
                    .bg(cx.theme().colors.fg_muted),
            )
        })
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
    collapsed: bool,
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
        colors.fg_muted,
    );
    let full_name = repo.full_name();
    let avatar_name = repo.namespace.clone();
    let run: Run = Box::new(run);
    let row = div()
        .id(("repo-row", index))
        .debug_selector(move || format!("repo-row-{index}"))
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(REPO_ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .when(collapsed, |row| row.justify_center())
        .when(!collapsed, |row| row.pl(px(REPO_LEFT_PADDING)).pr_2())
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
        .when(!collapsed, |row| {
            row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Ellipsis::new(full_name.clone())),
            )
        })
        .tooltip(Tooltip::text(full_name))
        .into_any_element();
    marked(row, active, px(REPO_ROW_HEIGHT).into(), cx)
}

/// The signed-in account; pressing it opens the profile.
pub(super) fn account_row(
    account: AccountFixture,
    active: bool,
    collapsed: bool,
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
    let (hover, fg, subtle, focus) = (colors.hover, colors.fg, colors.fg_subtle, colors.fg_muted);
    let label = format!("Profile: {} (@{})", account.name, account.login);
    let run: Run = Box::new(run);
    let row = div()
        .id("account-row")
        .debug_selector(|| "account-row".to_string())
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(ACCOUNT_ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .when(collapsed, |row| row.justify_center())
        .when(!collapsed, |row| row.px_2())
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
        .child(Avatar::new("account-avatar", account.name).size(AvatarSize::Md))
        .when(!collapsed, |row| {
            row.child(
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
        })
        .tooltip(Tooltip::text(label))
        .into_any_element();
    marked(row, active, px(ACCOUNT_ROW_HEIGHT).into(), cx)
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
        colors.fg_muted,
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
        .pl(px(SIDEBAR_PADDING))
        .children(buttons)
        .into_any_element()
}

/// A quiet group heading above the repository list.
pub(super) fn group_heading(text: String, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .debug_selector(|| "repo-heading".to_string())
        .flex_none()
        .pl(px(SIDEBAR_PADDING + 8.0))
        .pt_1()
        .text_size(theme.text_size(TextSize::Xs))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.fg_subtle)
        .child(text)
        .into_any_element()
}

/// Ely's menu supplies keyboard navigation, disabled rows, window snapping and focus takeover.
pub(super) fn overflow(
    shell: gpui::Entity<super::Shell>,
    route: &crate::workspace::Route,
    focus: &FocusHandle,
    open: std::rc::Rc<std::cell::Cell<bool>>,
    cx: &App,
) -> AnyElement {
    use crate::workspace::Route;
    use ely_gpui_component::menus::{Menu, MenuItem, OverflowMenu};
    let mut menu = Menu::new();
    for (label, icon, target) in [
        ("Notifications", IconName::Bell, Route::Notifications),
        ("Issues", IconName::CircleDot, Route::Issues),
        ("PRs", IconName::GitPullRequest, Route::PullRequests),
    ] {
        let this = shell.clone();
        menu = menu.item(
            MenuItem::check(label, *route == target)
                .icon(icon)
                .on_click(move |_, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(target.clone(), cx);
                    });
                }),
        );
    }
    menu = menu.item(MenuItem::new("New").icon(IconName::Plus).disabled(true));
    div()
        .id("sidebar-more-host")
        .track_focus(focus)
        .debug_selector(|| "sidebar-more".to_string())
        .flex()
        .flex_none()
        .justify_center()
        .pl(px(SIDEBAR_PADDING))
        .w_full()
        .child(SidebarOverflow {
            menu: OverflowMenu::new("sidebar-overflow", menu)
                .tooltip("Notifications, Issues, PRs, New"),
            restore: focus.clone(),
            open,
        })
        .text_color(cx.theme().colors.fg_muted)
        .into_any_element()
}

/// Ely owns the menu's focus and keyboard handling. Observe that focus takeover to place a
/// dismissal shield below its raised panel; an outside press then cannot click the page behind.
#[derive(IntoElement)]
struct SidebarOverflow {
    menu: ely_gpui_component::menus::OverflowMenu,
    restore: FocusHandle,
    open: std::rc::Rc<std::cell::Cell<bool>>,
}

impl gpui::RenderOnce for SidebarOverflow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state("sidebar-menu-focus", cx, |_, _| None::<FocusHandle>);
        let before = window.focused(cx);
        let menu = self.menu.render(window, cx).into_any_element();
        let after = window.focused(cx);
        if before != after {
            state.update(cx, |focus, _| *focus = after.clone());
        }
        let open = state
            .read(cx)
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window) && *focus != self.restore);
        self.open.set(open);
        let viewport = window.viewport_size();
        div().child(menu).when(open, |host| {
            host.child(
                gpui::deferred(
                    gpui::anchored().position(gpui::Point::default()).child(
                        div()
                            .id("sidebar-menu-dismiss")
                            .debug_selector(|| "sidebar-menu-dismiss".to_string())
                            .w(viewport.width)
                            .h(viewport.height)
                            .occlude()
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                window.focus(&self.restore, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }),
                    ),
                )
                .with_priority(0),
            )
        })
    }
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
    let gap = cx
        .theme()
        .control_height(ely_gpui_component::theme::ControlSize::Sm)
        / 12.0;
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
        .overflow_x_hidden()
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
    width: f32,
    collapsed: bool,
) -> AnyElement {
    div()
        .debug_selector(|| "sidebar".to_string())
        .flex()
        .flex_col()
        .flex_none()
        .w(px(width))
        .h_full()
        .min_h_0()
        .gap_1()
        .pt(px(SIDEBAR_PADDING))
        .pb(px(SIDEBAR_PADDING))
        .child(nav)
        .when(!collapsed, |sidebar| sidebar.child(heading))
        .child(repositories)
        .child(div().flex_none().h(px(REPO_ACCOUNT_GAP - 8.0)))
        .child(account)
        .child(quarters)
        .into_any_element()
}
