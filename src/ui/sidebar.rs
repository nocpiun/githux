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
        ACCOUNT_ROW_HEIGHT, FOOTER_DIVIDER_HEIGHT, FOOTER_DIVIDER_SPACE, REPO_LEFT_PADDING,
        REPO_ROW_HEIGHT, SIDEBAR_INDICATOR_HEIGHT_RATIO, SIDEBAR_INDICATOR_MAX_HEIGHT,
        SIDEBAR_INDICATOR_MIN_HEIGHT, SIDEBAR_INDICATOR_RADIUS, SIDEBAR_INDICATOR_WIDTH,
        SIDEBAR_PADDING, SIDEBAR_RAIL_ITEM_SIZE, SIDEBAR_ROW_GAP, STATUS_BUTTON_SIZE,
    },
    scrolling::{self, SmoothScroll},
    sidebar_state::SidebarFrame,
};

type Run = Box<dyn Fn(&mut Window, &mut App) + 'static>;

impl super::Shell {
    pub(super) fn sidebar_divider(
        &self,
        _window: &Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        use super::{geometry::*, sidebar_state::SidebarDrag};
        let width = self.sidebar_state.frame.width;
        let collapsed = self.sidebar_state.collapsed;
        let (bg, hover, fg) = (
            cx.theme().colors.bg,
            cx.theme().colors.hover,
            cx.theme().colors.fg_muted,
        );
        let this = cx.entity();
        let sash_opacity = self.sidebar_state.sash();
        let handle = div()
            .id("sidebar-resize")
            .debug_selector(|| "sidebar-resize".to_string())
            .absolute()
            .top_0()
            .bottom(px(SIDEBAR_PADDING))
            .left(px(width))
            .w(px(SIDEBAR_RESIZE_HIT_WIDTH))
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .child(
                // A thin foreground line centred in the hit band. Invisible until the pointer is
                // on the band or a drag is running, then a short foreground fade; the hit band
                // itself stays wide and independent of this visual width.
                div()
                    .debug_selector(|| "sidebar-sash".to_string())
                    .absolute()
                    .top(px(ISLAND_RADIUS))
                    .bottom(px(ISLAND_INSET + ISLAND_RADIUS))
                    .left(px((SIDEBAR_RESIZE_HIT_WIDTH - SIDEBAR_SASH_WIDTH) / 2.0))
                    .w(px(SIDEBAR_SASH_WIDTH))
                    .bg(fg.opacity(sash_opacity)),
            )
            .on_mouse_move(cx.listener(move |shell, _, _, cx| {
                if !shell.sidebar_state.sash_hovered() {
                    shell.sidebar_state.set_sash_hover(
                        true,
                        cx.background_executor().now(),
                        cx.theme().reduced_motion || cx.reduce_motion(),
                    );
                    cx.notify();
                }
            }))
            .on_mouse_exit(cx.listener(move |shell, _, _, cx| {
                shell.sidebar_state.set_sash_hover(
                    false,
                    cx.background_executor().now(),
                    cx.theme().reduced_motion || cx.reduce_motion(),
                );
                cx.notify();
            }))
            .when(!self.sidebar_state.collapsed, |handle| {
                handle.on_drag(
                    SidebarDrag {
                        owner: cx.entity_id(),
                        start_width: width,
                        start_x: std::cell::Cell::new(_window.mouse_position().x),
                    },
                    move |drag, _, window, cx| {
                        drag.start_x.set(window.mouse_position().x);
                        this.update(cx, |shell, cx| {
                            shell.sidebar_state.begin_resize();
                            shell
                                .sidebar_state
                                .note_drag(cx.background_executor().now());
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
            // Centre on the island's left border line, and on the island's own height (which
            // stops at the bottom inset), so both follow one geometry source through animation,
            // resize, maximize and restore.
            .left(px(
                width + ISLAND_GAP + ISLAND_BORDER_WIDTH / 2.0 - SIDEBAR_TOGGLE_SIZE / 2.0
            ))
            .top(gpui::relative(0.5))
            .mt(px(-(SIDEBAR_TOGGLE_SIZE + ISLAND_INSET) / 2.0))
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
                let reduced = cx.theme().reduced_motion || cx.reduce_motion();
                shell.sidebar_state.toggle(
                    cx.background_executor().now(),
                    window.viewport_size().width.into(),
                    reduced,
                );
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

/// Per-render row metrics shared by the sidebar navigation rows: the animation's expansion
/// ratio and the resulting row height, so the marker and the row agree.
#[derive(Clone, Copy)]
pub(super) struct RowMetrics {
    pub expansion: f32,
    pub height: f32,
}

/// Build navigation-row metrics from the current frame and the window's rem size.
pub(super) fn nav_metrics(expansion: f32, rem_size: gpui::Pixels, cx: &App) -> RowMetrics {
    let expanded = f32::from(
        cx.theme()
            .control_height(ely_gpui_component::theme::ControlSize::Md)
            .to_pixels(rem_size),
    );
    RowMetrics {
        expansion,
        height: SIDEBAR_RAIL_ITEM_SIZE + (expanded - SIDEBAR_RAIL_ITEM_SIZE) * expansion,
    }
}

/// Home or Settings; walks and clicks as every other sidebar row does.
pub(super) fn nav_row(
    selector: &'static str,
    icon: IconName,
    label: &'static str,
    active: bool,
    metrics: RowMetrics,
    run: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (fg, hover, muted) = (theme.colors.fg, theme.colors.hover, theme.colors.fg_muted);
    let (height, expansion) = (metrics.height, metrics.expansion);
    let collapsed = expansion == 0.0;
    let row = div()
        .id(selector)
        .debug_selector(move || selector.to_string())
        .flex()
        .items_center()
        .gap(px(8.0 * expansion))
        .h(px(height))
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
                row.px(px(6.0 + 2.0 * expansion)).overflow_hidden().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .opacity(expansion)
                        .child(Ellipsis::new(label)),
                )
            }
        })
        .tooltip(Tooltip::text(label));
    marked(row.into_any_element(), active, height, cx)
}

/// Full-width lane: clickable background starts at the inset; marker has no hitbox.
/// Repository lanes and their markers are clipped together by the repository viewport.
fn marked(row: AnyElement, active: bool, row_height: f32, cx: &App) -> AnyElement {
    let indicator_height = (row_height * SIDEBAR_INDICATOR_HEIGHT_RATIO)
        .clamp(SIDEBAR_INDICATOR_MIN_HEIGHT, SIDEBAR_INDICATOR_MAX_HEIGHT);
    let inset = ((row_height - indicator_height) / 2.0).max(0.0);
    div()
        .relative()
        .flex_none()
        .w_full()
        .min_w_0()
        .h(px(row_height))
        .pl(px(SIDEBAR_PADDING))
        .child(row)
        .when(active, |lane| {
            lane.child(
                div()
                    .debug_selector(|| "sidebar-indicator".to_string())
                    .absolute()
                    .left_0()
                    .top(px(inset))
                    .w(px(SIDEBAR_INDICATOR_WIDTH))
                    .h(px(indicator_height))
                    .rounded_tl(px(0.0))
                    .rounded_bl(px(0.0))
                    .rounded_tr(px(SIDEBAR_INDICATOR_RADIUS))
                    .rounded_br(px(SIDEBAR_INDICATOR_RADIUS))
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
    frame: SidebarFrame,
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
    let collapsed = frame.expansion == 0.0;
    let height =
        SIDEBAR_RAIL_ITEM_SIZE + (REPO_ROW_HEIGHT - SIDEBAR_RAIL_ITEM_SIZE) * frame.expansion;
    let full_name = repo.full_name();
    let avatar_name = repo.namespace.clone();
    let run: Run = Box::new(run);
    let row = div()
        .id(("repo-row", index))
        .debug_selector(move || format!("repo-row-{index}"))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.0 * frame.expansion))
        .h(px(height))
        .overflow_hidden()
        .w_full()
        .min_w_0()
        .when(collapsed, |row| row.justify_center())
        .when(!collapsed, |row| {
            row.pl(px(3.0 + (REPO_LEFT_PADDING - 3.0) * frame.expansion))
                .pr(px(8.0 * frame.expansion))
        })
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
        .child(
            div()
                .debug_selector(move || format!("sidebar-repo-avatar-{index}"))
                .flex_none()
                .child(Avatar::new(("repo-avatar", index), avatar_name).size(AvatarSize::Xs)),
        )
        .when(!collapsed, |row| {
            row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .opacity(frame.expansion)
                    .child(Ellipsis::new(full_name.clone())),
            )
        })
        .tooltip(Tooltip::text(full_name))
        .into_any_element();
    marked(row, active, height, cx)
}

/// The signed-in account. Only the compact avatar/name/@id cluster opens the profile; the row
/// itself is not a button and does not light up as a whole. Expanded mode adds a separate
/// placeholder status button on the right.
pub(super) fn account_row(
    account: AccountFixture,
    active: bool,
    frame: SidebarFrame,
    run: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (name_size, login_size) = (theme.text_size(TextSize::Sm), theme.text_size(TextSize::Xs));
    let (fg, subtle, focus) = (colors.fg, colors.fg_subtle, colors.fg_muted);
    let collapsed = frame.expansion == 0.0;
    let height =
        SIDEBAR_RAIL_ITEM_SIZE + (ACCOUNT_ROW_HEIGHT - SIDEBAR_RAIL_ITEM_SIZE) * frame.expansion;
    let identity_label = format!("Profile: {} (@{})", account.name, account.login);
    let collapsed_label = format!(
        "{} (@{}) · {} {}",
        account.name, account.login, account.status_emoji, account.status_label
    );
    let status_label = format!("Status: {}", account.status_label);
    let run: Run = Box::new(run);

    let identity = div()
        .id("account-identity")
        .debug_selector(|| "account-identity".to_string())
        .flex()
        .items_center()
        .min_w_0()
        .gap(px(8.0 * frame.expansion))
        .rounded(theme.radius(Radius::Sm))
        .border_1()
        .border_color(transparent_black())
        .role(Role::Button)
        .aria_label(if collapsed {
            collapsed_label.clone()
        } else {
            identity_label.clone()
        })
        .cursor_pointer()
        .tab_index(0)
        .focus(move |style| style.border_color(focus))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| run(window, cx))
        .tooltip(Tooltip::text(if collapsed {
            collapsed_label.clone()
        } else {
            identity_label.clone()
        }))
        .child(SidebarAvatar {
            name: account.name.into(),
            expansion: frame.expansion,
        })
        .when(!collapsed, |identity| {
            identity.child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .opacity(frame.expansion)
                    .child(
                        div()
                            .text_size(name_size)
                            .text_color(fg)
                            .child(Ellipsis::new(account.name)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "account-login".to_string())
                            .text_size(login_size)
                            .text_color(subtle)
                            .child(Ellipsis::new(format!("@{}", account.login)))
                            .hover(|style| style.underline()),
                    ),
            )
        });

    let status = div()
        .id("account-status")
        .debug_selector(|| "account-status".to_string())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(STATUS_BUTTON_SIZE))
        .rounded(theme.radius(Radius::Sm))
        .border_1()
        .border_color(transparent_black())
        .role(Role::Button)
        .aria_label(status_label.clone())
        .cursor_pointer()
        .tab_index(0)
        .focus(move |style| style.border_color(focus))
        .hover(move |style| style.bg(colors.hover))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, _, cx| {
            // Placeholder for a future GitHub status editor: no navigation, no write.
            cx.stop_propagation();
        })
        .tooltip(Tooltip::text(account.status_label))
        .child(
            div()
                .text_size(theme.text_size(TextSize::Sm))
                .child(account.status_emoji),
        );

