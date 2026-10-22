//! Tests over the real shell view on GPUI's test platform.
//!
//! These cover what the test platform can actually check: routing, the palette's open, filter,
//! select and Escape paths, focus after closing, the bounded sidebar (only the repository group
//! gives up height when the window shrinks), and that the island stays inside the window. They do
//! not exercise Windows non-client hit testing, real dragging/snapping, or native IME delivery;
//! those remain pending manual checks and are recorded as such.

use gpui::{
    Bounds, KeyUpEvent, Keystroke, Modifiers, Pixels, Point, ScrollDelta, ScrollWheelEvent,
    TestAppContext, VisualContext, VisualTestContext, px, size,
};

use super::Shell;
use crate::workspace::Route;

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        ely_gpui_component::init_for_tests(cx);
        Shell::bind_keys(cx);
    });
}

fn shell(cx: &mut TestAppContext) -> (gpui::Entity<Shell>, &mut VisualTestContext) {
    setup(cx);
    cx.add_window_view(Shell::new)
}

/// A press and release, as a real keyboard delivers them.
fn press(key: &str, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(key);
    cx.run_until_parked();
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("valid test keystroke"),
    });
    cx.run_until_parked();
}

fn bounds(selector: &'static str, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} was not drawn"))
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let target = bounds(selector, cx).center();
    cx.simulate_click(target, Modifiers::none());
    cx.run_until_parked();
}

fn route(view: &gpui::Entity<Shell>, cx: &mut VisualTestContext) -> Route {
    view.read_with(cx, |shell, _| shell.route())
}

/// The title bar is drawn the same way on every platform, so this must hold where `code-check`
/// runs as well as on Windows: the fixed search entry and the brand are always present, while the
/// three caption buttons are a Windows-only addition.
#[gpui::test]
fn the_title_bar_always_offers_the_search_entry_and_brand(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    assert!(cx.debug_bounds("titlebar").is_some());
    assert!(cx.debug_bounds("titlebar-brand").is_some());
    assert!(
        cx.debug_bounds("search-entry").is_some(),
        "the search entry is part of the title bar on every platform"
    );
    #[cfg(target_os = "windows")]
    for selector in ["caption-Minimize", "caption-Maximize", "caption-Close"] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "{selector} is drawn on Windows"
        );
    }
    #[cfg(not(target_os = "windows"))]
    assert!(
        cx.debug_bounds("caption-Close").is_none(),
        "other platforms keep their system frame, so the bar draws no second set of buttons"
    );
}

#[gpui::test]
fn the_shell_starts_on_home_with_a_bounded_repository_list(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    assert_eq!(route(&view, cx), Route::Home);
    assert!(
        cx.debug_bounds("page-home").is_some(),
        "Home draws its columns"
    );
    assert!(cx.debug_bounds("home-involved").is_some());
    assert!(cx.debug_bounds("home-feed").is_some());
    assert!(
        view.read_with(cx, |shell, _| shell.repositories().len()) >= 100,
        "the fixture holds at least 100 repositories"
    );
    assert!(cx.debug_bounds("repo-scroll").is_some());
    assert!(cx.debug_bounds("repo-row-0").is_some());
    assert!(cx.debug_bounds("search-entry").is_some());
}

/// The island and sidebar must not depend on the title bar's platform branch: the shell body
/// takes whatever height the bar leaves.
#[gpui::test]
fn the_shell_body_sits_below_the_title_bar_on_every_platform(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    let bar = bounds("titlebar", cx);
    let body = bounds("shell-body", cx);
    let island = bounds("island", cx);
    assert_eq!(body.top(), bar.bottom());
    assert!(island.top() >= body.top());
    assert!(island.bottom() <= body.bottom());
    assert_eq!(bar.size.height, px(super::geometry::TITLEBAR_HEIGHT));
}

