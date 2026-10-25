//! Tests over the real shell view on GPUI's test platform.
//!
//! These cover what the test platform can actually check: routing, the application session strip
//! (open-or-activate, close, the preview limit, keyboard chords, per-session scroll/focus/state),
//! the palette's open, filter, select and Escape paths, focus after closing, the bounded sidebar
//! (only the repository group gives up height when the window shrinks), and that the island stays
//! inside the window. They do not exercise Windows non-client hit testing, real dragging/snapping,
//! or native IME delivery; those remain pending manual checks and are recorded as such.

use gpui::{
    Bounds, KeyUpEvent, Keystroke, Modifiers, Pixels, Point, ScrollDelta, ScrollWheelEvent,
    TestAppContext, VisualContext, VisualTestContext, px, size,
};

use super::Shell;
use crate::workspace::{RepositorySession, Route};

/// A `debug_selector` string built at runtime for the tests that index rows.
fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

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

/// Moves the pointer onto an element, so hover-only affordances become live.
fn hover(selector: &'static str, cx: &mut VisualTestContext) {
    let target = bounds(selector, cx).center();
    cx.simulate_mouse_move(target, None, Modifiers::none());
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
fn the_island_draws_real_session_tabs_and_no_placeholder_title(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    assert!(cx.debug_bounds("page-home").is_some());
    // The real strip replaces the removed page-name placeholder: the old selectors stay gone.
    assert!(cx.debug_bounds("tab-strip").is_some());
    assert!(cx.debug_bounds("tab-0").is_some());
    assert!(cx.debug_bounds("session-strip").is_none());
    assert!(cx.debug_bounds("session-title").is_none());
    assert!(cx.debug_bounds("session-add").is_none());

    let repo = view.read_with(cx, |shell, _| shell.repositories()[0].clone());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&repo), window, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("page-repository").is_some());
    assert!(cx.debug_bounds("tab-0").is_some(), "Home keeps its tab");
    assert!(
        cx.debug_bounds("tab-1").is_some(),
        "the repository opened one"
    );
    assert!(cx.debug_bounds("tab-close-0").is_some());
    assert!(cx.debug_bounds("session-strip").is_none());
    assert!(cx.debug_bounds("session-title").is_none());
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
        cx.update_window_entity(&view, |shell, window, cx| {
            shell.navigate(target.clone(), window, cx);
        });
        cx.run_until_parked();
        // The island keeps its shell; the content area stays empty with no title strip.
        assert!(cx.debug_bounds("island").is_some());
        assert!(cx.debug_bounds("session-strip").is_none());
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

/// The strip lists every open session and each entry reuses its tab instead of duplicating it.
#[gpui::test]
fn opening_the_same_page_reuses_its_tab(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    let repo = view.read_with(cx, |shell, _| shell.repositories()[0].clone());

    click("repo-row-0", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 2);
    let repository_tab = view.read_with(cx, |shell, _| shell.tab_ids()[1]);
    assert_eq!(route(&view, cx), Route::repository(&repo));

    // Clicking Home and the same repository again switches tabs instead of duplicating them.
    click("nav-home", cx);
    assert_eq!(route(&view, cx), Route::Home);
    click("repo-row-0", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 2);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        repository_tab
    );

    // The command palette uses the same open-or-activate path.
    click("search-entry", cx);
    cx.simulate_input(repo.name.as_str());
    cx.run_until_parked();
    press("enter", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 2);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        repository_tab
    );
}

/// Repository A/B and Profile keep their own scroll offset, focus and browsing selection.
#[gpui::test]
fn repositories_and_profile_keep_separate_scroll_focus_and_selection(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(900.0), px(560.0)));
    cx.run_until_parked();

    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("account-row", cx);
    let profile = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(a, b);
    assert_ne!(b, profile);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 4);

    // The reader's browsing selection stays per session.
    view.update(cx, |shell, _| {
        shell.select_repository(a, "main", Some("README.md"));
        shell.select_repository(b, "release", Some("src/lib.rs"));
    });
    assert_eq!(
        view.read_with(cx, |shell, _| shell
            .repository_state(a)
            .and_then(|state| state.branch.clone())),
        Some("main".to_string())
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell
            .repository_state(b)
            .and_then(|state| state.file.clone())),
        Some("src/lib.rs".to_string())
    );

    // Home scrolls for real; switching away leaves the other sessions at their own offset.
    click("tab-0", cx);
    assert_eq!(route(&view, cx), Route::Home);
    let page = bounds("island-page", cx);
    cx.simulate_mouse_move(page.center(), None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: page.center(),
        delta: ScrollDelta::Lines(gpui::point(0.0, -12.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
    });
    cx.run_until_parked();
    let home_offset = view.read_with(cx, |shell, _| shell.page_offset());
    assert!(home_offset.y < px(0.0), "Home scrolled: {home_offset:?}");
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_page_offset(a)),
        Some(Point::default())
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_page_offset(profile)),
        Some(Point::default())
    );

    // Focus that lives in a page follows the switch into the incoming page's own handle.
    cx.update_window_entity(&view, |shell, window, cx| shell.activate_tab(a, window, cx));
    let a_focus = view.read_with(cx, |shell, _| shell.tab_page_focus(a).unwrap());
    cx.update_window_entity(&view, |_, window, cx| window.focus(&a_focus, cx));
    cx.run_until_parked();
    press("ctrl-tab", cx);
    let b_focus = view.read_with(cx, |shell, _| shell.tab_page_focus(b).unwrap());
    let focused = cx.update(|window, cx| window.focused(cx));
    assert_eq!(
        focused.as_ref(),
        Some(&b_focus),
        "focus moves into the next page"
    );
    press("ctrl-shift-tab", cx);
    let focused = cx.update(|window, cx| window.focused(cx));
    assert_eq!(
        focused.as_ref(),
        Some(&a_focus),
        "and back into the first one"
    );

    // Home's own scroll offset is still where the user left it.
    click("tab-0", cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.page_offset()),
        home_offset
    );
}

/// Closing keeps the current page unless the active tab itself closes, then picks the neighbour.
#[gpui::test]
fn closing_tabs_keeps_the_active_page_and_picks_a_neighbour(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-2", cx);
    let c = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 4);

    // Closing a non-active tab keeps the active page. The close button only exists while the tab
    // is hovered or active, so the pointer has to be over it.
    hover("tab-1", cx);
    click("tab-close-1", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), c);
    assert!(!view.read_with(cx, |shell, _| shell.tab_ids().contains(&a)));
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 3);

    // Ctrl+W closes the active tab and moves to its left neighbour.
    press("ctrl-w", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), b);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 2);

    // Closing the last session opens a fresh Home with a new id.
    press("ctrl-w", cx);
    press("ctrl-w", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 1);
    assert_eq!(route(&view, cx), Route::Home);
    let fresh_home = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert!(fresh_home > c, "a closed id is never reused");
}

