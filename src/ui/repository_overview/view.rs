//! Compact, natural-height repository summary strip. No I/O, scrolling or popovers.
use super::glyphs::{Glyph, glyph};
use super::summary::{
    Segment, contributor_display, contributor_is_linked, format_share, language_segments,
};
use super::widgets::{badge, badge_outline, empty, failed, loading, muted};
use super::{OverviewAction, OverviewBinding, OverviewSection, RepositoryOverview, SectionState};
use crate::domain::repository_extras::{
    ContributorKind, ContributorsState, ExtrasCompleteness, Release,
};
use crate::ui::foundation::theme as t;
use nsh_core::prelude::*;
use std::rc::Rc;

type Sink = Rc<dyn Fn(OverviewAction)>;

fn text(value: impl IntoReactive<SharedString>) -> View {
    view().min_w(px(0.0)).overflow_hidden().child(muted(value))
}

fn group(
    id: &'static str,
    title: &'static str,
    count: Option<String>,
    content: AnyElement,
) -> View {
    view()
        .test_id(id)
        .flex_col()
        .gap_1()
        .max_w(px(300.0))
        .flex_none()
        .min_w(px(0.0))
        .overflow_hidden()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    t::label(title)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM),
                )
                .children(
                    count.map(|count| view().test_id("overview-release-count").child(badge(count))),
                ),
        )
        .child(content)
}

fn retry(section: OverviewSection, message: &str, binding: &OverviewBinding, sink: &Sink) -> View {
    let sink = sink.clone();
    let binding = binding.clone();
    failed(
        format!("Retry {}", section.title()),
        message.to_string(),
        Some(Rc::new(move || {
            sink(OverviewAction::Refresh {
                binding: binding.clone(),
                section,
            })
        })),
    )
    .test_id(&format!("overview-retry-{}", section.id()))
}

/// Releases and Contributors, in that order. `state` is the snapshot the host last applied.
pub fn overview(state: &RepositoryOverview, sink: &Sink) -> AnyElement {
    view()
        .test_id("repository-overview")
        .w_full()
        .flex_row()
        .flex_wrap()
        .gap_x(px(32.0))
        .gap_y(px(12.0))
        .py_3()
        .child(release_summary(state, sink))
        .child(contributor_summary(state, sink))
        .into_element()
}

