//! GPUI tests for the repository TOP component.
//!
//! They run on GPUI's test platform. They cover the component's own behaviour: stale binding
//! rejection, independent section states, latest-404 ambiguity, draft/prerelease metadata,
//! anonymous and bounded contributor lists, 204 versus empty-content versus error, zero and
//! overflow-safe language output, bounded popover focus/dismissal/scroll, and the TOP layout
//! beside a simulated reader. They do not replace a native window or real GitHub data check.

use std::rc::Rc;

use gpui::{
    AppContext as _, Bounds, Entity, InteractiveElement, Modifiers, ParentElement, Pixels, Point,
    ScrollDelta, ScrollEvent, ScrollSource, StatefulInteractiveElement, Styled, TestAppContext,
    VisualContext, VisualTestContext, point, px, size,
};

use crate::domain::account::{AccountKey, HostId};
use crate::domain::repository_extras::{
    Contributor, ContributorsState, ExtrasCompleteness, ExtrasIncompleteReason, LanguagesOutcome,
    ReleasesOutcome,
};

use super::fixtures;
use super::view;
use super::{
    ContributorsState as OverviewContributorsState, LanguagesState as OverviewLanguagesState,
    OverviewAction, OverviewBinding, OverviewSection, ReleasesData, ReleasesState,
    RepositoryOverview, SectionState,
};

fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

fn setup(cx: &mut TestAppContext) {
    cx.update(ely_core::init_for_tests);
    cx.update(|cx| {
        ely_core::theme::Theme::update(cx, |theme| {
            theme.font_family = "Noto Sans".into();
            theme.mono_family = "Google Sans Code".into();
        });
    });
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
            .flex_1()
            .min_h_0()
            .w_full()
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
    state: OverviewContributorsState,
    cx: &mut VisualTestContext,
) -> bool {
    overview(harness, cx).update(cx, |view, cx| view.apply_contributors(binding, state, cx))
}

fn apply_languages(
    harness: &Entity<Harness>,
    binding: &OverviewBinding,
    state: OverviewLanguagesState,
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

fn scroll_over(selector: &'static str, dy: f32, cx: &mut VisualTestContext) {
    let target = bounds(selector, cx).center();
    cx.simulate_mouse_move(target, None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: target,
        delta: ScrollDelta::Lines(point(0.0, dy)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
}

#[gpui::test]
fn stale_repository_account_and_generation_bindings_are_rejected(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let current = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &current,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));

    let stale_generation = fixtures::binding(2);
    assert!(!apply_releases(
        &harness,
        &stale_generation,
        SectionState::Error {
            message: "stale generation".to_string()
        },
        cx
    ));

    let mut other_account = fixtures::binding(1);
    other_account.account = AccountKey::new(99, HostId::github());
    assert!(!apply_releases(
        &harness,
        &other_account,
        SectionState::Error {
            message: "stale account".to_string()
        },
        cx
    ));

    let state = overview(&harness, cx).read_with(cx, |view, _| view.releases().clone());
    assert!(
        matches!(state, SectionState::Ready(_)),
        "a stale update must not erase the current section"
    );
}

#[gpui::test]
fn one_section_failure_does_not_erase_the_other_sections(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));
    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_contributors()),
        cx
    ));
    assert!(apply_languages(
        &harness,
        &binding,
        SectionState::Error {
            message: "languages failed".to_string()
        },
        cx
    ));
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-contributors").is_some());
    assert!(cx.debug_bounds("overview-languages-error").is_some());

    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Error {
            message: "releases failed".to_string()
        },
        cx
    ));
    assert!(cx.debug_bounds("overview-releases-error").is_some());
    assert!(cx.debug_bounds("overview-contributors").is_some());
    assert!(
        cx.debug_bounds("overview-contributors-empty-content")
            .is_none()
    );
    assert!(cx.debug_bounds("overview-languages-error").is_some());
}

#[gpui::test]
fn latest_404_is_ambiguous_and_a_list_can_still_show_releases(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Ready(fixtures::latest_none_releases()),
        cx
    ));
    assert!(cx.debug_bounds("overview-releases-latest-none").is_some());
    assert!(cx.debug_bounds("overview-release-4").is_some());
    assert!(cx.debug_bounds("overview-releases-empty").is_none());
    assert!(view::latest_none_text().contains("does not prove"));
}

#[gpui::test]
fn draft_and_prerelease_labels_are_preserved(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));
    // Fixture order: index 1 is the pre-release, index 2 is the draft.
    assert!(
        cx.debug_bounds("overview-release-prerelease-row-1-3")
            .is_some()
    );
    assert!(cx.debug_bounds("overview-release-draft-row-2-2").is_some());
    assert!(cx.debug_bounds("overview-release-4").is_some());
}

