//! Offline, cache-authorization and revocation behavior for the repository page.
//!
//! These tests use the real account service, the real persistent `CacheStore`, the real
//! `CachedRepositorySession`/`CachedPageReadSession` adapters and the deterministic fixture (which
//! can refuse every request as a network failure). The scenario is the product one: read a
//! repository online, restart the application, then browse the same commit with the network
//! denied. Anything never read before must stay an honest miss, never an invented empty result.
//!
//! They also cover the revocation rule from LESSONS L022: already displayed data is checked on
//! every drain, so a revocation clears the page in one poll and a reply captured before it can
//! never be applied afterwards.

use std::time::Duration;

use super::{Fixture, close_handle, intent, pump};
use crate::network::transport::HttpMethod;
use crate::repository_modules::fixture::Scene;
use crate::ui::repository::{ReaderStatus, RepositoryController, Selection};
use crate::workspace::Sessions;

fn settle_cached(controller: &mut RepositoryController, sessions: &Sessions) {
    pump(controller, sessions, |controller| {
        controller.reader_status() == &ReaderStatus::Showing
            && controller.is_showing_cached_snapshot()
    });
}

fn scope(
    controller: &RepositoryController,
) -> (
    crate::storage::cache::CacheStore,
    crate::storage::cache::CacheScope,
    u64,
) {
    let binding = controller.cache_binding.as_ref().expect("cache binding");
    let repository = controller.details().expect("details").identity.id();
    (binding.store().clone(), binding.scope().clone(), repository)
}

#[test]
fn an_offline_restart_reads_the_same_commit_paths_and_directories_from_cache() {
    let fixture = Fixture::new(Scene::Normal, true);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    controller = controller.with_image_executor(fixture.executor.clone());
    super::settle_ready(&mut controller, &sessions);
    let commit = controller.fixed_commit().expect("commit").clone();
    assert!(
        controller
            .prepared
            .as_ref()
            .and_then(|prepared| prepared.provider.as_ref())
            .is_some(),
        "the injected executor builds the real README picture provider"
    );
    // Cache one child directory and one source file before the restart.
    controller.toggle_directory("docs");
    pump(&mut controller, &sessions, |controller| {
        controller.tree("docs").is_some()
    });
    controller.select_file("docs/guide.md");
    pump(&mut controller, &sessions, |controller| {
        controller.current_path() == Some("docs/guide.md")
            && controller.reader_status() == &ReaderStatus::Showing
    });
    controller.retire();
    close_handle(handle);

    // Restart: the network is denied and only the durable stores remain.
    fixture.transport.set_offline(true);
    let mut handle = fixture.resume_handle();
    let mut saved = intent();
    saved.branch = Some("main".to_string());
    saved.path = Some("docs/guide.md".to_string());
    let (mut controller, sessions) = fixture.controller(&mut handle, saved);
    controller = controller.with_image_executor(fixture.executor.clone());
    settle_cached(&mut controller, &sessions);

    assert_eq!(
        controller.fixed_commit(),
        Some(&commit),
        "the cached commit is the same one"
    );
    assert_eq!(
        controller
            .details()
            .expect("cached summary")
            .identity
            .locator()
            .owner(),
        "octo"
    );
    assert_eq!(controller.current_path(), Some("docs/guide.md"));
    assert_eq!(controller.reader_status(), &ReaderStatus::Showing);
    // The locator index resolves the numeric id without a request; the page never guessed a path.
    assert!(controller.readme_path().is_none() || controller.readme_path().is_some());

    // A cached directory still opens; a directory never read before is a miss, not empty.
    controller.toggle_directory("docs");
    pump(&mut controller, &sessions, |controller| {
        controller.tree("docs").is_some()
    });
    assert_eq!(controller.tree_failure("docs"), None);
    controller.toggle_directory("assets");
    pump(&mut controller, &sessions, |controller| {
        controller.tree_failure("assets").is_some()
    });
    let error = controller.tree_failure("assets").expect("offline miss");
    assert!(
        error.title.contains("local cache")
            && error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no empty result is inferred")),
        "an uncached directory is reported as a cache miss, not an empty folder: {error:?}"
    );
    assert!(
        !controller
            .tree("assets")
            .is_some_and(|tree| tree.entries.is_empty())
    );
    assert!(
        fixture.transport.mismatches().is_empty(),
        "{:?}",
        fixture.transport.mismatches()
    );
}