    let row = div()
        .id("account-row")
        .debug_selector(|| "account-row".to_string())
        .flex()
        .flex_none()
        .items_center()
        .h(px(height))
        .overflow_hidden()
        .w_full()
        .min_w_0()
        .when(collapsed, |row| row.justify_center())
        .when(!collapsed, |row| {
            row.justify_between().pr(px(3.0 + 5.0 * frame.expansion))
        })
        .text_color(fg)
        .child(identity)
        .when(!collapsed, |row| row.child(status))
        .into_any_element();
    marked(row, active, height, cx)
}

/// Account-only interpolated avatar. Ely Avatar exposes enum sizes, so use its initials/tint
/// convention here with a continuous diameter; Profile's own Ely avatar is untouched.
#[derive(IntoElement)]
struct SidebarAvatar {
    name: gpui::SharedString,
    expansion: f32,
}
impl gpui::RenderOnce for SidebarAvatar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let small = theme.avatar_size(AvatarSize::Xs);
        let large = theme.avatar_size(AvatarSize::Md);
        let side = small + (large - small) * self.expansion;
        let hash = self.name.bytes().fold(0u32, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u32::from(byte))
        });
        let tint = theme.colors.chart[hash as usize % theme.colors.chart.len()];
        let words = self.name.split_whitespace().collect::<Vec<_>>();
        let mut initials = words
            .first()
            .and_then(|s| s.chars().next())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        if words.len() > 1
            && let Some(last) = words.last().and_then(|s| s.chars().next())
        {
            initials.extend(last.to_uppercase());
        }
        div()
            .debug_selector(|| "sidebar-account-avatar".to_string())
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(side)
            .rounded_full()
            .bg(tint.opacity(0.16))
            .text_color(tint)
            .text_size(
                theme.text_size(TextSize::Xs) * (1.0 - self.expansion)
                    + theme.text_size(TextSize::Sm) * self.expansion,
            )
            .font_weight(FontWeight::SEMIBOLD)
            .child(initials)
    }
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
    let (radius, hover, muted, focus, border) = (
        theme.radius(Radius::Md),
        colors.hover,
        if active { colors.fg } else { colors.fg_muted },
        colors.fg_muted,
        colors.border,
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
        .border_color(border)
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

/// The collapsed rail's overflow trigger and menu.
///
/// The menu is drawn here with Ely's public theme tokens (overlay surface, border, elevation and
/// menu width), and the shell owns its open state, focus and keys. Ely's `OverflowMenu` keeps a
/// private focus takeover; a sidebar repaint during the collapse animation could re-arm it and
/// leave Escape or a window change unable to close the panel. Owning the state keeps every
/// dismissal path explicit (and testable) while the look still follows the theme.
pub(super) struct OverflowMenuView<'a> {
    pub shell: gpui::Entity<super::Shell>,
    pub route: &'a crate::workspace::Route,
    pub focus: &'a FocusHandle,
    pub menu_focus: &'a FocusHandle,
    pub open: std::rc::Rc<std::cell::Cell<bool>>,
    pub menu_open: bool,
    pub cursor: usize,
    pub viewport: gpui::Size<gpui::Pixels>,
}

