//! Deterministic GPUI tests for the reusable Home/Feed view.
//!
//! They cover two independent panels, bounded/virtualized rows, typed Feed
//! fallbacks, empty vs error vs unavailable, Partial honesty, stale binding
//! and generation rejection, isolated scrolling, keyboard selection and
//! one-shot typed callbacks. Native Windows input, IME and real GitHub data
//! remain manual checks recorded in the task devlog.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Entity, Modifiers, MouseButton, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext,
    VisualTestContext, point, px, size,
};

use super::presentation::{feed_summary, home_summary};
use super::rows::event_description;
use super::*;
use crate::domain::error::GitHubError;
use crate::domain::home::{
    FeedActor, FeedEvent, FeedEventType, FeedPartialReason, FeedPayload, FeedReadiness,
    FeedRepository, HomeItem, HomeItemKind, HomePartialReason, HomeReadiness, HomeRepositoryRef,
    HomeSearchOutcome,
};

fn setup(cx: &mut TestAppContext) {
    cx.update(ely_core::init_for_tests);
    cx.update(|cx| {
        ely_core::theme::Theme::update(cx, |theme| {
            theme.font_family = "Noto Sans".into();
            theme.mono_family = "Google Sans Code".into();
        })
    });
}

fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

fn open(cx: &mut TestAppContext) -> (Entity<HomeView>, &mut VisualTestContext) {
    setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        HomeView::new(RequestBinding::new("account-a/home"), 1, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    view.update(cx, |view, cx| {
        view.set_now(
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_793_188_800)),
            cx,
        )
    });
    cx.run_until_parked();
    (view, cx)
}

fn binding() -> RequestBinding {
    RequestBinding::new("account-a/home")
}

fn home_item(index: usize, kind: HomeItemKind) -> HomeItem {
    HomeItem {
        id: 10_000 + index as u64,
        kind,
        number: 100 + index as u64,
        title: if index == 0 {
            "A very long synthetic title that must be ellipsized rather than widen the Home column"
                .to_string()
        } else {
            format!("Synthetic work item {index}")
        },
        html_url: String::new(),
        state: if index.is_multiple_of(2) {
            "open"
        } else {
            "closed"
        }
        .to_string(),
        updated_at: format!("2026-10-{:02}T00:00:00Z", (index % 28) + 1),
        repository: HomeRepositoryRef {
            owner: "owner".to_string(),
            name: format!("repo-{}", index % 4),
        },
    }
}

fn home_outcome(items: Vec<HomeItem>, readiness: HomeReadiness) -> HomeSearchOutcome {
    let total_count = items.len() as u64;
    HomeSearchOutcome {
        items,
        total_count,
        incomplete_results: !readiness.is_complete(),
        readiness,
    }
}

fn feed_event(index: usize, event_type: FeedEventType, payload: FeedPayload) -> FeedEvent {
    FeedEvent {
        id: format!("event-{index}"),
        event_type,
        actor: FeedActor {
            id: 500 + index as u64,
            login: format!("actor-{index}"),
            display_login: None,
            avatar_url: None,
        },
        repository: FeedRepository {
            id: 700 + index as u64,
            name: "owner/repo".to_string(),
            url: String::new(),
        },
        created_at: Some("2026-10-28T00:00:00Z".to_string()),
        public: true,
        payload,
    }
}

fn feed_outcome(events: Vec<FeedEvent>, readiness: FeedReadiness) -> FeedOutcome {
    FeedOutcome {
        events,
        etag: None,
        poll_interval_seconds: Some(60),
        readiness,
    }
}

fn mixed() -> (HomeSearchOutcome, FeedOutcome) {
    let items = (0..6)
        .map(|index| {
            home_item(
                index,
                if index.is_multiple_of(2) {
                    HomeItemKind::Issue
                } else {
                    HomeItemKind::PullRequest
                },
            )
        })
        .collect();
    let events = vec![
        feed_event(
            0,
            FeedEventType::Push,
            FeedPayload::Push {
                git_ref: Some("refs/heads/main".into()),
                before: None,
                head: None,
                size: Some(2),
            },
        ),
        feed_event(
            1,
            FeedEventType::Unknown("FutureEvent".into()),
            FeedPayload::Unknown,
        ),
    ];
    (
        home_outcome(items, HomeReadiness::Complete),
        feed_outcome(events, FeedReadiness::Complete),
    )
}

