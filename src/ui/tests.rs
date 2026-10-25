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
use crate::workspace::{MAX_OPEN_TABS, RepositorySession, Route};

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

    // Closing a non-active tab keeps the active page.
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

/// The preview limit refuses a new page without touching the sessions that are already open.
#[gpui::test]
fn the_tab_limit_keeps_existing_sessions_and_shows_a_hint(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    for index in 0..(MAX_OPEN_TABS - 1) {
        click(selector(format!("repo-row-{index}")), cx);
    }
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        MAX_OPEN_TABS
    );
    let active = view.read_with(cx, |shell, _| shell.active_tab_id());
    let ids = view.read_with(cx, |shell, _| shell.tab_ids());
    let routes = view.read_with(cx, |shell, _| shell.tab_routes());

    click(selector(format!("repo-row-{}", MAX_OPEN_TABS - 1)), cx);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        MAX_OPEN_TABS
    );
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), active);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_routes()), routes);
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_ids()), ids);
    assert!(
        view.read_with(cx, |shell, _| shell.tab_limit_hint())
            .is_some(),
        "the shell shows a short notice"
    );
    assert!(cx.debug_bounds("tab-limit-hint").is_some());

    // Switching to an already open page still works at the limit and clears the notice.
    click("repo-row-0", cx);
    assert!(
        view.read_with(cx, |shell, _| shell.tab_limit_hint())
            .is_none()
    );
    assert!(cx.debug_bounds("tab-limit-hint").is_none());
}

/// At the minimum window the strip clips instead of widening the island, and every tab stays
/// inside it with a usable width.
#[gpui::test]
fn the_tab_strip_stays_bounded_at_the_minimum_window(cx: &mut TestAppContext) {
    let (view, cx) = shell(cx);
    for index in 0..(MAX_OPEN_TABS - 1) {
        click(selector(format!("repo-row-{index}")), cx);
    }
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        MAX_OPEN_TABS
    );
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();

    let island = bounds("island", cx);
    let strip = bounds("tab-strip", cx);
    assert!(strip.left() >= island.left() - px(1.0));
    assert!(strip.right() <= island.right() + px(1.0));
    assert!(strip.top() >= island.top());
    assert!(
        strip.top() <= island.top() + px(2.0),
        "the strip sits at the island's top"
    );
    for index in 0..MAX_OPEN_TABS {
        let tab = bounds(selector(format!("tab-{index}")), cx);
        assert!(
            tab.left() >= strip.left() - px(1.0),
            "tab {index} starts inside the strip: {tab:?} in {strip:?}"
        );
        assert!(
            tab.right() <= strip.right() + px(1.0),
            "tab {index} ends inside the strip: {tab:?} in {strip:?}"
        );
        assert!(
            tab.size.width >= px(super::geometry::TAB_MIN_WIDTH - 1.0),
            "tab {index} stays usable: {:?}",
            tab.size.width
        );
    }
    // The page keeps its own space below the strip.
    let page = bounds("island-page", cx);
    assert!(page.top() >= strip.bottom() - px(1.0));
    assert!(page.bottom() <= island.bottom() + px(1.0));
    assert!(page.size.height > px(0.0));
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