pub(super) fn overflow(view: OverflowMenuView<'_>, cx: &App) -> AnyElement {
    let OverflowMenuView {
        shell,
        route,
        focus,
        menu_focus,
        open,
        menu_open,
        cursor,
        viewport,
    } = view;
    let theme = cx.theme();
    let colors = &theme.colors;
    let this = shell.clone();
    let toggle = {
        let this = this.clone();
        let open = open.clone();
        move |window: &mut Window, cx: &mut App| {
            let opening = !open.get();
            open.set(opening);
            this.update(cx, |shell, cx| {
                if opening {
                    shell.open_sidebar_menu(window, cx);
                } else {
                    shell.close_sidebar_menu(window, cx);
                }
            });
        }
    };
    let trigger = div()
        .id("sidebar-more-trigger")
        .debug_selector(|| "sidebar-more".to_string())
        .track_focus(focus)
        .flex()
        .flex_none()
        .justify_center()
        .pl(px(SIDEBAR_PADDING))
        .w_full()
        .child(
            div()
                .id("sidebar-more-button")
                .debug_selector(|| "sidebar-more-button".to_string())
                .size(px(SIDEBAR_RAIL_ITEM_SIZE))
                .flex_none()
                .rounded(theme.radius(Radius::Sm))
                .tab_index(0)
                .role(Role::Button)
                .aria_label("More")
                .cursor_pointer()
                .focus(move |style| style.border_color(colors.focus))
                .hover(move |style| style.bg(colors.hover))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(move |_, window, cx| toggle(window, cx))
                .child(
                    Icon::new(IconName::Ellipsis)
                        .size(IconSize::Sm)
                        .color(colors.fg_muted),
                )
                .tooltip(Tooltip::text("More")),
        )
        .text_color(colors.fg_muted);

    if !menu_open {
        return trigger.into_any_element();
    }

    let entries = [
        (
            "Notifications",
            IconName::Bell,
            Some(crate::workspace::Route::Notifications),
        ),
        (
            "Issues",
            IconName::CircleDot,
            Some(crate::workspace::Route::Issues),
        ),
        (
            "PRs",
            IconName::GitPullRequest,
            Some(crate::workspace::Route::PullRequests),
        ),
        ("New", IconName::Plus, None),
    ];
    let row_height = theme.control_height(ely_gpui_component::theme::ControlSize::Md);
    let rows = entries
        .into_iter()
        .enumerate()
        .map(|(index, (label, icon, target))| {
            let active = target.as_ref().is_some_and(|target| route == target);
            let disabled = target.is_none();
            let highlighted = cursor == index && !disabled;
            let this = this.clone();
            let run = {
                let this = this.clone();
                let open = open.clone();
                let target = target.clone();
                move |window: &mut Window, cx: &mut App| {
                    open.set(false);
                    this.update(cx, |shell, cx| {
                        shell.pick_sidebar_menu(target.clone(), window, cx);
                    });
                }
            };
            let row = div()
                .id(("sidebar-menu-row", index))
                .debug_selector(move || format!("menu-row-{label}"))
                .flex()
                .items_center()
                .gap_2()
                .h(row_height)
                .w_full()
                .min_w_0()
                .px_2()
                .rounded(theme.radius(Radius::Md))
                .text_color(if disabled {
                    colors.fg_subtle
                } else {
                    colors.fg
                })
                .when(highlighted, |row| row.bg(colors.hover))
                .when(active && !disabled, |row| {
                    row.font_weight(FontWeight::MEDIUM)
                })
                .when(disabled, |row| row.opacity(0.45))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(14.0))
                        .child(Icon::new(icon).size(IconSize::Sm).color(if disabled {
                            colors.fg_subtle
                        } else {
                            colors.fg_muted
                        })),
                )
                .child(div().flex_1().min_w_0().child(Ellipsis::new(label)));
            if disabled {
                row.aria_description("Creating resources is not available in this round.")
                    .into_any_element()
            } else {
                row.role(Role::Button)
                    .aria_label(label)
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(move |_, window, cx| run(window, cx))
                    .when(active, |row| {
                        row.child(Icon::new(IconName::Check).size(IconSize::Xs))
                    })
                    .into_any_element()
            }
        })
        .collect::<Vec<_>>();

    let panel = div()
        .id("sidebar-menu-panel")
        .debug_selector(|| "menu-panel-0".to_string())
        .track_focus(menu_focus)
        .relative()
        .occlude()
        .w(theme.menu_width())
        .p_1()
        .flex()
        .flex_col()
        .gap_0p5()
        .rounded(theme.radius(Radius::Lg))
        .bg(colors.overlay)
        .border_1()
        .border_color(colors.border)
        .shadow(theme.elevation(ely_gpui_component::theme::Elevation::Floating))
        .text_size(theme.text_size(TextSize::Sm))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_key_down({
            let this = this.clone();
            let open = open.clone();
            move |event, window, cx| {
                let key = event.keystroke.key.as_str();
                match key {
                    "escape" => {
                        open.set(false);
                        this.update(cx, |shell, cx| shell.close_sidebar_menu(window, cx));
                        cx.stop_propagation();
                    }
                    "down" | "up" => {
                        let by = if key == "down" { 1 } else { -1 };
                        this.update(cx, |shell, cx| {
                            shell.move_sidebar_menu_cursor(by, cx);
                        });
                        cx.stop_propagation();
                    }
                    "enter" | "space" if !event.keystroke.modifiers.modified() => {
                        this.update(cx, |shell, cx| {
                            shell.pick_sidebar_menu_cursor(window, cx);
                        });
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }
        })
        .children(rows);

    // One raised layer: the full-window dismissal shield first, then the panel above it. Sharing
    // the layer keeps their stacking together, so the press and its release both land on the
    // shield (or the panel) and never on the row underneath.
    let shield = {
        let this = this.clone();
        let open = open.clone();
        div()
            .id("sidebar-menu-dismiss")
            .debug_selector(|| "sidebar-menu-dismiss".to_string())
            .absolute()
            .top_0()
            .left_0()
            .w(viewport.width)
            .h(viewport.height)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
                open.set(false);
                this.update(cx, |shell, cx| shell.close_sidebar_menu(window, cx));
            })
    };
    trigger
        .child(
            gpui::deferred(
                gpui::anchored()
                    .position(gpui::Point::default())
                    .child(
                        div()
                            .relative()
                            .w(viewport.width)
                            .h(viewport.height)
                            .child(shield)
                            .child(
                                gpui::anchored()
                                    .position(gpui::Point::new(px(SIDEBAR_PADDING), px(0.0)))
                                    .anchor(gpui::Anchor::BottomLeft)
                                    .snap_to_window()
                                    .child(panel),
                            ),
                    )
                    .snap_to_window(),
            )
            .with_priority(1),
        )
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