fn apply(view: &Entity<HomeView>, cx: &mut VisualTestContext) {
    let (home, feed) = mixed();
    view.update(cx, |view, cx| {
        assert!(view.apply_home(&binding(), 1, home, cx));
        assert!(view.apply_feed(&binding(), 1, feed, cx));
    });
    cx.run_until_parked();
}

fn action_sink(
    view: &Entity<HomeView>,
    cx: &mut VisualTestContext,
) -> Rc<RefCell<Vec<HomeAction>>> {
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = actions.clone();
    view.update(cx, |view, _| {
        view.on_action(Rc::new(move |action, _, _| {
            sink.borrow_mut().push(action);
        }));
    });
    actions
}

fn bounds(name: &'static str, cx: &mut VisualTestContext) -> gpui::Bounds<gpui::Pixels> {
    cx.debug_bounds(name)
        .unwrap_or_else(|| panic!("{name} was not drawn"))
}

fn click(name: &'static str, cx: &mut VisualTestContext) {
    let target = bounds(name, cx).center();
    cx.simulate_click(target, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn two_panels_render_their_exact_titles_and_independent_rows(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    cx.simulate_resize(size(px(1180.0), px(800.0)));
    cx.run_until_parked();
    let root = bounds("home-view", cx);
    let left = bounds("home-panel", cx);
    let right = bounds("feed-panel", cx);
    assert!(
        (left.size.width - right.size.width).abs() < px(1.0),
        "{left:?} {right:?}"
    );
    assert!(right.right() <= root.right());
    assert!(cx.debug_bounds("home-panel").is_some());
    assert!(cx.debug_bounds("feed-panel").is_some());
    assert!(cx.debug_bounds("home-item-row-0").is_some());
    assert!(cx.debug_bounds("feed-event-row-0").is_some());
    assert!(cx.debug_bounds("home-summary").is_none());
    assert!(cx.debug_bounds("feed-summary").is_none());
    // The panel headers are fixed, the row area is the bounded region.
    let home_header = bounds("home-panel", cx);
    let home_rows = bounds("home-rows", cx);
    assert!(home_rows.top() > home_header.top());
    assert!(home_rows.bottom() <= home_header.bottom());
}

#[gpui::test]
fn a_failure_in_one_column_does_not_erase_or_block_the_other(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let (home, _) = mixed();
    view.update(cx, |view, cx| {
        assert!(view.apply_home(&binding(), 1, home, cx));
        assert!(view.apply_error(
            Column::Feed,
            &binding(),
            1,
            HomeError::GitHub(GitHubError::Network {
                message: "synthetic offline".into(),
            }),
            cx,
        ));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-item-row-0").is_some());
    assert!(cx.debug_bounds("feed-status").is_some());
    assert!(cx.debug_bounds("feed-event-row-0").is_none());
    // The failed Feed can be replaced without touching Home.
    let (_, feed) = mixed();
    view.update(cx, |view, cx| {
        assert!(view.apply_feed(&binding(), 1, feed, cx));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("feed-event-row-0").is_some());
    assert!(cx.debug_bounds("home-item-row-0").is_some());
}

#[gpui::test]
fn true_empty_error_and_unavailable_are_visually_distinct(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        assert!(view.apply_home(
            &binding(),
            1,
            home_outcome(Vec::new(), HomeReadiness::Complete),
            cx,
        ));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-empty").is_some());
    assert!(cx.debug_bounds("home-status").is_none());

    view.update(cx, |view, cx| {
        assert!(view.apply_error(
            Column::Home,
            &binding(),
            1,
            HomeError::GitHub(GitHubError::Network {
                message: "synthetic offline".into(),
            }),
            cx,
        ));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-empty").is_none());
    assert!(cx.debug_bounds("home-status").is_some());

    view.update(cx, |view, cx| {
        assert!(view.set_unavailable(Column::Home, &binding(), 1, "No cached snapshot", cx,));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-status").is_some());
    assert!(cx.debug_bounds("home-empty").is_none());
}

#[gpui::test]
fn partial_home_keeps_rows_and_explains_that_it_is_not_a_complete_history(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        assert!(
            view.apply_home(
                &binding(),
                1,
                home_outcome(
                    (0..4)
                        .map(|index| home_item(index, HomeItemKind::Issue))
                        .collect(),
                    HomeReadiness::Partial {
                        reasons: vec![
                            HomePartialReason::ResultLimit,
                            HomePartialReason::IncompleteResults,
                        ],
                    },
                ),
                cx,
            )
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-item-row-0").is_some());
    assert!(cx.debug_bounds("home-partial").is_some());
    let summary = view.read_with(cx, |view, _| match view.home_state() {
        PanelState::Ready {
            content: PanelContent::Ready(outcome),
            ..
        } => home_summary(outcome),
        other => panic!("expected a ready Home outcome, got {other:?}"),
    });
    let notice = summary.notice.unwrap();
    assert!(notice.contains("1,000"));
    assert!(notice.contains("not a complete history"));
}

#[gpui::test]
fn a_thousand_search_items_and_three_hundred_events_are_virtualized(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        assert!(
            view.apply_home(
                &binding(),
                1,
                home_outcome(
                    (0..1000)
                        .map(|index| home_item(index, HomeItemKind::Issue))
                        .collect(),
                    HomeReadiness::Complete,
                ),
                cx,
            )
        );
        assert!(
            view.apply_feed(
                &binding(),
                1,
                feed_outcome(
                    (0..300)
                        .map(|index| {
                            feed_event(
                                index,
                                FeedEventType::Watch,
                                FeedPayload::Watch {
                                    action: "started".into(),
                                },
                            )
                        })
                        .collect(),
                    FeedReadiness::Complete,
                ),
                cx,
            )
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-item-row-0").is_some());
    assert!(
        cx.debug_bounds(selector("home-item-row-999".to_string()))
            .is_none(),
        "a far row must not be built"
    );
    assert!(cx.debug_bounds("feed-event-row-0").is_some());
    assert!(
        cx.debug_bounds(selector("feed-event-row-299".to_string()))
            .is_none(),
        "a far Feed row must not be built"
    );
}

#[gpui::test]
fn precise_wheel_scrolls_only_the_targeted_column_exactly_once(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        assert!(
            view.apply_home(
                &binding(),
                1,
                home_outcome(
                    (0..100)
                        .map(|index| home_item(index, HomeItemKind::Issue))
                        .collect(),
                    HomeReadiness::Complete,
                ),
                cx,
            )
        );
        assert!(
            view.apply_feed(
                &binding(),
                1,
                feed_outcome(
                    (0..100)
                        .map(|index| {
                            feed_event(
                                index,
                                FeedEventType::Watch,
                                FeedPayload::Watch {
                                    action: "started".into(),
                                },
                            )
                        })
                        .collect(),
                    FeedReadiness::Complete,
                ),
                cx,
            )
        );
    });
    cx.run_until_parked();
    let home_rows = bounds("home-rows", cx);
    cx.simulate_mouse_move(home_rows.center(), None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: home_rows.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-120.0))),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.offset(Column::Home).y),
        px(-120.0)
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.offset(Column::Feed).y),
        px(0.0),
        "the other column must not move"
    );
}

#[gpui::test]
fn switching_binding_clears_content_selection_and_scroll_before_new_input(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        assert!(
            view.apply_home(
                &binding(),
                1,
                home_outcome(
                    (0..50)
                        .map(|index| home_item(index, HomeItemKind::Issue))
                        .collect(),
                    HomeReadiness::Complete,
                ),
                cx,
            )
        );
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.select(Column::Home, 4, window, cx));
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.selected(Column::Home)),
        Some(4)
    );
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.bind(RequestBinding::new("account-b/home"), 2, window, cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.selected(Column::Home)),
        None
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.offset(Column::Home).y),
        px(0.0)
    );
    assert!(
        view.read_with(cx, |view, _| view.work_item_action(0))
            .is_none()
    );
    assert!(
        !view.update(cx, |view, cx| view.apply_home(
            &binding(),
            1,
            home_outcome(
                vec![home_item(0, HomeItemKind::Issue)],
                HomeReadiness::Complete
            ),
            cx,
        )),
        "an old binding must not overwrite the new one"
    );
    assert!(!view.update(cx, |view, cx| view.apply_error(
        Column::Feed,
        &binding(),
        1,
        HomeError::GitHub(GitHubError::Cancelled),
        cx,
    )));
    assert!(view.read_with(cx, |view, _| matches!(
        view.home_state(),
        PanelState::Ready {
            content: PanelContent::Unavailable { .. },
            ..
        }
    )));
}