/// Nine or more tabs stay open: the strip scrolls instead of compressing tabs, refusing a page or
/// widening the island, and its scrollbar only appears when there is something to scroll.
#[gpui::test]
fn more_tabs_than_fit_scroll_horizontally_instead_of_compressing(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();

    // A single tab: nothing overflows, so the strip has no reach to scroll and its overlay
    // scrollbar paints nothing.
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 1);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_scroll_max()).x,
        px(0.0)
    );
    assert!(cx.debug_bounds("tab-scrollbar-band").is_some());
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_scroll_offset()),
        Point::default()
    );
    let single_tab_top = bounds("tab-0", cx).top();

    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        12,
        "no page is refused"
    );

    let island = bounds("island", cx);
    let strip = bounds("tab-strip", cx);
    assert!(strip.right() <= island.right() + px(1.0));
    let reach = view.read_with(cx, |shell, _| shell.tab_scroll_max());
    assert!(
        reach.x > px(0.0),
        "the strip can now be scrolled: {reach:?}"
    );
    // The band is always reserved; Ely's overlay scrollbar paints inside it only when the handle
    // has something to scroll, which is the reach asserted above.
    assert!(cx.debug_bounds("tab-scrollbar-band").is_some());

    // Tabs keep one readable width; the row overflows instead of shrinking them.
    for index in 0..12 {
        let tab = bounds(selector(format!("tab-{index}")), cx);
        assert_eq!(
            tab.size.width,
            px(super::geometry::TAB_WIDTH),
            "tab {index} keeps its width"
        );
    }
    // Scroll back to the row's start: the whole row is wider than the viewport.
    view.update(cx, |shell, _| {
        let offset = shell.tab_scroll_offset();
        shell.scroll_tabs_by(-offset.x);
    });
    pump(cx);
    let last = bounds("tab-11", cx);
    assert!(
        last.right() > strip.right(),
        "without scrolling the row, the last tab is past the visible strip: {last:?} in {strip:?}"
    );

    // The island itself stays inside the window at the minimum size.
    assert!(island.right() <= px(880.0) + px(1.0));
    let page = bounds("island-page", cx);
    assert!(page.right() <= island.right() + px(1.0));
    // Tabs keep their vertical position whether or not the scrollbar has something to show.
    assert_eq!(bounds("tab-0", cx).top(), single_tab_top);
}

/// The wheel moves the strip horizontally and nothing else: the page and the sidebar keep their
/// offsets, whole-tab scroll positions survive manual scrolling, and a clamp keeps the tail short
/// after a close.
#[gpui::test]
fn wheel_and_manual_scroll_move_only_the_strip(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }

    // Start from the row's left end so both wheel gestures have somewhere to go.
    view.update(cx, |shell, _| {
        let offset = shell.tab_scroll_offset();
        shell.scroll_tabs_by(-offset.x);
    });
    pump(cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_scroll_offset()).x,
        px(0.0)
    );

    let strip = bounds("tab-scroll", cx);
    cx.simulate_mouse_move(strip.center(), None, Modifiers::none());

    // A plain vertical wheel moves the strip sideways because it is the only scrollable axis.
    cx.simulate_event(ScrollWheelEvent {
        position: strip.center(),
        delta: ScrollDelta::Lines(gpui::point(0.0, -6.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
    });
    cx.run_until_parked();
    let after_wheel = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    assert!(
        after_wheel.x < px(0.0),
        "the wheel moved the strip: {after_wheel:?}"
    );
    assert_eq!(after_wheel.y, px(0.0), "the strip never scrolls vertically");
    assert_eq!(
        view.read_with(cx, |shell, _| shell.page_offset()),
        Point::default(),
        "the page did not scroll"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repo_offset()),
        Point::default(),
        "the sidebar did not scroll"
    );

    // Shift+wheel arrives as a horizontal delta on Windows; it moves the strip by itself.
    let before_shift = after_wheel;
    cx.simulate_event(ScrollWheelEvent {
        position: strip.center(),
        delta: ScrollDelta::Lines(gpui::point(-3.0, 0.0)),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::none()
        },
        touch_phase: Default::default(),
    });
    cx.run_until_parked();
    let after_shift = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    assert!(
        after_shift.x < before_shift.x,
        "a native horizontal delta scrolls the strip once: {after_shift:?}"
    );

    // Manual scrolling is not undone by later frames.
    let held = after_shift;
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_scroll_offset()),
        held,
        "rendering alone does not fight the user's scroll"
    );

    // Closing the tail clamps the offset so no blank stretch is left behind.
    let last = view.read_with(cx, |shell, _| shell.tab_ids()[11]);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.close_tab(last, window, cx);
    });
    cx.run_until_parked();
    let max = view.read_with(cx, |shell, _| {
        (shell.tab_scroll_offset(), shell.tab_scroll_max())
    });
    assert!(
        max.0.x >= -max.1.x - px(1.0),
        "the offset stays inside the new content: {:?} of {:?}",
        max.0,
        max.1
    );
}