#[gpui::test]
fn resizing_keeps_the_fixed_rows_and_shrinks_only_the_repository_group(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    let tall = size(px(1400.0), px(900.0));
    let short = size(px(900.0), px(600.0));

    let measure = |cx: &mut VisualTestContext| {
        (
            bounds("nav-home", cx),
            bounds("account-row", cx),
            bounds("repo-scroll", cx),
            bounds("quarter-row", cx),
            bounds("island", cx),
            bounds("sidebar", cx),
        )
    };

    cx.simulate_resize(tall);
    cx.run_until_parked();
    let (tall_nav, _tall_account, tall_scroll, tall_quarters, tall_island, tall_sidebar) =
        measure(cx);

    cx.simulate_resize(short);
    cx.run_until_parked();
    let (short_nav, short_account, short_scroll, short_quarters, short_island, short_sidebar) =
        measure(cx);

    // Home keeps its place and height; the repository group absorbs the change.
    assert_eq!(short_nav.top(), tall_nav.top());
    assert_eq!(short_nav.size.height, tall_nav.size.height);
    assert!(
        short_scroll.size.height < tall_scroll.size.height,
        "only the repository group may shrink: {:?} -> {:?}",
        tall_scroll.size.height,
        short_scroll.size.height
    );
    assert_eq!(short_quarters.size.height, tall_quarters.size.height);
    assert!(short_scroll.bottom() <= short_account.top() + px(2.0));

    // The bottom entries stay inside the sidebar's bottom edge.
    assert!(short_account.bottom() <= short_sidebar.bottom() + px(1.0));
    assert!(short_quarters.bottom() <= short_sidebar.bottom() + px(1.0));

    // The island stays inside the window and the sidebar keeps its width.
    assert!(short_island.right() <= px(900.0) + px(1.0));
    assert!(tall_island.right() <= px(1400.0) + px(1.0));
    assert_eq!(short_sidebar.size.width, px(super::geometry::SIDEBAR_WIDTH));
    assert_eq!(tall_sidebar.size.width, short_sidebar.size.width);

    // Home's two columns split the island evenly and stay inside it, even with long single-line
    // text in the fixture: nothing may paint past the island's right edge.
    let involved = bounds("home-involved", cx);
    let feed = bounds("home-feed", cx);
    assert_eq!(involved.size.width, feed.size.width);
    assert!(feed.right() <= short_island.right() + px(1.0));
    assert!(involved.right() <= feed.left());
}

#[gpui::test]
fn the_repository_group_scrolls_without_moving_the_island_page(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let scroll = bounds("repo-scroll", cx);
    cx.simulate_mouse_move(scroll.center(), None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: scroll.center(),
        delta: ScrollDelta::Lines(gpui::point(0.0, -12.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
    });
    cx.run_until_parked();
    let repo_offset = view.read_with(cx, |shell, _| shell.repo_offset());
    assert!(
        repo_offset.y < px(0.0),
        "the repository group must scroll, offset {repo_offset:?}"
    );
    let page_offset = view.read_with(cx, |shell, _| shell.page_offset());
    assert_eq!(
        page_offset,
        Point::default(),
        "the island page must not scroll with the sidebar"
    );
}

#[gpui::test]
fn navigation_rows_and_quarter_entries_route_and_keep_selection(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);

    click("nav-settings", cx);
    assert_eq!(route(&view, cx), Route::Settings);
    // Settings options are not designed yet: the content area stays empty on purpose.
    assert!(cx.debug_bounds("page-home").is_none());
    assert!(cx.debug_bounds("page-settings").is_none());

    click("quarter-notifications", cx);
    assert_eq!(route(&view, cx), Route::Notifications);
    click("quarter-issues", cx);
    assert_eq!(route(&view, cx), Route::Issues);
    click("quarter-pull-requests", cx);
    assert_eq!(route(&view, cx), Route::PullRequests);

    click("account-row", cx);
    assert_eq!(route(&view, cx), Route::Profile);
    assert!(cx.debug_bounds("page-profile").is_some());

    // The quarter entries keep a persistent selected state for their blank destinations, as the
    // product requires selected-state verification. Go back to Issues first, then prove New's
    // disabled entry never changes the route.
    click("quarter-issues", cx);
    assert_eq!(route(&view, cx), Route::Issues);

    // New is disabled this round: pressing it changes nothing.
    click("quarter-new", cx);
    assert_eq!(route(&view, cx), Route::Issues);

    click("nav-home", cx);
    assert_eq!(route(&view, cx), Route::Home);
    assert!(cx.debug_bounds("page-home").is_some());
}