#[gpui::test]
fn a_stale_generation_is_rejected_for_both_success_and_error(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        assert!(view.apply_home(
            &binding(),
            1,
            home_outcome(
                vec![home_item(0, HomeItemKind::Issue)],
                HomeReadiness::Complete
            ),
            cx,
        ));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.bind(RequestBinding::new("account-a/home"), 2, window, cx);
        });
    });
    cx.run_until_parked();
    assert!(!view.update(cx, |view, cx| view.apply_home(
        &binding(),
        1,
        home_outcome(
            vec![home_item(1, HomeItemKind::PullRequest)],
            HomeReadiness::Complete
        ),
        cx,
    )));
    assert!(!view.update(cx, |view, cx| view.apply_error(
        Column::Home,
        &binding(),
        1,
        HomeError::GitHub(GitHubError::Timeout { seconds: 5 }),
        cx,
    )));
    assert!(view.update(cx, |view, cx| view.apply_feed(
        &binding(),
        2,
        feed_outcome(Vec::new(), FeedReadiness::Complete),
        cx,
    )));
}

#[gpui::test]
fn keyboard_selection_activates_the_stable_identity_once(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    let actions = action_sink(&view, cx);
    click("home-title-0", cx);
    assert_eq!(actions.borrow().len(), 1, "one click is one activation");
    let first = actions.borrow()[0].clone();
    let HomeAction::ActivateWorkItem(first) = first else {
        panic!("expected a Home work item activation");
    };
    assert_eq!(first.id, 10_000);
    assert_eq!(first.number, 100);
    assert_eq!(first.kind, HomeItemKind::Issue);
    assert_eq!(first.repository.full_name(), "owner/repo-0");

    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(actions.borrow().len(), 2, "Down then Enter activates once");
    let second = actions.borrow()[1].clone();
    let HomeAction::ActivateWorkItem(second) = second else {
        panic!("expected a Home work item activation");
    };
    assert_eq!(second.id, 10_001);
    assert_eq!(second.kind, HomeItemKind::PullRequest);

    // End and Home use saturating arithmetic and keep firing exactly once.
    cx.simulate_keystrokes("end enter home enter");
    cx.run_until_parked();
    assert_eq!(actions.borrow().len(), 4);
    let last = actions.borrow()[2].clone();
    let HomeAction::ActivateWorkItem(last) = last else {
        panic!("expected the last work item");
    };
    assert_eq!(last.id, 10_005);
    let first = actions.borrow()[3].clone();
    let HomeAction::ActivateWorkItem(first) = first else {
        panic!("expected the first work item");
    };
    assert_eq!(first.id, 10_000);
}