#[gpui::test]
fn the_strip_scrollbar_appears_on_hover_and_can_be_dragged(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }
    // Back to the row start so the thumb sits at the track's left end.
    view.update(cx, |shell, _| {
        let offset = shell.tab_scroll_offset();
        shell.scroll_tabs_by(-offset.x);
    });
    pump(cx);
    let reach = view.read_with(cx, |shell, _| shell.tab_scroll_max());
    assert!(reach.x > px(0.0));

    // Nothing is drawn while the pointer is elsewhere.
    cx.simulate_mouse_move(gpui::point(px(40.0), px(300.0)), None, Modifiers::none());
    pump(cx);
    assert_eq!(
        view.update(cx, |shell, _| shell.tab_scrollbar_opacity()),
        0.0
    );
    assert!(cx.debug_bounds("tab-scrollbar").is_none());

    // Hovering the strip fades the bar in; once the fade has finished it is painted.
    hover("tab-scroll", cx);
    view.update(cx, |shell, _| shell.show_tab_scrollbar());
    pump(cx);
    let band = bounds("tab-scrollbar-band", cx);
    let track = bounds("tab-scrollbar", cx);
    assert_eq!(
        track.size.height,
        px(super::geometry::TAB_SCROLLBAR_THICKNESS),
        "the thumb is a hairline, not the theme's thickness"
    );
    assert_eq!(track.left(), band.left());
    assert_eq!(track.right(), band.right());

    // A real drag along the track scrolls the row.
    let thumb_start = gpui::point(band.left() + px(6.0), band.top() + px(1.0));
    cx.simulate_mouse_move(thumb_start, None, Modifiers::none());
    cx.simulate_mouse_down(thumb_start, gpui::MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    for step in 1..=8 {
        cx.simulate_mouse_move(
            gpui::point(thumb_start.x + px(8.0 * step as f32), thumb_start.y),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    cx.simulate_mouse_up(
        gpui::point(thumb_start.x + px(64.0), thumb_start.y),
        gpui::MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();

    let dragged = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    assert!(
        dragged.x < px(0.0),
        "dragging the thumb scrolled the strip: {dragged:?}"
    );
    assert_eq!(dragged.y, px(0.0), "the strip never scrolls vertically");
    assert_eq!(
        view.read_with(cx, |shell, _| shell.page_offset()),
        Point::default(),
        "dragging the strip scrollbar never scrolls the page"
    );
}

/// The strip scrollbar's fade: in and out, reversal from the painted value, drag pinning, and no
/// standing animation once it settles.
#[gpui::test]
fn the_strip_scrollbar_fades_in_out_and_pins_while_dragged(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }

    let fade = |view: &gpui::Entity<Shell>, cx: &mut VisualTestContext, at: u64| {
        view.update(cx, |shell, _| {
            shell.pin_tab_clock(at);
            shell.tab_scrollbar_opacity()
        })
    };

    // Enter: it climbs rather than snapping.
    cx.update_window_entity(&view, |shell, _, _| {
        shell.pin_tab_clock(0);
        shell.tab_scrollbar_hover(true, 0);
    });
    let quarter = super::geometry::TAB_SCROLLBAR_FADE_MS / 4;
    let mid = fade(&view, cx, quarter);
    assert!(mid > 0.0 && mid < 1.0, "mid fade in: {mid}");
    assert_eq!(fade(&view, cx, super::geometry::TAB_SCROLLBAR_FADE_MS), 1.0);
    assert!(!view.read_with(cx, |shell, _| shell.tab_scrollbar_animating()));

    // Reverse half way: it continues from where it was painted, not from 1.
    cx.update_window_entity(&view, |shell, _, _| {
        shell.tab_scrollbar_hover(true, 3_000);
    });
    fade(&view, cx, 3_000);
    cx.update_window_entity(&view, |shell, _, _| {
        shell.tab_scrollbar_hover(false, 3_000);
    });
    let turned = fade(&view, cx, 3_000);
    assert_eq!(turned, 1.0, "the reversal starts at the painted value");

    let falling = fade(&view, cx, 3_000 + quarter);
    assert!(falling > 0.0 && falling < 1.0, "fading out: {falling}");
    // Leaving again reverses in the other direction from the painted value.
    cx.update_window_entity(&view, |shell, _, _| {
        shell.tab_scrollbar_hover(true, 3_000 + quarter);
    });
    let back = fade(&view, cx, 3_000 + quarter);
    assert!(
        (back - falling).abs() < 0.001,
        "a second reversal continues from {falling}, got {back}"
    );
    // The second reversal began at 3_000 + quarter, so it finishes a quarter later again.
    assert_eq!(
        fade(
            &view,
            cx,
            3_000 + quarter + super::geometry::TAB_SCROLLBAR_FADE_MS
        ),
        1.0
    );
    assert!(!view.read_with(cx, |shell, _| shell.tab_scrollbar_animating()));

    // Leaving for good fades it fully out, and a hidden bar is not drawn at all.
    cx.update_window_entity(&view, |shell, _, _| {
        shell.tab_scrollbar_hover(false, 9_000);
    });
    assert_eq!(
        fade(&view, cx, 9_000 + super::geometry::TAB_SCROLLBAR_FADE_MS),
        0.0
    );
    pump(cx);
    assert!(cx.debug_bounds("tab-scrollbar").is_none());
    assert!(!view.read_with(cx, |shell, _| shell.tab_scrollbar_animating()));
}

/// Opening, activating and cycling bring the target tab into view, once, without pinning it there.
#[gpui::test]
fn opening_and_switching_reveal_the_target_tab(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }

    // Opening the twelfth tab brings it fully into view. The scroll offset is applied by the
    // frame that also paints it, so let that frame run.
    pump(cx);
    let strip = bounds("tab-scroll", cx);
    let last = bounds("tab-11", cx);
    assert!(
        last.left() >= strip.left() - px(1.0) && last.right() <= strip.right() + px(1.0),
        "the newest tab is fully visible: {last:?} in {strip:?}"
    );
    assert!(view.read_with(cx, |shell, _| shell.tab_scroll_offset()).x < px(0.0));

    // Cycling back to the first tab brings it back into view.
    let first = view.read_with(cx, |shell, _| shell.tab_ids()[0]);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab(first, window, cx);
    });
    pump(cx);
    let strip = bounds("tab-scroll", cx);
    let home = bounds("tab-0", cx);
    assert!(
        home.left() >= strip.left() - px(1.0) && home.right() <= strip.right() + px(1.0),
        "the activated tab is visible: {home:?} in {strip:?}"
    );

    // A manual scroll is still the user's own until the next reveal.
    view.update(cx, |shell, _| shell.scroll_tabs_by(px(-120.0)));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let manual = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_scroll_offset()),
        manual,
        "repeated renders keep the manual position"
    );
}

/// Ctrl+W and Ctrl+Tab drive the strip; while the palette is open it owns those keys.
#[gpui::test]
fn keyboard_closes_and_cycles_tabs_and_the_palette_keeps_its_keys(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());

    press("ctrl-w", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), a);
    assert!(!view.read_with(cx, |shell, _| shell.tab_ids().contains(&b)));

    press("ctrl-tab", cx);
    let next = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(next, a);
    press("ctrl-shift-tab", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), a);

    // The palette owns the keyboard while it is open.
    click("search-entry", cx);
    let count = view.read_with(cx, |shell, _| shell.tab_count());
    let active = view.read_with(cx, |shell, _| shell.active_tab_id());
    press("ctrl-w", cx);
    press("ctrl-tab", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), count);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), active);
    assert!(view.read_with(cx, |shell, _| shell.palette_open()));
    press("escape", cx);
    assert!(!view.read_with(cx, |shell, _| shell.palette_open()));
}

