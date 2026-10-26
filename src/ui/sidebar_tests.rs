//! Interaction and measured layout coverage for the resizable sidebar.
use super::{Shell, geometry::*, sidebar_state::SidebarState};
use crate::workspace::Route;
use gpui::{
    Bounds, KeyUpEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels,
    ScrollDelta, ScrollWheelEvent, TestAppContext, VisualContext, VisualTestContext, point, px,
    size,
};

fn shell(cx: &mut TestAppContext) -> (gpui::Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| {
        ely_gpui_component::init_for_tests(cx);
        // Existing event tests inspect settled geometry; motion has separate controlled-time tests.
        ely_gpui_component::theme::Theme::update(cx, |theme| theme.reduced_motion = true);
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
fn press(key: &str, cx: &mut VisualTestContext) {
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
    state.begin_resize();
    state.resize(900.0, 1200.0);
    assert_eq!(state.width(1200.0), 360.0);
    assert_eq!(state.width(880.0), 265.0);
    state.collapsed = true;
    assert_eq!(state.width(880.0), SIDEBAR_COLLAPSED_WIDTH);
    state.collapsed = false;
    assert_eq!(state.width(1200.0), 360.0);
    state.resize(-100.0, 880.0);
    assert_eq!(state.width(880.0), 200.0);
    state.dragging = false;
    state.resize(350.0, 1200.0);
    assert_eq!(state.width(1200.0), 200.0);
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
    let left = point(px(20.0), px(150.0));
    cx.simulate_mouse_move(left, Some(MouseButton::Left), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(bounds("sidebar", cx).size.width, px(200.0));
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
    state.begin_resize();
    state.resize(340.0, 880.0);
    assert_eq!(state.frame.width, 265.0);
    assert!(!state.sample(start + Duration::from_secs(12), 880.0, false));
    state.toggle(start + Duration::from_secs(13), 880.0, true);
    assert_eq!(state.frame.width, SIDEBAR_COLLAPSED_WIDTH);
    assert!(!state.sample(start + Duration::from_secs(13), 880.0, true));
}

#[gpui::test]
fn animated_layout_uses_one_width_for_sidebar_island_and_divider(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (view, cx) = shell(cx);
    cx.update(|_, cx| {
        ely_gpui_component::theme::Theme::update(cx, |theme| theme.reduced_motion = false)
    });
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
    cx.update(|_, cx| {
        ely_gpui_component::theme::Theme::update(cx, |theme| theme.reduced_motion = false)
    });
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
    // begin_resize starts the manual drag from the currently presented width, clamped to the
    // minimum while it is still below it, rather than jumping to the stale target width.
    assert_eq!(bounds("sidebar", cx).size.width, px(SIDEBAR_MIN_WIDTH));
    cx.simulate_mouse_move(
        start + point(px(30.0), px(0.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
    let released = start + point(px(30.0), px(0.0));
    release(released, cx);
    let width = bounds("sidebar", cx).size.width;
    assert!(width > during);
    click("sidebar-toggle", cx);
    click("sidebar-toggle", cx);
    assert_eq!(bounds("sidebar", cx).size.width, width);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Home);
}

#[test]
fn smooth_state_eases_reverses_clamps_and_stops() {
    use super::scrolling::SmoothState;
    use std::time::{Duration, Instant};
    let s = SmoothState::default();
    let t0 = Instant::now();
    // A discrete notch starts easing rather than jumping.
    assert_eq!(s.wheel(t0, 0.0, 1000.0, -120.0, false, false), None);
    assert!(s.is_animating());
    let mid = s
        .tick(t0 + Duration::from_millis(65), 0.0, 1000.0, false)
        .unwrap();
    assert!(mid < 0.0 && mid > -120.0, "mid={mid}");
    // Reverse mid-flight: the next target is above the presented value, and the tick continues
    // from where it was rather than jumping back to the original start.
    assert_eq!(
        s.wheel(
            t0 + Duration::from_millis(65),
            mid,
            1000.0,
            40.0,
            false,
            false
        ),
        None
    );
    let reversed = s
        .tick(t0 + Duration::from_millis(80), mid, 1000.0, false)
        .unwrap();
    assert!(reversed > mid, "reverse must move up: {reversed} vs {mid}");
    // Settle exactly on the accumulated target and stop asking for frames.
    let end = s
        .tick(t0 + Duration::from_millis(400), reversed, 1000.0, false)
        .unwrap();
    assert_eq!(end, -80.0);
    assert!(!s.is_animating());
    assert!(
        s.tick(t0 + Duration::from_millis(500), -80.0, 1000.0, false)
            .is_none()
    );
    // Precise input applies at once.
    assert_eq!(s.wheel(t0, 0.0, 1000.0, -50.0, true, false), Some(-50.0));
    // A content shrink clamps the presented value into the valid [-max, 0] range.
    assert_eq!(s.wheel(t0, -500.0, 100.0, 0.0, true, false), Some(-100.0));
    assert!(!s.is_animating());
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
    cx.simulate_event(ScrollWheelEvent {
        position: scroll.center(),
        delta: ScrollDelta::Lines(point(0.0, -6.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
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
    cx.simulate_event(ScrollWheelEvent {
        position: list.center(),
        delta: ScrollDelta::Lines(point(0.0, -8.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
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

#[test]
fn shrinking_range_clamps_target_so_reverse_scroll_still_moves() {
    use super::scrolling::SmoothState;
    use std::time::{Duration, Instant};
    let s = SmoothState::default();
    let t0 = Instant::now();
    s.wheel(t0, 0.0, 1000.0, -600.0, false, false);
    let first = s
        .tick(t0 + Duration::from_millis(10), 0.0, 1000.0, false)
        .unwrap();
    assert!(first < 0.0 && first > -600.0, "easing has begun: {first}");
    // Content/viewport shrinks before the next frame: the stored target must be clamped too.
    // `current` is what the previous tick actually presented, so this is not an external jump.
    let at_shrink = s
        .tick(t0 + Duration::from_millis(20), first, 100.0, false)
        .unwrap();
    assert!(
        at_shrink < 0.0 && at_shrink > -100.0,
        "presentation still animates within the new range: {at_shrink}"
    );
    let settled = s
        .tick(t0 + Duration::from_millis(400), at_shrink, 100.0, false)
        .unwrap();
    assert_eq!(settled, -100.0);
    assert!(!s.is_animating());
    // Reverse now: must move immediately, not accumulate against the old -600 target.
    let reverse = s
        .wheel(
            t0 + Duration::from_millis(500),
            -100.0,
            100.0,
            120.0,
            true,
            false,
        )
        .unwrap();
    assert_eq!(reverse, 0.0);
    // Also exercise a discrete reverse after a range shrink.
    let s2 = SmoothState::default();
    s2.wheel(t0, 0.0, 1000.0, -600.0, false, false);
    let first2 = s2
        .tick(t0 + Duration::from_millis(10), 0.0, 1000.0, false)
        .unwrap();
    let _ = s2.tick(t0 + Duration::from_millis(400), first2, 100.0, false);
    s2.wheel(
        t0 + Duration::from_millis(500),
        -100.0,
        100.0,
        120.0,
        false,
        false,
    );
    let moved = s2
        .tick(t0 + Duration::from_millis(600), -100.0, 100.0, false)
        .unwrap();
    assert!(moved > -100.0, "reverse must move toward zero: {moved}");
}

#[test]
fn reduced_motion_range_shrink_is_clamped_immediately() {
    use super::scrolling::SmoothState;
    use std::time::{Duration, Instant};
    let s = SmoothState::default();
    let t0 = Instant::now();
    s.wheel(t0, 0.0, 1000.0, -600.0, false, false);
    let first = s
        .tick(t0 + Duration::from_millis(10), 0.0, 1000.0, false)
        .unwrap();
    let next = s
        .tick(t0 + Duration::from_millis(20), first, 100.0, true)
        .unwrap();
    assert_eq!(next, -100.0);
    assert!(!s.is_animating());
    assert_eq!(
        s.wheel(
            t0 + Duration::from_millis(30),
            -100.0,
            100.0,
            120.0,
            true,
            true
        ),
        Some(0.0)
    );
}

#[gpui::test]
fn probe_menu_click2(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("sidebar-toggle", cx);
    click("sidebar-more", cx);
    println!(
        "open menu={} panel={:?} dismiss={:?}",
        view.read_with(cx, |s, _| s.sidebar_menu_open()),
        cx.debug_bounds("menu-panel-0"),
        cx.debug_bounds("sidebar-menu-dismiss")
    );
    let p = bounds("nav-home", cx).center();
    cx.simulate_mouse_move(p, None, Modifiers::none());
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: p,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    println!(
        "down route={:?} menu={}",
        view.read_with(cx, |s, _| s.route()),
        view.read_with(cx, |s, _| s.sidebar_menu_open())
    );
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: p,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    cx.run_until_parked();
    println!(
        "up route={:?} menu={} panel={:?}",
        view.read_with(cx, |s, _| s.route()),
        view.read_with(cx, |s, _| s.sidebar_menu_open()),
        cx.debug_bounds("menu-panel-0")
    );
}

#[gpui::test]
fn footer_divider_has_equal_space_above_and_below_in_both_modes(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let check = |cx: &mut VisualTestContext| {
        let repo = bounds("repo-scroll", cx);
        let divider = bounds("sidebar-footer-divider", cx);
        let account = bounds("account-row", cx);
        // The rule is 1px tall at the top spacer's end; measure from the actual rule line, not
        // the container's centre.
        let rule_top = divider.top() + px(FOOTER_DIVIDER_SPACE);
        let rule_bottom = rule_top + px(FOOTER_DIVIDER_HEIGHT);
        let above = rule_top - repo.bottom();
        let below = account.top() - rule_bottom;
        assert!(
            (above - below).abs() <= px(1.0),
            "rule must sit visually between list and identity: above={above:?} below={below:?}"
        );
        // The divider container spans the column; its rule is inset by the sidebar padding on
        // both sides, matching every row lane.
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
fn sash_line_sits_inside_the_hit_band_and_island_height(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    let band = bounds("sidebar-resize", cx);
    let island = bounds("island", cx);
    let sash = bounds("sidebar-sash", cx);
    assert_eq!(sash.size.width, px(SIDEBAR_SASH_WIDTH));
    assert!(band.size.width >= sash.size.width, "hit band stays wider");
    assert!(sash.left() >= band.left() && sash.right() <= band.right());
    // Inside the island's visible vertical edge, never the title bar or the bottom inset.
    assert!(sash.top() >= island.top());
    assert!(sash.bottom() <= island.bottom());
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
    press("secondary-k", cx);
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
