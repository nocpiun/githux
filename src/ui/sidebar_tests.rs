//! Interaction and measured layout coverage for the resizable sidebar.
use super::{Shell, geometry::*, sidebar_state::SidebarState};
use crate::workspace::Route;
use gpui::{
    Bounds, KeyUpEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels,
    ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, VisualContext, VisualTestContext,
    point, px, size,
};
use std::time::{Duration, Instant};

fn shell(cx: &mut TestAppContext) -> (gpui::Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        // Existing event tests inspect settled geometry; motion has separate controlled-time tests.
        ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = true);
        Shell::bind_keys(cx);
    });
    let (view, cx) = cx.add_window_view(Shell::new);
    cx.simulate_resize(size(px(1200.0), px(700.0)));
    cx.run_until_parked();
    (view, cx)
}
fn bounds(name: &'static str, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.debug_bounds(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}
fn click(name: &'static str, cx: &mut VisualTestContext) {
    let p = bounds(name, cx).center();
    cx.simulate_click(p, Modifiers::none());
    cx.run_until_parked();
}
fn press(key: &'static str, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(key);
    cx.run_until_parked();
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).unwrap(),
    });
    cx.run_until_parked();
}
fn drag_start(cx: &mut VisualTestContext) -> gpui::Point<Pixels> {
    let band = bounds("sidebar-resize", cx);
    let start = point(band.center().x, band.top() + px(60.0));
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: start,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_mouse_move(
        start + point(px(10.0), px(0.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    start
}
fn release(p: gpui::Point<Pixels>, cx: &mut VisualTestContext) {
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: p,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    cx.run_until_parked();
}

#[test]
fn window_budget_and_remembered_width_are_independent() {
    let mut state = SidebarState::default();
    state.begin_resize(SIDEBAR_WIDTH, SIDEBAR_WIDTH);
    state.resize_pointer_at(232.0 + 128.0, Instant::now(), 1200.0, true);
    assert_eq!(state.width(1200.0), SIDEBAR_MAX_WIDTH);
    assert_eq!(state.width(880.0), 265.0);
    state.collapsed = true;
    assert_eq!(state.width(880.0), SIDEBAR_COLLAPSED_WIDTH);
    state.collapsed = false;
    assert_eq!(state.width(1200.0), SIDEBAR_MAX_WIDTH);
    state.begin_resize(SIDEBAR_WIDTH, SIDEBAR_WIDTH);
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH - 400.0, Instant::now(), 1200.0, true);
    assert!(state.collapsed, "a far overshoot collapses");
    state.dragging = false;
    assert_eq!(state.width(1200.0), SIDEBAR_COLLAPSED_WIDTH);
}

#[gpui::test]
fn measured_insets_footer_gap_and_marker_are_outside_the_row(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    let sidebar = bounds("sidebar", cx);
    let account = bounds("account-row", cx);
    let quarters = bounds("quarter-row", cx);
    let island = bounds("island", cx);
    let repo = bounds("repo-scroll", cx);
    assert_eq!(account.left(), px(SIDEBAR_PADDING));
    assert_eq!(island.left() - account.right(), account.left());
    assert_eq!(sidebar.bottom() - quarters.bottom(), account.left());
    // The footer divider owns the whole band between the list and the identity row now.
    assert_eq!(
        account.top() - repo.bottom(),
        px(FOOTER_DIVIDER_HEIGHT + 2.0 * FOOTER_DIVIDER_SPACE)
    );
    assert_eq!(account.size.height, px(48.0));
    let marker = bounds("sidebar-indicator", cx);
    let home = bounds("nav-home", cx);
    assert!(marker.right() < home.left());
    assert_eq!(marker.center().y, home.center().y);
}

#[gpui::test]
fn drag_clamps_outside_the_band_and_release_stops_updates(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let start = drag_start(cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_state.dragging));
    let far = start + point(px(700.0), px(25.0));
    cx.simulate_mouse_move(far, Some(MouseButton::Left), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(bounds("sidebar", cx).size.width, px(360.0));
    release(far, cx);
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(bounds("sidebar", cx).size.width, px(360.0));
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.dragging));
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Home);
    drag_start(cx);
    let into_min = point(px(20.0), px(150.0));
    cx.simulate_mouse_move(into_min, Some(MouseButton::Left), Modifiers::none());
    cx.run_until_parked();
    let left = point(px(-30.0), px(150.0));
    cx.simulate_mouse_move(left, Some(MouseButton::Left), Modifiers::none());
    cx.run_until_parked();
    // A far overshoot past the minimum collapses directly instead of widening past the limit.
    assert!(
        view.read_with(cx, |s, _| s.sidebar_state.collapsed)
            || bounds("sidebar", cx).size.width == px(SIDEBAR_MIN_WIDTH)
    );
    press("escape", cx);
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.dragging));
    release(left, cx);
}

#[gpui::test]
fn collapse_restores_width_route_scroll_and_has_a_real_menu(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    view.update(cx, |s, cx| {
        s.sidebar_state.expanded_width = 320.0;
        s.repo_scroll.set_offset(point(px(0.0), px(-300.0)));
        cx.notify();
    });
    cx.run_until_parked();
    let before = view.read_with(cx, |s, _| s.repo_offset());
    click("sidebar-toggle", cx);
    assert_eq!(
        bounds("sidebar", cx).size.width,
        px(SIDEBAR_COLLAPSED_WIDTH)
    );
    assert!(cx.debug_bounds("repo-heading").is_none());
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Home);
    assert_eq!(view.read_with(cx, |s, _| s.repo_offset()), before);
    click("sidebar-more", cx);
    for selector in [
        "menu-row-Notifications",
        "menu-row-Issues",
        "menu-row-PRs",
        "menu-row-New",
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "missing {selector}");
    }
    let panel = bounds("menu-panel-0", cx);
    assert!(panel.left() >= px(0.0) && panel.right() <= px(1200.0));
    assert!(panel.top() >= px(0.0) && panel.bottom() <= px(700.0));
    click("menu-row-New", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Home);
    press("escape", cx);
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    click("sidebar-more", cx);
    click("menu-row-Issues", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Issues);
    click("account-identity", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Profile);
    click("nav-settings", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Settings);
    click("sidebar-toggle", cx);
    assert_eq!(bounds("sidebar", cx).size.width, px(320.0));
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Settings);
    assert_eq!(view.read_with(cx, |s, _| s.repo_offset()), before);
}

#[gpui::test]
fn minimum_window_preserves_island_and_collapsed_keyboard_reveals_last_repo(
    cx: &mut TestAppContext,
) {
    let (view, cx) = shell(cx);
    view.update(cx, |s, cx| {
        s.sidebar_state.expanded_width = 360.0;
        cx.notify();
    });
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    assert_eq!(bounds("sidebar", cx).size.width, px(265.0));
    assert!(bounds("island", cx).size.width >= px(SIDEBAR_CONTENT_MIN_WIDTH));
    assert!(bounds("account-row", cx).bottom() <= px(560.0));
    click("sidebar-toggle", cx);
    let last = view.read_with(cx, |s, _| s.repo_focus().last().unwrap().clone());
    cx.update(|window, cx| window.focus(&last, cx));
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let row = bounds("repo-row-123", cx);
    let list = bounds("repo-scroll", cx);
    assert!(row.top() >= list.top() && row.bottom() <= list.bottom());
    assert!(bounds("account-row", cx).bottom() <= px(560.0));
}

#[gpui::test]
fn footer_stays_inside_each_client_size(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    for (w, h) in [(1200.0, 700.0), (1066.0, 795.0), (880.0, 560.0)] {
        cx.simulate_resize(size(px(w), px(h)));
        cx.run_until_parked();
        let sidebar = bounds("sidebar", cx);
        let account = bounds("account-row", cx);
        let quarters = bounds("quarter-row", cx);
        assert_eq!(sidebar.bottom(), px(h));
        assert!(account.bottom() < px(h));
        assert_eq!(quarters.bottom(), px(h - SIDEBAR_PADDING));
        assert_eq!(
            bounds("sidebar-toggle", cx).center().y,
            bounds("shell-body", cx).center().y - px(ISLAND_INSET / 2.0)
        );
    }
}