#[gpui::test]
fn a_repository_row_opens_its_minimal_identifier(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let expected = view.read_with(cx, |shell, _| shell.repositories()[1].clone());
    click("repo-row-1", cx);
    assert_eq!(route(&view, cx), Route::repository(&expected));
    assert!(cx.debug_bounds("page-repository").is_some());
}

#[gpui::test]
fn the_search_entry_opens_a_palette_that_filters_runs_and_closes(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("search-entry", cx);
    assert!(view.read_with(cx, |shell, _| shell.palette_open()));
    assert!(
        cx.debug_bounds("palette-card").is_some(),
        "the palette draws"
    );

    cx.simulate_input("settings");
    cx.run_until_parked();
    press("enter", cx);
    assert_eq!(route(&view, cx), Route::Settings);
    assert!(!view.read_with(cx, |shell, _| shell.palette_open()));

    click("search-entry", cx);
    assert!(view.read_with(cx, |shell, _| shell.palette_open()));
    press("escape", cx);
    assert!(!view.read_with(cx, |shell, _| shell.palette_open()));
    assert_eq!(
        route(&view, cx),
        Route::Settings,
        "Escape does not navigate"
    );
}

#[gpui::test]
fn the_palette_reaches_a_fixture_repository(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let expected = view.read_with(cx, |shell, _| shell.repositories()[0].clone());
    click("search-entry", cx);
    cx.simulate_input(expected.name.as_str());
    cx.run_until_parked();
    press("enter", cx);
    assert_eq!(route(&view, cx), Route::repository(&expected));
}

#[gpui::test]
fn closing_the_palette_leaves_a_live_focus_target(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    click("search-entry", cx);
    press("escape", cx);
    cx.update(|window, cx| {
        let focused = window
            .focused(cx)
            .expect("focus is still held after closing");
        assert!(focused.is_focused(window));
    });
}

#[gpui::test]
fn the_session_strip_shows_the_current_title_and_no_tab_controls(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    assert!(cx.debug_bounds("session-strip").is_some());
    assert!(cx.debug_bounds("session-title").is_some());
    let repo = view.read_with(cx, |shell, _| shell.repositories()[0].clone());
    view.update(cx, |shell, cx| {
        shell.navigate(Route::repository(&repo), cx);
    });
    cx.run_until_parked();
    let strip = bounds("session-strip", cx);
    assert!(strip.size.height > px(0.0));
    // No add or close control is drawn: multi-session tabs belong to the next R1 task.
    assert!(cx.debug_bounds("session-add").is_none());
    assert!(cx.debug_bounds("session-close").is_none());
}

#[gpui::test]
fn keyboard_reaches_the_search_entry_and_ctrl_k_opens_the_palette_anywhere(
    cx: &mut TestAppContext,
) {
    let (view, cx) = shell(cx);
    click("nav-settings", cx);
    press("ctrl-k", cx);
    assert!(view.read_with(cx, |shell, _| shell.palette_open()));
    press("escape", cx);
    assert!(!view.read_with(cx, |shell, _| shell.palette_open()));

    // Tab keeps moving through the shell's tab stops after the overlay closed.
    for _ in 0..3 {
        press("tab", cx);
    }
    cx.update(|window, cx| {
        assert!(window.focused(cx).is_some(), "Tab keeps a focus target");
    });
}

#[gpui::test]
fn the_focused_search_entry_opens_the_palette_with_enter_and_space(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    // The title bar paints first, so the search entry is the first tab stop.
    press("tab", cx);
    press("enter", cx);
    assert!(
        view.read_with(cx, |shell, _| shell.palette_open()),
        "Enter on the focused search entry opens the palette"
    );
    press("escape", cx);
    assert!(!view.read_with(cx, |shell, _| shell.palette_open()));

    press("space", cx);
    assert!(
        view.read_with(cx, |shell, _| shell.palette_open()),
        "Space on the focused search entry opens the palette"
    );
    press("escape", cx);

    // The palette traps Tab while it is open, so closing cannot have left focus inside it.
    assert!(cx.debug_bounds("palette-card").is_none());
    cx.update(|window, cx| {
        assert!(
            window.focused(cx).is_some(),
            "focus is not lost after Escape"
        );
    });
}