fn release_summary(state: &RepositoryOverview, sink: &Sink) -> View {
    let data = state.releases().value();
    let list_partial = state.releases().is_partial()
        || data.is_some_and(|data| {
            matches!(data.list.completeness, ExtrasCompleteness::Partial { .. })
        });
    let list_historical =
        state.releases().is_historical() || data.is_some_and(|data| data.historical);
    let latest_historical = state
        .latest_release()
        .is_some_and(SectionState::is_historical)
        || (state.latest_release().is_none() && data.is_some_and(|data| data.latest_historical));
    let sync_label = state
        .latest_release()
        .and_then(SectionState::sync_label)
        .or_else(|| state.releases().sync_label())
        .or_else(|| data.and_then(|data| data.sync_label.as_deref()));
    let (release, status): (Option<&Release>, &str) = match state.latest_release() {
        Some(
            SectionState::Ready(value)
            | SectionState::Historical { value, .. }
            | SectionState::Partial { value, .. },
        ) => (value.release.as_ref(), "No latest release"),
        Some(SectionState::Loading) => (None, "Loading…"),
        Some(SectionState::Error { .. }) => (None, "Latest unavailable"),
        Some(SectionState::Idle) => (None, "Not loaded"),
        None => match data {
            Some(data) => (
                data.latest
                    .as_ref()
                    .and_then(|latest| latest.release.as_ref()),
                if data.latest.is_some() || data.latest_error.is_some() {
                    "Latest unavailable"
                } else {
                    "Latest not loaded"
                },
            ),
            None if state.releases().is_loading() => (None, "Loading…"),
            None => (None, "Not loaded"),
        },
    };
    let loading_row = matches!(state.latest_release(), Some(SectionState::Loading))
        || (state.latest_release().is_none() && state.releases().is_loading());

    let mut row = view().flex_row().items_center().gap_2().min_w(px(0.0));
    if release.is_none() && loading_row {
        row = row.child(loading("Loading releases…").test_id("overview-releases-loading"));
    }
    if let Some(release) = release {
        row = row.child(
            view()
                .flex_none()
                .test_id("overview-release-tag-icon")
                .child(glyph(Glyph::Tag)),
        );
        let tag = release.tag_name.clone();
        let url = release.html_url.clone();
        let release_id = release.id;
        let release_tag = release.tag_name.clone();
        let version = view()
            .min_w(px(0.0))
            .overflow_hidden()
            .tooltip(t::label(tag.clone()).text_size(px(12.0)))
            .child(
                t::label(tag.clone())
                    .text_size(px(13.0))
                    .text_color(t::theme().muted_foreground),
            );
        row = row.child(match url {
            Some(url) => {
                let sink = sink.clone();
                let binding = state.binding().clone();
                let action = move || {
                    sink(OverviewAction::OpenRelease {
                        binding: binding.clone(),
                        release_id,
                        tag_name: release_tag.clone(),
                        html_url: Some(url.clone()),
                    })
                };
                version
                    .test_id("overview-release-version")
                    .role(AccessibilityRole::Link)
                    .focusable(true)
                    .cursor_pointer()
                    .label(format!("Release {tag}"))
                    .on_press(action)
            }
            None => version.test_id("overview-release-version"),
        });
        // Historical snapshots are never promoted to a current verified Latest badge.
        let trusted_latest = match state.latest_release() {
            Some(SectionState::Ready(value)) => value.release.is_some(),
            None => data.is_some_and(|data| {
                !data.latest_historical
                    && !data.historical
                    && data.latest_error.is_none()
                    && data
                        .latest
                        .as_ref()
                        .is_some_and(|latest| latest.release.is_some())
            }),
            _ => false,
        };
        if trusted_latest && !release.draft && !release.prerelease {
            row = row.child(
                view()
                    .test_id("overview-latest-badge")
                    .child(badge_outline("Latest", t::theme().primary)),
            );
        }
        if release.draft {
            row = row.child(view().test_id("overview-draft").child(text("Draft")));
        }
        if release.prerelease {
            row = row.child(
                view()
                    .test_id("overview-prerelease")
                    .child(text("Pre-release")),
            );
        }
        if let Some(label) = state.release_date_label(release.id) {
            row = row.child(text(format!("· {label}")));
        }
    } else if !loading_row {
        if status == "No latest release" {
            row = row.child(empty("No latest release"));
        } else {
            row = row.child(text(status));
        }
    }
    let list_failed = state.releases().error();
    let latest_failed = state
        .latest_release()
        .and_then(SectionState::error)
        .or_else(|| {
            state
                .latest_release()
                .is_none()
                .then(|| data.and_then(|data| data.latest_error.as_deref()))
                .flatten()
        });
    if let Some(error) = list_failed {
        row = row.child(retry(
            OverviewSection::Releases,
            error,
            state.binding(),
            sink,
        ));
    } else if let Some(error) = latest_failed {
        row = row.child(retry(
            OverviewSection::Releases,
            error,
            state.binding(),
            sink,
        ));
    }
    if list_partial || state.latest_release().is_some_and(SectionState::is_partial) {
        row = row.child(text("Partial"));
    }
    if list_historical || latest_historical {
        let tip = match sync_label {
            Some(label) => format!("Cached releases snapshot · synced {label}. Click to refresh."),
            None => "Cached releases snapshot. Click to refresh.".to_string(),
        };
        row = row.child(
            view()
                .test_id("overview-releases-cached")
                .tooltip(t::label(tip).text_size(px(12.0)))
                .child(badge_outline("Cached", t::WARNING)),
        );
        if list_failed.is_none() && latest_failed.is_none() {
            row = row.child(retry(
                OverviewSection::Releases,
                "Cached snapshot · refresh when online",
                state.binding(),
                sink,
            ));
        }
    }
    let count = data
        .filter(|_| !list_partial && !list_historical)
        .map(|data| data.list.releases.len().to_string());
    group("overview-releases", "Releases", count, row.into_element())
}