#[gpui::test]
fn feed_rows_emit_stable_event_profile_and_repository_actions(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    let actions = action_sink(&view, cx);
    click("feed-actor-0", cx);
    click("feed-avatar-0", cx);
    click("feed-0-repository", cx);
    let actions = actions.borrow();
    assert_eq!(actions.len(), 3, "each control fires exactly once");
    let HomeAction::Profile(first) = &actions[0] else {
        panic!("expected actor profile");
    };
    assert_eq!(first.id, Some(500));
    let HomeAction::Profile(profile) = &actions[1] else {
        panic!("expected a profile action");
    };
    assert_eq!(profile.id, Some(500));
    assert_eq!(profile.login, "actor-0");
    let HomeAction::Repository(repository) = &actions[2] else {
        panic!("expected a repository action");
    };
    assert_eq!(repository.id, Some(700));
    assert_eq!(repository.full_name(), "owner/repo");
}

#[gpui::test]
fn refresh_is_scoped_disabled_while_unavailable_and_cannot_double_fire(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    let actions = action_sink(&view, cx);
    view.update(cx, |view, cx| {
        assert!(view.set_refresh_availability(
            Column::Home,
            &binding(),
            1,
            RefreshAvailability::unavailable("host cooldown"),
            cx,
        ));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.refresh(Column::Home, window, cx));
    });
    assert!(
        actions.borrow().is_empty(),
        "unavailable refresh is ignored"
    );
    view.update(cx, |view, cx| {
        assert!(view.set_refresh_availability(
            Column::Home,
            &binding(),
            1,
            RefreshAvailability::available(),
            cx,
        ));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.refresh(Column::Home, window, cx);
            view.refresh(Column::Home, window, cx);
        });
    });
    assert_eq!(actions.borrow().len(), 1, "refreshing blocks a second fire");
    let HomeAction::Refresh(request) = &actions.borrow()[0] else {
        panic!("expected a refresh request");
    };
    assert_eq!(request.column, Column::Home);
    assert_eq!(request.binding, binding());
    assert_eq!(request.generation, 1);
    assert!(view.read_with(cx, |view, _| matches!(
        view.home_state(),
        PanelState::Ready {
            refreshing: true,
            ..
        }
    )));
    let (home, _) = mixed();
    view.update(cx, |view, cx| {
        assert!(view.apply_home(&binding(), 1, home, cx));
    });
    assert!(!view.read_with(cx, |view, _| view.home_state().refreshing()));
}