#[gpui::test]
fn menu_keyboard_outside_click_and_resize_close_without_navigation(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("sidebar-toggle", cx);
    click("sidebar-more", cx);
    // The first entry is highlighted when the menu opens; Down moves to Issues.
    press("down", cx);
    press("enter", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Issues);
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    click("sidebar-more", cx);
    // A press over the dismissal layer closes the menu and must not activate the row underneath.
    let shield = bounds("sidebar-menu-dismiss", cx);
    let away_from_panel = shield.center();
    cx.simulate_click(away_from_panel, Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    assert_eq!(
        view.read_with(cx, |s, _| s.route()),
        Route::Issues,
        "a dismissal click must not activate the row under it"
    );
    click("sidebar-more", cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    cx.update(|window, cx| assert!(window.focused(cx).is_some()));
}

#[gpui::test]
fn repository_marker_scrolls_with_its_row_and_never_moves_footer(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let before = bounds("sidebar-indicator", cx);
    let account = bounds("account-row", cx);
    let scroll = bounds("repo-scroll", cx);
    assert!(before.top() >= scroll.top() && before.bottom() <= scroll.bottom());
    view.update(cx, |s, cx| {
        s.repo_scroll.set_offset(point(px(0.0), px(-500.0)));
        cx.notify();
    });
    cx.run_until_parked();
    let marker = bounds("sidebar-indicator", cx);
    let row = bounds("repo-row-0", cx);
    assert_eq!(marker.center().y, row.center().y);
    assert!(marker.bottom() < scroll.top());
    assert_eq!(bounds("account-row", cx), account);
    assert_eq!(
        view.read_with(cx, |s, _| s.page_offset()),
        gpui::Point::default()
    );
}

#[gpui::test]
fn menu_dismissal_shield_leaves_and_window_resize_keeps_normal_focus(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("sidebar-toggle", cx);
    click("sidebar-more", cx);
    assert!(cx.debug_bounds("sidebar-menu-dismiss").is_some());
    press("escape", cx);
    assert!(cx.debug_bounds("sidebar-menu-dismiss").is_none());
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.menu_open.get()));
    let repo_focus = view.read_with(cx, |s, _| s.repo_focus[0].clone());
    cx.update(|window, cx| window.focus(&repo_focus, cx));
    cx.simulate_resize(size(px(900.0), px(600.0)));
    cx.run_until_parked();
    cx.update(|window, _| assert!(repo_focus.is_focused(window)));
    click("nav-settings", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Settings);
}

#[gpui::test]
fn rail_rows_share_square_hitboxes_and_avatar_diameter(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    click("sidebar-toggle", cx);
    let home = bounds("nav-home", cx);
    for selector in ["nav-settings", "repo-row-0", "account-row"] {
        let row = bounds(selector, cx);
        assert_eq!(row.size, home.size, "{selector}");
        assert_eq!(row.size.width, row.size.height, "{selector} must be square");
        assert_eq!(row.center().x, home.center().x);
    }
    let more = bounds("sidebar-more-button", cx);
    assert_eq!(more.size, home.size);
    assert_eq!(more.center().x, home.center().x);
    assert_eq!(
        bounds("sidebar-account-avatar", cx).size,
        bounds("sidebar-repo-avatar-0", cx).size
    );
    for selector in ["nav-home", "repo-row-0", "account-row"] {
        click(selector, cx);
        let marker = bounds("sidebar-indicator", cx);
        let row = bounds(selector, cx);
        let expected = (f32::from(row.size.height) * SIDEBAR_INDICATOR_HEIGHT_RATIO)
            .clamp(SIDEBAR_INDICATOR_MIN_HEIGHT, SIDEBAR_INDICATOR_MAX_HEIGHT);
        assert_eq!(marker.left(), bounds("shell-body", cx).left());
        assert_eq!(marker.size.width, px(SIDEBAR_INDICATOR_WIDTH));
        assert_eq!(marker.size.height, px(expected));
        assert_eq!(marker.center().y, row.center().y);
    }
    click("sidebar-toggle", cx);
    assert_eq!(bounds("sidebar-account-avatar", cx).size.width, px(32.0));
    assert_eq!(
        bounds("account-row", cx).size.height,
        px(ACCOUNT_ROW_HEIGHT)
    );
}

#[test]
fn motion_reverses_from_presented_value_and_resize_ends_it() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let mut state = SidebarState::default();
    state.toggle(start, 1200.0, false);
    assert!(state.sample(start + Duration::from_millis(80), 1200.0, false));
    let middle = state.frame;
    assert!(middle.width > SIDEBAR_COLLAPSED_WIDTH && middle.width < SIDEBAR_WIDTH);
    assert!(middle.expansion > 0.0 && middle.expansion < 1.0);
    state.toggle(start + Duration::from_millis(80), 1200.0, false);
    assert_eq!(state.frame.width, middle.width);
    assert!(state.sample(start + Duration::from_millis(120), 1200.0, false));
    assert!(state.frame.width > middle.width);
    assert!(!state.sample(start + Duration::from_millis(280), 1200.0, false));
    assert_eq!(state.frame.width, SIDEBAR_WIDTH);
    assert!(!state.sample(start + Duration::from_secs(10), 1200.0, false));
    state.toggle(start + Duration::from_secs(11), 1200.0, false);
    state.sample(start + Duration::from_millis(11050), 1200.0, false);
    state.begin_resize(232.0, 232.0);
    // Resizing interrupts the animation; widths follow the absolute pointer and clamp to the
    // window budget rather than to a remembered value.
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH, Instant::now(), 880.0, true);
    assert_eq!(state.frame.width, SIDEBAR_MIN_WIDTH);
    // The absolute pointer maps to 260 and the remaining travel is clamped by the window budget.
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH + 100.0, Instant::now(), 880.0, true);
    assert_eq!(state.frame.width, 260.0);
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH + 300.0, Instant::now(), 880.0, true);
    assert_eq!(state.frame.width, SidebarState::limit(880.0));
    assert!(!state.sample(start + Duration::from_secs(12), 880.0, false));
    state.toggle(start + Duration::from_secs(13), 880.0, true);
    assert_eq!(state.frame.width, SIDEBAR_COLLAPSED_WIDTH);
    assert!(!state.sample(start + Duration::from_secs(13), 880.0, true));
}

#[gpui::test]
fn animated_layout_uses_one_width_for_sidebar_island_and_divider(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (view, cx) = shell(cx);
    cx.update(|_, cx| ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = false));
    click("sidebar-toggle", cx);
    cx.update(|_, cx| {
        cx.background_executor()
            .advance_clock(Duration::from_millis(80))
    });
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let width = bounds("sidebar", cx).size.width;
    assert!(width > px(SIDEBAR_COLLAPSED_WIDTH) && width < px(SIDEBAR_WIDTH));
    assert_eq!(bounds("island", cx).left(), width + px(ISLAND_GAP));
    assert_eq!(bounds("sidebar-resize", cx).left(), width);
    assert_eq!(
        bounds("sidebar-toggle", cx).center().x,
        width + px(ISLAND_GAP + ISLAND_BORDER_WIDTH / 2.0)
    );
    click("sidebar-toggle", cx);
    assert_eq!(bounds("sidebar", cx).size.width, width);
    cx.update(|_, cx| {
        cx.background_executor()
            .advance_clock(Duration::from_millis(SIDEBAR_ANIMATION_MS))
    });
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(bounds("sidebar", cx).size.width, px(SIDEBAR_WIDTH));
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Home);
    // Expanding under a stationary pointer can start the independent repository-bar fade.
    // Move outside that region and settle its clock before asserting that all motion is idle.
    let header = bounds("titlebar", cx).center();
    cx.simulate_mouse_move(header, None, Modifiers::none());
    view.update(cx, |s, cx| {
        s.pin_tab_clock(s.tab_now_ms() + super::scroll::FADE_MS + 1);
        cx.notify();
    });
    cx.run_until_parked();
    // Drain the one callback already requested by the preceding frame. Settled renders
    // must not install another callback; there is no perpetual animation timer.
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| assert_eq!(window.simulate_next_frame(cx), 0));
    click("sidebar-toggle", cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    assert_eq!(
        bounds("sidebar", cx).size.width,
        px(SIDEBAR_COLLAPSED_WIDTH)
    );
}

#[gpui::test]
fn indicator_paint_has_square_left_corners_and_clips_scrolled_repo(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_scale_factor_change(1.5);
    cx.run_until_parked();
    for selector in ["nav-home", "repo-row-0", "account-row"] {
        click(selector, cx);
        let marker = bounds("sidebar-indicator", cx);
        cx.update(|window, _| {
            let scale = window.scale_factor();
            let painted = window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.bounds == marker.scale(scale))
                .expect("indicator painted");
            assert_eq!(painted.corner_radii.top_left, gpui::ScaledPixels(0.0));
            assert_eq!(painted.corner_radii.bottom_left, gpui::ScaledPixels(0.0));
            assert!(painted.corner_radii.top_right > gpui::ScaledPixels(0.0));
            assert!(painted.corner_radii.bottom_right > gpui::ScaledPixels(0.0));
        });
    }
    click("repo-row-0", cx);
    view.update(cx, |s, cx| {
        s.repo_scroll.set_offset(point(px(0.0), px(-500.0)));
        cx.notify();
    });
    cx.run_until_parked();
    let marker = bounds("sidebar-indicator", cx);
    cx.update(|window, _| {
        assert!(
            window
                .painted_quads()
                .iter()
                .all(|quad| quad.bounds != marker.scale(window.scale_factor()))
        )
    });
}

