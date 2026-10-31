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
    Entity, Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, VisualTestContext,
    point, px, size,
};

use super::presentation::{feed_summary, home_summary};
use super::rows::event_description;
use super::*;
use crate::domain::error::GitHubError;
use crate::domain::home::{
    AuthorResolutions, FeedActivity, FeedActivityKind, FeedActivityTarget, FeedActor, FeedAuthor,
    FeedEvent, FeedEventType, FeedPartialReason, FeedPayload, FeedReadiness, FeedRepository,
    FollowedAccount, Following, FollowingReadiness, HomeItem, HomeItemKind, HomePartialReason,
    HomeReadiness, HomeRepositoryRef, HomeSearchOutcome, ResolvedPullRequest, build_feed,
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
        // Home only presents open entries; closed/merged fixtures are added
        // explicitly where a test needs to prove they are rejected.
        state: "open".to_string(),
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

fn author(id: u64, login: &str) -> FeedAuthor {
    FeedAuthor {
        id,
        login: login.to_string(),
        avatar_url: None,
    }
}

/// A followed-account list of every actor/fixture login used by these tests, so
/// the view receives already-certified activities.
fn followed(accounts: &[(&str, u64)]) -> Following {
    Following {
        accounts: accounts
            .iter()
            .map(|(login, id)| FollowedAccount {
                id: *id,
                login: (*login).to_string(),
                avatar_url: None,
            })
            .collect(),
        readiness: FollowingReadiness::Complete,
    }
}

/// Build a visible activity directly, bypassing the host policy, for row/layout
/// tests that are not about the policy itself. The policy is covered by
/// `domain::home::feed_policy_tests` and by the adapter/data tests.
fn activity(
    kind: FeedActivityKind,
    subject: FeedActor,
    event: FeedEvent,
    pull_request: Option<FeedActivityTarget>,
) -> FeedActivity {
    FeedActivity {
        kind,
        subject,
        pull_request,
        event,
    }
}

fn starred_activity(index: usize) -> FeedActivity {
    let event = feed_event(
        index,
        FeedEventType::Watch,
        FeedPayload::Watch {
            action: "started".into(),
        },
    );
    let subject = event.actor.clone();
    activity(FeedActivityKind::RepositoryStarred, subject, event, None)
}

fn created_activity(index: usize) -> FeedActivity {
    let event = feed_event(
        index,
        FeedEventType::Create,
        FeedPayload::Create {
            ref_type: Some("repository".into()),
            git_ref: None,
        },
    );
    let subject = event.actor.clone();
    activity(FeedActivityKind::RepositoryCreated, subject, event, None)
}

/// A merged contribution whose subject is an author different from the event
/// actor (the maintainer who merged it).
fn merged_activity(index: usize, author: FeedAuthor) -> FeedActivity {
    let event = feed_event(
        index,
        FeedEventType::PullRequest,
        FeedPayload::PullRequest {
            action: "merged".into(),
            number: Some(40 + index as u64),
            title: None,
            html_url: None,
            state: None,
            merged: None,
            author: None,
        },
    );
    let subject = FeedActor {
        id: author.id,
        login: author.login,
        display_login: None,
        avatar_url: author.avatar_url,
    };
    activity(
        FeedActivityKind::PullRequestMerged,
        subject,
        event,
        Some(FeedActivityTarget {
            number: Some(40 + index as u64),
            title: Some(format!("Contributor change {index}")),
            html_url: None,
        }),
    )
}

fn feed_outcome(activities: Vec<FeedActivity>, readiness: FeedReadiness) -> FeedOutcome {
    FeedOutcome {
        activities,
        etag: None,
        poll_interval_seconds: Some(60),
        next_page: None,
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
    // One card per retained activity kind, in source order. The merge credits its
    // pull-request author, which is a different identity from the maintainer
    // actor that generated the event.
    let activities = vec![
        starred_activity(0),
        created_activity(1),
        merged_activity(2, author(900, "contributor-2")),
    ];
    (
        home_outcome(items, HomeReadiness::Complete),
        feed_outcome(activities, FeedReadiness::Complete),
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
fn closed_merged_and_unknown_home_states_never_become_rows_or_targets(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let mut items: Vec<HomeItem> = (0..3)
        .map(|index| home_item(index, HomeItemKind::Issue))
        .collect();
    // A retained fixture/partial page that still contains non-open entries: the
    // adapter asks for `is:open`, so this path must reject them too instead of
    // merely trusting the query.
    let mut closed = home_item(3, HomeItemKind::Issue);
    closed.id = 20_003;
    closed.state = "closed".into();
    let mut merged = home_item(4, HomeItemKind::PullRequest);
    merged.id = 20_004;
    merged.state = "closed".into();
    let mut unknown = home_item(5, HomeItemKind::Issue);
    unknown.id = 20_005;
    unknown.state = "mystery".into();
    let mut empty_state = home_item(6, HomeItemKind::PullRequest);
    empty_state.id = 20_006;
    empty_state.state = String::new();
    items.extend([closed, merged, unknown, empty_state]);

    view.update(cx, |view, cx| {
        assert!(view.apply_home(
            &binding(),
            1,
            home_outcome(items, HomeReadiness::Complete),
            cx,
        ));
    });
    cx.run_until_parked();

    let ids: Vec<u64> = view.read_with(cx, |view, _| match view.home_state() {
        PanelState::Ready {
            content: PanelContent::Ready(outcome),
            ..
        } => outcome.items.iter().map(|item| item.id).collect(),
        other => panic!("expected a ready Home outcome, got {other:?}"),
    });
    assert_eq!(ids, vec![10_000, 10_001, 10_002]);
    assert!(cx.debug_bounds("home-item-row-3").is_none());

    // Selection indices refer to the retained list only: the last retained item
    // is index 2 and activation maps to that same identity.
    let actions = action_sink(&view, cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.home_view.focus.focus(window, cx));
    });
    cx.simulate_keystrokes("end enter");
    cx.run_until_parked();
    let HomeAction::ActivateWorkItem(last) = actions.borrow()[0].clone() else {
        panic!("expected the last retained work item");
    };
    assert_eq!(last.id, 10_002);
    assert_eq!(last.number, 102);
    assert_eq!(
        view.read_with(cx, |view, _| view.selected(Column::Home)),
        Some(2)
    );
}

#[gpui::test]
fn a_refresh_that_returns_only_closed_items_clears_the_previous_open_snapshot(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    assert!(cx.debug_bounds("home-item-row-0").is_some());

    let mut closed = home_item(0, HomeItemKind::Issue);
    closed.state = "closed".into();
    view.update(cx, |view, cx| {
        assert!(view.apply_home(
            &binding(),
            1,
            home_outcome(vec![closed], HomeReadiness::Complete),
            cx,
        ));
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("home-item-row-0").is_none(),
        "a closed entry must not let an older open row survive"
    );
    assert!(cx.debug_bounds("home-empty").is_some());
    assert!(
        view.read_with(cx, |view, _| view.work_item_action(0))
            .is_none()
    );
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
        assert!(view.apply_feed(
            &binding(),
            1,
            feed_outcome(
                (0..300).map(starred_activity).collect(),
                FeedReadiness::Complete,
            ),
            cx,
        ));
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
        assert!(view.apply_feed(
            &binding(),
            1,
            feed_outcome(
                (0..100).map(starred_activity).collect(),
                FeedReadiness::Complete,
            ),
            cx,
        ));
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

/// End-to-end view test over the real product policy: live-shaped events go
/// through `build_feed` (kind filter, followed-subject filter, merged-author
/// attribution) and the resulting activities are what the view renders.
#[gpui::test]
fn the_visible_feed_is_the_policy_output_not_the_raw_event_list(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // Source events exactly as the adapter would parse them: a star by a followed
    // account, a star by a non-followed account, a push, a branch creation, a
    // repository creation by a followed account, an unmerged close, and a live
    // `action=merged` event whose author (not actor) is followed.
    let mut events = vec![
        feed_event(
            1,
            FeedEventType::Watch,
            FeedPayload::Watch {
                action: "started".into(),
            },
        ),
        feed_event(
            2,
            FeedEventType::Watch,
            FeedPayload::Watch {
                action: "started".into(),
            },
        ),
        feed_event(
            3,
            FeedEventType::Push,
            FeedPayload::Push {
                git_ref: Some("refs/heads/main".into()),
                before: None,
                head: None,
                size: Some(2),
            },
        ),
        feed_event(
            4,
            FeedEventType::Create,
            FeedPayload::Create {
                ref_type: Some("branch".into()),
                git_ref: Some("feature".into()),
            },
        ),
        feed_event(
            5,
            FeedEventType::Create,
            FeedPayload::Create {
                ref_type: Some("repository".into()),
                git_ref: None,
            },
        ),
        feed_event(
            6,
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                action: "closed".into(),
                number: Some(7),
                title: Some("Abandoned".into()),
                html_url: None,
                state: Some("closed".into()),
                merged: Some(false),
                author: Some(author(901, "contributor-6")),
            },
        ),
        feed_event(
            7,
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                action: "merged".into(),
                number: Some(144),
                title: None,
                html_url: None,
                state: None,
                merged: None,
                author: None,
            },
        ),
    ];
    events[6].repository.name = "subsurface/libdc".to_string();
    // actor-1 and actor-5 are followed; actor-2 is not, so its star is excluded.
    let base = followed(&[("actor-1", 501), ("actor-5", 505)]);
    let mut resolutions = AuthorResolutions::new();
    resolutions.insert(
        "subsurface/libdc#144",
        ResolvedPullRequest {
            author: author(31, "shulaoda"),
            merged: true,
            title: Some("Slim merged contribution".into()),
            html_url: None,
        },
    );
    // `shulaoda` must be followed for the contribution to appear.
    let following = Following {
        accounts: [
            base.accounts.clone(),
            vec![FollowedAccount {
                id: 31,
                login: "shulaoda".into(),
                avatar_url: None,
            }],
        ]
        .concat(),
        readiness: FollowingReadiness::Complete,
    };
    let outcome = build_feed(events, Vec::new(), &following, &resolutions);
    assert_eq!(outcome.activities.len(), 3, "{:?}", outcome.activities);
    assert_eq!(outcome.readiness, FeedReadiness::Complete);

    view.update(cx, |view, cx| {
        assert!(view.apply_feed(&binding(), 1, outcome, cx));
    });
    cx.run_until_parked();
    for index in 0..3 {
        assert!(
            cx.debug_bounds(selector(format!("feed-event-row-{index}")))
                .is_some(),
            "retained card {index} must render"
        );
    }
    assert!(
        cx.debug_bounds("feed-event-row-3").is_none(),
        "no filtered event may render a card or placeholder"
    );
    assert!(cx.debug_bounds("feed-empty").is_none());

    // The credited subject of the merged card is the author, and its link opens
    // the author's profile.
    let actions = action_sink(&view, cx);
    click("feed-actor-2", cx);
    assert_eq!(
        actions.borrow().as_slice(),
        &[HomeAction::Profile(ProfileIdentity {
            id: Some(31),
            login: "shulaoda".into(),
        })],
        "the merge card must link the contributor, not the merging actor"
    );
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.feed_view.focus.focus(window, cx));
    });
    cx.simulate_keystrokes("end enter");
    cx.run_until_parked();
    assert!(
        matches!(actions.borrow().last(), Some(HomeAction::ActivateFeed(identity))
        if identity.id == "event-7" && identity.repository.full_name() == "subsurface/libdc")
    );
}