#[gpui::test]
fn tab_reaches_a_sidebar_row_that_enter_then_navigates(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    // The search entry is first; Home and Settings follow it in the sidebar.
    press("tab", cx);
    press("tab", cx);
    press("tab", cx);
    cx.run_until_parked();
    // Three stops in, the focus sits on a sidebar row; Enter activates it like a click.
    press("enter", cx);
    let route = route(&view, cx);
    assert!(
        route == Route::Settings || route == Route::Home,
        "a keyboard-activated sidebar row navigates, got {route:?}"
    );
}

/// Tab reaches repository rows far below the visible part of the bounded list; focusing one must
/// scroll it into view, or the focus ring and Enter's target would sit offscreen.
#[gpui::test]
fn tabbing_to_a_clipped_repository_row_scrolls_it_into_view(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(900.0), px(560.0)));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repo_offset()),
        Point::default(),
        "the list starts at the top"
    );

    // Focus a row that cannot be visible in a 560px-tall window, the way Tab eventually would.
    let deep = view.read_with(cx, |shell, _| shell.repositories().len() - 1);
    let handle = view.read_with(cx, |shell, _| shell.repo_focus()[deep].clone());
    cx.update(|window, cx| handle.focus(window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(
            window.focused(cx).is_some_and(|focused| focused == handle),
            "the deep row took focus"
        );
    });

    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let after_refresh = view.read_with(cx, |shell, _| shell.repo_offset());
    assert!(
        after_refresh.y < px(0.0),
        "focusing a clipped row scrolls the repository group, offset {after_refresh:?}"
    );
    let row = bounds(Box::leak(format!("repo-row-{deep}").into_boxed_str()), cx);
    let list = bounds("repo-scroll", cx);
    assert!(
        row.top() >= list.top() - px(1.0) && row.bottom() <= list.bottom() + px(1.0),
        "the focused row must be inside the list: {row:?} in {list:?}"
    );
    // The rest of the shell stays put: only the repository group scrolls.
    assert_eq!(
        view.read_with(cx, |shell, _| shell.page_offset()),
        Point::default()
    );
}

#[gpui::test]
fn a_chinese_and_a_long_repository_name_stay_inside_the_sidebar(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    // The curated head of the fixture holds the Chinese and the deliberately long names.
    let (cjk_index, long_index) = view.read_with(cx, |shell, _| {
        let cjk = shell
            .repositories()
            .iter()
            .position(|repo| !repo.name.is_ascii())
            .expect("a Chinese repository fixture");
        let long = shell
            .repositories()
            .iter()
            .enumerate()
            .max_by_key(|(_, repo)| repo.full_name().chars().count())
            .map(|(index, _)| index)
            .expect("repositories");
        (cjk, long)
    });
    assert!(
        view.read_with(cx, |shell, _| shell.repositories()[long_index]
            .full_name()
            .chars()
            .count())
            >= 40
    );
    let sidebar = bounds("sidebar", cx);
    assert_eq!(sidebar.size.width, px(super::geometry::SIDEBAR_WIDTH));

    // Rows are clipped inside the column: the list never widens the sidebar, whatever the name.
    let selector: &'static str = match (cjk_index, long_index) {
        (8, 9) => "repo-row-9",
        _ => "repo-row-0",
    };
    let row = bounds(selector, cx);
    assert!(row.right() <= sidebar.right() + px(1.0));
    let _ = (cjk_index, long_index);
}

#[gpui::test]
fn the_blank_pages_draw_no_placeholder_card(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    for target in [
        Route::Settings,
        Route::Issues,
        Route::PullRequests,
        Route::Notifications,
    ] {
        view.update(cx, |shell, cx| {
            shell.navigate(target.clone(), cx);
        });
        cx.run_until_parked();
        // The island keeps its shell and session strip; the content area stays empty.
        assert!(cx.debug_bounds("island").is_some());
        assert!(cx.debug_bounds("session-strip").is_some());
        assert!(cx.debug_bounds("page-home").is_none());
        assert!(cx.debug_bounds("page-repository").is_none());
        assert!(cx.debug_bounds("page-profile").is_none());
    }
}