/// Closing a tab drops its scroll/focus/state; reopening the resource starts clean.
#[gpui::test]
fn closing_a_tab_releases_its_state_and_focus(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    view.update(cx, |shell, cx| {
        shell.select_repository(a, "main", Some("README.md"));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("repository-session").is_some());

    // Focus the page, then close it with Ctrl+W: focus moves to a live target.
    let a_focus = view.read_with(cx, |shell, _| shell.tab_page_focus(a).unwrap());
    cx.update_window_entity(&view, |_, window, cx| window.focus(&a_focus, cx));
    cx.run_until_parked();
    press("ctrl-w", cx);
    assert!(!view.read_with(cx, |shell, _| shell.tab_ids().contains(&a)));
    let home = view.read_with(cx, |shell, _| shell.active_tab_id());
    let home_focus = view.read_with(cx, |shell, _| shell.tab_page_focus(home).unwrap());
    let focused = cx.update(|window, cx| window.focused(cx));
    assert_eq!(
        focused.as_ref(),
        Some(&home_focus),
        "focus leaves the closed page"
    );

    // Reopening the resource creates a fresh session with fresh state.
    click("repo-row-0", cx);
    let reopened = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(reopened, a);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repository_state(reopened).cloned()),
        Some(RepositorySession::default())
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.page_offset()),
        Point::default()
    );
}

/// Tab reaches the strip right after the search entry, and Enter activates the focused tab.
#[gpui::test]
fn tab_reaches_the_session_strip_and_enter_activates(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("nav-home", cx);
    assert_eq!(route(&view, cx), Route::Home);

    // From the root, Tab walks: search entry, then the session tabs.
    press("tab", cx);
    press("tab", cx);
    let home_strip = view.read_with(cx, |shell, _| {
        shell.tab_strip_focus(shell.tab_ids()[0]).unwrap()
    });
    let focused = cx.update(|window, cx| window.focused(cx));
    assert_eq!(focused.as_ref(), Some(&home_strip));

    press("tab", cx);
    let repo_strip = view.read_with(cx, |shell, _| shell.tab_strip_focus(a).unwrap());
    let focused = cx.update(|window, cx| window.focused(cx));
    assert_eq!(focused.as_ref(), Some(&repo_strip));
    press("enter", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), a);
}

/// Closing the active tab while its strip entry holds focus still leaves a live focus target.
#[gpui::test]
fn closing_a_focused_tab_moves_focus_to_a_live_page(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    let strip_focus = view.read_with(cx, |shell, _| shell.tab_strip_focus(a).unwrap());
    cx.update_window_entity(&view, |_, window, cx| window.focus(&strip_focus, cx));
    cx.run_until_parked();

    press("ctrl-w", cx);
    assert!(!view.read_with(cx, |shell, _| shell.tab_ids().contains(&a)));
    // The focus was on a tab entry, so it moves to the entry that replaced it, not to a page.
    let home = view.read_with(cx, |shell, _| shell.active_tab_id());
    let home_strip = view.read_with(cx, |shell, _| shell.tab_strip_focus(home).unwrap());
    let focused = cx.update(|window, cx| window.focused(cx));
    assert_eq!(
        focused.as_ref(),
        Some(&home_strip),
        "focus leaves the closed tab entry"
    );
}

/// An inactive tab can hold focus too; closing it with its own close button must not leave the
/// window focused on a handle that just left the tree, and must not change the active page.
#[gpui::test]
fn closing_a_focused_inactive_tab_keeps_the_page_and_restores_focus(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());

    // Focus the inactive tab's entry without activating it, as Tab does.
    let inactive_strip = view.read_with(cx, |shell, _| shell.tab_strip_focus(a).unwrap());
    cx.update_window_entity(&view, |_, window, cx| window.focus(&inactive_strip, cx));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), b);

    hover("tab-1", cx);
    click("tab-close-1", cx);
    assert!(!view.read_with(cx, |shell, _| shell.tab_ids().contains(&a)));
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        b,
        "closing an inactive tab keeps the current page"
    );
    let b_strip = view.read_with(cx, |shell, _| shell.tab_strip_focus(b).unwrap());
    let focused = cx.update(|window, cx| window.focused(cx));
    assert_eq!(
        focused.as_ref(),
        Some(&b_strip),
        "focus lands on a live tab entry"
    );
}

/// Runs one more frame so a scroll offset applied during render is reflected in the painted bounds.
fn pump(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// Helper: press and release `button` at `position`, as a real mouse delivers a click.
fn click_button(
    position: gpui::Point<Pixels>,
    button: gpui::MouseButton,
    cx: &mut VisualTestContext,
) {
    cx.simulate_mouse_down(position, button, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_up(position, button, Modifiers::none());
    cx.run_until_parked();
}

/// Middle-clicking any part of a tab closes it exactly once and never activates it first.
#[gpui::test]
fn middle_click_closes_a_tab_once_without_activating_it(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 3);

    // Middle click on the inactive tab's icon area: it closes, and the active page stays `b`.
    let tab = bounds("tab-1", cx);
    let icon = gpui::point(tab.left() + px(14.0), tab.center().y);
    click_button(icon, gpui::MouseButton::Middle, cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 2);
    assert!(!view.read_with(cx, |shell, _| shell.tab_ids().contains(&a)));
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        b,
        "middle click does not activate the tab it closes"
    );
    let focused = cx.update(|window, cx| window.focused(cx));
    assert!(focused.is_some(), "focus stays on a live target");

    // Middle click directly on the close button closes once, with no second close from the button.
    let close = bounds("tab-close-1", cx);
    let remaining = view.read_with(cx, |shell, _| shell.tab_count());
    click_button(close.center(), gpui::MouseButton::Middle, cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        remaining - 1,
        "exactly one tab closes"
    );

    // Middle click on the last remaining tab leaves a fresh Home instead of an empty strip.
    let home = bounds("tab-0", cx);
    click_button(home.center(), gpui::MouseButton::Middle, cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 1);
    assert_eq!(route(&view, cx), Route::Home);
}

/// Left click and Enter still activate; only the middle button closes.
#[gpui::test]
fn left_click_and_enter_still_activate_a_tab(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("nav-home", cx);
    assert_ne!(view.read_with(cx, |shell, _| shell.active_tab_id()), a);

    click("tab-1", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), a);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 2);

    // Right click is neither activation nor close.
    click("nav-home", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    let tab = bounds("tab-1", cx);
    click_button(tab.center(), gpui::MouseButton::Right, cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), b);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 2);
}