#[gpui::test]
fn a_policy_empty_feed_is_a_normal_minimal_state(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // Complete source, complete following list, nothing followed did anything.
    view.update(cx, |view, cx| {
        assert!(view.apply_feed(
            &binding(),
            1,
            feed_outcome(Vec::new(), FeedReadiness::Complete),
            cx,
        ));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("feed-empty").is_some());
    assert!(cx.debug_bounds("feed-status").is_none());
    assert!(
        cx.debug_bounds("feed-partial").is_none(),
        "an empty policy result is not a partial read"
    );
    assert!(cx.debug_bounds("feed-event-row-0").is_none());
}

#[gpui::test]
fn an_incomplete_membership_read_shows_a_partial_notice_not_an_empty_feed(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        assert!(view.apply_feed(
            &binding(),
            1,
            feed_outcome(
                Vec::new(),
                FeedReadiness::Partial {
                    reasons: vec![FeedPartialReason::FollowingIncomplete {
                        details: vec!["a later followed-accounts page failed".into()],
                    }],
                },
            ),
            cx,
        ));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("feed-partial").is_some());
    let notice = view.read_with(cx, |view, _| match view.feed_state() {
        PanelState::Ready {
            content: PanelContent::Ready(outcome),
            ..
        } => feed_summary(outcome).notice,
        other => panic!("expected ready Feed, got {other:?}"),
    });
    let notice = notice.expect("partial notice");
    assert!(notice.contains("followed accounts"), "{notice}");
}