#[gpui::test]
fn contributors_handle_anonymous_and_large_sets_with_a_bounded_list(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_contributors()),
        cx
    ));
    assert!(
        cx.debug_bounds("overview-contributor-anonymous-1")
            .is_some()
    );
    assert!(cx.debug_bounds("overview-contributor-0").is_some());

    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Ready(fixtures::large_contributors()),
        cx
    ));
    assert!(cx.debug_bounds("overview-contributor-0").is_some());
    assert!(cx.debug_bounds("overview-contributor-2").is_some());
    assert!(
        cx.debug_bounds("overview-contributor-3").is_none(),
        "the panel must cap the visible contributor rows"
    );
}

#[gpui::test]
fn contributor_email_and_raw_json_are_not_displayed(cx: &mut TestAppContext) {
    let anonymous = &fixtures::ready_contributors().contributors[1];
    let secondary = view::contributor_secondary(anonymous);
    assert!(secondary.contains("anonymous"));
    assert!(!secondary.contains("anon@example.invalid"));
    assert_eq!(
        view::contributor_display(anonymous),
        "Anonymous contributor"
    );

    let linked = &fixtures::ready_contributors().contributors[0];
    assert_eq!(view::contributor_display(linked), "Nocpiun");
    assert!(!view::contributor_secondary(linked).contains("never-show@example.invalid"));

    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_contributors()),
        cx
    ));
    assert!(cx.debug_bounds("overview-contributor-0").is_some());
    assert!(
        cx.debug_bounds("overview-contributor-anonymous-1")
            .is_some()
    );
}

#[gpui::test]
fn contributors_204_empty_repository_differs_from_empty_content_and_error(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Ready(fixtures::empty_repository_contributors()),
        cx
    ));
    assert!(
        cx.debug_bounds("overview-contributors-empty-repository")
            .is_some()
    );
    assert!(
        cx.debug_bounds("overview-contributors-empty-content")
            .is_none()
    );

    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Ready(fixtures::empty_content_contributors()),
        cx
    ));
    assert!(
        cx.debug_bounds("overview-contributors-empty-content")
            .is_some()
    );
    assert!(
        cx.debug_bounds("overview-contributors-empty-repository")
            .is_none()
    );

    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Error {
            message: "403 forbidden".to_string()
        },
        cx
    ));
    assert!(cx.debug_bounds("overview-contributors-error").is_some());
    assert!(
        cx.debug_bounds("overview-contributors-empty-content")
            .is_none()
    );
}

#[gpui::test]
fn zero_language_bytes_do_not_infer_a_percentage_distribution(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_languages(
        &harness,
        &binding,
        SectionState::Ready(fixtures::zero_languages()),
        cx
    ));
    assert!(cx.debug_bounds("overview-languages-zero").is_some());
    assert!(cx.debug_bounds("overview-language-0").is_none());
    assert_eq!(view::format_share(None), "—");
}

#[gpui::test]
fn long_language_names_and_many_languages_stay_bounded_and_labelled(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_languages(
        &harness,
        &binding,
        SectionState::Ready(fixtures::many_languages()),
        cx
    ));
    assert!(cx.debug_bounds("overview-language-0").is_some());
    assert!(cx.debug_bounds("overview-language-2").is_some());
    assert!(cx.debug_bounds("overview-language-3").is_none());
    assert!(cx.debug_bounds("overview-language-share-0").is_some());
    assert_eq!(view::format_share(Some(1.0 / 3.0)), "33.3%");
    assert_eq!(view::format_share(Some(1.0)), "100.0%");
}

#[gpui::test]
fn partial_states_render_partial_notes_without_discarding_values(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        fixtures::partial_releases_state(),
        cx
    ));
    assert!(cx.debug_bounds("overview-releases-partial").is_some());
    assert!(cx.debug_bounds("overview-release-4").is_some());

    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Partial {
            value: fixtures::partial_contributors(),
            reason: vec![ExtrasIncompleteReason::PageLimit { limit: 1 }],
        },
        cx
    ));
    assert!(cx.debug_bounds("overview-contributors-partial").is_some());

    assert!(apply_languages(
        &harness,
        &binding,
        fixtures::partial_languages_state(),
        cx
    ));
    assert!(cx.debug_bounds("overview-languages-partial").is_some());
}

#[gpui::test]
fn release_notes_open_a_bounded_popover_and_escape_returns_focus(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));
    assert!(cx.debug_bounds("overview-notes-panel").is_none());
    let before = bounds("repository-overview", cx);

    click("overview-release-notes-4", cx);
    assert!(cx.debug_bounds("overview-notes-panel").is_some());
    assert!(cx.debug_bounds("overview-notes-body").is_some());
    let panel = bounds("overview-notes-panel", cx);
    assert!(
        panel.size.height <= px(260.0),
        "the notes popover must stay bounded, got {:?}",
        panel.size.height
    );
    let after = bounds("repository-overview", cx);
    assert_eq!(
        before.size.height, after.size.height,
        "opening a popover must not expand the TOP strip"
    );

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-notes-panel").is_none());
}

