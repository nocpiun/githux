//! Compact rendering; the host supplies all observations and decoded images.
use super::*;
use crate::domain::repository_extras::{
    Contributor, ContributorKind, ContributorsState as DomainContributorsState, ExtrasCompleteness,
    LanguagesOutcome, Release,
};
use crate::ui::badge::Badge;
use ely_core::{
    data_display::Avatar,
    data_display::Tone,
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, AvatarSize, IconSize, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Context, FontWeight, IntoElement, ParentElement, Render, Styled, Window, div,
    prelude::*, px, relative,
};

impl Render for RepositoryOverview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let releases = self.release_summary(cx);
        let contributors = self.contributor_summary(cx);
        div()
            .id("repository-overview")
            .debug_selector(|| "repository-overview".into())
            .w_full()
            .flex_none()
            .flex()
            .flex_wrap()
            .gap_x_8()
            .gap_y_3()
            .py_3()
            .text_color(cx.theme().colors.fg)
            .text_size(cx.theme().text_size(TextSize::Base))
            .child(releases)
            .child(contributors)
    }
}
fn group(id: &'static str, title: String, content: AnyElement, cx: &App) -> AnyElement {
    group_with_count(id, title, None, content, cx)
}
fn group_with_count(
    id: &'static str,
    title: String,
    count: Option<String>,
    content: AnyElement,
    cx: &App,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .flex_col()
        .gap_1p5()
        .max_w(px(300.0))
        .flex_none()
        .min_w_0()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_size(cx.theme().text_size(TextSize::Base))
                .font_weight(FontWeight::MEDIUM)
                .child(title)
                .children(count.map(|count| {
                    div()
                        .debug_selector(|| "overview-release-count".into())
                        .child(Badge::new(count))
                })),
        )
        .child(content)
        .into_any_element()
}
fn activation_key(event: &gpui::KeyDownEvent) -> bool {
    !event.is_held
        && !event.keystroke.modifiers.modified()
        && matches!(event.keystroke.key.as_str(), "enter" | "space")
}
fn text(label: impl Into<String>, cx: &App) -> AnyElement {
    div()
        .min_w_0()
        .text_color(cx.theme().colors.fg_muted)
        .child(Ellipsis::new(label.into()))
        .into_any_element()
}
impl RepositoryOverview {
    fn retry(&self, section: OverviewSection, message: &str, cx: &mut Context<Self>) -> AnyElement {
        let binding = self.binding.clone();
        let label = format!("Retry {}", section.title());
        let reason = message.to_string();
        let section_label = section.title().to_string();
        let _ = section_label;
        let this = cx.entity().downgrade();
        let id = format!("overview-retry-{}", section.id());
        crate::ui::compact_status::failed(
            gpui::ElementId::Name(id.clone().into()),
            label,
            reason,
            Some(move |window: &mut gpui::Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |view, cx| {
                    view.emit(
                        OverviewAction::Refresh {
                            binding: binding.clone(),
                            section,
                        },
                        window,
                        cx,
                    );
                });
            }),
            Some(Box::leak(id.into_boxed_str())),
            cx,
        )
    }
    fn release_summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let data = self.releases.value();
        let list_partial = self.releases.is_partial()
            || data.is_some_and(|data| {
                matches!(data.list.completeness, ExtrasCompleteness::Partial { .. })
            });
        let title = "Releases".into();
        let (release, status): (Option<&Release>, &str) = match self.latest.as_ref() {
            Some(SectionState::Ready(value) | SectionState::Partial { value, .. }) => {
                (value.release.as_ref(), "No latest release")
            }
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
                None if self.releases.is_loading() => (None, "Loading…"),
                None => (None, "Not loaded"),
            },
        };
        let loading_row = matches!(self.latest.as_ref(), Some(SectionState::Loading))
            || (self.latest.is_none() && self.releases.is_loading());
        let mut row = div().flex().items_center().gap_2().min_w_0();
        if release.is_none() && loading_row {
            row = row.child(crate::ui::compact_status::loading(
                "overview-releases-loading",
                "Loading releases…",
                cx,
            ));
        }
        if let Some(release) = release {
            let id = release.id;
            let tag = release.tag_name.clone();
            let url = release.html_url.clone();
            let binding = self.binding.clone();
            let key_action = OverviewAction::OpenRelease {
                binding: binding.clone(),
                release_id: id,
                tag_name: tag.clone(),
                html_url: url.clone(),
            };
            let version = div()
                .id("overview-release-version")
                .debug_selector(|| "overview-release-version".into())
                .tooltip(Tooltip::text(tag.clone()))
                .min_w_0()
                .child(Ellipsis::new(tag.clone()));
            row = row.child(
                div()
                    .flex_none()
                    .debug_selector(|| "overview-release-tag-icon".into())
                    .child(
                        Icon::new(IconName::Tag)
                            .size(IconSize::Xs)
                            .color(cx.theme().colors.fg_muted),
                    ),
            );
            row = row.child(if url.is_some() {
                version
                    .tab_index(0)
                    .cursor_pointer()
                    .text_color(cx.theme().colors.link)
                    .aria_label(format!("Release {tag}"))
                    .on_key_down(cx.listener(
                        move |view, event: &gpui::KeyDownEvent, window, cx| {
                            if activation_key(event) {
                                cx.stop_propagation();
                                view.emit(key_action.clone(), window, cx);
                            }
                        },
                    ))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.emit(
                            OverviewAction::OpenRelease {
                                binding: binding.clone(),
                                release_id: id,
                                tag_name: tag.clone(),
                                html_url: url.clone(),
                            },
                            window,
                            cx,
                        )
                    }))
                    .into_any_element()
            } else {
                version.into_any_element()
            });
            let trusted_latest = match self.latest.as_ref() {
                Some(SectionState::Ready(value)) => value.release.is_some(),
                None => data.is_some_and(|data| {
                    data.latest_error.is_none()
                        && data
                            .latest
                            .as_ref()
                            .is_some_and(|latest| latest.release.is_some())
                }),
                _ => false,
            };
            if trusted_latest && !release.draft && !release.prerelease {
                row = row.child(
                    div()
                        .debug_selector(|| "overview-latest-badge".into())
                        .child(Badge::new("Latest").tone(Tone::Success).outline()),
                );
            }
            if release.draft {
                row = row.child(
                    div()
                        .debug_selector(|| "overview-draft".into())
                        .child(text("Draft", cx)),
                );
            }
            if release.prerelease {
                row = row.child(
                    div()
                        .debug_selector(|| "overview-prerelease".into())
                        .child(text("Pre-release", cx)),
                );
            }
            if let Some((date_id, label)) = &self.date_label
                && *date_id == id
            {
                row = row.child(text(format!("· {label}"), cx));
            }
        } else if !loading_row {
            if status == "No latest release" {
                row = row.child(crate::ui::compact_status::empty("No latest release", cx));
            } else {
                row = row.child(text(status, cx));
            }
        }
        let list_failed = self.releases.error();
        let latest_failed = self
            .latest
            .as_ref()
            .and_then(SectionState::error)
            .or_else(|| {
                self.latest
                    .is_none()
                    .then(|| data.and_then(|data| data.latest_error.as_deref()))
                    .flatten()
            });
        if let Some(error) = list_failed {
            row = row.child(self.retry(OverviewSection::Releases, error, cx));
        } else if let Some(error) = latest_failed {
            row = row.child(self.retry(OverviewSection::Releases, error, cx));
        }
        if list_partial || self.latest.as_ref().is_some_and(SectionState::is_partial) {
            row = row.child(text("Partial", cx));
        }
        let count = data
            .filter(|_| !list_partial)
            .map(|data| data.list.releases.len().to_string());
        group_with_count(
            "overview-releases",
            title,
            count,
            row.into_any_element(),
            cx,
        )
    }
    fn contributor_summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().flex().items_center().gap_1().min_w_0();
        if let Some(outcome) = self.contributors.value() {
            if outcome.state == DomainContributorsState::EmptyRepository {
                row = row.child(text("Empty repository", cx));
            } else if outcome.contributors.is_empty() {
                row = row.child(text("None returned", cx));
            }
            for (index, contributor) in outcome.contributors.iter().take(5).enumerate() {
                let name = contributor_display(contributor);
                let linked = contributor.kind() == ContributorKind::GitHubUser
                    && contributor.html_url.is_some();
                let label = if contributor.kind() == ContributorKind::Anonymous {
                    format!("{name} · Anonymous contributor")
                } else {
                    name.clone()
                };
                let mut avatar =
                    Avatar::new(("overview-avatar-image", index), name).size(AvatarSize::Sm);
                if let Some(image) = contributor.id.and_then(|id| self.avatar_images.get(&id)) {
                    avatar = avatar.image(image.clone());
                }
                let avatar = div()
                    .id(("overview-avatar", index))
                    .debug_selector(move || format!("overview-avatar-{index}"))
                    .aria_label(label.clone())
                    .tooltip(Tooltip::text(label))
                    .child(avatar);
                row = row.child(if linked {
                    let binding = self.binding.clone();
                    let login = contributor.login.clone();
                    let url = contributor.html_url.clone();
                    let key_action = OverviewAction::OpenContributor {
                        binding: binding.clone(),
                        login: login.clone(),
                        html_url: url.clone(),
                    };
                    avatar
                        .tab_index(0)
                        .cursor_pointer()
                        .on_key_down(cx.listener(
                            move |view, event: &gpui::KeyDownEvent, window, cx| {
                                if activation_key(event) {
                                    cx.stop_propagation();
                                    view.emit(key_action.clone(), window, cx);
                                }
                            },
                        ))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.emit(
                                OverviewAction::OpenContributor {
                                    binding: binding.clone(),
                                    login: login.clone(),
                                    html_url: url.clone(),
                                },
                                window,
                                cx,
                            )
                        }))
                        .into_any_element()
                } else {
                    avatar.into_any_element()
                });
            }
            if outcome.contributors.len() > 5 {
                row = row.child(text(format!("+{}", outcome.contributors.len() - 5), cx));
            }
            if self.contributors.is_partial()
                || matches!(outcome.completeness, ExtrasCompleteness::Partial { .. })
            {
                row = row.child(text("Partial", cx));
            }
        } else if let Some(error) = self.contributors.error() {
            row = row.child(self.retry(OverviewSection::Contributors, error, cx));
        } else if self.contributors.is_loading() {
            row = row.child(crate::ui::compact_status::loading(
                "overview-contributors-loading",
                "Loading contributors…",
                cx,
            ));
        } else {
            row = row.child(text("Not loaded", cx));
        }
        group(
            "overview-contributors",
            "Contributors".into(),
            row.into_any_element(),
            cx,
        )
    }
    /// Uses the same languages state as the repository service composition.
    /// The host places this at the page bottom; no title, legend or text row.
    pub fn language_bar(&self, cx: &App) -> AnyElement {
        let Some(segments) = self.languages.value().and_then(language_segments) else {
            return div().into_any_element();
        };
        if segments.is_empty() {
            return div().into_any_element();
        }
        div()
            .id("overview-language-bar")
            .debug_selector(|| "overview-language-bar".into())
            .flex()
            .flex_none()
            .w_full()
            .h(px(4.0))
            .overflow_hidden()
            .children(segments.into_iter().enumerate().map(|(index, segment)| {
                let label = format!("{} {}", segment.name, format_share(segment.share));
                div()
                    .id(("overview-language-segment", index))
                    .debug_selector(move || format!("overview-language-segment-{index}"))
                    .w(relative(segment.share as f32))
                    .h_full()
                    .flex_none()
                    .bg(cx.theme().colors.chart[segment.color])
                    .aria_label(label.clone())
                    .tooltip(Tooltip::rich(move |_, _| {
                        div()
                            .debug_selector(|| "repository-language-tooltip".into())
                            .child(label.clone())
                            .into_any_element()
                    }))
            }))
            .into_any_element()
    }
}