#[gpui::test]
fn a_merged_pull_request_credits_the_author_not_the_merging_actor(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // The event actor is the maintainer who clicked merge; the activity subject is
    // the contributor who must be credited and linked.
    let merged = merged_activity(0, author(905, "contributor-login"));
    let description = event_description(&merged);
    assert!(
        description.contains("contributor-login"),
        "the contributor must be credited: {description}"
    );
    assert!(
        !description.contains("actor-0"),
        "the merge actor must not be credited as the contributor: {description}"
    );
    assert!(description.contains("contributed to"));
    assert!(!description.contains('{'), "no raw payload is dumped");

    let actions = action_sink(&view, cx);
    view.update(cx, |view, cx| {
        assert!(view.apply_feed(
            &binding(),
            1,
            feed_outcome(vec![merged], FeedReadiness::Complete),
            cx,
        ));
    });
    cx.run_until_parked();
    let rendered = bounds("feed-actor-0", cx);
    assert!(rendered.size.width > px(0.0));
    click("feed-actor-0", cx);
    let actions = actions.borrow();
    assert_eq!(
        actions.as_slice(),
        &[HomeAction::Profile(ProfileIdentity {
            id: Some(905),
            login: "contributor-login".into(),
        })],
        "the actor link must open the contributor's profile, not the merge actor's"
    );
}

