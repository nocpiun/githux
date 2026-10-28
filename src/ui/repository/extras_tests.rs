//! Combined component tests run against the same request-addressed fixture as the native page.
use super::*;
use crate::repository_modules::fixture::Scene;
use crate::ui::repository_overview::{OverviewAction, OverviewSection, SectionState};
use gpui::{Modifiers, TestAppContext, px};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

fn overview_ready(page: &RepositoryPage, cx: &gpui::App) -> bool {
    page.overview().is_some_and(|view| {
        let view = view.read(cx);
        !view.releases().is_loading()
            && view
                .latest_release()
                .is_some_and(|state| !state.is_loading())
            && !view.contributors().is_loading()
            && !view.languages().is_loading()
    })
}
fn ready(
    cx: &mut TestAppContext,
    scene: Scene,
) -> (
    Fixture,
    Entity<RepositoryPage>,
    &mut gpui::VisualTestContext,
) {
    let fixture = Fixture::new(scene);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    page.update(cx, |p, cx| p.activate(cx));
    settle(&page, cx, |p| p.reader_status() == &ReaderStatus::Showing);
    for _ in 0..600 {
        page.update_in(cx, |p, w, cx| p.drain(w, cx));
        frames(cx);
        std::thread::sleep(std::time::Duration::from_millis(5));
        if page.read_with(cx, overview_ready) {
            return (fixture, page, cx);
        }
    }
    panic!("overview did not settle")
}
#[gpui::test]
fn overview_reads_once_and_branch_changes_keep_the_embedded_entity(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::Normal);
    let overview = page.read_with(cx, |p, _| p.overview().unwrap().clone());
    for resource in [
        "/releases?",
        "/releases/latest",
        "/contributors?",
        "/languages",
    ] {
        assert_eq!(fixture.transport.count(resource), 1, "{resource}");
    }
    page.update(cx, |p, cx| p.select_branch("feature/docs-polish", cx));
    settle(&page, cx, |p| {
        p.reader_status() == &ReaderStatus::Showing
            && p.selected_branch() == Some("feature/docs-polish")
    });
    assert_eq!(
        overview.entity_id(),
        page.read_with(cx, |p, _| p.overview().unwrap().entity_id())
    );
    for resource in [
        "/releases?",
        "/releases/latest",
        "/contributors?",
        "/languages",
    ] {
        assert_eq!(fixture.transport.count(resource), 1, "{resource}");
    }
    assert!(fixture.transport.mismatches().is_empty());
    for (width, height) in [(1100., 800.), (880., 560.)] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
        frames(cx);
        let strip = cx.debug_bounds("repository-overview").unwrap();
        let reader = cx.debug_bounds("repository-reader-body").unwrap();
        assert!(strip.bottom() <= reader.origin.y);
        assert!(
            reader.size.height > px(140.),
            "usable reader at {width}x{height}: {reader:?}"
        );
        assert!(strip.size.height < px(150.));
    }
}
#[gpui::test]
fn scoped_retry_keeps_source_and_sibling_groups(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::OverviewError);
    page.update(cx, |p, cx| p.toggle_directory("src", cx));
    settle(&page, cx, |p| p.tree("src").is_some());
    page.update(cx, |p, cx| p.select_file("src/main.rs", cx));
    settle(&page, cx, |p| p.reader_status() == &ReaderStatus::Showing);
    let reader = page.read_with(cx, |p, _| p.reader().unwrap().entity_id());
    let overview = page.read_with(cx, |p, _| p.overview().unwrap().clone());
    assert!(overview.read_with(cx, |v, _| v.languages().error().is_some()));
    assert!(cx.debug_bounds("overview-retry-languages").is_none());
    // Languages have no permanent UI chrome; the scoped host retry remains available.
    let binding = overview.read_with(cx, |v, _| v.binding().clone());
    page.update_in(cx, |p, w, cx| {
        p.overview_action(
            OverviewAction::Refresh {
                binding,
                section: OverviewSection::Languages,
            },
            w,
            cx,
        )
    });
    for _ in 0..200 {
        page.update_in(cx, |p, w, cx| p.drain(w, cx));
        frames(cx);
        if overview.read_with(cx, |v, _| v.languages().value().is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    assert!(overview.read_with(cx, |v, _| v.languages().value().is_some()));
    assert_eq!(
        reader,
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id())
    );
    assert_eq!(fixture.transport.count("/languages"), 2);
    assert_eq!(fixture.transport.count("/contributors?"), 1);
    assert_eq!(fixture.transport.count("/releases/latest"), 1);
}
#[gpui::test]
fn typed_actions_emit_once_and_old_binding_is_inert(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = ready(cx, Scene::Normal);
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    page.update(cx, |p, _| {
        p.on_action(Rc::new(move |action, _, _| {
            sink.lock().unwrap().push(action)
        }))
    });
    let version = cx.debug_bounds("overview-release-version").unwrap();
    cx.simulate_click(version.center(), Modifiers::none());
    frames(cx);
    assert_eq!(received.lock().unwrap().len(), 1);
    let binding = page.read_with(cx, |p, cx| p.overview().unwrap().read(cx).binding().clone());
    page.update(cx, |p, cx| p.deactivate(cx));
    page.update_in(cx, |p, w, cx| {
        p.overview_action(
            OverviewAction::Refresh {
                binding: binding.clone(),
                section: OverviewSection::Languages,
            },
            w,
            cx,
        )
    });
    page.update_in(cx, |p, w, cx| {
        p.overview_action(
            OverviewAction::OpenRelease {
                binding,
                release_id: 501,
                tag_name: "v1.0.0".into(),
                html_url: None,
            },
            w,
            cx,
        )
    });
    assert_eq!(received.lock().unwrap().len(), 1);
    assert!(page.read_with(cx, |p, _| p.overview().is_none()));
}
#[gpui::test]
fn partial_latest404_and_zero_keep_honest_independent_values(cx: &mut TestAppContext) {
    for scene in [
        Scene::OverviewPartial,
        Scene::LatestUnavailable,
        Scene::OverviewZero,
    ] {
        let (_fixture, page, cx) = ready(cx, scene);
        page.read_with(cx, |p, cx| {
            let view = p.overview().unwrap().read(cx);
            assert!(view.languages().value().is_some());
            assert!(view.releases().value().is_some());
            if scene == Scene::LatestUnavailable {
                assert!(
                    view.latest_release()
                        .unwrap()
                        .value()
                        .unwrap()
                        .release
                        .is_none()
                );
                assert!(!view.releases().value().unwrap().list.releases.is_empty());
            }
            if scene == Scene::OverviewZero {
                assert_eq!(view.languages().value().unwrap().total_bytes, 0);
            }
            if scene == Scene::OverviewPartial {
                assert!(matches!(
                    view.contributors().value().unwrap().completeness,
                    crate::domain::repository_extras::ExtrasCompleteness::Partial { .. }
                ));
            }
        });
    }
}
#[gpui::test]
fn delayed_overview_is_rejected_after_authorization_change(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::OverviewLoading);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    page.update(cx, |p, cx| p.activate(cx));
    settle(&page, cx, |p| p.overview().is_some());
    let old = page.read_with(cx, |p, _| p.overview().unwrap().clone());
    fixture.login_as(8, "other");
    page.update_in(cx, |p, w, cx| p.drain(w, cx));
    assert!(page.read_with(cx, |p, _| p.overview().is_none()));
    std::thread::sleep(std::time::Duration::from_millis(20));
    page.update_in(cx, |p, w, cx| p.drain(w, cx));
    assert!(old.read_with(cx, |v, _| matches!(v.languages(), SectionState::Loading)));
}

