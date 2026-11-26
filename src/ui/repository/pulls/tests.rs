//! Page-level tests for the repository Pull requests tab.
//!
//! These drive the real `RepositoryPage` over the shared repository fixture: the same
//! transport the Issues tab and the preview use. They cover the product promises that
//! only exist once the tab is mounted — filter switch, continuation, retry without losing
//! page one, a confirmed empty list, opening the row as a typed action with the verified
//! identity, and returning to the retained list.

use super::*;
use crate::domain::repository_pulls::PullFilter;
use crate::ui::repository::tests::{frames, normal, settle};
use crate::ui::work_item_row::{Tone, WORK_ITEM_ROW_HEIGHT};
use gpui::{Modifiers, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;

fn open_tab(page: &gpui::Entity<RepositoryPage>, cx: &mut gpui::VisualTestContext) {
    page.update(cx, |page, cx| {
        page.select_tab(super::super::RepositoryTab::PullRequests, cx)
    });
}

fn row_count(page: &gpui::Entity<RepositoryPage>, cx: &mut gpui::VisualTestContext) -> usize {
    page.read_with(cx, |page, _| {
        page.pulls_model().map_or(0, |model| model.rows().len())
    })
}

#[gpui::test]
fn the_open_tab_lists_pages_and_reports_a_typed_open_action_with_the_verified_identity(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);
    let actions = Rc::new(RefCell::new(Vec::new()));
    page.update(cx, |page, _| {
        let actions = actions.clone();
        page.on_action(Rc::new(move |action, _, _| {
            actions.borrow_mut().push(action)
        }));
    });
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    // Page one is two open pull requests, the first of them non-draft.
    assert_eq!(row_count(&page, cx), 2);
    let states: Vec<_> = page.read_with(cx, |page, _| {
        page.pulls_model()
            .unwrap()
            .rows()
            .iter()
            .map(|row| row.state)
            .collect()
    });
    assert_eq!(
        states,
        vec![
            crate::domain::repository_pulls::PullState::Open,
            crate::domain::repository_pulls::PullState::Draft,
        ]
    );
    assert!(cx.debug_bounds("repository-pull-12").is_some());
    assert!(cx.debug_bounds("repository-pull-11").is_some());
    // A visible continuation offers "Load more" instead of claiming the list ended.
    assert!(cx.debug_bounds("repository-pulls-more").is_some());
    assert_eq!(fixture.transport.count("/pulls?"), 1);

    // The row title is the activation target and carries the identity it was listed under.
    let title = cx
        .debug_bounds("repository-pull-title-12")
        .expect("row title");
    cx.simulate_click(title.center(), Modifiers::none());
    assert!(
        matches!(
            &actions.borrow()[..],
            [PageAction::OpenPullRequest { locator, number: 12, .. }]
                if locator.owner() == "octo" && locator.name() == "ledger"
        ),
        "{:?}",
        actions.borrow()
    );
}

#[gpui::test]
fn a_failed_continuation_keeps_page_one_and_retry_continues_from_its_own_cursor(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);
    fixture.switch_scene(crate::repository_modules::fixture::Scene::PullPageFailure);
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    assert_eq!(row_count(&page, cx), 2);

    // Request the next page: the fixture fails it once.
    page.update(cx, |page, cx| page.request_pulls(false, cx));
    settle(&page, cx, |page| {
        page.pulls_model()
            .is_some_and(|model| model.error().is_some())
            && !page.pulls.busy()
    });
    // The rows already read are still there and the tab says why the page failed.
    assert_eq!(row_count(&page, cx), 2);
    let failed = page.read_with(cx, |page, _| page.pulls_failed());
    assert!(failed.as_ref().is_some_and(|(first, _)| !*first));
    assert!(cx.debug_bounds("repository-pulls-retry").is_some());
    assert!(cx.debug_bounds("repository-pull-12").is_some());

    // Retry continues the same page rather than restarting the list.
    page.update(cx, |page, cx| page.retry_pulls(cx));
    settle(&page, cx, |page| {
        page.pulls_model()
            .is_some_and(|model| model.rows().len() == 3 && model.error().is_none())
            && !page.pulls.busy()
    });
    assert_eq!(row_count(&page, cx), 3);
    // One read for page one, the failed page-two read, and the retried page two.
    assert_eq!(fixture.transport.count("/pulls?"), 3);
    assert!(cx.debug_bounds("repository-pulls-retry").is_none());
}