#[gpui::test]
fn a_loading_panel_refuses_refresh_until_a_result_arrives(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let actions = action_sink(&view, cx);
    view.update(cx, |view, cx| {
        assert!(view.set_loading(Column::Home, &binding(), 1, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.refresh(Column::Home, window, cx);
            view.refresh(Column::Home, window, cx);
        });
    });
    assert!(
        actions.borrow().is_empty(),
        "a first load owns the request slot"
    );
    let (home, _) = mixed();
    view.update(cx, |view, cx| {
        assert!(view.apply_home(&binding(), 1, home, cx));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.refresh(Column::Home, window, cx));
    });
    assert_eq!(actions.borrow().len(), 1);
}

#[gpui::test]
fn home_and_end_from_no_selection_choose_the_correct_boundary(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    let actions = action_sink(&view, cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.home_view.focus.focus(window, cx));
    });
    cx.simulate_keystrokes("end enter");
    cx.run_until_parked();
    let HomeAction::ActivateWorkItem(last) = actions.borrow()[0].clone() else {
        panic!("expected the last work item");
    };
    assert_eq!(last.id, 10_005);

    // A new snapshot clears selection again, so Home must choose the first row.
    apply(&view, cx);
    cx.simulate_keystrokes("home enter");
    cx.run_until_parked();
    let HomeAction::ActivateWorkItem(first) = actions.borrow()[1].clone() else {
        panic!("expected the first work item");
    };
    assert_eq!(first.id, 10_000);
}

#[gpui::test]
fn narrow_windows_stack_both_columns_inside_the_view(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    cx.simulate_resize(size(px(640.0), px(620.0)));
    cx.run_until_parked();
    let root = bounds("home-view", cx);
    let home = bounds("home-panel", cx);
    let feed = bounds("feed-panel", cx);
    assert!(feed.top() >= home.bottom());
    assert!(home.right() <= root.right() + px(1.0));
    assert!(feed.right() <= root.right() + px(1.0));
    assert!(home.size.width > px(200.0));
    let row = bounds("home-item-row-0", cx);
    assert!(
        row.right() <= home.right() + px(1.0),
        "long titles stay bounded"
    );
}