#[gpui::test]
fn resizing_interrupts_animation_and_restores_clamped_user_width(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (view, cx) = shell(cx);
    view.update(cx, |s, cx| {
        s.sidebar_state.expanded_width = 340.0;
        cx.notify();
    });
    cx.update(|_, cx| ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = false));
    click("sidebar-toggle", cx);
    cx.update(|_, cx| {
        cx.background_executor()
            .advance_clock(Duration::from_millis(70))
    });
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let during = bounds("sidebar", cx).size.width;
    // The resize hitband belongs to expanded mode, so leave the collapsing transition through
    // the user's own toggle first, then drag from the presented expanded width.
    click("sidebar-toggle", cx);
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.collapsed));
    let start = drag_start(cx);
    assert!(during < px(340.0));
    cx.simulate_mouse_move(
        start + point(px(30.0), px(0.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    let released = start + point(px(30.0), px(0.0));
    release(released, cx);
    let width = bounds("sidebar", cx).size.width;
    assert!(
        width >= px(SIDEBAR_MIN_WIDTH) && width <= px(SIDEBAR_MAX_WIDTH),
        "released width {width:?} must stay inside the resize limits"
    );
    click("sidebar-toggle", cx);
    click("sidebar-toggle", cx);
    assert_eq!(bounds("sidebar", cx).size.width, width);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Home);
}

#[gpui::test]
fn wheel_overlay_is_painted_over_the_repository_viewport(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    let region = bounds("repo-scroll-region", cx);
    let overlay = bounds("wheel-overlay-repo", cx);
    assert_eq!(overlay.origin, region.origin);
    assert_eq!(overlay.size, region.size);
}

#[gpui::test]
fn footer_divider_separates_the_repository_viewport_from_the_account(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let divider = bounds("sidebar-footer-divider", cx);
    let repo = bounds("repo-scroll", cx);
    let account = bounds("account-row", cx);
    assert!(
        divider.top() >= repo.bottom() && divider.bottom() <= account.top(),
        "divider must sit between the viewport and the footer"
    );
    // The divider and footer are fixed: scrolling the repository must not move them.
    view.update(cx, |s, cx| {
        s.repo_scroll.set_offset(point(px(0.0), px(-500.0)));
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(bounds("sidebar-footer-divider", cx), divider);
    assert_eq!(bounds("account-row", cx), account);
    assert_eq!(bounds("account-row", cx).bottom(), account.bottom());
}

#[gpui::test]
fn account_identity_opens_profile_while_status_and_blank_do_not(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    // Expanded: the separate status button exists and never navigates.
    let status = bounds("account-status", cx);
    cx.simulate_click(status.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Home);
    // The compact identity cluster opens the profile.
    click("account-identity", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Profile);
    // The blank space between the identity and the status is not a Profile hit area.
    cx.update_window_entity(&view, |s, window, cx| {
        s.navigate(Route::Home, window, cx);
    });
    cx.run_until_parked();
    let identity = bounds("account-identity", cx);
    let status = bounds("account-status", cx);
    assert!(
        identity.right() < status.left(),
        "the fixture leaves a blank gap to test"
    );
    let blank_x = px((f32::from(identity.right()) + f32::from(status.left())) / 2.0);
    cx.simulate_click(point(blank_x, identity.center().y), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |s, _| s.route()),
        Route::Home,
        "the blank right side must not open the profile"
    );
    // Collapsed: the status button is hidden, but the identity still opens the profile.
    click("sidebar-toggle", cx);
    assert!(cx.debug_bounds("account-status").is_none());
    click("account-identity", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Profile);
}

#[gpui::test]
fn one_wheel_notch_is_consumed_once_and_settles_at_that_delta(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let scroll = bounds("repo-scroll", cx);
    let line = cx.update(|window, _| window.line_height());
    cx.simulate_mouse_move(scroll.center(), None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: scroll.center(),
        delta: ScrollDelta::Lines(point(0.0, -6.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: ScrollSource::Pointer,
    });
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(40));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
    let offset = view.read_with(cx, |s, _| s.repo_offset());
    let expected = px(-6.0 * f32::from(line));
    assert!(
        (offset.y - expected).abs() <= px(1.0),
        "the built-in jump must not double the delta: offset={offset:?} expected={expected:?}"
    );
}

/// A touch pan must apply directly. GPUI already synthesizes the finger's momentum, so easing a
/// `Touch` delta again would add a second inertia on top of the platform's.
#[gpui::test]
fn touch_pan_applies_directly_without_a_second_inertia(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.update(|_, cx| ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = false));
    let scroll = bounds("repo-scroll", cx);
    cx.simulate_mouse_move(scroll.center(), None, Modifiers::none());
    let before = view.read_with(cx, |s, _| s.repo_offset());
    cx.simulate_event(ScrollEvent {
        position: scroll.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: ScrollSource::Touch,
    });
    cx.run_until_parked();
    // No settle loop: a direct application is already at the target on the next frame.
    let after = view.read_with(cx, |s, _| s.repo_offset());
    assert!(
        (after.y - (before.y - px(80.0))).abs() <= px(1.0),
        "touch pan must apply at once: before={before:?} after={after:?}"
    );
}

#[gpui::test]
fn wheel_over_the_palette_scrolls_the_palette_not_the_page(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    // A short window makes the Home page scrollable behind the palette.
    cx.simulate_resize(size(px(900.0), px(500.0)));
    cx.run_until_parked();
    assert!(
        f32::from(view.read_with(cx, |s, _| s.page_max_offset().y)) > 0.0,
        "the Home page must be scrollable for this regression"
    );
    click("search-entry", cx);
    let list = bounds("palette-list", cx);
    let page_before = view.read_with(cx, |s, _| s.page_offset());
    cx.simulate_mouse_move(list.center(), None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: list.center(),
        delta: ScrollDelta::Lines(point(0.0, -8.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: ScrollSource::Pointer,
    });
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(40));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
    assert_eq!(
        view.read_with(cx, |s, _| s.page_offset()),
        page_before,
        "the page must not scroll under the open palette"
    );
    assert!(
        view.read_with(cx, |s, _| s.palette_scroll_offset()).y < px(0.0),
        "the palette itself must scroll"
    );
}

#[gpui::test]
fn footer_divider_is_horizontally_centred_and_has_equal_vertical_space(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let check = |cx: &mut VisualTestContext| {
        let repo = bounds("repo-scroll", cx);
        let divider = bounds("sidebar-footer-divider", cx);
        let rule = bounds("sidebar-footer-divider-rule", cx);
        let account = bounds("account-row", cx);
        let island = bounds("island", cx);
        // The rule is 1px tall at the top spacer's end; measure from the actual rule line, not
        // the container's centre. This part is unchanged by the horizontal fix.
        let rule_top = divider.top() + px(FOOTER_DIVIDER_SPACE);
        let rule_bottom = rule_top + px(FOOTER_DIVIDER_HEIGHT);
        let above = rule_top - repo.bottom();
        let below = account.top() - rule_bottom;
        assert!(
            (above - below).abs() <= px(1.0),
            "rule must sit visually between list and identity: above={above:?} below={below:?}"
        );
        // Horizontal centring is measured against the *visible* sidebar area: the column plus the
        // island gap, which the layout deliberately treats as the sidebar's right margin so rows
        // have 8px on both sides. Centring inside the column alone leaves the line ~4px left of
        // that area's midpoint.
        let visible_left = bounds("sidebar", cx).left();
        let visible_right = island.left();
        let visible_mid = (visible_left + visible_right) / 2.0;
        assert!(
            (rule.center().x - visible_mid).abs() <= px(0.5),
            "rule centre {:?} must match the visible sidebar midpoint {:?}",
            rule.center().x,
            visible_mid
        );
        // Same lane as the row backgrounds: inset by the left padding and stopping one island gap
        // short of the island, so both visible margins are equal.
        assert_eq!(rule.left(), visible_left + px(SIDEBAR_PADDING));
        assert_eq!(rule.right(), visible_right - px(ISLAND_GAP));
        assert_eq!(divider.left(), px(0.0));
        assert_eq!(divider.size.width, bounds("sidebar", cx).size.width);
    };
    check(cx);
    click("sidebar-toggle", cx);
    cx.run_until_parked();
    check(cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Home);
}

#[gpui::test]
fn identity_has_no_block_hover_and_only_the_login_underlines(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    let identity = bounds("account-identity", cx);
    cx.simulate_mouse_move(identity.center(), None, Modifiers::none());
    cx.run_until_parked();
    // Hovering the identity cluster must not paint a whole-row/block background: the login line is
    // the affordance. The row itself stays inert.
    let login = bounds("account-login", cx);
    assert!(login.size.height > px(0.0));
}

#[gpui::test]
fn account_identity_sits_left_and_is_vertically_centred_in_its_row(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    let row = bounds("account-row", cx);
    let identity = bounds("account-identity", cx);
    let avatar = bounds("sidebar-account-avatar", cx);
    let name = bounds("account-name", cx);
    let login = bounds("account-login", cx);
    // "A little to the left": the cluster starts just inside the row's own left padding rather
    // than being pushed right by a large inset.
    assert!(
        identity.left() - row.left() <= px(4.0),
        "identity starts {:?} inside the row",
        identity.left() - row.left()
    );
    // The avatar plus the two text lines form one block that is vertically centred in the row,
    // measured from real layout rather than assumed from an items-center declaration.
    let text_top = name.top();
    let text_bottom = login.bottom();
    let block_top = avatar.top().min(text_top);
    let block_bottom = avatar.bottom().max(text_bottom);
    let above = block_top - row.top();
    let below = row.bottom() - block_bottom;
    assert!(
        (above - below).abs() <= px(1.0),
        "identity block must be centred: above={above:?} below={below:?}"
    );
}

#[test]
fn sash_fade_is_reversible_and_settles_at_its_target() {
    use std::time::{Duration, Instant};
    let mut state = SidebarState::default();
    let t0 = Instant::now();
    assert_eq!(state.sash(), 0.0);
    state.set_sash_hover(true, t0, false);
    assert!(state.sample_sash(t0 + Duration::from_millis(70), false));
    let mid_in = state.sash();
    assert!(mid_in > 0.0 && mid_in < 1.0, "fading in: {mid_in}");
    state.set_sash_hover(false, t0 + Duration::from_millis(70), false);
    state.sample_sash(t0 + Duration::from_millis(110), false);
    let mid_out = state.sash();
    assert!(
        mid_out < mid_in,
        "reverse must fade out from the presented value"
    );
    assert!(!state.sample_sash(t0 + Duration::from_millis(400), false));
    assert_eq!(state.sash(), 0.0);
    // Reduced motion jumps straight to the target and stops requesting frames.
    state.set_sash_hover(true, t0, true);
    assert_eq!(state.sash(), 1.0);
    assert!(!state.sample_sash(t0, true));
}

#[gpui::test]
fn sash_grows_right_to_the_island_and_keeps_its_anchor_and_height(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let check = |cx: &mut VisualTestContext| {
        let band = bounds("sidebar-resize", cx);
        let island = bounds("island", cx);
        let sash = bounds("sidebar-sash", cx);
        // The left edge keeps the anchor the old 2px line had; only the right edge grows.
        assert_eq!(sash.left(), band.left() + px(SIDEBAR_SASH_LEFT));
        assert_eq!(sash.right(), island.left(), "right edge hugs the island");
        assert_eq!(sash.size.width, px(SIDEBAR_SASH_WIDTH));
        assert_eq!(sash.top(), island.top());
        assert_eq!(sash.bottom(), island.bottom());
        assert!(sash.left() < island.left());
        // The toggle is drawn above the line and owns its own hit target, so the line may pass
        // behind it; it must not turn the button into a wider band.
        let toggle = bounds("sidebar-toggle", cx);
        assert_eq!(
            toggle.size,
            size(px(SIDEBAR_TOGGLE_SIZE), px(SIDEBAR_TOGGLE_SIZE))
        );
        assert!(toggle.left() < sash.right() && toggle.top() > sash.top());
    };
    check(cx);
    drag_start(cx);
    let start = bounds("sidebar-resize", cx).center();
    cx.simulate_mouse_move(
        start + point(px(24.0), px(0.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    check(cx);
    release(start + point(px(24.0), px(0.0)), cx);
    click("sidebar-toggle", cx);
    cx.run_until_parked();
    assert!(view.read_with(cx, |s, _| s.sidebar_state.collapsed));
    check(cx);
}

#[gpui::test]
fn sash_hides_without_hover_and_never_covers_the_toggle(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let band = bounds("sidebar-resize", cx);
    let toggle = bounds("sidebar-toggle", cx);
    let island = bounds("island", cx);
    let sash = bounds("sidebar-sash", cx);
    // No hover: the line has no opacity at all, so the collapsed/initial state leaves nothing lit.
    assert_eq!(view.read_with(cx, |s, _| s.sidebar_state.sash()), 0.0);
    // The visible line stays narrower than the heat band, so the original resize hit width and the
    // toggle's own press target are untouched.
    assert!(sash.size.width < band.size.width);
    assert!(sash.size.width <= px(SIDEBAR_SASH_WIDTH));
    // The toggle is occluding and sits on the band, so a press there can never start a resize or
    // reach the line behind it.
    assert!(
        toggle.left() <= band.right() && toggle.right() >= band.left(),
        "toggle overlaps the band by design"
    );
    // The line matches the island's outer frame at both ends (see the dedicated span test); here
    // only confirm it is not reaching into the title bar above or the window's bottom inset.
    assert!(sash.top() >= island.top());
    assert!((sash.bottom() - island.bottom()).abs() <= px(0.5));
}

/// Entering the resize band and then moving to the sidebar or the island must fade the sash out on
/// the real event path, in both layouts and without pressing the toggle. `on_mouse_exit` is only a
/// window exit, so this is the regression that kept the line lit after the pointer had left.
#[gpui::test]
fn leaving_the_resize_band_fades_the_sash_out_in_both_modes(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let check = |cx: &mut VisualTestContext| {
        let band = bounds("sidebar-resize", cx);
        let toggle = bounds("sidebar-toggle", cx);
        let island = bounds("island", cx);
        let sidebar = bounds("sidebar", cx);
        // Stay clear of the toggle, which occludes the band's middle.
        let inside = point(band.center().x, toggle.bottom() + px(40.0));
        cx.simulate_mouse_move(inside, None, Modifiers::none());
        cx.run_until_parked();
        assert!(
            view.read_with(cx, |s, _| s.sidebar_state.sash()) > 0.0,
            "entering the band must reveal the sash"
        );

        // Move to the island: still inside the window, outside the element. This is the case that
        // a window-only mouse-exit listener never reports.
        cx.simulate_mouse_move(island.center(), None, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |s, _| s.sidebar_state.sash()),
            0.0,
            "the sash must fade out after the pointer leaves the band"
        );

        // The same holds when leaving into the sidebar body.
        cx.simulate_mouse_move(inside, None, Modifiers::none());
        cx.run_until_parked();
        cx.simulate_mouse_move(
            point(sidebar.center().x, island.center().y),
            None,
            Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |s, _| s.sidebar_state.sash()), 0.0);
    };

    check(cx);
    click("sidebar-toggle", cx);
    cx.run_until_parked();
    check(cx);
    // Leaving the band must not have navigated or changed the mode by itself.
    assert!(view.read_with(cx, |s, _| s.sidebar_state.collapsed));
}

/// The visible line spans the island's full frame, not an inset straight segment: top and bottom
/// must match the island's own bounds in both layouts and at two window heights.
#[gpui::test]
fn sash_spans_the_island_frame_in_both_modes_and_window_heights(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    let assert_span = |cx: &mut VisualTestContext| {
        let island = bounds("island", cx);
        let sash = bounds("sidebar-sash", cx);
        assert_eq!(sash.size.width, px(SIDEBAR_SASH_WIDTH));
        assert!(
            (sash.top() - island.top()).abs() <= px(0.5),
            "sash top {:?} must match island top {:?}",
            sash.top(),
            island.top()
        );
        assert!(
            (sash.bottom() - island.bottom()).abs() <= px(0.5),
            "sash bottom {:?} must match island bottom {:?}",
            sash.bottom(),
            island.bottom()
        );
    };

    assert_span(cx);
    for height in [560.0, 900.0] {
        cx.simulate_resize(size(px(1200.0), px(height)));
        cx.run_until_parked();
        assert_span(cx);
    }
    click("sidebar-toggle", cx);
    cx.run_until_parked();
    assert_span(cx);
    cx.simulate_resize(size(px(1200.0), px(560.0)));
    cx.run_until_parked();
    assert_span(cx);
}

/// The collapsed More button's ellipsis must be centred in its existing square slot, not stuck in
/// a corner because the slot itself has no layout for its child.
#[gpui::test]
fn collapsed_more_icon_is_centred_in_its_unchanged_slot(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    click("sidebar-toggle", cx);
    let button = bounds("sidebar-more-button", cx);
    let icon = bounds("more-icon", cx);
    assert_eq!(
        button.size,
        size(px(SIDEBAR_RAIL_ITEM_SIZE), px(SIDEBAR_RAIL_ITEM_SIZE))
    );
    assert!(
        (icon.center().x - button.center().x).abs() <= px(0.5),
        "icon centre {:?} must match the slot centre {:?}",
        icon.center().x,
        button.center().x
    );
    assert!(
        (icon.center().y - button.center().y).abs() <= px(0.5),
        "icon centre {:?} must match the slot centre {:?}",
        icon.center().y,
        button.center().y
    );
    // The slot keeps its place directly under the account row inside the same rail lane.
    let account = bounds("account-row", cx);
    assert!(button.top() > account.top());
    assert_eq!(button.left(), account.left());
}

#[gpui::test]
fn menu_contains_tab_and_closes_for_other_overlays_or_a_mode_change(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let more = view.read_with(cx, |s, _| s.sidebar_more_focus.clone());
    click("sidebar-toggle", cx);
    click("sidebar-more", cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_menu_open()));

    // Tab must not move focus out of the open menu and leave it modal but keyboard-dead.
    let menu_focus = view.read_with(cx, |s, _| s.sidebar_menu_focus.clone());
    press("tab", cx);
    let focused = cx.update(|window, cx| window.focused(cx));
    assert!(
        focused.as_ref() == Some(&menu_focus) || focused.as_ref() == Some(&more),
        "focus stays inside the open menu"
    );
    assert!(view.read_with(cx, |s, _| s.sidebar_menu_open()));

    // Ctrl+K opens the palette and must not stack an overlay on the still-visible menu.
    press("ctrl-k", cx);
    assert!(view.read_with(cx, |s, _| s.palette_open()));
    assert!(!view.read_with(cx, |s, _| s.sidebar_menu_open()));
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    press("escape", cx);

    // A mode change closes it too.
    click("sidebar-more", cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_menu_open()));
    click("sidebar-toggle", cx);
    assert!(!view.read_with(cx, |s, _| s.sidebar_menu_open()));
    assert!(cx.debug_bounds("menu-panel-0").is_none());
}