#[gpui::test]
fn popover_scrolling_and_dismissal_do_not_leak_to_the_neighbour_reader(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));
    click("overview-release-notes-4", cx);
    assert!(cx.debug_bounds("overview-notes-panel").is_some());

    let reader_before = reader_offset(&harness, cx);
    scroll_over("overview-notes-body", -40.0, cx);
    let notes_after = overview(&harness, cx).read_with(cx, |view, _| view.notes_offset());
    assert!(
        notes_after.y < px(0.0),
        "the notes popover should scroll, got {notes_after:?}"
    );
    assert_eq!(
        reader_offset(&harness, cx),
        reader_before,
        "scrolling the popover must not move the neighbouring reader"
    );

    // Clicking outside dismisses the popover without changing the reader's own scroll.
    let notes_before_dismiss = overview(&harness, cx).read_with(cx, |view, _| view.notes_offset());
    assert!(notes_before_dismiss.y < px(0.0));
    let reader_bounds = bounds("probe-reader", cx);
    let reader_point = point(
        reader_bounds.right() - px(40.0),
        reader_bounds.top() + px(10.0),
    );
    cx.simulate_click(reader_point, Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-notes-panel").is_none());
    let notes_after_close = overview(&harness, cx).read_with(cx, |view, _| view.notes_offset());
    let reader_before = reader_offset(&harness, cx);
    cx.simulate_mouse_move(reader_point, None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: reader_point,
        delta: ScrollDelta::Lines(point(0.0, -30.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
    let reader_after = reader_offset(&harness, cx);
    assert!(
        reader_after.y < reader_before.y,
        "the reader should still scroll independently"
    );
    let notes_after = overview(&harness, cx).read_with(cx, |view, _| view.notes_offset());
    assert_eq!(
        notes_after, notes_after_close,
        "the closed popover must not share the reader's scroll target"
    );
}

#[gpui::test]
fn notes_panel_fits_a_short_window_and_traps_tab(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));
    let viewport = size(px(480.0), px(700.0));
    cx.simulate_resize(viewport);
    cx.run_until_parked();
    click("overview-release-notes-4", cx);
    assert!(cx.debug_bounds("overview-notes-panel").is_some());
    let panel = bounds("overview-notes-panel", cx);
    assert!(panel.left() >= px(0.0));
    assert!(panel.top() >= px(0.0));
    assert!(
        panel.right() <= viewport.width + px(1.0),
        "panel must fit horizontally, got {:?}",
        panel
    );
    assert!(
        panel.bottom() <= viewport.height + px(1.0),
        "panel must fit vertically, got {:?}",
        panel
    );

    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("overview-notes-panel").is_some(),
        "Tab must not move focus out of the open popover"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("overview-notes-panel").is_none());
}

#[gpui::test]
fn callbacks_use_the_current_binding_and_fire_once_per_press(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));
    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_contributors()),
        cx
    ));

    click("overview-refresh-releases", cx);
    let (count, last) = harness.read_with(cx, |harness, _| {
        (harness.action_count, harness.last_action.clone())
    });
    assert_eq!(count, 1);
    assert_eq!(
        last,
        Some(OverviewAction::Refresh {
            binding: binding.clone(),
            section: OverviewSection::Releases,
        })
    );

    click("overview-release-open-4", cx);
    let last = harness.read_with(cx, |harness, _| harness.last_action.clone());
    assert_eq!(
        last,
        Some(OverviewAction::OpenRelease {
            binding: binding.clone(),
            release_id: 4,
            tag_name: "v1.3.0".to_string(),
            html_url: Some("https://example.invalid/nocpiun/githux/releases/v1.3.0".to_string()),
        })
    );

    click("overview-contributor-open-0", cx);
    let last = harness.read_with(cx, |harness, _| harness.last_action.clone());
    assert_eq!(
        last,
        Some(OverviewAction::OpenContributor {
            binding: binding.clone(),
            login: Some("nocpiun".to_string()),
            html_url: Some("https://example.invalid/nocpiun".to_string()),
        })
    );
}