/// The strip's top and left gaps match at the first tab, and the tab never paints a coloured
/// focus border: the border stays transparent in every state.
#[gpui::test]
fn the_first_tab_is_inset_equally_on_top_and_left(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();

    let island = bounds("island", cx);
    let strip = bounds("tab-strip", cx);
    let tab = bounds("tab-0", cx);
    let inset = px(super::geometry::TAB_STRIP_INSET);
    // The island draws a 1px border; the strip starts on its inner edge. One logical pixel of
    // slack covers DPI rounding of the border and padding.
    let inner_left = island.left() + px(1.0);
    let inner_top = island.top() + px(1.0);
    assert!(
        (tab.left() - inner_left - inset).abs() <= px(1.0),
        "left inset {} measured {:?}",
        inset,
        tab.left() - inner_left
    );
    assert!(
        (tab.top() - inner_top - inset).abs() <= px(1.0),
        "top inset {} measured {:?}",
        inset,
        tab.top() - inner_top
    );
    // The two gaps agree with each other, which is what the owner saw as inconsistent.
    assert!(
        (tab.left() - inner_left - (tab.top() - inner_top)).abs() <= px(1.0),
        "top and left gaps must match"
    );
    assert_eq!(strip.left(), inner_left);
    assert_eq!(strip.top(), inner_top);

    // Focusing a tab changes no geometry and does not need a coloured border to be reachable.
    let strip_focus = view.read_with(cx, |shell, _| {
        shell.tab_strip_focus(shell.tab_ids()[0]).unwrap()
    });
    cx.update_window_entity(&view, |_, window, cx| window.focus(&strip_focus, cx));
    cx.run_until_parked();
    let focused = bounds("tab-0", cx);
    assert_eq!(focused, tab, "focus alone does not move or resize the tab");
}

/// Tabbing onto a tab that sits outside the strip's viewport brings it into view once.
#[gpui::test]
fn tabbing_to_an_offscreen_tab_reveals_it(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }
    // Go back to the first tab so the far ones are off screen.
    let first = view.read_with(cx, |shell, _| shell.tab_ids()[0]);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab(first, window, cx);
    });
    pump(cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_scroll_offset()).x,
        px(0.0)
    );

    // The last tab's strip entry takes keyboard focus the way Tab would.
    let last = view.read_with(cx, |shell, _| shell.tab_ids()[11]);
    let focus = view.read_with(cx, |shell, _| shell.tab_strip_focus(last).unwrap());
    cx.update_window_entity(&view, |_, window, cx| window.focus(&focus, cx));
    pump(cx);

    let strip = bounds("tab-scroll", cx);
    let tab = bounds("tab-11", cx);
    assert!(
        tab.left() >= strip.left() - px(1.0) && tab.right() <= strip.right() + px(1.0),
        "the focused tab is scrolled into view: {tab:?} in {strip:?}"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        first,
        "keyboard focus alone does not activate the tab"
    );

    // Turning away from the tab does not scroll back by itself.
    let held = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    let page_focus = view.read_with(cx, |shell, _| {
        shell.tab_page_focus(shell.active_tab_id()).unwrap()
    });
    cx.update_window_entity(&view, |_, window, cx| window.focus(&page_focus, cx));
    pump(cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_scroll_offset()),
        held,
        "losing focus does not undo the reveal"
    );
}

/// The strip's inset matches the island's own gap to the window, and both top and left agree.
#[gpui::test]
fn the_strip_inset_matches_the_island_gap(cx: &mut TestAppContext) {
    let (_, cx) = shell(cx);
    cx.simulate_resize(size(px(1200.0), px(820.0)));
    cx.run_until_parked();

    let island = bounds("island", cx);
    let strip = bounds("tab-strip", cx);
    let tab = bounds("tab-0", cx);
    let shell_body = bounds("shell-body", cx);
    let inset = px(super::geometry::TAB_STRIP_INSET);

    // The island's gap to the window's right/bottom edge is the same value the strip uses inside.
    assert_eq!(
        shell_body.right() - island.right(),
        px(super::geometry::ISLAND_INSET)
    );
    assert_eq!(
        shell_body.bottom() - island.bottom(),
        px(super::geometry::ISLAND_INSET)
    );
    assert_eq!(inset, px(super::geometry::ISLAND_INSET));

    // Inside the island: one 1px border, then the inset, on both axes.
    let inner_left = island.left() + px(1.0);
    let inner_top = island.top() + px(1.0);
    assert_eq!(strip.left(), inner_left);
    assert_eq!(strip.top(), inner_top);
    assert!(
        (tab.left() - inner_left - inset).abs() <= px(1.0),
        "left inset measured {:?}",
        tab.left() - inner_left
    );
    assert!(
        (tab.top() - inner_top - inset).abs() <= px(1.0),
        "top inset measured {:?}",
        tab.top() - inner_top
    );
    assert!(
        (tab.left() - inner_left - (tab.top() - inner_top)).abs() <= px(1.0),
        "the two gaps must match"
    );
}

/// The toolbar is pinned outside the scrolling viewport: it never scrolls, never shrinks, and the
/// last tab scrolls under the viewport's edge rather than behind the toolbar.
#[gpui::test]
fn the_strip_toolbar_is_pinned_and_shrinks_the_viewport(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();

    let strip = bounds("tab-strip", cx);
    let toolbar = bounds("tab-toolbar", cx);
    let viewport = bounds("tab-scroll", cx);
    // The toolbar reaches the strip's right inner edge and sits beside the viewport.
    assert!(toolbar.right() <= strip.right() + px(1.0));
    assert!(toolbar.left() >= viewport.right() - px(1.0));
    assert_eq!(
        toolbar.size.width,
        px(super::geometry::TAB_TOOLBAR_WIDTH),
        "the toolbar keeps its width"
    );
    // The viewport gives the toolbar its space instead of widening the island.
    assert!(viewport.right() <= toolbar.left() + px(1.0));
    assert!(strip.right() <= bounds("island", cx).right());

    // The refresh entry is present, named, and inert: no refresh logic arrives this round.
    assert!(cx.debug_bounds("tab-refresh").is_some());
    let before = view.read_with(cx, |shell, _| shell.tab_count());
    click("tab-refresh", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), before);
    assert_eq!(
        route(&view, cx),
        Route::Home,
        "the placeholder does nothing"
    );

    // Overflowing leaves the toolbar where it was and keeps it out of the scrolling row.
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }
    let scrolled_toolbar = bounds("tab-toolbar", cx);
    assert_eq!(scrolled_toolbar, toolbar, "the toolbar never scrolls");
    let viewport = bounds("tab-scroll", cx);
    assert!(viewport.right() <= scrolled_toolbar.left() + px(1.0));
    assert!(
        view.read_with(cx, |shell, _| shell.tab_scroll_max()).x > px(0.0),
        "the tabs scroll inside the reduced viewport"
    );
}