#[test]
fn every_retained_activity_kind_has_a_concise_description() {
    let activities = [
        starred_activity(0),
        created_activity(1),
        merged_activity(2, author(905, "contributor")),
    ];
    for activity in &activities {
        let description = event_description(activity);
        assert!(!description.is_empty());
        assert!(!description.contains('{'), "no raw payload: {description}");
    }
    assert!(event_description(&activities[0]).contains("starred the repository"));
    assert!(event_description(&activities[1]).contains("created the repository"));
    assert!(event_description(&activities[2]).contains("contributed to owner/repo"));
    // No branch/ref/SHA/commit prose appears anywhere in a card description.
    for activity in &activities {
        let description = event_description(activity);
        for forbidden in ["refs/heads", "Before:", "Head:", "commits"] {
            assert!(
                !description.contains(forbidden),
                "{forbidden} must not appear: {description}"
            );
        }
    }
}

#[test]
fn feed_summary_states_the_events_window_without_claiming_full_history() {
    let outcome = feed_outcome(
        vec![starred_activity(0)],
        FeedReadiness::Partial {
            reasons: vec![FeedPartialReason::PageLimit],
        },
    );
    let summary = feed_summary(&outcome);
    let notice = summary.notice.unwrap();
    assert!(notice.contains("Partial"));
    assert!(notice.contains("300"));
    assert!(notice.contains("delayed"));
    // The notice states the followed-account constraint, not a generic event dump.
    assert!(notice.contains("accounts you follow"));

    // A policy-only empty result keeps the source's Complete readiness and
    // therefore adds no partial warning at all.
    let filtered = feed_outcome(Vec::new(), FeedReadiness::Complete);
    assert!(feed_summary(&filtered).notice.is_none());
}