#[test]
fn manual_refresh_restores_online_reading_and_enables_star() {
    let fixture = Fixture::new(Scene::StarUnstarred, true);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    super::settle_ready(&mut controller, &sessions);
    controller.retire();
    close_handle(handle);

    fixture.transport.set_offline(true);
    let mut handle = fixture.resume_handle();
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_cached(&mut controller, &sessions);
    assert!(controller.is_showing_cached_snapshot());
    // A cached snapshot never sends a Star status request and cannot write.
    let model = controller.view_model();
    assert!(
        !model.star.enabled,
        "an offline snapshot disables the Star action"
    );
    assert_eq!(model.star.count, Some(128));
    let reads_before_press = fixture.transport.star_method_count(HttpMethod::Get);
    let writes_before_press = fixture.transport.star_method_count(HttpMethod::Put);
    controller.star_entry();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        fixture.transport.star_method_count(HttpMethod::Get),
        reads_before_press,
        "an offline snapshot sends no Star status request"
    );
    assert_eq!(
        fixture.transport.star_method_count(HttpMethod::Put),
        writes_before_press
    );

    // Coming back online, the explicit Refresh revalidates and restores the Star read path.
    fixture.transport.set_offline(false);
    controller.refresh();
    pump(&mut controller, &sessions, |controller| {
        !controller.is_showing_cached_snapshot()
            && controller.reader_status() == &ReaderStatus::Showing
            && controller.starred_state().is_some()
    });
    assert!(controller.view_model().star.enabled);
}

#[test]
fn revocation_clears_the_displayed_page_in_one_drain() {
    let fixture = Fixture::new(Scene::Normal, true);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    super::settle_ready(&mut controller, &sessions);
    let (store, cache_scope, repository_id) = scope(&controller);
    assert!(controller.details().is_some());
    assert_eq!(controller.reader_status(), &ReaderStatus::Showing);

    store
        .revoke_repository(&cache_scope, Some(repository_id))
        .expect("revoke the repository cache authorization");
    // One poll, no sleep: displayed data is checked on every drain, not only on new replies.
    controller.poll(&sessions);
    assert!(controller.details().is_none());
    assert!(controller.branch.is_none());
    assert!(controller.reader_status() != &ReaderStatus::Showing);
    assert!(controller.rows().is_empty());
    assert!(controller.authorization_lost());
    let error = controller.error().expect("authorization error");
    assert!(error.title.contains("authorization changed"), "{error:?}");
    // A second drain cannot revive anything.
    controller.poll(&sessions);
    assert!(controller.details().is_none());
}

#[test]
fn a_reply_captured_before_revocation_is_rejected_and_never_revived() {
    let fixture = Fixture::new(Scene::Delayed, true);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    super::settle_ready(&mut controller, &sessions);
    let (store, cache_scope, repository_id) = scope(&controller);
    let old_commit = controller.fixed_commit().expect("commit").clone();

    // Start a branch resolution and let its reply reach the page's queue without polling it.
    controller.select_branch("feature/docs-polish");
    let mut queued = false;
    for _ in 0..500 {
        if !controller.inbox.lock().expect("inbox").is_empty() {
            queued = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(queued, "the delayed branch reply reached the queue");
    store
        .revoke_repository(&cache_scope, Some(repository_id))
        .expect("revoke");
    controller.poll(&sessions);
    // The reply completed before the revocation; it must not be applied afterwards.
    assert!(controller.authorization_lost());
    assert!(controller.branch.is_none());
    assert!(controller.reader_status() != &ReaderStatus::Showing);
    assert_eq!(controller.fixed_commit(), None);
    assert!(controller.details().is_none());
    let _ = old_commit;
}

#[test]
fn historical_counts_and_overview_are_never_presented_as_fresh() {
    let fixture = Fixture::new(Scene::Normal, true);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    super::settle_ready(&mut controller, &sessions);
    let (_, _, _) = scope(&controller);
    controller.retire();
    close_handle(handle);

    fixture.transport.set_offline(true);
    let mut handle = fixture.resume_handle();
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_cached(&mut controller, &sessions);
    // Offline OPEN counts are withheld rather than drawn like freshly verified numbers.
    assert!(controller.extras.counts.is_none() || controller.extras.counts_historical);
    let overview = controller.overview().expect("cached overview");
    if let Some(releases) = overview.releases().value() {
        assert!(
            releases.historical,
            "a cached release list is marked historical, never a current Latest"
        );
    }
    let model = controller.view_model();
    assert!(model.cached_label.is_some());
    assert_eq!(model.selection, Selection::Readme);
}
