//! Interaction and measured layout coverage for the resizable sidebar.
use super::{Shell, geometry::*, sidebar_state::SidebarState};
use crate::workspace::Route;
use gpui::{
    Bounds, KeyUpEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels,
    TestAppContext, VisualTestContext, point, px, size,
};

fn shell(cx: &mut TestAppContext) -> (gpui::Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| {
        ely_gpui_component::init_for_tests(cx);
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
    let mut state = SidebarState {
        dragging: true,
        ..Default::default()
    };
    state.resize(900.0, 1200.0);
    assert_eq!(state.width(1200.0), 360.0);
    assert_eq!(state.width(880.0), 265.0);
    state.collapsed = true;
    assert_eq!(state.width(880.0), 56.0);
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
    assert_eq!(account.top() - repo.bottom(), px(REPO_ACCOUNT_GAP));
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
    click("account-row", cx);
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
            bounds("shell-body", cx).center().y
        );
    }
}

#[gpui::test]
fn menu_keyboard_outside_click_and_resize_close_without_navigation(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("sidebar-toggle", cx);
    click("sidebar-more", cx);
    press("down", cx);
    press("enter", cx);
    assert_eq!(view.read_with(cx, |s, _| s.route()), Route::Notifications);
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    click("sidebar-more", cx);
    click("nav-home", cx);
    assert!(cx.debug_bounds("menu-panel-0").is_none());
    assert_eq!(
        view.read_with(cx, |s, _| s.route()),
        Route::Notifications,
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