#[gpui::test]
fn content_links_use_canonical_logins_and_never_double_dispatch(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    apply(&view, cx);
    view.update(cx, |view, cx| {
        // A retained activity with a real title, so the Feed card has a content
        // link to activate. The credited subject is the contributor.
        let mut merged = merged_activity(0, author(905, "contributor-login"));
        merged.event.actor.display_login = Some("Display Name".into());
        view.apply_feed(
            &binding(),
            1,
            feed_outcome(vec![merged], FeedReadiness::Complete),
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
            id: Some(905),
            login: "contributor-login".into()
        }),
        "the credited contributor, not the merge actor"
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
        let mut events: Vec<_> = (0..300)
            .map(|index| {
                // A merged contribution keeps the taller card (repository on the
                // header line plus the pull-request inset) next to a shorter star,
                // so the variable-height behaviour is exercised without any
                // branch/ref/SHA prose.
                if index % 2 == 0 {
                    merged_activity(index, author(900 + index as u64, "contributor"))
                } else {
                    starred_activity(index)
                }
            })
            .collect();
        events[0].event.actor.display_login =
            Some("A deliberately long actor display name that wraps within a narrow card".into());
        view.apply_feed(
            &binding(),
            1,
            feed_outcome(events, FeedReadiness::Complete),
            cx,
        );
    });
    cx.run_until_parked();
    cx.simulate_resize(size(px(880.0), px(560.0)));
    cx.run_until_parked();
    let root = bounds("home-view", cx);
    let left = bounds("home-panel", cx);
    let right = bounds("feed-panel", cx);
    assert!(right.left() > left.right());
    assert!((left.size.width - right.size.width).abs() <= px(1.0));
    assert!(right.right() <= root.right());
    // The real scroll viewport reaches the bottom inset, rather than leaving
    // an extra flex-child gap below both lists.
    let viewport = bounds("feed-rows", cx);
    assert!(root.bottom() - viewport.bottom() < left.left() - root.left());
    let header = bounds("feed-header-0", cx);
    let avatar = bounds("feed-avatar-0", cx);
    let actor = bounds("feed-actor-0", cx);
    assert!(avatar.right() < actor.left());
    assert!(actor.right() <= header.right());
    assert!((avatar.center().y - header.center().y).abs() <= px(1.0));
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
    let merged = merged_activity(0, author(905, "contributor"));
    let details = super::rows::event_details(&merged);
    assert_eq!(details.status.as_deref(), Some("Merged"));
    assert!(details.title.unwrap().contains("#40"));
    assert!(details.lines.is_empty(), "no branch/ref/SHA prose");

    // Star / create cards carry no pull-request details, so nothing false is
    // invented for them.
    for activity in [starred_activity(1), created_activity(2)] {
        let details = super::rows::event_details(&activity);
        assert!(details.title.is_none());
        assert!(details.status.is_none());
        assert!(details.lines.is_empty());
    }
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