#[gpui::test]
fn missing_feed_fields_and_unknown_events_have_safe_concise_fallbacks(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let mut event = feed_event(
        0,
        FeedEventType::Unknown("FutureEvent".into()),
        FeedPayload::Unknown,
    );
    event.actor.login = String::new();
    event.actor.display_login = None;
    event.repository.name = String::new();
    event.created_at = None;
    let description = event_description(&event);
    assert!(description.contains("Unknown actor"));
    assert!(description.contains("FutureEvent"));
    assert!(!description.contains('{'), "no raw payload is dumped");
    let missing_payload = feed_event(
        1,
        FeedEventType::PullRequest,
        FeedPayload::PullRequest {
            action: String::new(),
            number: None,
            title: None,
            html_url: None,
            state: None,
            merged: None,
        },
    );
    let description = event_description(&missing_payload);
    assert!(description.contains("updated a pull request"));
    assert!(description.contains("actor-1"));

    view.update(cx, |view, cx| {
        assert!(view.apply_feed(
            &binding(),
            1,
            feed_outcome(vec![event, missing_payload], FeedReadiness::Complete),
            cx,
        ));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("feed-event-row-0").is_some());
    assert!(cx.debug_bounds("feed-event-row-1").is_some());
}

#[test]
fn every_typed_event_kind_has_a_concise_description() {
    let payloads = [
        FeedPayload::Push {
            git_ref: None,
            before: None,
            head: None,
            size: None,
        },
        FeedPayload::PullRequest {
            action: "closed".into(),
            number: Some(1),
            title: None,
            html_url: None,
            state: None,
            merged: Some(true),
        },
        FeedPayload::Issues {
            action: "opened".into(),
            number: None,
            title: None,
            html_url: None,
            state: None,
        },
        FeedPayload::IssueComment {
            action: "created".into(),
            issue_number: None,
            comment_url: None,
        },
        FeedPayload::Watch {
            action: "started".into(),
        },
        FeedPayload::Create {
            ref_type: None,
            git_ref: None,
        },
        FeedPayload::Delete {
            ref_type: None,
            git_ref: None,
        },
        FeedPayload::Fork {
            forkee_full_name: None,
        },
        FeedPayload::Release {
            action: "published".into(),
            tag_name: None,
            name: None,
            html_url: None,
        },
        FeedPayload::Public,
        FeedPayload::Member {
            action: "added".into(),
            member_login: None,
        },
        FeedPayload::Unknown,
    ];
    let kinds = [
        FeedEventType::Push,
        FeedEventType::PullRequest,
        FeedEventType::Issues,
        FeedEventType::IssueComment,
        FeedEventType::Watch,
        FeedEventType::Create,
        FeedEventType::Delete,
        FeedEventType::Fork,
        FeedEventType::Release,
        FeedEventType::Public,
        FeedEventType::Member,
        FeedEventType::Unknown("FutureEvent".into()),
    ];
    for (index, (kind, payload)) in kinds.into_iter().zip(payloads).enumerate() {
        let event = feed_event(index, kind, payload);
        let description = event_description(&event);
        assert!(!description.is_empty());
        assert!(!description.contains('{'), "no raw payload: {description}");
    }
    let merged = feed_event(
        99,
        FeedEventType::PullRequest,
        FeedPayload::PullRequest {
            action: "closed".into(),
            number: Some(7),
            title: Some("Merged work".into()),
            html_url: None,
            state: Some("closed".into()),
            merged: Some(true),
        },
    );
    assert!(event_description(&merged).contains("merged a pull request"));
}

#[test]
fn feed_summary_states_the_events_window_without_claiming_full_history() {
    let outcome = feed_outcome(
        vec![feed_event(0, FeedEventType::Public, FeedPayload::Public)],
        FeedReadiness::Partial {
            reasons: vec![FeedPartialReason::PageLimit],
        },
    );
    let summary = feed_summary(&outcome);
    let notice = summary.notice.unwrap();
    assert!(notice.contains("Partial"));
    assert!(notice.contains("300"));
    assert!(notice.contains("delayed"));
}

#[gpui::test]
fn content_links_use_canonical_logins_and_never_double_dispatch(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    view.update(cx, |view, cx| {
        let mut event = feed_event(
            0,
            FeedEventType::Issues,
            FeedPayload::Issues {
                action: "opened".into(),
                number: Some(8),
                title: Some("Actual issue title".into()),
                html_url: None,
                state: Some("open".into()),
            },
        );
        event.actor.display_login = Some("Display Name".into());
        view.apply_feed(
            &binding(),
            1,
            feed_outcome(vec![event], FeedReadiness::Complete),
            cx,
        );
    });
    cx.run_until_parked();
    let actions = action_sink(&view, cx);
    click("home-0-owner", cx);
    click("home-0-repository", cx);
    click("feed-actor-0", cx);
    click("feed-0-owner", cx);
    click("feed-0-repository", cx);
    click("feed-title-0", cx);
    let actions = actions.borrow();
    assert_eq!(actions.len(), 6);
    assert_eq!(
        actions[0],
        HomeAction::Profile(ProfileIdentity {
            id: None,
            login: "owner".into()
        })
    );
    assert!(
        matches!(&actions[1], HomeAction::Repository(repo) if repo.full_name() == "owner/repo-0")
    );
    assert_eq!(
        actions[2],
        HomeAction::Profile(ProfileIdentity {
            id: Some(500),
            login: "actor-0".into()
        })
    );
    assert_eq!(
        actions[3],
        HomeAction::Profile(ProfileIdentity {
            id: None,
            login: "owner".into()
        })
    );
    assert!(matches!(&actions[4], HomeAction::Repository(repo) if repo.id == Some(700)));
    assert!(matches!(&actions[5], HomeAction::ActivateFeed(event) if event.id == "event-0"));
}

#[gpui::test]
fn refresh_spinner_replaces_the_icon_in_the_same_slot(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    let before = bounds("home-refresh-slot", cx);
    click("home-refresh-slot", cx);
    assert!(cx.debug_bounds("home-refresh-spinner").is_some());
    assert_eq!(before, bounds("home-refresh-slot", cx));
    assert!(cx.debug_bounds("feed-refresh-spinner").is_none());
    let (home, _) = mixed();
    view.update(cx, |view, cx| {
        view.apply_home(&binding(), 1, home, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-refresh-spinner").is_none());
    assert_eq!(before, bounds("home-refresh-slot", cx));
}

#[gpui::test]
fn home_icon_shares_the_title_line_and_metadata_stays_below(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    for dimensions in [size(px(1180.0), px(800.0)), size(px(640.0), px(620.0))] {
        cx.simulate_resize(dimensions);
        cx.run_until_parked();
        let icon = bounds("home-icon-0", cx);
        let title = bounds("home-title-0", cx);
        let metadata = bounds("home-metadata-0", cx);
        assert!((icon.center().y - title.center().y).abs() <= px(1.0));
        assert!(metadata.top() >= title.bottom());
        let owner = bounds("home-0-owner", cx);
        let repo = bounds("home-0-repository", cx);
        assert!(owner.right() <= repo.left());
        assert!(cx.debug_bounds("home-summary").is_none());
        assert!(cx.debug_bounds("feed-summary").is_none());
        assert!(cx.debug_bounds("home-repository-0").is_none());
    }
}

#[gpui::test]
fn variable_feed_cards_have_details_below_header_and_last_card_is_reachable(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        let events = (0..300)
            .map(|index| {
                if index % 2 == 0 {
                    feed_event(
                        index,
                        FeedEventType::Push,
                        FeedPayload::Push {
                            git_ref: Some("refs/heads/main".into()),
                            size: Some(2),
                            before: Some("a".repeat(40)),
                            head: Some("b".repeat(40)),
                        },
                    )
                } else {
                    feed_event(
                        index,
                        FeedEventType::Watch,
                        FeedPayload::Watch {
                            action: "started".into(),
                        },
                    )
                }
            })
            .collect();
        view.apply_feed(
            &binding(),
            1,
            feed_outcome(events, FeedReadiness::Complete),
            cx,
        );
    });
    cx.run_until_parked();
    let header = bounds("feed-header-0", cx);
    let details = bounds("feed-details-0", cx);
    assert!(details.top() > header.bottom());
    assert!(bounds("feed-card-0", cx).size.height > bounds("feed-card-1", cx).size.height);
    assert!(bounds("feed-event-row-0", cx).bottom() <= bounds("feed-event-row-1", cx).top());
    assert!(cx.debug_bounds("feed-event-row-299").is_none());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.feed_view.focus.focus(window, cx));
    });
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.selected(Column::Feed)),
        Some(299)
    );
    let last = bounds("feed-card-299", cx);
    let viewport = bounds("feed-rows", cx);
    assert!(last.top() >= viewport.top() - px(1.0));
    assert!(last.bottom() <= viewport.bottom() + px(1.0));
    assert_eq!(
        view.read_with(cx, |view, _| view.selected(Column::Feed)),
        Some(299)
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.offset(Column::Home).y),
        px(0.0)
    );
}

