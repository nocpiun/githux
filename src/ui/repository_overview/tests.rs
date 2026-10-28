//! Behavioral coverage for compact summaries and scoped updates.
use super::view::summary::{contributor_display, language_segments};
use super::{fixtures, *};
use crate::domain::account::{AccountKey, HostId};
use gpui::{
    AppContext as _, Bounds, Entity, InteractiveElement, Modifiers, ParentElement, Pixels, Point,
    StatefulInteractiveElement, Styled, TestAppContext, VisualContext, VisualTestContext, px, size,
};
use std::rc::Rc;
fn setup(cx: &mut TestAppContext) {
    cx.update(ely_core::init_for_tests);
}
struct ReaderProbe {
    scroll: gpui::ScrollHandle,
    focus: gpui::FocusHandle,
}

impl ReaderProbe {
    fn new(cx: &mut gpui::Context<Self>) -> Self {
        Self {
            scroll: gpui::ScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
        }
    }

    fn offset(&self) -> Point<Pixels> {
        self.scroll.offset()
    }
}

impl gpui::Render for ReaderProbe {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
            .id("probe-reader")
            .debug_selector(|| "probe-reader".to_string())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .track_focus(&self.focus)
            .child(gpui::div().h(px(2000.0)).child("reader"))
    }
}

struct Harness {
    overview: Entity<RepositoryOverview>,
    reader: Entity<ReaderProbe>,
    action_count: usize,
    last_action: Option<OverviewAction>,
}

impl gpui::Render for Harness {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
            .debug_selector(|| "harness".to_string())
            .size_full()
            .flex()
            .flex_col()
            .child(self.overview.clone())
            .child(
                gpui::div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.reader.clone()),
            )
    }
}

fn harness(cx: &mut TestAppContext) -> (Entity<Harness>, &mut VisualTestContext) {
    setup(cx);
    cx.add_window_view(|window, cx| {
        let overview = cx.new(|cx| RepositoryOverview::new(fixtures::binding(1), window, cx));
        let reader = cx.new(ReaderProbe::new);
        let weak = cx.entity().downgrade();
        overview.update(cx, |view, _| {
            view.on_action(Rc::new(move |action, _window, cx| {
                if let Some(harness) = weak.upgrade() {
                    harness.update(cx, |harness: &mut Harness, cx| {
                        harness.action_count += 1;
                        harness.last_action = Some(action);
                        cx.notify();
                    });
                }
            }));
        });
        Harness {
            overview,
            reader,
            action_count: 0,
            last_action: None,
        }
    })
}

fn overview(harness: &Entity<Harness>, cx: &mut VisualTestContext) -> Entity<RepositoryOverview> {
    harness.read_with(cx, |harness, _| harness.overview.clone())
}

fn reader_offset(harness: &Entity<Harness>, cx: &mut VisualTestContext) -> Point<Pixels> {
    harness.read_with(cx, |harness, cx| harness.reader.read(cx).offset())
}

fn apply_releases(
    harness: &Entity<Harness>,
    binding: &OverviewBinding,
    state: ReleasesState,
    cx: &mut VisualTestContext,
) -> bool {
    overview(harness, cx).update(cx, |view, cx| view.apply_releases(binding, state, cx))
}

fn apply_contributors(
    harness: &Entity<Harness>,
    binding: &OverviewBinding,
    state: ContributorsState,
    cx: &mut VisualTestContext,
) -> bool {
    overview(harness, cx).update(cx, |view, cx| view.apply_contributors(binding, state, cx))
}