#[gpui::test]
fn feed_end_of_scroll_emits_one_load_more_per_append(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let actions = action_sink(&view, cx);
    // A short feed with a validated next page: underfilled, so the first render
    // triggers a bounded continuation (an empty/short batch must not strand).
    let mut outcome = feed_outcome(vec![starred_activity(0)], FeedReadiness::Complete);
    outcome.next_page = Some(crate::domain::home::FeedSourceCursor { page: 2 });
    view.update(cx, |view, cx| {
        assert!(view.apply_feed(&binding(), 1, outcome, cx));
    });
    // Ensure layout happens so the underfilled/end check can observe the list.
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    // The view emits at most one request, then the host would mark it loading;
    // here we assert exactly one LoadMoreFeed was emitted for this page.
    let load_more_count = actions
        .borrow()
        .iter()
        .filter(|action| matches!(action, HomeAction::LoadMoreFeed(_)))
        .count();
    assert_eq!(load_more_count, 1, "one end-of-scroll append per page");

    // The host would call feed_append_loading; simulate the in-flight guard by
    // marking loading directly, then repeated renders must not duplicate.
    view.update(cx, |view, cx| {
        assert!(view.feed_append_loading(&binding(), 1, cx));
    });
    cx.run_until_parked();
    assert_eq!(
        actions
            .borrow()
            .iter()
            .filter(|action| matches!(action, HomeAction::LoadMoreFeed(_)))
            .count(),
        1,
        "in-flight guard prevents duplicates from repeated renders"
    );
}

#[gpui::test]
fn feed_append_keeps_selection_and_scroll_anchor(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // A tall first page so the list is actually scrollable, then scroll down to a
    // non-zero offset before appending. The previous bug replaced every item and
    // reset the anchor to the first row.
    let first = feed_outcome(
        (0..40).map(starred_activity).collect(),
        FeedReadiness::Complete,
    );
    view.update(cx, |view, cx| {
        assert!(view.apply_feed(&binding(), 1, first, cx));
    });
    cx.simulate_resize(size(px(800.0), px(500.0)));
    cx.run_until_parked();
    let rows = bounds("feed-rows", cx);
    cx.simulate_mouse_move(rows.center(), None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: rows.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-900.0))),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.select(Column::Feed, 20, window, cx));
    });
    cx.run_until_parked();
    let offset_before = view.read_with(cx, |view, _| view.offset(Column::Feed).y);
    assert!(
        offset_before < px(-100.0),
        "the list must really be scrolled, got {offset_before:?}"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.selected(Column::Feed)),
        Some(20)
    );

    // Append older cards at the tail: selection and the real scroll offset survive.
    let merged = feed_outcome(
        {
            let mut all: Vec<_> = (0..40).map(starred_activity).collect();
            all.push(merged_activity(40, author(905, "contributor")));
            all.push(merged_activity(41, author(906, "contributor")));
            all
        },
        FeedReadiness::Complete,
    );
    view.update(cx, |view, cx| {
        assert!(view.apply_feed_page(&binding(), 1, merged, cx));
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.selected(Column::Feed)),
        Some(20),
        "append must not clear selection"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.offset(Column::Feed).y),
        offset_before,
        "append must preserve the scroll anchor"
    );
}

#[gpui::test]
fn feed_append_failure_shows_warning_and_retries(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let actions = action_sink(&view, cx);
    let mut outcome = feed_outcome(vec![starred_activity(0)], FeedReadiness::Complete);
    outcome.next_page = Some(crate::domain::home::FeedSourceCursor { page: 2 });
    view.update(cx, |view, cx| {
        assert!(view.apply_feed(&binding(), 1, outcome, cx));
        assert!(view.feed_append_failed(&binding(), 1, 2, cx));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("feed-append-failed").is_some());
    // Click-to-retry re-emits exactly one LoadMoreFeed for the same cursor.
    click("feed-append-retry", cx);
    assert!(
        actions
            .borrow()
            .iter()
            .any(|action| matches!(action, HomeAction::LoadMoreFeed(_)))
    );
}