#[test]
fn merged_status_and_missing_details_are_truthful() {
    let event = feed_event(
        0,
        FeedEventType::PullRequest,
        FeedPayload::PullRequest {
            action: "closed".into(),
            number: Some(12),
            title: Some("Merge me".into()),
            html_url: None,
            state: Some("closed".into()),
            merged: Some(true),
        },
    );
    let details = super::rows::event_details(&event);
    assert_eq!(details.status.as_deref(), Some("Merged"));
    assert!(details.title.unwrap().contains("#12"));
    assert!(details.lines.is_empty());
}

#[gpui::test]
fn content_link_keyboard_activation_is_one_action(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    let actions = action_sink(&view, cx);
    // FocusNext walks native Tab stops. Find the owner link through activation,
    // checking that no list-level Enter handler turns it into an item action.
    for _ in 0..12 {
        cx.update(|window, cx| window.focus_next(cx));
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        cx.run_until_parked();
        if actions
            .borrow()
            .iter()
            .any(|action| matches!(action, HomeAction::Profile(profile) if profile.id.is_none()))
        {
            break;
        }
    }
    let actions = actions.borrow();
    assert!(actions.iter().any(|action| matches!(action, HomeAction::Profile(profile) if profile.login == "owner" && profile.id.is_none())), "{actions:?}");
}