#[gpui::test]
fn escape_returns_focus_to_the_more_button_and_enter_reopens(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let more = view.read_with(cx, |s, _| s.sidebar_more_focus.clone());
    click("sidebar-toggle", cx);
    click("sidebar-more", cx);
    press("escape", cx);
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    cx.update(|window, cx| {
        assert_eq!(window.focused(cx).as_ref(), Some(&more));
    });
    // Enter on the restored button opens the menu again, so keyboard users are not stranded.
    press("enter", cx);
    assert!(
        view.read_with(cx, |s, _| s.sidebar_menu_open()),
        "Enter must reopen the menu from the restored button"
    );
}

#[test]
fn expanded_drag_docks_then_collapses_with_animation() {
    let start = Instant::now();
    let mut state = SidebarState::default();
    state.begin_resize(SIDEBAR_WIDTH + 4.0, SIDEBAR_WIDTH);
    // Continuous shrink keeps following the pointer.
    state.resize_pointer_at(SIDEBAR_WIDTH - 20.0, start, 1200.0, false);
    assert!((SIDEBAR_MIN_WIDTH..SIDEBAR_MAX_WIDTH).contains(&state.width(1200.0)));
    assert_eq!(state.frame.width, state.width(1200.0));
    // Dock: at the minimum the rail must not appear yet.
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH + 4.0, start, 1200.0, false);
    assert_eq!(state.width(1200.0), SIDEBAR_MIN_WIDTH);
    assert!(!state.collapsed);
    // Overshoot past the threshold flips the mode, and presentation eases rather than jumping.
    let collapse_at = SIDEBAR_MIN_WIDTH + 4.0 - SIDEBAR_DRAG_COLLAPSE_THRESHOLD - 4.0;
    state.resize_pointer_at(collapse_at, start, 1200.0, false);
    assert!(state.collapsed);
    assert_eq!(state.frame.width, SIDEBAR_MIN_WIDTH, "no instant jump");
    assert!(state.sample(start + Duration::from_millis(100), 1200.0, false));
    let mid = state.frame.width;
    assert!(mid > SIDEBAR_COLLAPSED_WIDTH && mid < SIDEBAR_MIN_WIDTH);
    assert!(state.frame.expansion > 0.0 && state.frame.expansion < 1.0);
    assert!(!state.sample(start + Duration::from_millis(400), 1200.0, false));
    assert_eq!(state.frame.width, SIDEBAR_COLLAPSED_WIDTH);
    // A stationary pointer completes the animation without any further events.
    assert!(!state.sample(start + Duration::from_secs(2), 1200.0, false));
    // Reduced motion finishes the same transition immediately.
    state.begin_resize(SIDEBAR_WIDTH, SIDEBAR_WIDTH);
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH - 400.0, start, 1200.0, true);
    assert!(state.collapsed);
    assert_eq!(state.frame.width, SIDEBAR_COLLAPSED_WIDTH);
    assert!(!state.sample(start, 1200.0, true));
}

