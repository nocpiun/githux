//! Current-request failure indicators over the real persistent-cache page composition.
use super::*;
use crate::ui::repository_overview::SectionState;

const MARKERS: [&str; 6] = [
    "repository-cached-badge",
    "repository-branches-cached",
    "repository-code-cached",
    "overview-releases-cached",
    "overview-contributors-cached",
    "overview-languages-cached",
];
fn drive(
    page: &Entity<RepositoryPage>,
    cx: &mut VisualTestContext,
    mut ready: impl FnMut(&Entity<RepositoryPage>, &mut VisualTestContext) -> bool,
) {
    let end = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        frames(cx);
        if ready(page, cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < end,
            "indicator page did not settle"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn observed(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext, historical: bool) -> bool {
    page.read_with(cx, |p, cx| {
        p.overview().is_some_and(|v| {
            let v = v.read(cx);
            v.releases().value().is_some()
                && v.latest_release().and_then(SectionState::value).is_some()
                && v.contributors().value().is_some()
                && v.languages().value().is_some()
                && (!historical
                    || (v.releases().is_historical()
                        && v.contributors().is_historical()
                        && v.languages().is_historical()))
        })
    })
}
fn warm(cx: &mut TestAppContext, f: &CachedFixture) {
    let (host, page, cx) = open_cached(cx, f, RepositoryNavigation::default());
    drive(&page, cx, |p, cx| {
        p.read_with(cx, |p, _| p.reader_status() == &ReaderStatus::Showing)
            && observed(p, cx, false)
    });
    release_page(&host, cx);
}
fn quiet(cx: &mut VisualTestContext) {
    for id in MARKERS {
        assert!(
            cx.debug_bounds(id).is_none(),
            "{id} must be quiet without a current failed request"
        );
    }
    for id in [
        "repository-refresh",
        "overview-retry-releases",
        "overview-retry-contributors",
        "overview-retry-languages",
    ] {
        assert!(
            cx.debug_bounds(id).is_none(),
            "{id} must not accompany an ordinary cache preview"
        );
    }
    assert!(
        cx.debug_bounds("repository-refresh-all").is_some(),
        "the ordinary manual refresh remains available"
    );
}
#[gpui::test]
fn indicator_cached_first_frame_and_success_are_quiet(cx: &mut TestAppContext) {
    let mut f = CachedFixture::new(Scene::Normal);
    f.login(1);
    warm(cx, &f);
    f.restart();
    f.transport.hold(true);
    let (_host, page, cx) = open_cached(cx, &f, RepositoryNavigation::default());
    drive(&page, cx, |p, cx| {
        observed(p, cx, true) && p.read_with(cx, |p, _| p.reader_status() == &ReaderStatus::Showing)
    });
    assert!(reader_text(&page, cx).contains("Ledger"));
    quiet(cx);
    let reader = page.read_with(cx, |p, _| p.reader().unwrap().entity_id());
    for _ in 0..4 {
        frames(cx);
        quiet(cx);
    }
    f.transport.release_all();
    drive(&page, cx, |p, cx| {
        p.read_with(cx, |p, cx| {
            !p.is_showing_cached_snapshot()
                && p.overview().is_some_and(|v| {
                    let v = v.read(cx);
                    !v.releases().is_historical()
                        && !v.contributors().is_historical()
                        && !v.languages().is_historical()
                })
        })
    });
    quiet(cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id()),
        reader
    );
}
fn scoped_failure(
    cx: &mut TestAppContext,
    path: &str,
    marker: &'static str,
    retry: &'static str,
    endpoint: usize,
) {
    let mut f = CachedFixture::new(Scene::Normal);
    f.login(1);
    warm(cx, &f);
    f.restart();
    f.transport.hold(true);
    *f.transport.held_path.lock().unwrap() = Some(path.into());
    let (_host, page, cx) = open_cached(cx, &f, RepositoryNavigation::default());
    drive(&page, cx, |p, cx| observed(p, cx, true));
    quiet(cx);
    let reader = page.read_with(cx, |p, _| p.reader().unwrap().entity_id());
    f.transport.release_all();
    drive(&page, cx, |p, cx| {
        f.transport
            .held_requests
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
            && p.read_with(cx, |p, cx| {
                let v = p.overview().unwrap().read(cx);
                let states = [
                    v.releases().is_historical(),
                    v.latest_release().unwrap().is_historical(),
                    v.contributors().is_historical(),
                    v.languages().is_historical(),
                ];
                states.iter().enumerate().all(|(i, h)| i == endpoint || !*h)
            })
    });
    quiet(cx);
    *f.transport.failed_path.lock().unwrap() = Some(path.into());
    f.transport
        .failed_status
        .store(503, std::sync::atomic::Ordering::SeqCst);
    f.transport.tree_release.send_replace(true);
    drive(&page, cx, |_, cx| cx.debug_bounds(marker).is_some());
    assert!(reader_text(&page, cx).contains("Ledger"));
    for other in MARKERS {
        if other != marker {
            assert!(
                cx.debug_bounds(other).is_none(),
                "{other} is a successful sibling"
            );
        }
    }
    assert_eq!(
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id()),
        reader
    );
    // A second failed request must remain retryable, rather than treating retry dispatch as success.
    let attempts = f
        .transport
        .held_requests
        .load(std::sync::atomic::Ordering::SeqCst);
    let retry_point = cx.debug_bounds(retry).unwrap().center();
    cx.simulate_click(retry_point, Modifiers::none());
    drive(&page, cx, |_, cx| {
        f.transport
            .held_requests
            .load(std::sync::atomic::Ordering::SeqCst)
            > attempts
            && cx.debug_bounds(marker).is_some()
    });
    // Retry through the rendered control, preserving data until the next response succeeds.
    *f.transport.failed_path.lock().unwrap() = None;
    *f.transport.held_path.lock().unwrap() = None;
    let button = cx
        .debug_bounds(retry)
        .expect("failed cached region has a real retry control");
    cx.simulate_click(button.center(), Modifiers::none());
    drive(&page, cx, |p, cx| {
        cx.debug_bounds(marker).is_none()
            && p.read_with(cx, |p, cx| {
                let v = p.overview().unwrap().read(cx);
                match endpoint {
                    0 => !v.releases().is_historical(),
                    1 => !v.latest_release().unwrap().is_historical(),
                    2 => !v.contributors().is_historical(),
                    _ => !v.languages().is_historical(),
                }
            })
    });
    quiet(cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id()),
        reader
    );
}
#[gpui::test]
fn indicator_release_list_failure_and_retry_are_scoped(cx: &mut TestAppContext) {
    scoped_failure(
        cx,
        "/releases?",
        "overview-releases-cached",
        "overview-retry-releases",
        0,
    );
}
#[gpui::test]
fn indicator_latest_failure_and_retry_are_scoped(cx: &mut TestAppContext) {
    scoped_failure(
        cx,
        "/releases/latest",
        "overview-releases-cached",
        "overview-retry-releases",
        1,
    );
}
#[gpui::test]
fn indicator_contributors_failure_and_retry_are_scoped(cx: &mut TestAppContext) {
    scoped_failure(
        cx,
        "/contributors",
        "overview-contributors-cached",
        "overview-retry-contributors",
        2,
    );
}
#[gpui::test]
fn indicator_languages_failure_and_retry_are_scoped(cx: &mut TestAppContext) {
    scoped_failure(
        cx,
        "/languages",
        "overview-languages-cached",
        "overview-retry-languages",
        3,
    );
}
#[gpui::test]
fn indicator_no_cache_failure_has_no_cached_marks(cx: &mut TestAppContext) {
    let f = CachedFixture::new(Scene::Normal);
    f.login(1);
    f.transport.set_offline(true);
    let (_host, page, cx) = open_cached(cx, &f, RepositoryNavigation::default());
    settle(&page, cx, |p| p.error().is_some());
    for id in MARKERS {
        assert!(cx.debug_bounds(id).is_none());
    }
    assert!(cx.debug_bounds("repository-error").is_some());
}

