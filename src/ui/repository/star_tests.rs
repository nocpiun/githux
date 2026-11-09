//! Star / Unstar behavior over the real service chain and the deterministic fixture.
//!
//! The rule under test is the confirmed one: only a server 204 changes the observed state, the
//! numeric count comes from the authoritative repository summary (never a local +1/-1), a write
//! that times out is only reconciled by a GET (never replayed), and Fork has its own disabled
//! capability.

use std::time::Duration;

use super::{Fixture, intent, pump, settle_ready};
use crate::domain::repository_actions::{RepositoryActionError, StarredState};
use crate::network::transport::HttpMethod;
use crate::repository_modules::fixture::Scene;
use crate::ui::repository::ReaderStatus;

fn settle_star(
    controller: &mut super::super::RepositoryController,
    sessions: &crate::workspace::Sessions,
) {
    pump(controller, sessions, |controller| {
        !controller.star_is_busy() && controller.starred_state().is_some()
    });
}

#[test]
fn star_observes_then_toggles_both_directions_with_authoritative_counts() {
    let fixture = Fixture::new(Scene::StarUnstarred, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    super::settle_ready(&mut controller, &sessions);
    settle_star(&mut controller, &sessions);
    assert_eq!(controller.starred_state(), Some(StarredState::Unstarred));
    let count = controller.details().expect("details").stars;
    assert_eq!(count, 128);

    controller.star_entry();
    assert!(
        controller.star_is_busy(),
        "a submitted write is visibly pending"
    );
    // A second press while the write is pending never submits another one.
    controller.star_entry();
    settle_star(&mut controller, &sessions);
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
    assert_eq!(controller.starred_state(), Some(StarredState::Starred));
    assert_eq!(
        controller.details().expect("details").stars,
        129,
        "the count comes from the authoritative summary, not a local increment"
    );

    controller.star_entry();
    settle_star(&mut controller, &sessions);
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Delete), 1);
    assert_eq!(controller.starred_state(), Some(StarredState::Unstarred));
    assert_eq!(controller.details().expect("details").stars, 128);
    assert_eq!(controller.reader_status(), &ReaderStatus::Showing);
}

#[test]
fn a_rejected_write_keeps_the_server_observation_and_retry_only_reads() {
    let fixture = Fixture::new(Scene::StarRejected, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);
    let reads_before = fixture.transport.star_method_count(HttpMethod::Get);
    settle_star(&mut controller, &sessions);
    assert_eq!(controller.starred_state(), Some(StarredState::Unstarred));
    let count = controller.details().expect("details").stars;

    controller.star_entry();
    pump(&mut controller, &sessions, |controller| {
        !controller.star_is_busy() && controller.star_phase_failure().is_some()
    });
    // No optimistic flip, no invented count, and the failure is actionable.
    assert_eq!(controller.starred_state(), Some(StarredState::Unstarred));
    assert_eq!(controller.details().expect("details").stars, count);
    let puts = fixture.transport.star_method_count(HttpMethod::Put);
    assert_eq!(puts, 1);

    // Retry only re-reads the status; the failed PUT is never replayed.
    controller.retry_star();
    pump(&mut controller, &sessions, |controller| {
        !controller.star_is_busy() && controller.star_phase_failure().is_none()
    });
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
    assert!(fixture.transport.star_method_count(HttpMethod::Get) > reads_before);
    assert_eq!(controller.starred_state(), Some(StarredState::Unstarred));
}

#[test]
fn an_unknown_write_outcome_is_only_reconciled_by_get() {
    let fixture = Fixture::new(Scene::StarTimeoutUnknown, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);
    pump(&mut controller, &sessions, |controller| {
        !controller.star_is_busy() && controller.starred_state().is_some()
    });
    // The fixture's first GET already succeeded; the write and its reconcile time out.
    controller.star_entry();
    pump(&mut controller, &sessions, |controller| {
        !controller.star_is_busy() && controller.star_phase_failure().is_some()
    });
    let puts = fixture.transport.star_method_count(HttpMethod::Put);
    assert_eq!(puts, 1);
    // The reported error is a transport failure, not a claimed success.
    assert!(
        controller
            .star_phase_failure()
            .is_some_and(|message| message.to_lowercase().contains("timed out")
                || message.to_lowercase().contains("timeout")),
        "{:?}",
        controller.star_phase_failure()
    );
    // Retrying sends another GET and never a second PUT.
    let reads = fixture.transport.star_method_count(HttpMethod::Get);
    controller.retry_star();
    std::thread::sleep(Duration::from_millis(100));
    pump(&mut controller, &sessions, |controller| {
        !controller.star_is_busy()
    });
    assert_eq!(fixture.transport.star_method_count(HttpMethod::Put), 1);
    assert!(fixture.transport.star_method_count(HttpMethod::Get) > reads);
    let _ = RepositoryActionError::GitHub(crate::domain::GitHubError::Timeout { seconds: 1 });
}
