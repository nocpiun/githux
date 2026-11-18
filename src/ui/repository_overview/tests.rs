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
        cx: &mut gpui::Context<Self>,
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
            .child(self.overview.read(cx).language_bar(cx))
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
    assert!(cx.debug_bounds("overview-languages").is_none());
    let language = bounds("overview-language-bar", cx);
    assert!(language.origin.y >= bounds("probe-reader", cx).bottom());
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
fn avatar_image_presence_is_queryable_so_hosts_skip_repeated_decoding(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    ready(&h, cx);
    let view = overview(&h, cx);
    let binding = view.read_with(cx, |v, _| v.binding().clone());
    // Nothing attached yet: a host must decode on the first frame.
    assert!(!view.read_with(cx, |v, _| v.has_avatar_image(&binding, 100)));
    let pixels = image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 200, 30, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let image = cx.update(|_, cx| {
        gpui::Image::from_bytes(gpui::ImageFormat::Png, png.into_inner())
            .to_image_data(cx.svg_renderer())
            .unwrap()
    });
    view.update(cx, |v, cx| {
        assert!(v.set_avatar_image(&binding, 100, image.clone(), cx));
    });
    // Attached: the host can skip the decode entirely on later repaints.
    assert!(view.read_with(cx, |v, _| v.has_avatar_image(&binding, 100)));
    // A different contributor or a stale binding is still a miss.
    assert!(!view.read_with(cx, |v, _| v.has_avatar_image(&binding, 101)));
    assert!(!view.read_with(cx, |v, _| v.has_avatar_image(&fixtures::binding(2), 100)));
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
    assert!(cx.debug_bounds("overview-retry-languages").is_some());
    assert!(cx.debug_bounds("overview-languages-cached").is_none());
    assert!(cx.debug_bounds("overview-language-bar").is_none());
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
fn languages_keep_every_identity_and_checked_bytes() {
    let value = fixtures::many_languages();
    let segments = language_segments(&value).unwrap();
    assert_eq!(
        segments.len(),
        value.languages.iter().filter(|v| v.bytes > 0).count()
    );
    assert!(
        segments
            .iter()
            .all(|segment| value.languages.iter().any(|v| v.name == segment.name))
    );
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
fn latest_failure_is_a_warning_mark_with_visible_recovery(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let binding = fixtures::binding(1);
    apply_releases(
        &h,
        &binding,
        SectionState::Ready(fixtures::latest_error_releases()),
        cx,
    );
    cx.run_until_parked();
    let mark = cx
        .debug_bounds("overview-retry-releases")
        .expect("latest/list failure leaves a retry mark, not body text");
    assert!(mark.size.height > px(0.0));
    assert!(
        cx.debug_bounds("overview-retry-releases").is_some(),
        "the shared retry mark keeps its stable debug selector"
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
    assert!(
        cx.debug_bounds("overview-retry-releases").is_none(),
        "a recovered latest read removes the warning mark"
    );
    assert!(cx.debug_bounds("overview-release-version").is_some());
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

#[gpui::test]
fn latest_badge_follows_independent_truth_and_groups_stay_left(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    ready(&h, cx);
    assert!(cx.debug_bounds("overview-latest-badge").is_some());
    assert!(cx.debug_bounds("overview-release-tag-icon").is_some());
    let view = overview(&h, cx);
    let binding = fixtures::binding(1);
    for width in [1100., 1600.] {
        cx.simulate_resize(size(px(width), px(800.)));
        cx.run_until_parked();
        let releases = bounds("overview-releases", cx);
        let people = bounds("overview-contributors", cx);
        assert!(
            people.origin.x - releases.right() < px(50.),
            "groups remain adjacent at {width}"
        );
        assert!(people.right() < px(width / 2.), "right half remains free");
    }
    for state in [
        SectionState::Loading,
        SectionState::Error {
            message: "offline".into(),
        },
        SectionState::Ready(fixtures::latest_none_releases().latest.unwrap()),
    ] {
        view.update(cx, |v, cx| {
            v.apply_latest_release(&binding, state, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("overview-latest-badge").is_none());
    }
    view.update(cx, |v, cx| {
        v.apply_latest_release(
            &binding,
            SectionState::Ready(fixtures::ready_releases().latest.unwrap()),
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-latest-badge").is_some());
    let mut prerelease = fixtures::ready_releases().latest.unwrap();
    prerelease.release.as_mut().unwrap().prerelease = true;
    view.update(cx, |v, cx| {
        v.apply_latest_release(&binding, SectionState::Ready(prerelease), cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-prerelease").is_some());
    assert!(cx.debug_bounds("overview-latest-badge").is_none());
    cx.update_window_entity(&view, |v, w, cx| v.set_binding(fixtures::binding(2), w, cx));
    view.update(cx, |v, cx| {
        assert!(!v.apply_latest_release(
            &binding,
            SectionState::Ready(fixtures::ready_releases().latest.unwrap()),
            cx
        ))
    });
    assert!(cx.debug_bounds("overview-latest-badge").is_none());
}

#[gpui::test]
fn no_latest_is_a_true_empty_not_a_warning(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    apply_releases(
        &h,
        &fixtures::binding(1),
        SectionState::Ready(fixtures::latest_none_releases()),
        cx,
    );
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("overview-retry-releases").is_none(),
        "a confirmed no-latest response is not a warning"
    );
    assert!(cx.debug_bounds("overview-release-version").is_none());
}

#[gpui::test]
fn confirmed_empty_repository_omits_the_whole_releases_block(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let b = fixtures::binding(1);
    apply_contributors(
        &h,
        &b,
        SectionState::Ready(fixtures::ready_contributors()),
        cx,
    );
    apply_releases(&h, &b, SectionState::Ready(fixtures::empty_releases()), cx);
    cx.simulate_resize(size(px(1100.0), px(760.0)));
    cx.run_until_parked();
    // No heading, zero badge, row, loading mark or retry mark survives the omission.
    for omitted in [
        "overview-releases",
        "overview-release-count",
        "overview-release-version",
        "overview-release-tag-icon",
        "overview-latest-badge",
        "overview-releases-cached",
        "overview-retry-releases",
    ] {
        assert!(
            cx.debug_bounds(omitted).is_none(),
            "{omitted} should be omitted"
        );
    }
    // The sibling group stays, and takes the strip's leading edge: no gap is reserved for the
    // omitted block.
    let top = bounds("repository-overview", cx);
    let people = bounds("overview-contributors", cx);
    assert!(people.origin.x <= top.origin.x + px(0.5));
    assert!(people.origin.x >= top.origin.x - px(0.5));
    assert!(bounds("probe-reader", cx).size.height > px(600.0));

    // The production read applies the list and the latest endpoint separately. The complete empty
    // list is already authoritative, so an in-flight or failed latest read neither draws chrome
    // nor waits for the network before the block disappears.
    let latest = fixtures::empty_releases().latest.unwrap();
    let mut list_only = fixtures::empty_releases();
    list_only.latest = None;
    apply_releases(&h, &b, SectionState::Ready(list_only), cx);
    assert!(cx.debug_bounds("overview-releases").is_none());
    overview(&h, cx).update(cx, |view, cx| {
        view.apply_latest_release(&b, SectionState::Loading, cx);
    });
    assert!(cx.debug_bounds("overview-releases").is_none());
    overview(&h, cx).update(cx, |view, cx| {
        view.apply_latest_release(&b, SectionState::Ready(latest), cx);
    });
    assert!(cx.debug_bounds("overview-releases").is_none());
}

#[gpui::test]
fn no_latest_stable_release_is_not_a_repository_without_releases(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let b = fixtures::binding(1);
    // A latest 404 only means no published stable release; the list keeps the block visible.
    apply_releases(
        &h,
        &b,
        SectionState::Ready(fixtures::latest_none_releases()),
        cx,
    );
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-release-count").is_some());
    assert!(cx.debug_bounds("overview-release-version").is_none());
    assert!(cx.debug_bounds("overview-retry-releases").is_none());
    // Draft/prerelease-only data is still a release list, not an empty one. The latest endpoint
    // has no qualifying stable release, but the count and the block stay.
    let mut prerelease_only = fixtures::empty_releases();
    let mut list = fixtures::ready_releases().list;
    list.releases
        .retain(|release| release.prerelease || release.draft);
    assert_eq!(list.releases.len(), 2);
    prerelease_only.list = list;
    apply_releases(&h, &b, SectionState::Ready(prerelease_only), cx);
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-release-count").is_some());
    assert!(cx.debug_bounds("overview-latest-badge").is_none());
    assert!(cx.debug_bounds("overview-release-version").is_none());
}

#[gpui::test]
fn unknown_loading_failed_and_partial_releases_keep_their_block(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let b = fixtures::binding(1);
    // Nothing observed yet is not an empty repository.
    assert!(cx.debug_bounds("overview-releases").is_some());
    apply_releases(&h, &b, SectionState::Loading, cx);
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-release-count").is_none());
    apply_releases(
        &h,
        &b,
        SectionState::Error {
            message: "offline".into(),
        },
        cx,
    );
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-retry-releases").is_some());
    // A partial list may hide releases on unread pages, so it cannot prove emptiness.
    apply_releases(&h, &b, fixtures::partial_releases_state(), cx);
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-release-count").is_none());
    // A latest failure over an empty list keeps its actionable retry.
    apply_releases(
        &h,
        &b,
        SectionState::Ready(fixtures::latest_error_releases()),
        cx,
    );
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-retry-releases").is_some());
}

#[gpui::test]
fn cached_empty_releases_never_paints_the_block_before_or_during_revalidation(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    let b = fixtures::binding(1);
    apply_contributors(
        &h,
        &b,
        SectionState::Ready(fixtures::ready_contributors()),
        cx,
    );
    cx.simulate_resize(size(px(1100.0), px(760.0)));
    // Frame 1 of cache-first paint: the persisted snapshot is historical, both endpoints are
    // historical, and the latest snapshot is a 404 (`release: None`). No Cached / spinner / warning
    // / retry chrome may appear for a section that has no content.
    apply_releases(&h, &b, fixtures::historical_empty_releases(), cx);
    cx.run_until_parked();
    for omitted in [
        "overview-releases",
        "overview-release-count",
        "overview-releases-cached",
        "overview-retry-releases",
    ] {
        assert!(
            cx.debug_bounds(omitted).is_none(),
            "{omitted} must not paint for a cached empty release list"
        );
    }
    let leading_edge = |cx: &mut VisualTestContext| {
        let top = bounds("repository-overview", cx);
        (top.origin.x, bounds("overview-contributors", cx).origin.x)
    };
    let (strip, people) = leading_edge(cx);
    assert!(
        (people - strip).abs() < px(0.5),
        "contributors keep the strip's leading edge with no reserved gap"
    );

    // Background revalidation re-issues the latest read: still no block, and the sibling does not
    // move, so the omission cannot turn into a layout jump.
    overview(&h, cx).update(cx, |view, cx| {
        view.apply_latest_release(&b, SectionState::Loading, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-releases").is_none());
    assert_eq!(leading_edge(cx), (strip, people));

    // Offline revalidation keeps the cached empty list while latest degrades to a historical empty
    // value with a partial reason: the list is still authoritative, so nothing is painted.
    overview(&h, cx).update(cx, |view, cx| {
        view.apply_latest_release(
            &b,
            SectionState::Historical {
                value: fixtures::empty_releases().latest.unwrap(),
                sync_label: Some("5 minutes ago".into()),
                reason: Some("cached snapshot · refresh when online".into()),
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-releases").is_none());
    assert_eq!(leading_edge(cx), (strip, people));

    // A transient offline failure of the latest endpoint cannot resurrect the block either; a
    // complete `/releases` page already listed drafts and prereleases, so it cannot have a latest.
    overview(&h, cx).update(cx, |view, cx| {
        view.apply_latest_release(
            &b,
            SectionState::Error {
                message: "offline fixture".into(),
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-releases").is_none());
    assert_eq!(leading_edge(cx), (strip, people));
    assert!(cx.debug_bounds("overview-contributors").is_some());
}

#[gpui::test]
fn cached_non_empty_releases_keep_their_block_and_cached_mark(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let b = fixtures::binding(1);
    let sync_label = Some("5 minutes ago".to_string());
    let historical = SectionState::Historical {
        value: fixtures::ready_releases().with_provenance(true, false, sync_label.clone()),
        sync_label,
        reason: None,
    };
    apply_releases(&h, &b, historical, cx);
    cx.run_until_parked();
    // Non-empty previews retain their group and provenance without failure chrome.
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-releases-cached").is_none());
    assert!(cx.debug_bounds("overview-retry-releases").is_none());
    overview(&h, cx).update(cx, |view, cx| {
        view.apply_result(
            &b,
            OverviewUpdate::Releases(SectionState::Error {
                message: "request failed".into(),
            }),
            true,
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-releases-cached").is_some());
    assert!(cx.debug_bounds("overview-retry-releases").is_some());
    assert!(cx.debug_bounds("overview-latest-badge").is_none());
    // A list that only holds drafts/prereleases is still a release list, cached or not.
    let mut prerelease_only = fixtures::empty_releases();
    let mut list = fixtures::ready_releases().list;
    list.releases
        .retain(|release| release.prerelease || release.draft);
    prerelease_only.list = list;
    let sync_label = Some("5 minutes ago".to_string());
    apply_releases(
        &h,
        &b,
        SectionState::Historical {
            value: prerelease_only.with_provenance(true, true, sync_label.clone()),
            sync_label,
            reason: None,
        },
        cx,
    );
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-releases").is_some());
    // A cached count would read as freshly verified, so the historical snapshot keeps the group,
    // its Cached mark and its release row without claiming an exact total.
    assert!(cx.debug_bounds("overview-release-count").is_none());
    assert!(cx.debug_bounds("overview-releases-cached").is_some());
}

#[gpui::test]
fn conflicting_empty_list_and_supplied_latest_keep_the_block(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let b = fixtures::binding(1);
    // Two observations that disagree: the list says there are no releases while the independently
    // applied latest endpoint still supplies one. Do not silently drop the only release on screen.
    let mut data = fixtures::empty_releases();
    data.latest = None;
    apply_releases(&h, &b, SectionState::Ready(data), cx);
    overview(&h, cx).update(cx, |view, cx| {
        view.apply_latest_release(
            &b,
            SectionState::Ready(fixtures::ready_releases().latest.unwrap()),
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-release-version").is_some());
}

#[test]
fn language_colors_come_from_the_pinned_linguist_table() {
    let segments = language_segments(&fixtures::github_languages()).unwrap();
    let color = |name: &str| {
        segments
            .iter()
            .find(|segment| segment.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from the bar"))
            .color
    };
    // Values from github-linguist/linguist @ d456527 lib/linguist/languages.yml.
    assert_eq!(color("Rust"), Some(gpui::rgb(0xdea584)));
    assert_eq!(color("TypeScript"), Some(gpui::rgb(0x3178c6)));
    assert_eq!(color("JavaScript"), Some(gpui::rgb(0xf1e05a)));
    assert_eq!(color("Python"), Some(gpui::rgb(0x3572a5)));
    assert_eq!(color("C++"), Some(gpui::rgb(0xf34b7d)));
    assert_eq!(color("Java"), Some(gpui::rgb(0xb07219)));
    // Linguist leaves COBOL colorless and does not know the other name: neutral fallback.
    assert_eq!(color("COBOL"), None);
    assert_eq!(color("Not A Real GitHub Language"), None);
    // Colors do not reorder segments: still by descending checked bytes.
    let names: Vec<&str> = segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Rust",
            "TypeScript",
            "JavaScript",
            "Python",
            "C++",
            "Java",
            "COBOL",
            "Not A Real GitHub Language",
        ]
    );
    assert!((segments[0].share - 0.5).abs() < 1e-12);
    assert!((segments.iter().map(|segment| segment.share).sum::<f64>() - 1.0).abs() < 1e-12);
}

#[gpui::test]
fn colorless_and_unknown_languages_still_draw_one_segment_each(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    apply_languages(
        &h,
        &fixtures::binding(1),
        SectionState::Ready(fixtures::github_languages()),
        cx,
    );
    assert!(cx.debug_bounds("overview-language-bar").is_some());
    // Every language keeps its identity and its own share, including the fallback pair.
    for selector in [
        "overview-language-segment-0",
        "overview-language-segment-1",
        "overview-language-segment-2",
        "overview-language-segment-3",
        "overview-language-segment-4",
        "overview-language-segment-5",
        "overview-language-segment-6",
        "overview-language-segment-7",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "{selector} was not drawn"
        );
    }
    assert!(cx.debug_bounds("overview-language-segment-8").is_none());
}