fn metadata_failure(
    cx: &mut TestAppContext,
    path: &str,
    marker: &'static str,
    retry: &'static str,
    slot: usize,
) {
    let mut f = CachedFixture::new(Scene::Normal);
    f.login(1);
    warm(cx, &f);
    f.restart();
    f.transport.hold(true);
    let (_host, page, cx) = open_cached(cx, &f, RepositoryNavigation::default());
    drive(&page, cx, |p, cx| observed(p, cx, true));
    quiet(cx);
    let reader = page.read_with(cx, |p, _| p.reader().unwrap().entity_id());
    *f.transport.failed_path.lock().unwrap() = Some(path.into());
    f.transport
        .failed_status
        .store(503, std::sync::atomic::Ordering::SeqCst);
    f.transport.release_all();
    drive(&page, cx, |_, cx| cx.debug_bounds(marker).is_some());
    for other in MARKERS {
        if other != marker {
            assert!(
                cx.debug_bounds(other).is_none(),
                "{other} must not inherit another request's failure"
            );
        }
    }
    assert_eq!(
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id()),
        reader
    );
    *f.transport.failed_path.lock().unwrap() = None;
    let button = cx.debug_bounds(retry).unwrap().center();
    cx.simulate_click(button, Modifiers::none());
    drive(&page, cx, |p, cx| {
        cx.debug_bounds(marker).is_none()
            && p.read_with(cx, |p, _| {
                !p.cache_failures[slot]
                    && match slot {
                        0 => p
                            .details_provenance()
                            .is_some_and(|p| p.origin == RepositoryCacheOrigin::Network),
                        1 => p
                            .branches_provenance()
                            .is_some_and(|p| p.origin == RepositoryCacheOrigin::Network),
                        _ => p
                            .revision_provenance()
                            .is_some_and(|p| p.origin == RepositoryCacheOrigin::Network),
                    }
            })
    });
    assert_eq!(
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id()),
        reader
    );
}
#[gpui::test]
fn indicator_summary_failure_is_local_and_retry_keeps_content(cx: &mut TestAppContext) {
    metadata_failure(
        cx,
        "/repos/octo/ledger",
        "repository-cached-badge",
        "repository-refresh",
        0,
    );
}
#[gpui::test]
fn indicator_branch_failure_is_local_and_retry_keeps_content(cx: &mut TestAppContext) {
    metadata_failure(
        cx,
        "/branches?",
        "repository-branches-cached",
        "repository-refresh-branches",
        1,
    );
}
#[gpui::test]
fn indicator_revision_failure_is_local_and_retry_keeps_content(cx: &mut TestAppContext) {
    metadata_failure(
        cx,
        "/commits/main",
        "repository-code-cached",
        "repository-refresh-code",
        2,
    );
}