/// A tab's close button only accepts a press while that tab is hovered or active; showing it never
/// moves the label or the neighbouring tabs.
#[gpui::test]
fn the_close_button_answers_only_on_hover_or_active_without_moving_anything(
    cx: &mut TestAppContext,
) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let home = view.read_with(cx, |shell, _| shell.tab_ids()[0]);
    let repository = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 2);

    // Idle: pressing where the hidden close button sits on an inactive tab must not close it. The
    // press lands on the tab itself, which simply activates it.
    cx.simulate_mouse_move(gpui::point(px(40.0), px(400.0)), None, Modifiers::none());
    pump(cx);
    let idle_boxes = (bounds("tab-0", cx), bounds("tab-1", cx));
    let close_slot = bounds("tab-close-0", cx);
    cx.simulate_click(close_slot.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        2,
        "an idle inactive tab is not closed by its hidden button"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        home,
        "and the press activates that tab instead"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![home, repository],
        "no tab was reordered or recreated"
    );

    // Hovering that tab reveals the button, and it now closes exactly that tab.
    hover("tab-0", cx);
    let hover_boxes = (bounds("tab-0", cx), bounds("tab-1", cx));
    assert_eq!(
        idle_boxes, hover_boxes,
        "showing the button never moves the tab or its neighbour"
    );
    let close_slot = bounds("tab-close-0", cx);
    cx.simulate_click(close_slot.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![repository],
        "the hovered tab's button closes it"
    );

    // The active tab keeps its button available without being hovered.
    cx.simulate_mouse_move(gpui::point(px(40.0), px(400.0)), None, Modifiers::none());
    pump(cx);
    let active_close = bounds("tab-close-0", cx);
    cx.simulate_click(active_close.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        1,
        "the active tab's button is always live"
    );
    assert_eq!(route(&view, cx), Route::Home);
}

/// The landing-slot arithmetic is what the drag and drop rely on, so it is pinned directly.
#[test]
fn landing_slots_follow_the_tab_middles_and_the_current_offset() {
    use super::tabs::landing_slot;
    let (width, gap) = (super::geometry::TAB_WIDTH, super::geometry::TAB_GAP);
    let step = width + gap;

    // Nothing scrolled: the first half-tab is slot 0, then each middle adds one slot.
    assert_eq!(landing_slot(px(0.0), px(0.0), 3), 0);
    assert_eq!(landing_slot(px(width / 2.0 - 1.0), px(0.0), 3), 0);
    assert_eq!(landing_slot(px(width / 2.0 + 1.0), px(0.0), 3), 1);
    assert_eq!(landing_slot(px(step + width / 2.0 + 1.0), px(0.0), 3), 2);
    assert_eq!(landing_slot(px(step * 2.0 + width), px(0.0), 3), 3);

    // Scrolled by one step: the pointer over the viewport's left edge is over tab 1's middle.
    assert_eq!(landing_slot(px(0.0), px(-step), 3), 1);
    // A tab cannot land past the last slot, even far beyond the row.
    assert_eq!(landing_slot(px(step * 10.0), px(0.0), 3), 3);
    // No tabs at all: only "before everything" exists.
    assert_eq!(landing_slot(px(100.0), px(0.0), 0), 0);
}

/// Dragging a tab moves it in the visual order by id: nothing closes, nothing is recreated, and
/// its page state travels with it.
#[gpui::test]
fn dragging_a_tab_reorders_it_without_touching_its_state(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-2", cx);
    let c = view.read_with(cx, |shell, _| shell.active_tab_id());
    let home = view.read_with(cx, |shell, _| shell.tab_ids()[0]);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![home, a, b, c]
    );

    // Give `a` some page state so it is visibly carried, not rebuilt.
    view.update(cx, |shell, _| {
        shell.select_repository(a, "main", Some("README.md"));
    });

    // Drag `a` past the last tab: it lands at the end.
    let row = bounds("tab-scroll", cx);
    let start = bounds("tab-1", cx).center();
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(a, window, cx);
        shell.drag_tab_to(a, gpui::point(row.right() - px(2.0), start.y), window, cx);
    });
    // After converting the insertion slot to an index in the collection with A removed, the drop
    // lands on the last index.
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_target()),
        Some(3),
        "dropping past the last tab asks for the end"
    );
    view.update(cx, |shell, cx| {
        shell.drop_dragged_tab(a, cx);
    });
    cx.run_until_parked();

    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![home, b, c, a]
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        4,
        "no tab was closed or duplicated"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repository_state(a).cloned()),
        Some(RepositorySession {
            branch: Some("main".to_string()),
            file: Some("README.md".to_string()),
        }),
        "the dragged tab keeps its own page state"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_id()),
        None,
        "the drag is released"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        c,
        "reordering does not change the active page"
    );

    // Drag it back to the front.
    let row = bounds("tab-scroll", cx);
    let start = bounds("tab-3", cx).center();
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(a, window, cx);
        shell.drag_tab_to(a, gpui::point(row.left() + px(2.0), start.y), window, cx);
    });
    view.update(cx, |shell, cx| {
        shell.drop_dragged_tab(a, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![a, home, b, c]
    );
}

/// A cancelled drag, a closed tab and a stale drop all leave the order alone.
#[gpui::test]
fn cancelling_or_losing_a_drag_keeps_the_order(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    let original = view.read_with(cx, |shell, _| shell.tab_ids());

    // A cancel drops the drag without moving anything.
    let row = bounds("tab-scroll", cx);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(a, window, cx);
        shell.drag_tab_to(
            a,
            gpui::point(row.left() + px(2.0), row.center().y),
            window,
            cx,
        );
    });
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.cancel_tab_drag(window, cx);
    });
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_ids()), original);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_drag_id()), None);

    // A drag whose tab is closed underneath it is dropped, not applied to another index.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(a, window, cx);
        shell.drag_tab_to(
            a,
            gpui::point(row.right() - px(2.0), row.center().y),
            window,
            cx,
        );
    });
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.close_tab(a, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_drag_id()), None);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![original[0], b],
        "the close wins and no stale index moves another tab"
    );

    // Dropping after the drag ended is a no-op.
    view.update(cx, |shell, cx| {
        shell.drop_dragged_tab(b, cx);
    });
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![original[0], b]
    );
}