#[gpui::test]
fn old_actions_are_rejected_after_account_aba_and_rebind(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::Normal);
    let binding = page.read_with(cx, |p, cx| p.overview().unwrap().read(cx).binding().clone());
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    page.update(cx, |p, _| {
        p.on_action(Rc::new(move |action, _, _| {
            sink.lock().unwrap().push(action)
        }))
    });
    fixture.login_as(8, "other");
    fixture
        ._runtime
        .block_on(fixture.accounts.set_active(crate::domain::AccountKey::new(
            1,
            crate::domain::HostId::github(),
        )))
        .unwrap();
    page.update_in(cx, |p, w, cx| {
        p.overview_action(
            OverviewAction::OpenContributor {
                binding: binding.clone(),
                login: Some("ledger-0".into()),
                html_url: Some("https://github.com/ledger-0".into()),
            },
            w,
            cx,
        )
    });
    assert!(received.lock().unwrap().is_empty());
    page.update(cx, |p, cx| p.rebind(fixture.services(), cx));
    settle(&page, cx, |p| p.reader_status() == &ReaderStatus::Showing);
    let requests = fixture.transport.count("/languages");
    page.update_in(cx, |p, w, cx| {
        p.overview_action(
            OverviewAction::Refresh {
                binding,
                section: OverviewSection::Languages,
            },
            w,
            cx,
        )
    });
    assert_eq!(requests, fixture.transport.count("/languages"));
}