#[gpui::test]
fn top_layout_stays_bounded_with_space_for_a_reader_at_desktop_and_narrow_widths(
    cx: &mut TestAppContext,
) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &binding,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));
    assert!(apply_contributors(
        &harness,
        &binding,
        SectionState::Ready(fixtures::large_contributors()),
        cx
    ));
    assert!(apply_languages(
        &harness,
        &binding,
        SectionState::Ready(fixtures::many_languages()),
        cx
    ));

    cx.simulate_resize(size(px(1200.0), px(800.0)));
    cx.run_until_parked();
    let desktop = bounds("repository-overview", cx);
    let desktop_reader = bounds("probe-reader", cx);
    assert_eq!(desktop.top(), px(0.0));
    assert!(desktop.bottom() <= desktop_reader.top());
    assert!(
        desktop.size.height <= px(520.0),
        "TOP strip must stay bounded, got {:?}",
        desktop.size.height
    );
    assert_eq!(desktop_reader.left(), desktop.left());
    assert_eq!(desktop_reader.size.width, desktop.size.width);

    cx.simulate_resize(size(px(480.0), px(800.0)));
    cx.run_until_parked();
    let narrow = bounds("repository-overview", cx);
    let narrow_reader = bounds("probe-reader", cx);
    assert_eq!(narrow.top(), px(0.0));
    assert!(narrow.bottom() <= narrow_reader.top());
    assert!(
        narrow.size.height <= px(900.0),
        "narrow TOP strip must stay bounded, got {:?}",
        narrow.size.height
    );
    assert!(cx.debug_bounds("overview-releases").is_some());
    assert!(cx.debug_bounds("overview-contributors").is_some());
    assert!(cx.debug_bounds("overview-languages").is_some());
    assert_eq!(narrow_reader.left(), narrow.left());
    assert_eq!(narrow_reader.size.width, narrow.size.width);
}

#[gpui::test]
fn refresh_actions_are_scoped_per_section(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let binding = fixtures::binding(1);
    click("overview-refresh-contributors", cx);
    let last = harness.read_with(cx, |harness, _| harness.last_action.clone());
    assert_eq!(
        last,
        Some(OverviewAction::Refresh {
            binding: binding.clone(),
            section: OverviewSection::Contributors,
        })
    );
    click("overview-refresh-languages", cx);
    let last = harness.read_with(cx, |harness, _| harness.last_action.clone());
    assert_eq!(
        last,
        Some(OverviewAction::Refresh {
            binding: binding.clone(),
            section: OverviewSection::Languages,
        })
    );
}

#[gpui::test]
fn set_binding_clears_states_and_rejects_old_updates(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let old = fixtures::binding(1);
    assert!(apply_releases(
        &harness,
        &old,
        SectionState::Ready(fixtures::ready_releases()),
        cx
    ));
    let new = fixtures::binding(2);
    let overview_entity = overview(&harness, cx);
    cx.update_window_entity(&overview_entity, |view, window, cx| {
        view.set_binding(new.clone(), window, cx);
    });
    cx.run_until_parked();
    let state = overview_entity.read_with(cx, |view, _| view.releases().clone());
    assert!(matches!(state, SectionState::Idle));
    assert!(!apply_releases(
        &harness,
        &old,
        SectionState::Error {
            message: "old".to_string()
        },
        cx
    ));
}

#[test]
fn helpers_keep_byte_shares_and_reasons_honest() {
    assert_eq!(view::format_share(Some(0.0)), "0.0%");
    assert_eq!(view::format_share(Some(0.125)), "12.5%");
    assert_eq!(view::format_share(None), "—");
    assert_eq!(view::human_bytes(512), "512 bytes");
    assert_eq!(view::human_bytes(1024), "1.0 KiB");
    let text = view::partial_reason_text(&[
        ExtrasIncompleteReason::PageLimit { limit: 2 },
        ExtrasIncompleteReason::SsoPartial {
            organizations: vec!["acme".to_string()],
        },
    ]);
    assert!(text.contains("page limit 2"));
    assert!(text.contains("acme"));
}

#[test]
fn fixtures_keep_latest_and_list_independent() {
    let data = fixtures::latest_none_releases();
    assert!(data.latest.is_some());
    assert!(data.latest.as_ref().unwrap().release.is_none());
    assert!(!data.list.releases.is_empty());
    let list = ReleasesOutcome {
        repository: fixtures::repository(),
        releases: data.list.releases.clone(),
        completeness: ExtrasCompleteness::Complete,
        page_metadata: Vec::new(),
    };
    let _ = ReleasesData::new(list, None);
    let _ = ContributorsState::Content;
    let _ = LanguagesOutcome {
        repository: fixtures::repository(),
        languages: Vec::new(),
        total_bytes: 0,
        metadata: Default::default(),
    };
    let _ = Contributor {
        login: None,
        id: None,
        name: None,
        email: None,
        avatar_url: None,
        html_url: None,
        contributions: 0,
        raw: serde_json::json!({}),
    };
}
