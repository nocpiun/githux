//! Whole-page Star tests. They use the same real RepositoryPage, fixture and
//! preview wiring as the existing tree/reader tests, not an isolated button.

use std::time::Duration;

use super::super::StarPhase;
use super::Fixture;
use super::{
    ReaderStatus, RepositoryNavigation, RepositoryPage, activate, frames, normal, open, settle,
};
use crate::domain::repository_actions::StarredState;
use crate::network::transport::HttpMethod;
use crate::repository_modules::fixture::Scene;
use gpui::{Entity, TestAppContext, VisualTestContext};

fn ready(
    cx: &mut TestAppContext,
    scene: Scene,
) -> (Fixture, Entity<RepositoryPage>, &mut VisualTestContext) {
    let fixture = Fixture::new(scene);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing && page.starred_state().is_some()
    });
    (fixture, page, cx)
}

fn click_star(_page: &Entity<RepositoryPage>, cx: &mut VisualTestContext) {
    frames(cx);
    let star = cx.debug_bounds("repository-star").expect("Star button");
    cx.simulate_click(star.center(), gpui::Modifiers::none());
}

fn wait_star(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext, state: StarredState) {
    settle(page, cx, |page| {
        page.starred_state() == Some(state) && !page.star_is_busy()
    });
}

#[gpui::test]
fn initial_starred_and_unstarred_bidirectional_writes_and_authoritative_counts(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = ready(cx, Scene::StarUnstarred);
    assert_eq!(
        page.read_with(cx, |page, _| page.starred_state()),
        Some(StarredState::Unstarred)
    );
    assert!(!page.read_with(cx, |page, _| page.fork_mutations_enabled()));
    click_star(&page, cx);
    // The busy state suppresses a rapid second click before the response.
    assert!(page.read_with(cx, |page, _| page.star_is_busy()));
    click_star(&page, cx);
    wait_star(&page, cx, StarredState::Starred);
    assert_eq!(
        page.read_with(cx, |page, _| page.details().unwrap().stars),
        129
    );
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
    click_star(&page, cx);
    wait_star(&page, cx, StarredState::Unstarred);
    assert_eq!(
        page.read_with(cx, |page, _| page.details().unwrap().stars),
        128
    );
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Delete), 1);
    assert!(fixture.transport.mismatches().is_empty());

    let (fixture, page, cx) = ready(cx, Scene::StarStarred);
    assert_eq!(
        page.read_with(cx, |page, _| page.starred_state()),
        Some(StarredState::Starred)
    );
    click_star(&page, cx);
    wait_star(&page, cx, StarredState::Unstarred);
    assert_eq!(
        page.read_with(cx, |page, _| page.details().unwrap().stars),
        127
    );
    assert!(fixture.transport.mismatches().is_empty());
}

#[gpui::test]
fn rejection_preserves_server_state_and_warning_retries_only_with_get(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::StarRejected);
    click_star(&page, cx);
    settle(&page, cx, |page| {
        !page.star_is_busy() && matches!(page.star.phase, StarPhase::Failed { .. })
    });
    assert_eq!(
        page.read_with(cx, |page, _| page.starred_state()),
        Some(StarredState::Unstarred)
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.details().unwrap().stars),
        128
    );
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
    page.update(cx, |page, cx| page.retry_star(cx));
    wait_star(&page, cx, StarredState::Unstarred);
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
}

#[gpui::test]
fn timeouts_reconcile_applied_and_unapplied_writes_without_replay(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::StarTimeoutApplied);
    click_star(&page, cx);
    wait_star(&page, cx, StarredState::Starred);
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
    assert!(fixture.transport.star_method_count(HttpMethod::Get) >= 2);

    let (fixture, page, cx) = ready(cx, Scene::StarTimeoutNotApplied);
    click_star(&page, cx);
    wait_star(&page, cx, StarredState::Unstarred);
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
    assert!(fixture.transport.star_method_count(HttpMethod::Get) >= 2);
}

#[gpui::test]
fn unknown_outcome_does_not_fake_a_state_or_replay_on_retry(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::StarTimeoutUnknown);
    click_star(&page, cx);
    settle(&page, cx, |page| {
        matches!(page.star.phase, StarPhase::Failed { .. })
    });
    assert_eq!(
        page.read_with(cx, |page, _| page.starred_state()),
        Some(StarredState::Unstarred)
    );
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
    page.update(cx, |page, cx| page.retry_star(cx));
    settle(&page, cx, |page| {
        matches!(page.star.phase, StarPhase::Failed { .. })
    });
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
}

#[gpui::test]
fn account_a_b_a_and_close_cancel_old_star_publication(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::StarUnstarred);
    click_star(&page, cx);
    fixture.login_as(8, "other");
    fixture.login_as(1, "octo");
    for _ in 0..50 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(page.read_with(cx, |page, _| page.starred_state()), None);
    page.update(cx, |page, cx| page.rebind(fixture.services(), cx));
    settle(&page, cx, |page| page.starred_state().is_some());
    click_star(&page, cx);
    page.update(cx, |page, cx| page.deactivate(cx));
    for _ in 0..50 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(page.read_with(cx, |page, _| page.starred_state()), None);
    assert_eq!(page.read_with(cx, |page, _| page.queued_star_results()), 0);
}