/// The repository scroll group: the only part of the sidebar that scrolls. A transparent overlay
/// routes wheel deltas through the shared smoothing instead of GPUI's per-event jump.
pub(super) fn repository_group(
    rows: Vec<AnyElement>,
    scroll: &ScrollHandle,
    smooth: SmoothScroll,
    cx: &App,
) -> AnyElement {
    let gap = cx
        .theme()
        .control_height(ely_gpui_component::theme::ControlSize::Sm)
        / 12.0;
    div()
        .debug_selector(|| "repo-scroll-region".to_string())
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(
            div()
                .id("repo-scroll-list")
                .debug_selector(|| "repo-scroll".to_string())
                .flex()
                .flex_col()
                .gap(gap)
                .size_full()
                .overflow_y_scroll()
                .overflow_x_hidden()
                .track_scroll(scroll)
                .children(rows),
        )
        .child(scrolling::wheel_overlay("repo", smooth))
        .into_any_element()
}

/// A thin, low-contrast rule between the repository viewport and the fixed footer.
fn footer_divider(cx: &App) -> AnyElement {
    div()
        .debug_selector(|| "sidebar-footer-divider".to_string())
        .flex_none()
        .w_full()
        .flex()
        .flex_col()
        // Equal spacers on both sides of the rule. The column's own gap adds below the whole
        // band, so the lower spacer is shortened by exactly that much: the rendered space from
        // the list to the rule equals the space from the rule to the identity in both modes.
        .pt(px(FOOTER_DIVIDER_SPACE))
        .pb(px(FOOTER_DIVIDER_SPACE - SIDEBAR_ROW_GAP))
        .px(px(SIDEBAR_PADDING))
        .child(
            div()
                .w_full()
                .h(px(FOOTER_DIVIDER_HEIGHT))
                .bg(cx.theme().colors.border),
        )
        .into_any_element()
}