fn contributor_summary(state: &RepositoryOverview, sink: &Sink) -> View {
    let mut row = view().flex_row().items_center().gap_1().min_w(px(0.0));
    if let Some(outcome) = state.contributors().value() {
        if outcome.state == ContributorsState::EmptyRepository {
            row = row.child(text("Empty repository"));
        } else if outcome.contributors.is_empty() {
            row = row.child(text("None returned"));
        }
        for (index, contributor) in outcome.contributors.iter().take(5).enumerate() {
            let name = contributor_display(contributor);
            let linked = contributor_is_linked(contributor);
            let label = if contributor.kind() == ContributorKind::Anonymous {
                format!("{name} · Anonymous contributor")
            } else {
                name.clone()
            };
            let avatar = view()
                .test_id(&format!("overview-avatar-{index}"))
                .size(px(24.0))
                .rounded(px(12.0))
                .items_center()
                .justify_center()
                .bg(t::theme().muted)
                .label(label.clone())
                .tooltip(t::label(label.clone()).text_size(px(12.0)))
                .child(
                    t::label(initial(&name))
                        .text_size(px(11.0))
                        .text_color(t::theme().muted_foreground),
                );
            row = row.child(if linked {
                let sink = sink.clone();
                let binding = state.binding().clone();
                let login = contributor.login.clone();
                let url = contributor.html_url.clone();
                let action = move || {
                    sink(OverviewAction::OpenContributor {
                        binding: binding.clone(),
                        login: login.clone(),
                        html_url: url.clone(),
                    })
                };
                avatar
                    .role(AccessibilityRole::Link)
                    .focusable(true)
                    .cursor_pointer()
                    .on_press(action)
            } else {
                avatar
            });
        }
        if outcome.contributors.len() > 5 {
            row = row.child(text(format!("+{}", outcome.contributors.len() - 5)));
        }
        if state.contributors().is_partial()
            || matches!(outcome.completeness, ExtrasCompleteness::Partial { .. })
        {
            row = row.child(text("Partial"));
        }
        if state.contributors().is_historical() {
            let tip = match state.contributors().sync_label() {
                Some(label) => {
                    format!("Cached contributors snapshot · synced {label}. Click to refresh.")
                }
                None => "Cached contributors snapshot. Click to refresh.".to_string(),
            };
            row = row.child(
                view()
                    .test_id("overview-contributors-cached")
                    .tooltip(t::label(tip).text_size(px(12.0)))
                    .child(badge_outline("Cached", t::WARNING)),
            );
            row = row.child(retry(
                OverviewSection::Contributors,
                "Cached snapshot · refresh when online",
                state.binding(),
                sink,
            ));
        }
    } else if let Some(error) = state.contributors().error() {
        row = row.child(retry(
            OverviewSection::Contributors,
            error,
            state.binding(),
            sink,
        ));
    } else if state.contributors().is_loading() {
        row = row.child(loading("Loading contributors…").test_id("overview-contributors-loading"));
    } else {
        row = row.child(text("Not loaded"));
    }
    group(
        "overview-contributors",
        "Contributors",
        None,
        row.into_element(),
    )
}

fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default()
}

/// Uses the same languages state as the repository service composition.
/// The host places this at the page bottom; no title, legend or text row.
pub fn language_bar(state: &RepositoryOverview) -> AnyElement {
    let Some(segments) = state.languages().value().and_then(language_segments) else {
        return view().into_element();
    };
    if segments.is_empty() {
        return view().into_element();
    }
    let historical = state.languages().is_historical();
    let sync_label = state.languages().sync_label().map(str::to_string);
    view()
        .test_id("overview-language-bar")
        .flex_row()
        .flex_none()
        .w_full()
        .h(px(4.0))
        .overflow_hidden()
        .children(segments.into_iter().enumerate().map(|(index, segment)| {
            let label = language_label(&segment, historical, sync_label.as_deref());
            view()
                .test_id(&format!("overview-language-segment-{index}"))
                .w(Percentage(segment.share as f32))
                .h_full()
                .flex_none()
                .bg(chart_color(segment.color))
                .label(label.clone())
                .tooltip(t::label(label).text_size(px(12.0)))
        }))
        .into_element()
}

fn language_label(segment: &Segment, historical: bool, sync: Option<&str>) -> String {
    match (historical, sync) {
        (true, Some(sync)) => format!(
            "{} {} · cached ({sync})",
            segment.name,
            format_share(segment.share)
        ),
        (true, None) => format!("{} {} · cached", segment.name, format_share(segment.share)),
        (false, _) => format!("{} {}", segment.name, format_share(segment.share)),
    }
}

/// The strip is laid out by flex weights, so each segment takes its share of the row.
fn _percentage_width(segment: &Segment) -> f32 {
    (segment.share * 100.0) as f32
}

fn chart_color(index: usize) -> AdaptiveColor {
    let theme = t::theme();
    match index % 5 {
        0 => theme.chart_1,
        1 => theme.chart_2,
        2 => theme.chart_3,
        3 => theme.chart_4,
        _ => theme.chart_5,
    }
}