#[gpui::test]
fn switching_branch_does_not_cancel_star_and_old_repository_cannot_publish(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = ready(cx, Scene::StarUnstarred);
    click_star(&page, cx);
    page.update(cx, |page, cx| page.select_branch("feature/docs-polish", cx));
    wait_star(&page, cx, StarredState::Starred);
    assert_eq!(
        page.read_with(cx, |page, _| page.details().unwrap().stars),
        129
    );
    let binding = page.read_with(cx, |page, _| page.repository_binding().unwrap());
    page.update_in(cx, |page, window, cx| {
        page.repository_action(true, binding, window, cx)
    });
    page.update(cx, |page, cx| page.deactivate(cx));
    assert_eq!(page.read_with(cx, |page, _| page.starred_state()), None);
    assert!(fixture.transport.mismatches().is_empty());
}

#[gpui::test]
fn summary_retry_restores_count_with_get_only_and_keeps_warning_until_success(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = ready(cx, Scene::StarSummaryFailure);
    click_star(&page, cx);
    settle(&page, cx, |page| {
        matches!(page.star.phase, StarPhase::Failed { .. })
    });
    assert_eq!(
        page.read_with(cx, |page, _| page.starred_state()),
        Some(StarredState::Starred)
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.details().unwrap().stars),
        128,
        "the last authoritative count is not presented as refreshed"
    );
    let writes = fixture.transport.star_method_count(HttpMethod::Put);
    page.update(cx, |page, cx| page.retry_star(cx));
    settle(&page, cx, |page| {
        matches!(page.star.phase, StarPhase::Ready) && page.details().unwrap().stars == 129
    });
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), writes);
    assert!(fixture.transport.star_method_count(HttpMethod::Get) >= 2);
}

#[gpui::test]
fn an_older_repository_summary_cannot_roll_back_a_write_or_its_count(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = ready(cx, Scene::StarUnstarred);
    let old = page.read_with(cx, |page, _| page.details().cloned().unwrap());
    click_star(&page, cx);
    wait_star(&page, cx, StarredState::Starred);
    assert_eq!(
        page.read_with(cx, |page, _| page.details().unwrap().stars),
        129
    );
    page.update_in(cx, |page, window, cx| {
        let message = super::super::Message {
            generation: page.generation,
            branch_request: page.branch_request,
            authorization: None,
            context: page.context.clone().unwrap(),
            preliminary: false,
            body: super::super::Body::Repository {
                result: Ok((old, None)),
                star_write_generation: 0,
            },
        };
        page.apply(message, window, cx);
    });
    assert_eq!(
        page.read_with(cx, |page, _| page.starred_state()),
        Some(StarredState::Starred)
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.details().unwrap().stars),
        129
    );
}

#[gpui::test]
fn an_accepted_star_reply_notifies_render_without_a_forced_refresh(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = ready(cx, Scene::StarUnstarred);
    click_star(&page, cx);
    wait_star(&page, cx, StarredState::Starred);
    let observed = std::rc::Rc::new(std::cell::Cell::new(false));
    let sink = observed.clone();
    cx.update(|_, cx| {
        cx.observe(&page, move |_, _| sink.set(true)).detach();
    });
    page.update_in(cx, |page, window, cx| {
        let message = super::super::StarReply {
            binding: page.star_binding().unwrap(),
            context: page.context.clone().unwrap(),
            request: page.star_request,
            body: super::super::StarReplyBody::Changed(Box::new(Ok(
                crate::domain::repository_actions::StarActionOutcome {
                    observation: crate::domain::repository_actions::StarObservation {
                        repository: page.details().unwrap().identity.clone(),
                        state: StarredState::Starred,
                    },
                    summary: Ok(page.details().cloned().unwrap()),
                    confirmation: crate::domain::repository_actions::StarConfirmation::Observed,
                },
            ))),
        };
        // Reconstruct the same accepted-work identity, then drain without any
        // explicit window.refresh() or draw call in this regression.
        page.star_work = Some(super::super::StarWork {
            service: super::super::RepositoryActionService::new(
                page.session.as_ref().unwrap(),
                &page.services.accounts,
            )
            .unwrap(),
            work: super::super::Work::new(),
            binding: message.binding.clone(),
            request: message.request,
        });
        page.star_inbox.lock().unwrap().push_back(message);
        page.drain(window, cx);
    });
    assert!(
        observed.get(),
        "an accepted Star reply must invalidate the page"
    );
}

#[gpui::test]
fn normal_preview_fixture_supports_repeated_star_and_unstar(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    settle(&page, cx, |page| page.starred_state().is_some());
    for expected in [
        StarredState::Starred,
        StarredState::Unstarred,
        StarredState::Starred,
    ] {
        click_star(&page, cx);
        wait_star(&page, cx, expected);
    }
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 2);
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Delete), 1);
}