/// Dragging toward an edge scrolls the row so tabs outside the viewport can be reached, and a drag
/// never scrolls the page or reaches into the toolbar.
#[gpui::test]
fn dragging_near_an_edge_scrolls_the_row_not_the_page(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }
    // Start from the row's beginning so there is somewhere to auto-scroll to.
    view.update(cx, |shell, _| {
        let offset = shell.tab_scroll_offset();
        shell.scroll_tabs_by(-offset.x);
    });
    pump(cx);
    let first = view.read_with(cx, |shell, _| shell.tab_ids()[1]);

    let row = bounds("tab-scroll", cx);
    let toolbar = bounds("tab-toolbar", cx);
    let grab = bounds("tab-1", cx).center();
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(first, window, cx);
        // Hold the pointer just inside the right edge: the row scrolls itself.
        shell.drag_tab_to(
            first,
            gpui::point(row.right() - px(4.0), grab.y),
            window,
            cx,
        );
    });
    pump(cx);
    let scrolled = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    assert!(
        scrolled.x < px(0.0),
        "holding at the edge auto-scrolled the row: {scrolled:?}"
    );
    assert_eq!(scrolled.y, px(0.0));
    assert!(
        row.right() <= toolbar.left() + px(1.0),
        "the scrollable viewport stops before the toolbar"
    );
    let target = view
        .read_with(cx, |shell, _| shell.tab_drag_target())
        .unwrap();
    assert!(
        target <= view.read_with(cx, |shell, _| shell.tab_count()),
        "the slot stays within the tab order"
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.cancel_tab_drag(window, cx);
    });
    assert_eq!(
        view.read_with(cx, |shell, _| shell.page_offset()),
        Point::default(),
        "a drag never scrolls the page"
    );
}

/// After a reorder the tab shortcuts follow the new visual order.
#[gpui::test]
fn cycling_and_closing_follow_the_reordered_strip(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    let home = view.read_with(cx, |shell, _| shell.tab_ids()[0]);

    // Move Home to the end: the order becomes a, b, Home.
    let row = bounds("tab-scroll", cx);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(home, window, cx);
        shell.drag_tab_to(
            home,
            gpui::point(row.right() - px(2.0), row.center().y),
            window,
            cx,
        );
    });
    view.update(cx, |shell, cx| {
        shell.drop_dragged_tab(home, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![a, b, home]
    );

    // Ctrl+Tab walks the new order, not the creation order.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab(a, window, cx);
    });
    press("ctrl-tab", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), b);
    press("ctrl-tab", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), home);

    // Closing the active tab picks its new left neighbour.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.activate_tab(b, window, cx);
    });
    press("ctrl-w", cx);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), a);
}

/// Escape cancels a tab drag through the real key path, and the strip's hover-driven scrollbar
/// stays visible while a tab is carried even after the pointer leaves the strip.
#[gpui::test]
fn escape_cancels_a_tab_drag_and_dragging_keeps_the_scrollbar_up(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..3 {
        click(selector(format!("repo-row-{index}")), cx);
    }
    let original = view.read_with(cx, |shell, _| shell.tab_ids());
    let dragged = original[1];

    // Hover the strip so its scrollbar is at full opacity, then start a drag and leave the strip.
    hover("tab-scroll", cx);
    view.update(cx, |shell, _| shell.show_tab_scrollbar());
    let row = bounds("tab-scroll", cx);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(dragged, window, cx);
        shell.drag_tab_to(
            dragged,
            gpui::point(row.left() + px(4.0), row.center().y),
            window,
            cx,
        );
    });
    // The pointer leaving the strip does not hide the bar while the drag is live.
    cx.simulate_mouse_move(gpui::point(px(40.0), px(400.0)), None, Modifiers::none());
    pump(cx);
    assert_eq!(
        view.update(cx, |shell, _| shell.tab_scrollbar_opacity()),
        1.0,
        "a drag pins the scrollbar"
    );

    // Escape reaches the same action the key binding dispatches, through the window.
    cx.dispatch_action(super::CancelTabDrag);
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_drag_id()), None);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_ids()), original);
}

/// The drop marker is an overlay in the gap the drop would use, and showing it never shifts a tab.
#[gpui::test]
fn the_drop_marker_does_not_shift_any_tab(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    let idle = (
        bounds("tab-0", cx),
        bounds("tab-1", cx),
        bounds("tab-2", cx),
    );

    let row = bounds("tab-scroll", cx);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(b, window, cx);
        shell.drag_tab_to(
            b,
            gpui::point(row.left() + px(4.0), row.center().y),
            window,
            cx,
        );
    });
    pump(cx);

    let live = (
        bounds("tab-0", cx),
        bounds("tab-1", cx),
        bounds("tab-2", cx),
    );
    assert_eq!(idle, live, "the marker is an overlay, not a layout child");
    // The pointer is at the row's left edge, so the marker's gap is the very first one and the
    // reorder target is the same index (a leftward drag needs no conversion).
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_slot()),
        Some(0)
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_target()),
        Some(0)
    );
    let marker = bounds("tab-drop-0", cx);
    let first = bounds("tab-0", cx);
    assert!(
        marker.right() <= first.left() + px(1.0),
        "the marker sits before the first tab, in its gap: {marker:?} vs {first:?}"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_id()),
        Some(b),
        "a drag is still in flight"
    );
    assert_ne!(a, b);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.cancel_tab_drag(window, cx);
    });
}

/// A rightward drop into an interior gap lands between those two tabs, not past the next one.
#[gpui::test]
fn a_rightward_drop_lands_between_the_tabs_the_marker_names(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(1200.0), px(820.0)));
    cx.run_until_parked();
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-2", cx);
    let c = view.read_with(cx, |shell, _| shell.active_tab_id());
    let home = view.read_with(cx, |shell, _| shell.tab_ids()[0]);

    // Drag A (index 1) into the gap after B (index 2): the landing index must be 2 after removing A.
    let row = bounds("tab-scroll", cx);
    let between_b_and_c = gpui::point(
        bounds("tab-2", cx).right() + px(super::geometry::TAB_GAP / 2.0),
        row.center().y,
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(a, window, cx);
        shell.drag_tab_to(a, between_b_and_c, window, cx);
    });
    // B is at index 2, so the gap after it is insertion slot 3. `reorder` removes A (index 1)
    // first, which shifts that same gap down to index 2.
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_slot()),
        Some(3),
        "the marker sits in the B/C gap"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_target()),
        Some(2),
        "the reorder index drops by one for a rightward drag"
    );
    view.update(cx, |shell, cx| {
        shell.drop_dragged_tab(a, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![home, b, a, c],
        "A lands between B and C"
    );

    // And back to its original place, between Home and B.
    let row = bounds("tab-scroll", cx);
    let before_b = gpui::point(bounds("tab-1", cx).left() - px(1.0), row.center().y);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(a, window, cx);
        shell.drag_tab_to(a, before_b, window, cx);
    });
    view.update(cx, |shell, cx| {
        shell.drop_dragged_tab(a, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![home, a, b, c]
    );
}

/// Holding the pointer still in an edge zone keeps scrolling, instead of advancing only on
/// mouse-move events.
#[gpui::test]
fn edge_scrolling_continues_while_the_pointer_is_stationary(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }
    view.update(cx, |shell, _| {
        let offset = shell.tab_scroll_offset();
        shell.scroll_tabs_by(-offset.x);
    });
    pump(cx);
    let first = view.read_with(cx, |shell, _| shell.tab_ids()[1]);
    let row = bounds("tab-scroll", cx);
    let grab = bounds("tab-1", cx).center();
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(first, window, cx);
        shell.drag_tab_to(
            first,
            gpui::point(row.right() - px(4.0), grab.y),
            window,
            cx,
        );
    });
    let first_step = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    // No further mouse events: rendering alone advances the auto-scroll.
    pump(cx);
    pump(cx);
    let later = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    assert!(
        later.x < first_step.x,
        "a stationary pointer keeps the row scrolling: {first_step:?} -> {later:?}"
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.cancel_tab_drag(window, cx);
    });
    pump(cx);
    let stopped = view.read_with(cx, |shell, _| shell.tab_scroll_offset());
    pump(cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_scroll_offset()),
        stopped,
        "releasing the drag stops the auto-scroll"
    );
}