pub(crate) fn contributor_display(contributor: &Contributor) -> String {
    contributor
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .or(contributor.login.as_deref())
        .unwrap_or("Anonymous contributor")
        .to_string()
}
#[derive(Debug)]
pub(crate) struct Segment {
    pub name: String,
    pub share: f64,
    pub color: usize,
}
/// Checked byte arithmetic; never trust independently rounded/malformed supplied shares.
pub(crate) fn language_segments(outcome: &LanguagesOutcome) -> Option<Vec<Segment>> {
    let total = outcome
        .languages
        .iter()
        .try_fold(0u64, |sum, item| sum.checked_add(item.bytes))?;
    if total != outcome.total_bytes {
        return None;
    }
    if total == 0 {
        return Some(Vec::new());
    }
    let mut languages: Vec<_> = outcome
        .languages
        .iter()
        .filter(|item| item.bytes > 0)
        .collect();
    languages.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
    let result: Vec<_> = languages
        .iter()
        .map(|item| Segment {
            name: item.name.clone(),
            share: item.bytes as f64 / total as f64,
            color: language_color(&item.name),
        })
        .collect();
    Some(result)
}
fn language_color(name: &str) -> usize {
    name.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as usize)
    }) % 8
}
pub(crate) fn format_share(share: f64) -> String {
    if share > 0.0 && share < 0.005 {
        "<1%".into()
    } else {
        format!("{:.0}%", share * 100.0)
    }
}