#[gpui::test]
fn counts_are_independent_and_actions_are_host_entries(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::Normal);
    for _ in 0..200 {
        page.update_in(cx, |p, w, cx| p.drain(w, cx));
        if page.read_with(cx, |p, _| p.extras.counts.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    page.read_with(cx, |p, _| {
        let c = p.extras.counts.as_ref().unwrap();
        assert_eq!(c.issues, Ok(3));
        assert_eq!(c.pull_requests, Ok(1));
        assert_eq!(p.details().unwrap().open_issues_and_prs, 4);
    });
    assert!(cx.debug_bounds("repository-badge-issues").is_some());
    assert!(cx.debug_bounds("repository-badge-pulls").is_some());
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    page.update(cx, |p, cx| {
        p.on_action(Rc::new(move |a, _, _| sink.lock().unwrap().push(a)));
        cx.notify();
    });
    frames(cx);
    let star = cx.debug_bounds("repository-star").unwrap();
    cx.simulate_click(star.center(), Modifiers::none());
    page.update_in(cx, |p, w, cx| w.focus(&p.star_focus.clone(), cx));
    cx.simulate_keystrokes("enter space");
    assert_eq!(received.lock().unwrap().len(), 3);
    let binding = page.read_with(cx, |p, _| p.repository_binding().unwrap());
    page.update(cx, |p, cx| p.deactivate(cx));
    page.update_in(cx, |p, w, cx| p.repository_action(false, binding, w, cx));
    assert_eq!(received.lock().unwrap().len(), 3);
    assert_eq!(fixture.transport.count("/graphql"), 1);
}
#[gpui::test]
fn count_badges_hide_unknown_fields_and_branch_changes_do_not_refetch(cx: &mut TestAppContext) {
    let (fixture, page, cx) = ready(cx, Scene::OverviewPartial);
    for _ in 0..200 {
        page.update_in(cx, |p, w, cx| p.drain(w, cx));
        if page.read_with(cx, |p, _| p.extras.counts.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    frames(cx);
    assert!(cx.debug_bounds("repository-badge-issues").is_some());
    assert!(cx.debug_bounds("repository-badge-pulls").is_none());
    page.update(cx, |p, cx| p.select_branch("feature/docs-polish", cx));
    settle(&page, cx, |p| {
        p.reader_status() == &ReaderStatus::Showing
            && p.selected_branch() == Some("feature/docs-polish")
    });
    assert_eq!(fixture.transport.count("/graphql"), 1);
    let bar = cx.debug_bounds("overview-language-bar").unwrap();
    let root = cx.debug_bounds("repository-page-root").unwrap();
    assert!((bar.bottom() - root.bottom()).abs() < px(1.));
    page.update(cx, |p, cx| p.select_tab(RepositoryTab::Issues, cx));
    frames(cx);
    assert_eq!(
        bar.origin.y,
        cx.debug_bounds("overview-language-bar").unwrap().origin.y
    );
    fixture.login_as(8, "new");
    page.update_in(cx, |p, w, cx| p.drain(w, cx));
    assert!(page.read_with(cx, |p, _| p.extras.counts.is_none()));
}

#[gpui::test]
fn real_zero_counts_are_values_and_unknown_counts_are_hidden(cx: &mut TestAppContext) {
    for scene in [Scene::OverviewZero, Scene::OverviewError] {
        let (_fixture, page, cx) = ready(cx, scene);
        for _ in 0..30 {
            page.update_in(cx, |p, w, cx| p.drain(w, cx));
            frames(cx);
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        if scene == Scene::OverviewZero {
            page.read_with(cx, |p, _| {
                assert_eq!(p.extras.counts.as_ref().unwrap().issues, Ok(0));
                assert_eq!(p.extras.counts.as_ref().unwrap().pull_requests, Ok(0));
            });
            assert!(cx.debug_bounds("repository-badge-issues").is_some());
        } else {
            assert!(cx.debug_bounds("repository-badge-issues").is_none());
            assert!(cx.debug_bounds("repository-badge-pulls").is_none());
        }
        assert!(cx.debug_bounds("overview-language-bar").is_none());
    }
}

#[gpui::test]
fn bottom_language_tooltip_fits_and_disappears_on_leave(cx: &mut TestAppContext) {
    let (_fixture, _page, cx) = ready(cx, Scene::Normal);
    cx.simulate_resize(gpui::size(px(880.), px(560.)));
    frames(cx);
    let segment = cx.debug_bounds("overview-language-segment-0").unwrap();
    cx.simulate_mouse_move(segment.center(), None, Modifiers::none());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    frames(cx);
    let tooltip = cx
        .debug_bounds("repository-language-tooltip")
        .expect("tooltip is visible at bottom");
    assert!(tooltip.bottom() <= px(560.));
    assert!(tooltip.origin.y < segment.origin.y);
    cx.simulate_mouse_move(gpui::point(px(500.), px(250.)), None, Modifiers::none());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    frames(cx);
    assert!(cx.debug_bounds("repository-language-tooltip").is_none());
}

#[gpui::test]
fn neutral_count_pill_stays_inside_its_tab_click_target(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = ready(cx, Scene::Normal);
    for _ in 0..200 {
        page.update_in(cx, |p, w, cx| p.drain(w, cx));
        frames(cx);
        if cx.debug_bounds("repository-badge-issues").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    let badge = cx.debug_bounds("repository-badge-issues").unwrap();
    cx.simulate_click(badge.center(), Modifiers::none());
    frames(cx);
    assert_eq!(page.read_with(cx, |p, _| p.tab()), RepositoryTab::Issues);
    assert!(cx.debug_bounds("repository-tab-active-issues").is_some());
    assert!(cx.debug_bounds("repository-body").is_none());
    let tab = cx.debug_bounds("repository-tab-code").unwrap();
    cx.simulate_click(tab.center(), Modifiers::none());
    frames(cx);
    assert!(page.read_with(cx, |p, _| p.reader().is_some()));
}