#[gpui::test]
fn one_elapsed_time_task_stops_for_inactive_hidden_and_disposed_views(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.set_now(None, cx);
            view.sync_time_task(window, cx);
            assert!(view.time_task.is_some());
            view.set_active(false, cx);
            assert!(view.time_task.is_none());
            view.set_active(true, cx);
            view.sync_time_task(window, cx);
            assert!(view.time_task.is_some());
        })
    });
    cx.deactivate_window();
    assert!(view.read_with(cx, |view, _| view.time_task.is_none()));
    cx.update(|window, _| window.activate_window());
    cx.simulate_visibility_change(gpui::WindowVisibility::Hidden);
    assert!(view.read_with(cx, |view, _| view.time_task.is_none()));
    let weak = view.downgrade();
    cx.replace_root_view(|_, _| gpui::EmptyView);
    drop(view);
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "the timer holds only a weak view");
}

#[gpui::test]
fn a_retained_unmounted_view_does_not_restart_its_clock_on_activation(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    view.update(cx, |view, cx| view.set_now(None, cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.time_task.is_some()));
    cx.replace_root_view(|_, _| gpui::EmptyView);
    cx.run_until_parked();
    // GPUI retains the prior frame's element state until the next paint.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.time_task.is_none()));
    cx.deactivate_window();
    cx.simulate_visibility_change(gpui::WindowVisibility::Hidden);
    cx.update(|window, _| window.activate_window());
    cx.simulate_visibility_change(gpui::WindowVisibility::Visible);
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.time_task.is_none()));
}