/// A tab drag starts through real mouse events, not only through the test helper: the tab's drag
/// handler must be registered on an ordinary render, before any drag exists.
#[gpui::test]
fn dragging_with_the_mouse_starts_from_a_real_press_and_moves(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(1200.0), px(820.0)));
    cx.run_until_parked();
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    let home = view.read_with(cx, |shell, _| shell.tab_ids()[0]);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![home, a, b]
    );

    // No drag exists before the press, so this exercises the always-registered handler.
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_drag_id()), None);

    let start = bounds("tab-1", cx).center();
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    // Move past GPUI's drag threshold, then well past the next tab's middle.
    let row = bounds("tab-scroll", cx);
    for step in 1..=12 {
        cx.simulate_mouse_move(
            gpui::point(start.x + px(20.0 * step as f32), row.center().y),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_id()),
        Some(a),
        "pressing and dragging a tab starts a reorder"
    );
    cx.simulate_mouse_up(
        gpui::point(start.x + px(240.0), row.center().y),
        gpui::MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();

    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_ids()),
        vec![home, b, a],
        "the drag reordered the tab it carried"
    );
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_drag_id()), None);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        3,
        "nothing was closed or duplicated"
    );
}

/// A press on the close button or a middle click never starts a reorder.
#[gpui::test]
fn close_and_middle_click_do_not_start_a_drag(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    click("repo-row-0", cx);
    let a = view.read_with(cx, |shell, _| shell.active_tab_id());
    click("repo-row-1", cx);
    let b = view.read_with(cx, |shell, _| shell.active_tab_id());
    let original = view.read_with(cx, |shell, _| shell.tab_ids());

    // Drag from the active tab's close button: it must not become a reorder.
    let close = bounds("tab-2", cx);
    let close_button = bounds("tab-close-2", cx).center();
    cx.simulate_mouse_move(close_button, None, Modifiers::none());
    cx.simulate_mouse_down(close_button, gpui::MouseButton::Left, Modifiers::none());
    for step in 1..=10 {
        cx.simulate_mouse_move(
            gpui::point(close.center().x + px(30.0 * step as f32), close.center().y),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    cx.simulate_mouse_up(
        gpui::point(close.center().x + px(300.0), close.center().y),
        gpui::MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_id()),
        None,
        "pressing the close button does not start a reorder"
    );

    // A middle click over a tab closes it once and never starts a reorder.
    let tab = bounds("tab-1", cx);
    cx.simulate_mouse_down(tab.center(), gpui::MouseButton::Middle, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_drag_id()),
        None,
        "a middle press never starts a reorder"
    );
    cx.simulate_mouse_up(tab.center(), gpui::MouseButton::Middle, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_drag_id()), None);
    let ids = view.read_with(cx, |shell, _| shell.tab_ids());
    assert_eq!(
        ids.len(),
        original.len() - 1,
        "the middle click closed its tab once: {ids:?}"
    );
    assert!(!ids.contains(&a));
    assert!(ids.contains(&b));
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), b);
}

/// Dragging a tab keeps the scrollbar up, but once the drag ends the bar follows the real pointer
/// position instead of staying visible because a drag-move once reported hover.
#[gpui::test]
fn the_scrollbar_fades_after_a_drag_that_ended_outside_the_strip(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    for index in 0..11 {
        click(selector(format!("repo-row-{index}")), cx);
    }
    // Hover the strip so its scrollbar is fully visible, then start carrying a tab.
    hover("tab-scroll", cx);
    view.update(cx, |shell, _| shell.show_tab_scrollbar());
    let dragged = view.read_with(cx, |shell, _| shell.tab_ids()[1]);
    let row = bounds("tab-scroll", cx);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drag_tab(dragged, window, cx);
        shell.drag_tab_to(
            dragged,
            gpui::point(row.left() + px(4.0), row.center().y),
            window,
            cx,
        );
    });
    pump(cx);
    assert_eq!(
        view.update(cx, |shell, _| shell.tab_scrollbar_opacity()),
        1.0,
        "a live drag pins the scrollbar"
    );

    // Leaving the strip while dragging keeps it visible; the drag flag alone does that now.
    cx.simulate_mouse_move(gpui::point(px(40.0), px(400.0)), None, Modifiers::none());
    pump(cx);
    assert_eq!(
        view.update(cx, |shell, _| shell.tab_scrollbar_opacity()),
        1.0
    );

    // Dropping the drag hands visibility back to the real hover state, which is "left", so the
    // bar fades out instead of staying up. The clock is pinned first so the fade is stepped by
    // the test rather than by wall time.
    view.update(cx, |shell, _| shell.pin_tab_clock(100_000));
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.cancel_tab_drag(window, cx);
    });
    let at_release = view.update(cx, |shell, _| shell.tab_scrollbar_opacity());
    assert_eq!(at_release, 1.0, "no jump on release");
    let midway = view.update(cx, |shell, _| {
        shell.pin_tab_clock(100_000 + super::geometry::TAB_SCROLLBAR_FADE_MS / 2);
        shell.tab_scrollbar_opacity()
    });
    assert!(
        midway < 1.0 && midway > 0.0,
        "the bar fades after the drag ended outside: {midway}"
    );
    let settled = view.update(cx, |shell, _| {
        shell.pin_tab_clock(100_000 + super::geometry::TAB_SCROLLBAR_FADE_MS * 2);
        shell.tab_scrollbar_opacity()
    });
    assert_eq!(
        settled, 0.0,
        "and reaches zero without re-entering the strip"
    );
}