/// The whole fixed column, from top navigation to the quick entries.
pub(super) fn sidebar(
    nav: AnyElement,
    heading: AnyElement,
    repositories: AnyElement,
    account: AnyElement,
    quarters: AnyElement,
    frame: SidebarFrame,
    cx: &App,
) -> AnyElement {
    div()
        .debug_selector(|| "sidebar".to_string())
        .flex()
        .flex_col()
        .flex_none()
        .w(px(frame.width))
        .h_full()
        .min_h_0()
        .overflow_x_hidden()
        .gap(px(SIDEBAR_ROW_GAP))
        .pt(px(SIDEBAR_PADDING))
        .pb(px(SIDEBAR_PADDING))
        .child(nav)
        .when(frame.expansion > 0.0, |sidebar| {
            sidebar.child(
                div()
                    .flex_none()
                    .h(px(24.0 * frame.expansion))
                    .overflow_hidden()
                    .opacity(frame.expansion)
                    .child(heading),
            )
        })
        .child(repositories)
        // The divider owns its full band; the column gap is cancelled for this child so the rule
        // has equal space above and below in both modes.
        .child(
            div()
                .flex_none()
                .mt(px(-SIDEBAR_ROW_GAP))
                .child(footer_divider(cx)),
        )
        .child(account)
        .child(quarters)
        .into_any_element()
}