#[gpui::test]
fn a_confirmed_empty_closed_list_is_a_real_empty_result_and_open_stays_separate(
    cx: &mut TestAppContext,
) {
    let (_fixture, page, cx) = normal(cx);
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    page.update(cx, |page, cx| page.filter_pulls(PullFilter::Closed, cx));
    settle(&page, cx, |page| {
        page.pulls_model()
            .is_some_and(|model| model.loaded() && model.filter() == PullFilter::Closed)
            && !page.pulls.busy()
    });
    // Closed keeps merged and closed-without-merge distinguishable.
    let states: Vec<_> = page.read_with(cx, |page, _| {
        page.pulls_model()
            .unwrap()
            .rows()
            .iter()
            .map(|row| row.state)
            .collect()
    });
    assert_eq!(
        states,
        vec![
            crate::domain::repository_pulls::PullState::Merged,
            crate::domain::repository_pulls::PullState::Closed,
            crate::domain::repository_pulls::PullState::Closed,
        ]
    );
    assert!(cx.debug_bounds("repository-pulls-empty").is_none());

    // The scene with no pull requests at all is a confirmed empty result, not a failure.
    let (fixture, page, cx) = normal(cx);
    fixture.switch_scene(crate::repository_modules::fixture::Scene::NoPulls);
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model()
            .is_some_and(|model| model.is_empty_result())
            && !page.pulls.busy()
    });
    assert!(cx.debug_bounds("repository-pulls-empty").is_some());
    assert!(cx.debug_bounds("repository-pulls-retry").is_none());
    assert!(cx.debug_bounds("repository-pulls-more").is_none());
}

#[gpui::test]
fn a_stale_selection_cannot_crash_or_open_a_missing_row(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    let actions = Rc::new(RefCell::new(Vec::new()));
    page.update(cx, |page, _| {
        let actions = actions.clone();
        page.on_action(Rc::new(move |action, _, _| {
            actions.borrow_mut().push(action)
        }));
    });
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    // An index from a previous, longer result set must never index the current rows.
    page.update(cx, |page, _| page.pulls.selected = Some(99));
    page.update_in(cx, |page, window, cx| page.pulls.focus.focus(window, cx));
    cx.simulate_keystrokes("enter");
    frames(cx);
    assert!(actions.borrow().is_empty(), "no row was opened");
    // A filter change replaces the rows, so the host selection is dropped with them.
    page.update(cx, |page, _| page.pulls.selected = Some(1));
    page.update(cx, |page, cx| page.filter_pulls(PullFilter::Closed, cx));
    assert_eq!(page.read_with(cx, |page, _| page.pulls.selected), None);
    settle(&page, cx, |page| {
        page.pulls_model()
            .is_some_and(|model| model.loaded() && model.filter() == PullFilter::Closed)
            && !page.pulls.busy()
    });
    // Keyboard activation on the closed list opens a real closed row, not the old index.
    page.update_in(cx, |page, window, cx| page.pulls.focus.focus(window, cx));
    cx.simulate_keystrokes("end");
    cx.simulate_keystrokes("enter");
    assert!(
        matches!(
            &actions.borrow()[..],
            [PageAction::OpenPullRequest { number, .. }] if *number == 7
        ),
        "{:?}",
        actions.borrow()
    );
}

#[gpui::test]
fn the_draft_row_keeps_the_open_colour_but_reads_draft_visibly(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    page.read_with(cx, |page, _| {
        let rows = page.pulls_model().unwrap().rows();
        let draft = rows.iter().find(|row| row.number == 11).expect("draft row");
        assert_eq!(
            draft.state,
            crate::domain::repository_pulls::PullState::Draft
        );
        let display = draft.row(std::time::SystemTime::now());
        assert_eq!(display.state_label, "Draft");
        // The colour stays the open one; the visible wording comes from the explicit label.
        assert_eq!(
            crate::ui::work_item_row::state_text(Some(display.state_label), Tone::Open),
            "Draft"
        );
        let ready = rows.iter().find(|row| row.number == 12).expect("ready row");
        assert_eq!(
            ready.state,
            crate::domain::repository_pulls::PullState::Open
        );
    });
    assert!(cx.debug_bounds("repository-pull-11-state").is_some());
    assert!(cx.debug_bounds("repository-pull-12-state").is_some());
}