#[gpui::test]
fn indicator_failure_before_disk_publication_and_late_account_reply(cx: &mut TestAppContext) {
    use super::super::super::{Body, Message};
    let mut f = CachedFixture::new(Scene::Normal);
    f.login(1);
    warm(cx, &f);
    f.restart();
    f.transport.hold(true);
    let (_host, page, cx) = open_cached(cx, &f, RepositoryNavigation::default());
    let inbox = page.read_with(cx, |p, _| p.inbox.clone());
    // Hold UI publication while the real disk job and failed network job complete, then deliver
    // the failure first. No guessed disk/network speed or timer drives the product state.
    let end = std::time::Instant::now() + Duration::from_secs(60);
    while !inbox
        .lock()
        .unwrap()
        .iter()
        .any(|m| matches!(m.body, Body::Snapshot(_)))
    {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    *f.transport.failed_path.lock().unwrap() = Some("/repos/octo/ledger".into());
    f.transport
        .failed_status
        .store(404, std::sync::atomic::Ordering::SeqCst);
    f.transport.release_all();
    while !inbox
        .lock()
        .unwrap()
        .iter()
        .any(|m| matches!(m.body, Body::Repository { .. }))
    {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    let (snapshot, failure) = {
        let mut q = inbox.lock().unwrap();
        let i = q
            .iter()
            .position(|m| matches!(m.body, Body::Snapshot(_)))
            .unwrap();
        let snapshot = q.remove(i).unwrap();
        let i = q
            .iter()
            .position(|m| matches!(m.body, Body::Repository { .. }))
            .unwrap();
        (snapshot, q.remove(i).unwrap())
    };
    let stale = Message {
        generation: failure.generation,
        branch_request: failure.branch_request,
        authorization: failure.authorization.clone(),
        context: failure.context.clone(),
        preliminary: false,
        body: match &failure.body {
            Body::Repository {
                result,
                star_write_generation,
            } => Body::Repository {
                result: result.clone(),
                star_write_generation: *star_write_generation,
            },
            _ => unreachable!(),
        },
    };
    page.update_in(cx, |p, w, cx| {
        p.apply(failure, w, cx);
        p.apply(snapshot, w, cx);
    });
    drive(&page, cx, |p, cx| observed(p, cx, true));
    assert!(reader_text(&page, cx).contains("Ledger"));
    assert!(cx.debug_bounds("repository-cached-badge").is_some());
    for id in &MARKERS[1..] {
        assert!(cx.debug_bounds(id).is_none());
    }
    f.transport.hold(true);
    f.login(2);
    page.update_in(cx, |p, w, cx| {
        p.activate(cx);
        p.apply(stale, w, cx);
    });
    frames(cx);
    for id in MARKERS {
        assert!(
            cx.debug_bounds(id).is_none(),
            "old account failure must not label the new page"
        );
    }
    assert!(page.read_with(cx, |p, _| p.details().is_none()));
}

#[gpui::test]
fn indicator_reopening_same_account_does_not_restore_previous_failure(cx: &mut TestAppContext) {
    let mut f = CachedFixture::new(Scene::Normal);
    f.login(1);
    warm(cx, &f);
    f.restart();
    f.transport.set_offline(true);
    {
        let (host, page, cx) = open_cached(cx, &f, RepositoryNavigation::default());
        drive(&page, cx, |_, cx| {
            cx.debug_bounds("repository-cached-badge").is_some()
        });
        release_page(&host, cx);
    }
    f.restart();
    f.transport.set_offline(false);
    f.transport.hold(true);
    let (_host, page, cx) = open_cached(cx, &f, RepositoryNavigation::default());
    drive(&page, cx, |p, cx| observed(p, cx, true));
    quiet(cx);
}