fn apply_languages(
    harness: &Entity<Harness>,
    binding: &OverviewBinding,
    state: LanguagesState,
    cx: &mut VisualTestContext,
) -> bool {
    overview(harness, cx).update(cx, |view, cx| view.apply_languages(binding, state, cx))
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

#[gpui::test]
fn compact_ready_has_one_version_avatars_and_one_bar(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    ready(&h, cx);
    cx.simulate_resize(size(px(1100.0), px(760.0)));
    cx.run_until_parked();
    let top = bounds("repository-overview", cx);
    assert!(top.size.height < px(100.0));
    assert!(bounds("probe-reader", cx).size.height > px(600.0));
    assert!(cx.debug_bounds("overview-release-version").is_some());
    assert!(cx.debug_bounds("overview-avatar-0").is_some());
    assert!(cx.debug_bounds("overview-language-bar").is_some());
    let language = bounds("overview-languages", cx);
    assert!(language.right() <= top.right());
    assert!(language.left() >= bounds("overview-contributors", cx).right());
    for obsolete in [
        "overview-release-4",
        "overview-notes-panel",
        "overview-refresh-releases",
        "overview-language-0",
    ] {
        assert!(cx.debug_bounds(obsolete).is_none());
    }
}
fn ready(h: &Entity<Harness>, cx: &mut VisualTestContext) {
    let b = fixtures::binding(1);
    apply_releases(h, &b, SectionState::Ready(fixtures::ready_releases()), cx);
    apply_contributors(
        h,
        &b,
        SectionState::Ready(fixtures::ready_contributors()),
        cx,
    );
    apply_languages(h, &b, SectionState::Ready(fixtures::ready_languages()), cx);
}
#[gpui::test]
fn actions_are_scoped_and_anonymous_is_not_navigable(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    ready(&h, cx);
    click("overview-release-version", cx);
    assert_eq!(h.read_with(cx, |h, _| h.action_count), 1);
    assert!(
        matches!(h.read_with(cx,|h,_|h.last_action.clone()),Some(OverviewAction::OpenRelease{binding,release_id:4,..}) if binding==fixtures::binding(1))
    );
    click("overview-avatar-0", cx);
    assert_eq!(h.read_with(cx, |h, _| h.action_count), 2);
    cx.simulate_keystrokes("tab shift-tab escape");
    cx.run_until_parked();
    click("overview-avatar-1", cx);
    assert_eq!(h.read_with(cx, |h, _| h.action_count), 2);
    apply_languages(
        &h,
        &fixtures::binding(1),
        SectionState::Error {
            message: "Offline".into(),
        },
        cx,
    );
    click("overview-retry-languages", cx);
    assert!(matches!(
        h.read_with(cx, |h, _| h.last_action.clone()),
        Some(OverviewAction::Refresh {
            section: OverviewSection::Languages,
            ..
        })
    ));
    assert!(cx.debug_bounds("overview-release-version").is_some());
}
#[gpui::test]
fn stale_bindings_and_wrong_payloads_are_rejected(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    ready(&h, cx);
    for mut binding in [fixtures::binding(2), fixtures::binding(1)] {
        if binding.generation == 1 {
            binding.account = AccountKey::new(99, HostId::github());
        }
        assert!(!apply_releases(&h, &binding, SectionState::Loading, cx));
    }
    let mut value = fixtures::ready_languages();
    value.repository.id = 99;
    assert!(!apply_languages(
        &h,
        &fixtures::binding(1),
        SectionState::Ready(value),
        cx
    ));
    let entity = overview(&h, cx);
    cx.update_window_entity(&entity, |view, window, cx| {
        view.set_binding(fixtures::binding(2), window, cx)
    });
    assert!(!apply_contributors(
        &h,
        &fixtures::binding(1),
        SectionState::Ready(fixtures::ready_contributors()),
        cx
    ));
}
#[gpui::test]
fn latest_404_does_not_infer_a_version_from_the_list(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    ready(&h, cx);
    apply_releases(
        &h,
        &fixtures::binding(1),
        SectionState::Ready(fixtures::latest_none_releases()),
        cx,
    );
    assert!(cx.debug_bounds("overview-release-version").is_none());
    let entity = overview(&h, cx);
    entity.update(cx, |v, cx| {
        v.apply_latest_release(
            &fixtures::binding(1),
            SectionState::Ready(fixtures::ready_releases().latest.unwrap()),
            cx,
        );
    });
    apply_releases(
        &h,
        &fixtures::binding(1),
        SectionState::Error {
            message: "list unavailable".into(),
        },
        cx,
    );
    assert!(cx.debug_bounds("overview-release-version").is_some());
}
#[gpui::test]
fn narrow_and_stress_leave_reader_space_without_local_scroll(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    ready(&h, cx);
    apply_contributors(
        &h,
        &fixtures::binding(1),
        SectionState::Ready(fixtures::large_contributors()),
        cx,
    );
    apply_languages(
        &h,
        &fixtures::binding(1),
        SectionState::Ready(fixtures::many_languages()),
        cx,
    );
    cx.simulate_resize(size(px(480.0), px(480.0)));
    cx.run_until_parked();
    assert!(bounds("repository-overview", cx).size.height < px(220.0));
    assert!(bounds("probe-reader", cx).size.height > px(240.0));
    assert!(cx.debug_bounds("overview-avatar-4").is_some());
    assert!(cx.debug_bounds("overview-avatar-5").is_none());
    assert!(cx.debug_bounds("overview-sections-viewport").is_none());
}
#[gpui::test]
fn empty_204_zero_and_partial_remain_independent(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    ready(&h, cx);
    for value in [
        fixtures::empty_repository_contributors(),
        fixtures::empty_content_contributors(),
        fixtures::partial_contributors(),
    ] {
        apply_contributors(
            &h,
            &fixtures::binding(1),
            SectionState::Ready(value.clone()),
            cx,
        );
        assert_eq!(
            overview(&h, cx).read_with(cx, |v, _| v.contributors().value().unwrap().state),
            value.state
        );
        assert!(cx.debug_bounds("overview-release-version").is_some());
    }
    apply_languages(
        &h,
        &fixtures::binding(1),
        SectionState::Ready(fixtures::zero_languages()),
        cx,
    );
    assert!(cx.debug_bounds("overview-language-bar").is_none());
}
#[gpui::test]
fn supplied_latest_flags_and_partial_do_not_disappear(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let mut data = fixtures::partial_releases();
    let release = data.latest.as_mut().unwrap().release.as_mut().unwrap();
    release.draft = true;
    release.prerelease = true;
    apply_releases(&h, &fixtures::binding(1), SectionState::Ready(data), cx);
    assert!(cx.debug_bounds("overview-draft").is_some());
    assert!(cx.debug_bounds("overview-prerelease").is_some());
    assert!(overview(&h, cx).read_with(cx, |v, _| matches!(
        v.releases().value().unwrap().list.completeness,
        crate::domain::repository_extras::ExtrasCompleteness::Partial { .. }
    )));
}

#[test]
fn languages_group_checked_bytes_into_three_segments() {
    let value = fixtures::many_languages();
    let segments = language_segments(&value).unwrap();
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[2].name, "Other");
    assert!((segments.iter().map(|v| v.share).sum::<f64>() - 1.0).abs() < 1e-12);
    let mut invalid = fixtures::ready_languages();
    invalid.languages[0].bytes = u64::MAX;
    assert!(language_segments(&invalid).is_none());
    assert!(
        language_segments(&fixtures::zero_languages())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        contributor_display(&fixtures::ready_contributors().contributors[1]),
        "Anonymous Builder"
    );
}

#[gpui::test]
fn independent_latest_recovery_removes_snapshot_error(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let binding = fixtures::binding(1);
    apply_releases(
        &h,
        &binding,
        SectionState::Ready(fixtures::latest_error_releases()),
        cx,
    );
    let v = overview(&h, cx);
    v.update(cx, |v, cx| {
        v.apply_latest_release(
            &binding,
            SectionState::Ready(fixtures::ready_releases().latest.unwrap()),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update(|w, cx| w.simulate_next_frame(cx));
    assert!(cx.debug_bounds("overview-release-version").is_some());
    assert!(cx.debug_bounds("overview-retry-releases").is_none());
}