#[test]
fn collapsed_drag_expands_to_the_pointer_position_not_the_remembered_width() {
    let mut state = SidebarState::default();
    state.expanded_width = SIDEBAR_MAX_WIDTH;
    state.collapsed = true;
    state.settle(1200.0);
    state.begin_resize(SIDEBAR_COLLAPSED_WIDTH + 4.0, SIDEBAR_COLLAPSED_WIDTH);
    // Before the threshold the rail stays put.
    state.resize_pointer_at(
        SIDEBAR_COLLAPSED_WIDTH + 4.0 + 8.0,
        Instant::now(),
        1200.0,
        true,
    );
    assert!(state.collapsed);
    assert_eq!(state.frame.width, SIDEBAR_COLLAPSED_WIDTH);
    // Past the threshold: width comes from the pointer, not the remembered 360.
    let pointer = SIDEBAR_MIN_WIDTH + 12.0;
    let target = pointer - 4.0;
    state.resize_pointer_at(pointer, Instant::now(), 1200.0, true);
    assert!(!state.collapsed);
    assert_eq!(state.frame.width, target);
    assert!(target < SIDEBAR_MAX_WIDTH);
    // Same final pointer with a different remembered width yields the same sash.
    let mut other = SidebarState::default();
    other.expanded_width = SIDEBAR_MIN_WIDTH;
    other.collapsed = true;
    other.settle(1200.0);
    other.begin_resize(SIDEBAR_COLLAPSED_WIDTH + 4.0, SIDEBAR_COLLAPSED_WIDTH);
    other.resize_pointer_at(pointer, Instant::now(), 1200.0, true);
    assert_eq!(other.frame.width, state.frame.width);
}

#[test]
fn grab_offset_inside_the_band_shifts_the_width_by_only_that_offset() {
    let mut a = SidebarState::default();
    let mut b = SidebarState::default();
    a.begin_resize(200.0 + 1.0, 200.0);
    b.begin_resize(200.0 + 7.0, 200.0);
    a.resize_pointer_at(240.0, Instant::now(), 1200.0, true);
    b.resize_pointer_at(240.0, Instant::now(), 1200.0, true);
    assert_eq!(a.frame.width, 239.0);
    assert_eq!(b.frame.width, 233.0);
    assert_eq!(a.frame.width - b.frame.width, 6.0);
}

#[gpui::test]
fn toggle_has_no_border_after_click_or_keyboard_focus(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let before = bounds("sidebar-toggle", cx);
    let quads_before = cx.update(|window, _| window.painted_quads().len());
    click("sidebar-toggle", cx);
    let after = bounds("sidebar-toggle", cx);
    // Collapsing moves the button with the island edge; only its own size must stay the same.
    assert_eq!(after.size, before.size);
    assert_eq!(after.center().y, before.center().y);
    let focused = cx.update(|window, cx| window.focused(cx));
    let toggle = view.read_with(cx, |s, _| s.sidebar_toggle_focus.clone());
    assert_eq!(focused.as_ref(), Some(&toggle));
    // Focusing the toggle must not add a visible outline. The button keeps a background only.
    let expanded = view.read_with(cx, |s, _| s.sidebar_state.collapsed);
    click("sidebar-toggle", cx);
    assert_ne!(
        expanded,
        view.read_with(cx, |s, _| s.sidebar_state.collapsed)
    );
    let outline = cx.update(|window, _| {
        window
            .painted_quads()
            .into_iter()
            .filter(|quad| {
                let b = quad.bounds;
                let w = b.size.width;
                let h = b.size.height;
                // A 1px ring drawn around the 24px button.
                let (w, h) = (w.as_f32(), h.as_f32());
                (24.0..=26.0).contains(&w) && (24.0..=26.0).contains(&h)
            })
            .count()
    });
    assert_eq!(outline, 0, "no border ring is painted around the toggle");
    let _ = quads_before;
}

#[gpui::test]
fn keyboard_toggle_keeps_focus_and_reverses_immediately(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let toggle = view.read_with(cx, |s, _| s.sidebar_toggle_focus.clone());
    cx.update(|window, cx| window.focus(&toggle, cx));
    cx.run_until_parked();
    press("enter", cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_state.collapsed));
    cx.update(|window, cx| {
        assert_eq!(
            window.focused(cx).as_ref(),
            Some(&toggle),
            "focus stays on the same actionable toggle"
        );
    });
    // A second activation reverses the mode without walking the tab order again.
    press("enter", cx);
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.collapsed));
    press("space", cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_state.collapsed));
}

#[test]
fn a_drag_follows_the_pointer_and_does_not_pin_the_sash_on() {
    use std::time::{Duration, Instant};
    let mut state = SidebarState::default();
    let t0 = Instant::now();
    // A drag begins inside the band, so the line is up...
    state.begin_resize(SIDEBAR_WIDTH, SIDEBAR_WIDTH);
    state.note_drag(t0, true);
    assert_eq!(state.sash(), 1.0);
    assert!(state.sash_hovered());
    // ...and while the gesture continues, moving outside the band must NOT extinguish it: the
    // valid resize drag owns visibility until release.
    state.note_pointer(false, t0 + Duration::from_millis(10), true);
    assert_eq!(state.sash(), 1.0, "an active drag keeps the line visible");
    // Releasing outside the band leaves it off.
    state.end_resize(t0 + Duration::from_millis(20), false, true);
    assert_eq!(state.sash(), 0.0);
    assert!(!state.sash_hovered());
}