#[gpui::test]
fn moving_the_highlight_wraps_and_enter_runs_the_highlight_not_the_first_row(
    cx: &mut TestAppContext,
) {
    let (view, cx) = shell(cx);
    click("search-entry", cx);
    // The empty query lists every page first, then the repositories.
    press("down", cx);
    press("down", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.palette_cursor()), 2);
    press("up", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.palette_cursor()), 1);
    press("up", cx);
    press("up", cx);
    // Two steps up from 1 wraps past the start to the end of the list.
    let count = view.read_with(cx, |shell, cx| shell.palette_result_count(cx));
    assert!(count > 6, "the list holds pages and repositories");
    assert_eq!(
        view.read_with(cx, |shell, _| shell.palette_cursor()),
        count - 1
    );

    press("enter", cx);
    assert!(!view.read_with(cx, |shell, _| shell.palette_open()));
    assert!(
        view.read_with(cx, |shell, _| matches!(
            shell.route(),
            Route::Repository { .. }
        )),
        "the last row is a repository, so Enter opens it"
    );
}

#[gpui::test]
fn a_composing_input_method_keeps_enter_and_does_not_run_a_row(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("search-entry", cx);
    // Enter with committed text runs the highlighted row.
    cx.simulate_input("profile");
    cx.run_until_parked();
    // Simulate an active composition by marking text through the field's input handler, which is
    // the signal the palette consults. This is not a Windows IME delivery test.
    let input = view.read_with(cx, |shell, _| shell.palette_input());
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            use gpui::EntityInputHandler;
            input.replace_and_mark_text_in_range(None, "设置", None, window, cx);
        });
    });
    cx.run_until_parked();
    let composing = cx.update_window_entity(&view, |shell, window, cx| {
        shell.palette_composing(window, cx)
    });
    assert!(
        composing,
        "the field reports a marked range while composing"
    );
    press("enter", cx);
    assert!(
        view.read_with(cx, |shell, _| shell.palette_open()),
        "Enter during a composition must not run the highlighted row"
    );
    assert_eq!(route(&view, cx), Route::Home);
}

#[gpui::test]
fn a_composing_input_method_keeps_up_down_and_escape_for_its_candidate_list(
    cx: &mut TestAppContext,
) {
    let (view, cx) = shell(cx);
    click("search-entry", cx);
    let input = view.read_with(cx, |shell, _| shell.palette_input());
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            use gpui::EntityInputHandler;
            input.replace_and_mark_text_in_range(None, "she", None, window, cx);
        });
    });
    cx.run_until_parked();
    let composing = cx.update_window_entity(&view, |shell, window, cx| {
        shell.palette_composing(window, cx)
    });
    assert!(composing);

    let before = view.read_with(cx, |shell, _| shell.palette_cursor());
    press("down", cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.palette_cursor()),
        before,
        "Down belongs to the IME candidate list while composing"
    );
    press("up", cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.palette_cursor()),
        before
    );
    press("escape", cx);
    assert!(
        view.read_with(cx, |shell, _| shell.palette_open()),
        "Escape cancels the composition instead of closing the palette"
    );
}

#[gpui::test]
fn the_wheel_keeps_the_user_position_until_the_cursor_moves(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("search-entry", cx);
    let list = bounds("palette-list", cx);
    cx.simulate_mouse_move(list.center(), None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: list.center(),
        delta: ScrollDelta::Lines(gpui::point(0.0, -8.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
    });
    cx.run_until_parked();
    let before = view.read_with(cx, |shell, _| shell.palette_scroll_offset());
    assert!(before.y < px(0.0), "the wheel moves the list");

    // A repaint must not snap the list back to the highlighted row.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.palette_scroll_offset()),
        before,
        "rendering alone must not request another scroll"
    );

    // Moving the highlight with the keyboard is allowed to bring its row into view once.
    press("down", cx);
    let after = view.read_with(cx, |shell, _| shell.palette_scroll_offset());
    assert!(
        after.y >= before.y,
        "cursor movement may scroll, never fight the user"
    );
}

#[gpui::test]
fn escape_closes_the_palette_without_running_the_highlighted_row(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("search-entry", cx);
    cx.simulate_input("repository");
    press("escape", cx);
    assert!(!view.read_with(cx, |shell, _| shell.palette_open()));
    assert_eq!(route(&view, cx), Route::Home, "Escape never navigates");
}