#[gpui::test]
fn the_filter_and_loaded_pages_survive_leaving_the_tab_and_returning(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    page.update(cx, |page, cx| page.request_pulls(false, cx));
    settle(&page, cx, |page| {
        page.pulls_model()
            .is_some_and(|model| model.rows().len() == 3 && model.next().is_none())
            && !page.pulls.busy()
    });
    // Put the list somewhere other than the top before leaving.
    let viewport = cx.debug_bounds("repository-pulls-viewport").unwrap();
    cx.simulate_event(gpui::ScrollEvent {
        position: viewport.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-40.))),
        modifiers: Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    });
    frames(cx);
    let offset = page.read_with(cx, |page, _| {
        page.pulls.list_scroll.0.borrow().base_handle.offset()
    });
    let reads = fixture.transport.count("/pulls?");

    // Switching the page's in-page tab away and back is what the Shell does around a detail tab.
    page.update(cx, |page, cx| {
        page.select_tab(super::super::RepositoryTab::Code, cx)
    });
    frames(cx);
    page.update(cx, |page, _cx| page.suspend_pulls());
    frames(cx);
    open_tab(&page, cx);
    frames(cx);

    page.read_with(cx, |page, _| {
        let model = page.pulls_model().expect("the model is retained");
        assert_eq!(model.filter(), PullFilter::Open);
        assert_eq!(model.rows().len(), 3, "loaded pages are retained");
        assert!(model.next().is_none());
        assert!(
            model
                .scroll()
                .is_some_and(|anchor| anchor.pull_id == model.rows()[0].id),
            "the position was recorded against the top row"
        );
        assert_eq!(
            page.pulls.list_scroll.0.borrow().base_handle.offset().y,
            offset.y,
            "the virtual list kept its own offset"
        );
    });
    // Returning does not re-read: a refresh is an explicit action.
    assert_eq!(fixture.transport.count("/pulls?"), reads);
    assert!(cx.debug_bounds("repository-pull-10").is_some());
}

#[gpui::test]
fn switching_the_filter_discards_the_previous_rows_and_position(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    assert_eq!(row_count(&page, cx), 2);
    page.update(cx, |page, cx| page.filter_pulls(PullFilter::Closed, cx));
    // The old open rows are not shown while the closed list loads.
    assert_eq!(row_count(&page, cx), 0);
    settle(&page, cx, |page| {
        page.pulls_model()
            .is_some_and(|model| model.loaded() && model.filter() == PullFilter::Closed)
            && !page.pulls.busy()
    });
    assert!(cx.debug_bounds("repository-pull-12").is_none());
    assert!(cx.debug_bounds("repository-pull-9").is_some());
    // Selecting the same filter again is not a reload and starts no request.
    let reads = fixture.transport.count("/pulls?");
    assert!(!page.read_with(cx, |page, _| page.pulls.busy()));
    page.update(cx, |page, cx| page.filter_pulls(PullFilter::Closed, cx));
    frames(cx);
    assert!(!page.read_with(cx, |page, _| page.pulls.busy()));
    assert_eq!(fixture.transport.count("/pulls?"), reads);
    assert!(cx.debug_bounds("repository-pull-9").is_some());
}

#[gpui::test]
fn switching_the_filter_returns_the_real_list_to_the_top(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    // Simulate a long list that was scrolled away from the top before the filter changed.
    page.update(cx, |page, _| {
        page.pulls.motion.set_offset(gpui::point(
            gpui::px(0.0),
            gpui::px(-WORK_ITEM_ROW_HEIGHT * 2.0),
        ));
    });
    frames(cx);
    assert!(
        page.read_with(cx, |page, _| {
            page.pulls.list_scroll.0.borrow().base_handle.offset().y
        }) < gpui::px(0.0)
    );
    page.update(cx, |page, cx| page.filter_pulls(PullFilter::Closed, cx));
    // The replacement list starts at its own top, not the previous filter's offset.
    assert_eq!(
        page.read_with(cx, |page, _| {
            page.pulls.list_scroll.0.borrow().base_handle.offset().y
        }),
        gpui::px(0.0)
    );
    settle(&page, cx, |page| {
        page.pulls_model()
            .is_some_and(|model| model.loaded() && model.filter() == PullFilter::Closed)
            && !page.pulls.busy()
    });
    assert_eq!(
        page.read_with(cx, |page, _| {
            page.pulls.list_scroll.0.borrow().base_handle.offset().y
        }),
        gpui::px(0.0),
        "the loaded closed list still starts at the top"
    );
}

#[gpui::test]
fn an_account_change_clears_the_rows_and_keeps_the_selected_filter(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    open_tab(&page, cx);
    settle(&page, cx, |page| {
        page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
    });
    assert_eq!(row_count(&page, cx), 2);
    fixture.login_as(2, "other-user");
    page.update_in(cx, |page, window, cx| page.drain(window, cx));
    page.read_with(cx, |page, _| {
        // Rows read under the previous authorization never stay on screen.
        assert!(
            page.pulls_model()
                .is_none_or(|model| model.rows().is_empty())
        );
        assert_eq!(page.pulls.filter(), PullFilter::Open);
    });
}