/// A valid resize drag keeps the line visible for the whole gesture even when the moving band
/// leaves the pointer outside it; releasing outside then fades it, releasing inside keeps it.
#[gpui::test]
fn a_resize_drag_keeps_the_sash_visible_until_release(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let band = bounds("sidebar-resize", cx);
    let start = point(band.center().x, band.top() + px(120.0));
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: start,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_mouse_move(
        start + point(px(6.0), px(0.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    assert!(view.read_with(cx, |s, _| s.sidebar_state.dragging));
    let far = point(px(700.0), px(320.0));
    cx.simulate_mouse_move(far, Some(MouseButton::Left), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |s, _| s.sidebar_state.sash()),
        1.0,
        "the gesture keeps the line visible outside the band"
    );
    release(far, cx);
    assert_eq!(
        view.read_with(cx, |s, _| s.sidebar_state.sash()),
        0.0,
        "releasing outside hands visibility back to real hover"
    );
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.sash_hovered()));

    let inside = bounds("sidebar-resize", cx).center();
    cx.simulate_mouse_move(inside, None, Modifiers::none());
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: inside,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_mouse_move(
        inside + point(px(4.0), px(0.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    cx.simulate_mouse_move(
        inside + point(px(4.0), px(80.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    release(inside + point(px(4.0), px(80.0)), cx);
    assert_eq!(
        view.read_with(cx, |s, _| s.sidebar_state.sash()),
        0.0,
        "the release point is outside the band, so it fades"
    );
    let island = bounds("island", cx).center();
    cx.simulate_mouse_move(island, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |s, _| s.sidebar_state.sash()), 0.0);
}

#[gpui::test]
fn releasing_the_drag_outside_the_band_clears_the_sash_target(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    // Reduced motion is on by default in these tests, so the sash lands on its target at once.
    let band = bounds("sidebar-resize", cx);
    let start = point(band.left() + px(2.0), band.top() + px(120.0));
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.run_until_parked();
    assert!(view.read_with(cx, |s, _| s.sidebar_state.sash_hovered()));
    drag_start(cx);
    let far = point(px(900.0), px(200.0));
    cx.simulate_mouse_move(far, Some(MouseButton::Left), Modifiers::none());
    cx.run_until_parked();
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: far,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.dragging));
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.sash_hovered()));
    assert_eq!(view.read_with(cx, |s, _| s.sidebar_state.sash()), 0.0);
}

#[gpui::test]
fn repository_bar_drags_real_content_and_cancels_pending_wheel(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let region = bounds("repo-scroll", cx);
    let footer = bounds("account-row", cx);
    let tabs_before = view.read_with(cx, |s, _| s.tab_scroll.offset());
    let page_before = view.read_with(cx, |s, _| s.page_offset());
    assert!(cx.debug_bounds("repo-scrollbar").is_none());
    cx.simulate_mouse_move(region.center(), None, Modifiers::none());
    cx.run_until_parked();
    let line = bounds("repo-scrollbar-line", cx);
    let thumb = bounds("repo-scrollbar-thumb", cx);
    assert_eq!(line.size.width, px(super::scroll::THICKNESS));
    assert!(thumb.top() >= region.top() && thumb.bottom() <= region.bottom());

    // Leave an actual discrete wheel target pending, then hand control to the thumb immediately.
    view.update(cx, |s, _| {
        assert!(s.repo_smooth.wheel(
            std::time::Instant::now(),
            -2000.0,
            gpui::Axis::Vertical,
            false,
            false
        ));
    });
    cx.simulate_mouse_down(thumb.center(), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        thumb.center() + point(px(0.0), px(8.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |s, _| s.repo_bars.dragging()),
        Some(gpui::Axis::Vertical)
    );
    assert!(
        !view.read_with(cx, |s, _| s
            .repo_smooth
            .tick(std::time::Instant::now(), false)),
        "the thumb must cancel the wheel target at drag start"
    );
    cx.simulate_mouse_move(
        thumb.center() + point(px(0.0), px(150.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    let dragged = view.read_with(cx, |s, _| s.repo_offset());
    assert!(
        dragged.y < px(-100.0),
        "thumb moves the real rows: {dragged:?}"
    );
    assert!(bounds("repo-row-0", cx).top() < region.top());
    let outside = bounds("island", cx).center();
    release(outside, cx);
    cx.simulate_mouse_move(outside, None, Modifiers::none());
    cx.run_until_parked();
    assert!(view.read_with(cx, |s, _| s.repo_bars.dragging().is_none()));
    view.update(cx, |s, _| {
        assert!(!s.repo_smooth.tick(
            std::time::Instant::now() + std::time::Duration::from_secs(1),
            false
        ));
    });
    assert_eq!(
        view.read_with(cx, |s, _| s.repo_offset()),
        dragged,
        "release must not spring back"
    );
    assert!(cx.debug_bounds("repo-scrollbar").is_none());
    assert_eq!(bounds("account-row", cx), footer);
    assert_eq!(
        view.read_with(cx, |s, _| s.tab_scroll.offset()),
        tabs_before
    );
    assert_eq!(view.read_with(cx, |s, _| s.page_offset()), page_before);
}

#[gpui::test]
fn repository_bar_hover_fades_reversibly_and_tracks_collapse_and_resize(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    view.update(cx, |s, _| s.pin_tab_clock(0));
    cx.update(|_, cx| ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = false));
    let enter = bounds("repo-scroll", cx).center();
    cx.simulate_mouse_move(enter, None, Modifiers::none());
    cx.run_until_parked();
    view.update(cx, |s, cx| {
        s.pin_tab_clock(70);
        cx.notify();
    });
    cx.run_until_parked();
    let mid = view.update(cx, |s, _| s.repo_bars.opacity(70));
    assert!(mid > 0.0 && mid < 1.0);
    let away = bounds("island", cx).center();
    cx.simulate_mouse_move(away, None, Modifiers::none());
    view.update(cx, |s, cx| {
        s.pin_tab_clock(105);
        cx.notify();
    });
    cx.run_until_parked();
    let out = view.update(cx, |s, _| s.repo_bars.opacity(105));
    assert!(out < mid);
    cx.simulate_mouse_move(enter, None, Modifiers::none());
    assert_eq!(
        view.update(cx, |s, _| s.repo_bars.opacity(105)),
        out,
        "reversal is continuous"
    );
    view.update(cx, |s, cx| {
        s.pin_tab_clock(250);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(view.update(cx, |s, _| s.repo_bars.opacity(250)), 1.0);
    assert!(!view.read_with(cx, |s, _| s.repo_bars.is_animating()));

    cx.update(|_, cx| ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = true));
    click("sidebar-toggle", cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    let region = bounds("repo-scroll", cx);
    cx.simulate_mouse_move(region.center(), None, Modifiers::none());
    cx.run_until_parked();
    let track = bounds("repo-scrollbar", cx);
    assert!(track.top() >= region.top() && track.bottom() <= region.bottom());
    assert!(track.right() <= region.right());
    let away = bounds("account-row", cx).center();
    cx.simulate_mouse_move(away, None, Modifiers::none());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("repo-scrollbar").is_none(),
        "footer is outside the hover region"
    );
}

#[gpui::test]
fn repository_thumb_cleanup_is_local_on_escape_resize_and_window_blur(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let start_thumb = |cx: &mut VisualTestContext| {
        let region = bounds("repo-scroll", cx);
        cx.simulate_mouse_move(region.center(), None, Modifiers::none());
        cx.run_until_parked();
        let thumb = bounds("repo-scrollbar-thumb", cx).center();
        cx.simulate_mouse_down(thumb, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            thumb + point(px(0.0), px(8.0)),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |s, _| s.repo_bars.dragging()),
            Some(gpui::Axis::Vertical)
        );
        assert!(view.read_with(cx, |s, _| s.tab_bars.dragging().is_none()));
        assert!(!view.read_with(cx, |s, _| s.sidebar_state.dragging));
    };
    start_thumb(cx);
    press("escape", cx);
    assert!(view.read_with(cx, |s, _| s.repo_bars.dragging().is_none()));
    release(bounds("island", cx).center(), cx);
    start_thumb(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    assert!(view.read_with(cx, |s, _| s.repo_bars.dragging().is_none()));
    release(bounds("island", cx).center(), cx);
    start_thumb(cx);
    cx.update(|window, _| window.activate_window());
    cx.deactivate_window();
    cx.run_until_parked();
    assert!(view.read_with(cx, |s, _| s.repo_bars.dragging().is_none()));
    assert_eq!(
        view.update(cx, |s, _| s.repo_bars.opacity(s.tab_now_ms())),
        0.0
    );
    assert!(!view.read_with(cx, |s, _| s.repo_bars.is_animating()));
}

/// Continued pointer movement during the mode-switch animation must bend the same curve rather
/// than restart it or snap back to the old width.
#[test]
fn moving_pointer_during_the_switch_keeps_the_animation_continuous() {
    let start = Instant::now();
    let mut state = SidebarState::default();
    state.begin_resize(SIDEBAR_WIDTH + 4.0, SIDEBAR_WIDTH);
    let collapse_at = SIDEBAR_MIN_WIDTH + 4.0 - SIDEBAR_DRAG_COLLAPSE_THRESHOLD - 4.0;
    state.resize_pointer_at(collapse_at, start, 1200.0, false);
    assert!(state.collapsed);
    assert!(state.sample(start + Duration::from_millis(80), 1200.0, false));
    let mid = state.frame.width;
    assert!(mid > SIDEBAR_COLLAPSED_WIDTH && mid < SIDEBAR_MIN_WIDTH);
    // Moving back out of the overshoot zone during the transition keeps the expanded mode and
    // eases from the presented frame instead of jumping back to the old width.
    state.resize_pointer_at(
        SIDEBAR_MIN_WIDTH + 4.0,
        start + Duration::from_millis(80),
        1200.0,
        false,
    );
    assert!(!state.collapsed);
    assert!(
        state.frame.width >= mid,
        "reversal continues from the presented frame"
    );
    assert!(state.sample(start + Duration::from_millis(140), 1200.0, false));
    assert!(!state.sample(start + Duration::from_millis(500), 1200.0, false));
    // A stationary pointer then completes normally and leaves no transition behind.
    assert!(!state.sample(start + Duration::from_secs(3), 1200.0, false));
    assert_eq!(state.frame.width, SIDEBAR_MIN_WIDTH);
}

/// One gesture expanding out of the rail can collapse again only by crossing the collapse
/// overshoot, and release stops the gesture without leaving a stale mode.
#[gpui::test]
fn repeated_mode_switches_in_one_gesture_do_not_flap(cx: &mut TestAppContext) {
    let start = Instant::now();
    let mut state = SidebarState::default();
    state.collapsed = true;
    state.settle(1200.0);
    state.begin_resize(SIDEBAR_COLLAPSED_WIDTH + 4.0, SIDEBAR_COLLAPSED_WIDTH);
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH + 10.0, start, 1200.0, true);
    assert!(!state.collapsed);
    let expanded = state.frame.width;
    // Moving left to exactly the minimum docks without collapsing.
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH + 4.0, start, 1200.0, true);
    assert!(!state.collapsed);
    assert_eq!(state.frame.width, SIDEBAR_MIN_WIDTH);
    // Reversing slightly does not create a collapse.
    state.resize_pointer_at(SIDEBAR_MIN_WIDTH + 30.0, start, 1200.0, true);
    assert!(!state.collapsed);
    assert!(state.frame.width > expanded - 1.0 || state.frame.width >= SIDEBAR_MIN_WIDTH);
    state.end_resize(start, false, true);
    assert!(!state.dragging);
    assert!(!state.collapsed);
    let _ = cx;
}

/// The real GPT event path: dragging out of the rail must place the sash where the pointer is,
/// independent of the previously remembered expanded width.
#[gpui::test]
fn expanding_from_the_rail_binds_the_sash_to_the_pointer(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    view.update(cx, |s, cx| {
        s.sidebar_state.expanded_width = SIDEBAR_MAX_WIDTH;
        cx.notify();
    });
    cx.run_until_parked();
    click("sidebar-toggle", cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_state.collapsed));
    let band = bounds("sidebar-resize", cx);
    let anchor = point(band.left() + px(3.0), band.top() + px(60.0));
    cx.simulate_mouse_move(anchor, None, Modifiers::none());
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: anchor,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    for step in [4.0, 60.0, 140.0] {
        cx.simulate_mouse_move(
            anchor + point(px(step), px(0.0)),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    // The gesture may not have crossed the drag threshold in this synthetic sequence; drive one
    // more small move so GPUI's drag is definitely active before asserting the mode.
    cx.simulate_mouse_move(
        anchor + point(px(120.0), px(0.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    assert!(!view.read_with(cx, |s, _| s.sidebar_state.collapsed));
    // Advance the switch animation the way the frame loop does.
    for _ in 0..8 {
        view.update(cx, |s, cx| {
            let now = cx.background_executor().now();
            s.sidebar_state
                .sample(now + std::time::Duration::from_millis(60), 1200.0, true);
            cx.notify();
        });
        cx.run_until_parked();
    }
    let island = bounds("island", cx);
    let sash = bounds("sidebar-sash", cx);
    let pointer = anchor + point(px(140.0), px(0.0));
    // A few pixels of real grab offset are allowed; a remembered-width jump is not.
    assert!(
        (sash.right() - pointer.x).abs() <= px(12.0),
        "sash right {:?} must track pointer {:?}",
        sash.right(),
        pointer.x
    );
    assert_eq!(sash.right(), island.left());
    release(pointer, cx);
}

/// The old bug: from the rail, a pointer just past the old 12px expand threshold produced an
/// expanded-but-below-minimum state whose next event immediately collapsed again, so the footer
/// flipped between More and the four entries. Repeated identical or 1px-jittering positions must
/// now be idempotent, and the footer must stay compact throughout the critical band.
#[test]
fn critical_band_is_idempotent_and_keeps_the_footer_compact() {
    let start = Instant::now();
    let mut state = SidebarState::default();
    state.expanded_width = SIDEBAR_MAX_WIDTH;
    state.collapsed = true;
    state.settle(1200.0);
    let press = SIDEBAR_COLLAPSED_WIDTH + 4.0;
    state.begin_resize(press, SIDEBAR_COLLAPSED_WIDTH);
    let mut modes = Vec::new();
    let mut compact = Vec::new();
    // Move right in 1px steps from the rail, across the whole band up to a clearly wide sidebar,
    // repeating each position twice to make a flapping mode visible.
    let stop = (SIDEBAR_MIN_WIDTH + 80.0 - press).ceil() as i32;
    for step in 1..=stop {
        for _ in 0..2 {
            state.resize_pointer_at(press + step as f32, start, 1200.0, true);
            modes.push(state.collapsed);
            compact.push(state.footer_compact());
        }
    }
    // Once out of the rail the mode never flips back while the pointer keeps moving right.
    let first_expanded = modes.iter().position(|collapsed| !*collapsed).unwrap();
    assert!(modes[first_expanded..].iter().all(|collapsed| !*collapsed));
    // The footer changes presentation exactly once on the way out, with no alternation.
    let switches = compact.windows(2).filter(|pair| pair[0] != pair[1]).count();
    assert_eq!(switches, 1, "footer sequence: {compact:?}");
    assert!(*compact.first().unwrap(), "starts compact on the rail");
    assert!(
        !state.footer_compact(),
        "ends on the four entries once clearly wide"
    );
}

/// Same from the expanded side: shrinking alone must keep one footer presentation per band and
/// never alternate, including at the docking plateau.
#[test]
fn shrinking_into_the_critical_band_does_not_alternate_the_footer() {
    let start = Instant::now();
    let mut state = SidebarState::default();
    let press = SIDEBAR_WIDTH + 4.0;
    state.begin_resize(press, SIDEBAR_WIDTH);
    let mut compact = Vec::new();
    for step in (0..=110).rev() {
        state.resize_pointer_at(press - step as f32, start, 1200.0, true);
        compact.push(state.footer_compact());
    }
    // Shrinking yields exactly one switch into the compact presentation, with no alternation.
    let switches = compact.windows(2).filter(|pair| pair[0] != pair[1]).count();
    assert_eq!(switches, 1, "one switch while shrinking, got {compact:?}");
}

/// Collapse/expand transitions themselves are compact, and a release inside the critical band
/// keeps More after all animation frames have finished.
#[test]
fn transition_and_release_in_the_critical_band_stay_compact() {
    let start = Instant::now();
    let mut state = SidebarState::default();
    let press = SIDEBAR_WIDTH + 4.0;
    state.begin_resize(press, SIDEBAR_WIDTH);
    let collapse_at =
        press - (SIDEBAR_WIDTH - (SIDEBAR_MIN_WIDTH - SIDEBAR_DRAG_COLLAPSE_THRESHOLD - 2.0));
    state.resize_pointer_at(collapse_at, start, 1200.0, false);
    assert!(state.collapsed);
    assert!(state.footer_compact(), "compact during the transition");
    assert!(state.sample(start + Duration::from_millis(80), 1200.0, false));
    assert!(state.footer_compact(), "compact on an animation mid-frame");
    assert!(!state.sample(start + Duration::from_millis(400), 1200.0, false));
    assert!(
        state.footer_compact(),
        "compact after the animation finished"
    );
    state.end_resize(start, false, true);
    assert!(state.footer_compact(), "compact after release at the rail");
    assert!(!state.sample(start + Duration::from_secs(2), 1200.0, false));
    assert!(
        state.footer_compact(),
        "staying still must not bring the four entries back"
    );
}

/// Real GPUI events through the critical band: the footer keeps exactly one presentation, and
/// expanded-but-compact still gives a working More menu (open, keyboard, close, focus restored).
#[gpui::test]
fn compact_footer_in_the_critical_band_keeps_more_working(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let band = bounds("sidebar-resize", cx);
    let anchor = point(band.center().x, band.top() + px(60.0));
    cx.simulate_mouse_move(anchor, None, Modifiers::none());
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: anchor,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    // Drag far left to the rail, then walk back right through the critical band, repeating each
    // position so a flapping presentation would be visible.
    for step in [-600.0, -300.0, -120.0] {
        cx.simulate_mouse_move(
            anchor + point(px(step), px(0.0)),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    assert!(view.read_with(cx, |s, _| s.sidebar_state.collapsed));
    assert!(cx.debug_bounds("sidebar-more").is_some());
    assert!(
        cx.debug_bounds("quarter-row").is_none(),
        "only More while on the rail"
    );
    for step in 1..=40 {
        let at = anchor + point(px(-120.0 + step as f32), px(0.0));
        for _ in 0..2 {
            cx.simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::none());
            cx.run_until_parked();
        }
        // Exactly one footer presentation is drawn at every point of the sequence.
        let more = cx.debug_bounds("sidebar-more").is_some();
        let four = cx.debug_bounds("quarter-row").is_some();
        assert!(more ^ four, "both or neither footer drawn at step {step}");
    }
    release(anchor + point(px(-80.0), px(0.0)), cx);
    // Landed in the compact/docking band: More, and it must actually work.
    assert!(cx.debug_bounds("sidebar-more").is_some());
    assert!(cx.debug_bounds("quarter-row").is_none());
    click("sidebar-more", cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_menu_open()));
    press("down", cx);
    press("enter", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Issues);
    assert!(!view.read_with(cx, |s, _| s.sidebar_menu_open()));
    assert!(cx.debug_bounds("menu-panel-0").is_none());
}

/// If the footer leaves the compact presentation while More is still open, the shell must close
/// the now-hidden menu and keep Tab usable instead of leaving an invisible modal behind.
#[gpui::test]
fn leaving_compact_presentation_drops_an_open_more_menu(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("sidebar-toggle", cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_state.collapsed));
    click("sidebar-more", cx);
    assert!(view.read_with(cx, |s, _| s.sidebar_menu_open()));
    // Force the same state an animation-completed expansion would produce: compact no longer.
    view.update(cx, |s, cx| {
        s.sidebar_state.collapsed = false;
        s.sidebar_state.expanded_width = SIDEBAR_MIN_WIDTH + 120.0;
        s.sidebar_state.settle(1200.0);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("quarter-row").is_some(),
        "four entries are shown"
    );
    assert!(
        !view.read_with(cx, |s, _| s.sidebar_menu_open()),
        "the hidden menu is closed, not left open"
    );
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    let before = cx.update(|window, cx| window.focused(cx));
    press("tab", cx);
    let after = cx.update(|window, cx| window.focused(cx));
    assert!(after.is_some());
    assert_ne!(before, after, "Tab moves focus after the menu is gone");
}

// Unlike settled-layout tests, these keep motion enabled and deliver further pointer events
// inside the 200ms transition. A single threshold event alone did not expose the regression.
fn assert_resize_switch_survives_moves(expanding: bool) {
    let start = Instant::now();
    let mut state = SidebarState::default();
    state.collapsed = expanding;
    state.settle(1200.0);
    let initial = state.frame.width;
    state.begin_resize(initial + 4.0, initial);
    let mut pointer = if expanding {
        SIDEBAR_MIN_WIDTH + 4.0
    } else {
        SIDEBAR_MIN_WIDTH - SIDEBAR_DRAG_COLLAPSE_THRESHOLD
    };
    state.resize_pointer_at(pointer, start, 1200.0, false);
    assert_eq!(
        state.frame.width, initial,
        "switch starts at the presented width"
    );
    // Keep moving all the way to the original deadline. Updating the target must neither snap
    // the frame nor postpone completion until the mouse stops.
    for elapsed in (16..SIDEBAR_ANIMATION_MS).step_by(16) {
        let now = start + Duration::from_millis(elapsed);
        assert!(state.sample(now, 1200.0, false));
        let before = state.frame;
        pointer += if expanding { 3.0 } else { -1.0 };
        state.resize_pointer_at(pointer, now, 1200.0, false);
        assert!(
            (state.frame.width - before.width).abs() < 0.001,
            "move must not snap width"
        );
        assert!((state.frame.expansion - before.expansion).abs() < 0.001);
        assert!(state.frame.expansion > 0.0 && state.frame.expansion < 1.0);
        assert!(
            state.footer_compact(),
            "mode transition keeps More even if target is roomy"
        );
    }
    assert!(!state.sample(
        start + Duration::from_millis(SIDEBAR_ANIMATION_MS),
        1200.0,
        false
    ));
    let expected = if expanding {
        pointer - 4.0
    } else {
        SIDEBAR_COLLAPSED_WIDTH
    };
    assert!((state.frame.width - expected).abs() < 0.001);
    assert_eq!(state.frame.expansion, if expanding { 1.0 } else { 0.0 });
    assert_eq!(state.footer_compact(), !expanding);
    assert!(!state.sample(start + Duration::from_secs(1), 1200.0, false));
}

#[test]
fn resize_collapse_survives_continued_pointer_events() {
    assert_resize_switch_survives_moves(false);
}

#[test]
fn resize_expand_survives_continued_pointer_events() {
    assert_resize_switch_survives_moves(true);
}

#[test]
fn resize_reversal_and_release_keep_the_presented_animation() {
    let start = Instant::now();
    let mut state = SidebarState::default();
    state.begin_resize(SIDEBAR_WIDTH + 4.0, SIDEBAR_WIDTH);
    state.resize_pointer_at(100.0, start, 1200.0, false);
    let reverse_at = start + Duration::from_millis(60);
    assert!(state.sample(reverse_at, 1200.0, false));
    let mid = state.frame;
    state.resize_pointer_at(180.0, reverse_at, 1200.0, false);
    assert_eq!(
        state.frame.width, mid.width,
        "reversal starts at the current frame"
    );
    assert_eq!(state.frame.expansion, mid.expansion);
    let next = reverse_at + Duration::from_millis(40);
    assert!(state.sample(next, 1200.0, false));
    let before_move = state.frame;
    state.resize_pointer_at(200.0, next, 1200.0, false);
    assert!((state.frame.width - before_move.width).abs() < 0.001);
    state.end_resize(next, false, false);
    assert!(!state.dragging);
    assert!(state.sample(next + Duration::from_millis(40), 1200.0, false));
    assert!(state.footer_compact());
    assert!(!state.sample(
        reverse_at + Duration::from_millis(SIDEBAR_ANIMATION_MS),
        1200.0,
        false
    ));
    assert_eq!(state.frame.width, 196.0);
    assert_eq!(state.frame.expansion, 1.0);
    assert!(!state.footer_compact());
}

#[gpui::test]
fn resize_switch_animation_survives_real_drag_events_and_layout_frames(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    for expanding in [false, true] {
        view.update(cx, |s, cx| {
            s.sidebar_state.collapsed = expanding;
            s.sidebar_state.expanded_width = SIDEBAR_WIDTH;
            s.sidebar_state.settle(1200.0);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|_, cx| ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = false));
        let press = drag_start(cx);
        let initial = bounds("sidebar", cx).size.width;
        let mut pointer = point(px(if expanding { 200.0 } else { 120.0 }), press.y);
        cx.simulate_mouse_move(pointer, Some(MouseButton::Left), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(bounds("sidebar", cx).size.width, initial);
        assert_eq!(
            view.read_with(cx, |s, _| s.sidebar_state.collapsed),
            !expanding
        );
        for _ in 0..3 {
            cx.update(|_, cx| {
                cx.background_executor()
                    .advance_clock(Duration::from_millis(40))
            });
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let before_move = bounds("sidebar", cx).size.width;
            let expansion = view.read_with(cx, |s, _| s.sidebar_state.frame.expansion);
            assert!(expansion > 0.0 && expansion < 1.0);
            pointer.x += px(if expanding { 6.0 } else { -6.0 });
            cx.simulate_mouse_move(pointer, Some(MouseButton::Left), Modifiers::none());
            cx.run_until_parked();
            assert!((bounds("sidebar", cx).size.width - before_move).abs() < px(0.1));
            assert_eq!(bounds("island", cx).left(), before_move + px(ISLAND_GAP));
            assert_eq!(bounds("sidebar-resize", cx).left(), before_move);
            assert!(cx.debug_bounds("sidebar-more").is_some());
            assert!(cx.debug_bounds("quarter-row").is_none());
        }
        cx.update(|_, cx| {
            cx.background_executor()
                .advance_clock(Duration::from_millis(80))
        });
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let width = bounds("sidebar", cx).size.width;
        if expanding {
            assert_eq!(width, pointer.x - px(SIDEBAR_RESIZE_HIT_WIDTH / 2.0));
            assert!(bounds("sidebar-resize", cx).contains(&pointer));
            assert!(cx.debug_bounds("quarter-row").is_some());
        } else {
            assert_eq!(width, px(SIDEBAR_COLLAPSED_WIDTH));
        }
        release(pointer, cx);
    }
}
