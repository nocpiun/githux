//! Native Shell rendering for the Profile overview.
//!
//! One page-owned scroll view: header, the overview tab marker, the optional
//! README (flow layout, omitted entirely when its absence is confirmed), pinned
//! repositories and the contribution heatmap. Everything reads the published
//! signal; every action is a typed `ProfileActionKind` handed to the page.

use std::rc::Rc;
use std::time::SystemTime;

use nsh_core::prelude::*;

use crate::domain::{ContributionCalendar, ContributionRange, ProfileBasic, ProfilePinned};
use crate::ui::foundation::{controls, theme as t};

use super::view::{heatmap_model, repository_presentation_of};
use super::{
    ProfileActionKind, ProfileProvenance, ProfileRepo, ProfileView, PresentationError,
    RepositoryTarget, SectionState,
};

pub(super) fn page(owner: &ProfileView) -> AnyElement {
    let this = owner.clone();
    dynamic(move || {
        let ui = this.ui.get();
        let _ = this.reader_epoch.get();
        let now = ui.now;
        let request = ui.request();
        let emit: Rc<dyn Fn(ProfileActionKind)> = {
            let this = this.clone();
            Rc::new(move |kind: ProfileActionKind| {
                if this.ui.with(|ui| ui.request()) == request {
                    this.emit(kind);
                }
            })
        };
        let header = identity_header(&this, &ui, &emit);
        let readme = readme_section(&this, &ui, now, &emit);
        let repositories = repositories_section(&ui, now, &emit);
        let contributions = contributions_section(&this, &ui, now, &emit);
        scroll()
            .test_id("profile-overview")
            .size_full()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .scrollbar(true)
            .bg(t::theme().background)
            .child(
                view()
                    .w_full()
                    .min_w(px(0.0))
                    .p(px(t::PAGE_GUTTER))
                    .pt(px(t::PAGE_TOP))
                    .pb(px(t::PAGE_BOTTOM))
                    .flex_col()
                    .gap_3()
                    .child(header)
                    .child(overview_marker())
                    .children(readme)
                    .child(repositories)
                    .child(contributions),
            )
            .into_element()
    })
    .into_element()
}

fn overview_marker() -> AnyElement {
    view()
        .w_full()
        .border_b_1()
        .border_color(t::theme().border)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_2()
                .py_2()
                .px_2()
                .w(px(112.0))
                .border_b(px(2.0))
                .border_color(t::theme().primary)
                .child(
                    text("Overview")
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t::theme().foreground),
                ),
        )
        .into_element()
}

fn clean(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

fn initials(basic: &ProfileBasic) -> String {
    let display = clean(&basic.name).unwrap_or_else(|| basic.login.trim());
    let mut words = display.split_whitespace().filter(|word| !word.is_empty());
    let first = words.next().unwrap_or("?");
    let second = words.next();
    let letter = |word: &str| {
        word.trim_start_matches('@')
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string()
    };
    match second {
        Some(second) => format!("{}{}", letter(first), letter(second)),
        None => letter(first),
    }
}

fn website_label(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_string()
}

fn state_mark(
    id: &str,
    message: &str,
    error: &PresentationError,
    retry: Option<ProfileActionKind>,
    emit: &Rc<dyn Fn(ProfileActionKind)>,
) -> AnyElement {
    let retry = retry.map(|kind| {
        let emit = emit.clone();
        Rc::new(move || emit(kind.clone())) as Rc<dyn Fn()>
    });
    view()
        .test_id(id)
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            crate::ui::compact_status::failed(message.to_string(), error.message(), retry)
                .into_element(),
        )
        .child(
            text(message.to_string())
                .text_size(px(13.0))
                .text_color(t::theme().muted_foreground),
        )
        .into_element()
}

fn loading(id: &str, label: &str) -> AnyElement {
    view()
        .test_id(id)
        .flex_row()
        .items_center()
        .gap_2()
        .py_1()
        .child(
            crate::ui::compact_status::loading(label.to_string()).into_element(),
        )
        .child(
            text(label.to_string())
                .text_size(px(13.0))
                .text_color(t::theme().muted_foreground),
        )
        .into_element()
}

fn provenance_badges(
    id: &str,
    provenance: Option<&ProfileProvenance>,
    now: SystemTime,
) -> Vec<AnyElement> {
    let mut badges = Vec::new();
    let Some(provenance) = provenance else {
        return badges;
    };
    if provenance.is_historical() {
        let label = provenance
            .relative_time(now)
            .map(|relative| format!("Cached · {relative}"))
            .unwrap_or_else(|| "Cached".to_string());
        badges.push(
            view()
                .test_id(&format!("{id}-provenance-historical"))
                .tooltip(controls::tooltip(
                    "Historical snapshot loaded from local cache.",
                ))
                .child(badge(label))
                .into_element(),
        );
    }
    if let Some(reason) = provenance.partial_reason() {
        let reason = reason.to_string();
        badges.push(
            view()
                .test_id(&format!("{id}-provenance-partial"))
                .tooltip(controls::tooltip(reason))
                .child(badge("Partial".to_string()))
                .into_element(),
        );
    }
    badges
}

fn badge(label: String) -> AnyElement {
    view()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(t::theme().radius)
        .bg(t::theme().muted)
        .child(
            text(label)
                .text_size(px(11.0))
                .text_color(t::theme().muted_foreground),
        )
        .into_element()
}

fn identity_header(
    _view: &ProfileView,
    ui: &super::view::ProfileUi,
    emit: &Rc<dyn Fn(ProfileActionKind)>,
) -> AnyElement {
    match &ui.basic {
        SectionState::Ready(basic) => ready_header(ui, basic, emit),
        SectionState::Error(error) => view()
            .test_id("profile-basic")
            .w_full()
            .child(state_mark(
                "profile-basic-error",
                "Profile could not be loaded",
                error,
                Some(ProfileActionKind::RetryBasic),
                emit,
            ))
            .into_element(),
        _ => view()
            .test_id("profile-basic")
            .w_full()
            .child(loading("profile-basic-loading", "Loading profile…"))
            .into_element(),
    }
}

fn ready_header(
    ui: &super::view::ProfileUi,
    basic: &ProfileBasic,
    emit: &Rc<dyn Fn(ProfileActionKind)>,
) -> AnyElement {
    let display = clean(&basic.name).unwrap_or(&basic.login).to_string();
    let avatar = match &ui.avatar {
        Some(source) => image(source.clone())
            .test_id("profile-avatar-image")
            .size(px(84.0))
            .rounded_full()
            .object_fit(ResizeMode::Cover)
            .into_element(),
        None => view()
            .test_id("profile-avatar-initials")
            .size(px(84.0))
            .rounded_full()
            .bg(t::theme().muted)
            .items_center()
            .justify_center()
            .child(
                text(initials(basic))
                    .text_size(px(28.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t::theme().muted_foreground),
            )
            .into_element(),
    };
    let mut metadata: Vec<AnyElement> = Vec::new();
    if let Some(location) = clean(&basic.location) {
        metadata.push(meta(location.to_string()));
    }
    if let Some(url) = clean(&basic.website_url) {
        let url = url.to_string();
        let label = website_label(&url);
        let emit = emit.clone();
        metadata.push(
            view()
                .test_id("profile-website")
                .role(AccessibilityRole::Link)
                .focusable(true)
                .cursor_pointer()
                .tooltip(controls::tooltip(url.clone()))
                .on_press(move |_: &()| {
                    emit(ProfileActionKind::ExternalLink { url: url.clone() })
                })
                .child(
                    text(label)
                        .text_size(px(13.0))
                        .text_color(t::theme().primary),
                )
                .into_element(),
        );
    }
    let follow = match (basic.followers, basic.following) {
        (Some(a), Some(b)) => Some(format!("{a} followers · {b} following")),
        (Some(a), None) => Some(format!("{a} followers")),
        (None, Some(b)) => Some(format!("{b} following")),
        _ => None,
    };
    if let Some(follow) = follow {
        metadata.push(meta(follow));
    }
    if let Some(company) = clean(&basic.company) {
        metadata.push(meta(company.to_string()));
    }
    let badges = provenance_badges("profile-basic", ui.basic_provenance.as_ref(), ui.now);
    view()
        .test_id("profile-basic")
        .w_full()
        .min_w(px(0.0))
        .flex_row()
        .items_start()
        .gap_5()
        .child(avatar)
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_col()
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            view().test_id("profile-name").child(
                                text(display)
                                    .text_size(px(24.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t::theme().foreground),
                            ),
                        )
                        .children(badges),
                )
                .child(
                    view().test_id("profile-login").child(
                        text(format!("@{}", basic.login))
                            .text_size(px(14.0))
                            .text_color(t::theme().muted_foreground),
                    ),
                )
                .children(clean(&basic.bio).map(|bio| {
                    view()
                        .test_id("profile-bio")
                        .mt_1()
                        .w_full()
                        .child(
                            text(bio.to_string())
                                .text_size(px(13.0))
                                .text_color(t::theme().muted_foreground),
                        )
                }))
                .children((!metadata.is_empty()).then(|| {
                    view()
                        .test_id("profile-public-metadata")
                        .w_full()
                        .min_w(px(0.0))
                        .mt_1()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_x(px(16.0))
                        .gap_y(px(4.0))
                        .children(metadata)
                })),
        )
        .into_element()
}

fn meta(text_value: String) -> AnyElement {
    view()
        .min_w(px(0.0))
        .child(
            text(text_value)
                .text_size(px(13.0))
                .text_color(t::theme().muted_foreground),
        )
        .into_element()
}

fn readme_section(
    owner: &ProfileView,
    ui: &super::view::ProfileUi,
    now: SystemTime,
    emit: &Rc<dyn Fn(ProfileActionKind)>,
) -> Option<AnyElement> {
    // A confirmed absence is a normal optional outcome: the whole section, its
    // border, header and any error sentence are omitted (LESSONS L014).
    if matches!(ui.readme, SectionState::Unavailable(_)) {
        return None;
    }
    let body = match &ui.readme {
        SectionState::Unavailable(_) => return None,
        SectionState::Loading => loading("profile-readme-loading", "Loading profile README…"),
        SectionState::Error(error) => state_mark(
            "profile-readme-error",
            "Profile README could not be loaded",
            error,
            Some(ProfileActionKind::RetryReadme),
            emit,
        ),
        SectionState::Ready(()) => match super::view::reader_element(owner) {
            Some(reader) => view()
                .test_id("profile-readme-reader")
                .w_full()
                .max_w(px(f32::MAX))
                .min_w(px(0.0))
                // Flow: the reader reports its natural height and this page owns
                // the only scroll view.
                .child(reader)
                .into_element(),
            None => loading("profile-readme-empty", "Loading profile README…"),
        },
    };
    let badges = provenance_badges("profile-readme", ui.readme_provenance.as_ref(), now);
    let identity = ui.readme_identity.clone();
    let header = (identity.is_some() || !badges.is_empty()).then(|| {
        view()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap_2()
            .children(identity.map(|identity| {
                view().child(
                    text(identity)
                        .font_family(crate::ui::foundation::fonts::CODE_FAMILY)
                        .text_size(px(11.0))
                        .text_color(t::theme().muted_foreground),
                )
            }))
            .children(badges)
    });
    Some(
        view()
            .test_id("profile-readme-section")
            .w_full()
            .min_w(px(0.0))
            .flex_col()
            .gap_1()
            .p_3()
            .rounded(t::theme().radius)
            .border_1()
            .border_color(t::theme().border)
            .children(header)
            .child(body)
            .into_element(),
    )
}

fn section_heading(title: &str, detail: Option<String>, badges: Vec<AnyElement>) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(title.to_string())
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t::theme().foreground),
                ),
        )
        .children(detail.map(|detail| {
            view().child(
                text(detail)
                    .text_size(px(12.0))
                    .text_color(t::theme().muted_foreground),
            )
        }))
        .children(badges)
        .into_element()
}

fn repositories_section(
    ui: &super::view::ProfileUi,
    now: SystemTime,
    emit: &Rc<dyn Fn(ProfileActionKind)>,
) -> AnyElement {
    let Some(presentation) = repository_presentation_of(&ui.repositories) else {
        let (body, retry) = match &ui.repositories {
            SectionState::Loading => (
                loading("profile-repositories-loading", "Loading repository pins…"),
                None,
            ),
            SectionState::Error(error) => (
                state_mark(
                    "profile-repositories-error",
                    "Pinned repositories could not be loaded",
                    error,
                    Some(ProfileActionKind::RetryRepositories),
                    emit,
                ),
                Some(()),
            ),
            SectionState::Unavailable(message) => (
                view()
                    .p_3()
                    .child(
                        text(message.clone())
                            .text_size(px(13.0))
                            .text_color(t::theme().muted_foreground),
                    )
                    .into_element(),
                None,
            ),
            SectionState::Ready(_) => unreachable!(),
        };
        let _ = retry;
        return view()
            .test_id("profile-repositories-section")
            .w_full()
            .flex_col()
            .gap_2()
            .child(section_heading("Pinned repositories", None, Vec::new()))
            .child(panel(body))
            .into_element();
    };
    let count = presentation.repositories.len();
    let badges = provenance_badges("profile-repositories", ui.repositories_provenance.as_ref(), now);
    let cards: Vec<AnyElement> = presentation
        .repositories
        .iter()
        .enumerate()
        .map(|(index, repository)| repository_card(index, repository, emit))
        .collect();
    let body = if cards.is_empty() {
        view()
            .test_id("profile-repositories-empty")
            .p_3()
            .child(
                text(
                    presentation
                        .empty
                        .unwrap_or("No repository pins were returned.")
                        .to_string(),
                )
                .text_size(px(13.0))
                .text_color(t::theme().muted_foreground),
            )
            .into_element()
    } else {
        view()
            .test_id("profile-repository-list")
            .flex_row()
            .flex_wrap()
            .gap_2()
            .w_full()
            .children(cards)
            .into_element()
    };
    view()
        .test_id("profile-repositories-section")
        .w_full()
        .min_w(px(0.0))
        .flex_col()
        .gap_2()
        .child(section_heading(
            presentation.title,
            Some(format!("{count}")),
            badges,
        ))
        .children(presentation.note.map(|note| {
            view().test_id("profile-repository-note").child(
                text(note)
                    .text_size(px(11.0))
                    .text_color(t::theme().muted_foreground),
            )
        }))
        .child(body)
        .into_element()
}

fn repository_card(
    index: usize,
    repository: &ProfileRepo,
    emit: &Rc<dyn Fn(ProfileActionKind)>,
) -> AnyElement {
    let target = RepositoryTarget {
        id: repository.id,
        name_with_owner: repository
            .name_with_owner
            .clone()
            .unwrap_or_else(|| repository.name.clone()),
        url: repository.url.clone(),
    };
    let emit = emit.clone();
    let owner = repository
        .name_with_owner
        .as_deref()
        .and_then(|name| name.split_once('/'))
        .map(|(owner, _)| owner.to_string());
    let mut stats: Vec<AnyElement> = Vec::new();
    if let Some(language) = clean(&repository.primary_language) {
        stats.push(meta(language.to_string()));
    }
    if let Some(stars) = repository.stargazer_count {
        stats.push(meta(format!("★ {stars}")));
    }
    if let Some(forks) = repository.fork_count {
        stats.push(meta(format!("{} forks", forks)));
    }
    view()
        .test_id(&format!("profile-repository-{index}"))
        .flex_1()
        .min_w(px(300.0))
        .min_h(px(96.0))
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(t::theme().radius)
        .border_1()
        .border_color(t::theme().border)
        .child(
            view()
                .flex_col()
                .gap_1()
                .min_w(px(0.0))
                .children(owner.map(|owner| {
                    view().child(
                        text(owner)
                            .text_size(px(11.0))
                            .text_color(t::theme().muted_foreground),
                    )
                }))
                .child(
                    view()
                        .test_id(&format!("profile-repository-name-{index}"))
                        .role(AccessibilityRole::Link)
                        .label(repository.name.clone())
                        .focusable(true)
                        .cursor_pointer()
                        .on_press(move |_: &()| {
                            emit(ProfileActionKind::Repository(target.clone()))
                        })
                        .child(
                            text(repository.name.clone())
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t::theme().primary),
                        ),
                ),
        )
        .children(
            clean(&repository.description).map(|description| {
                view()
                    .test_id(&format!("profile-repository-description-{index}"))
                    .min_w(px(0.0))
                    .child(
                        text(description.to_string())
                            .number_of_lines(3u32)
                            .text_size(px(12.0))
                            .text_color(t::theme().muted_foreground),
                    )
            }),
        )
        .children((!stats.is_empty()).then(|| {
            view()
                .test_id(&format!("profile-repository-stats-{index}"))
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap_3()
                .children(stats)
        }))
        .into_element()
}

fn contributions_section(
    owner: &ProfileView,
    ui: &super::view::ProfileUi,
    now: SystemTime,
    emit: &Rc<dyn Fn(ProfileActionKind)>,
) -> AnyElement {
    let range = ui.range.clone();
    let detail = format!("{} → {}", range.from(), range.to());
    let badges = provenance_badges(
        "profile-contributions",
        ui.contributions_provenance.as_ref(),
        now,
    );
    let body = match &ui.contributions {
        SectionState::Loading => loading("profile-contributions-loading", "Loading contributions…"),
        SectionState::Error(error) => state_mark(
            "profile-contributions-error",
            "Contributions could not be loaded",
            error,
            Some(ProfileActionKind::RetryContributions),
            emit,
        ),
        SectionState::Unavailable(message) => view()
            .p_3()
            .child(
                text(message.clone())
                    .text_size(px(13.0))
                    .text_color(t::theme().muted_foreground),
            )
            .into_element(),
        SectionState::Ready(calendar) => heatmap(owner, ui, &range, calendar),
    };
    view()
        .test_id("profile-contributions-section")
        .w_full()
        .min_w(px(0.0))
        .flex_col()
        .gap_2()
        .child(section_heading("Contributions", Some(detail), badges))
        .child(body)
        .into_element()
}

fn level_color(level: Option<super::CellLevel>) -> AdaptiveColor {
    match level {
        Some(super::CellLevel::Known(0)) => t::theme().muted,
        Some(super::CellLevel::Known(1)) => AdaptiveColor {
            light: rgb(0x9be9a8).to_hsla(),
            dark: rgb(0x0e4429).to_hsla(),
        },
        Some(super::CellLevel::Known(2)) => AdaptiveColor {
            light: rgb(0x40c463).to_hsla(),
            dark: rgb(0x006d32).to_hsla(),
        },
        Some(super::CellLevel::Known(3)) => AdaptiveColor {
            light: rgb(0x30a14e).to_hsla(),
            dark: rgb(0x26a641).to_hsla(),
        },
        Some(super::CellLevel::Known(4)) => AdaptiveColor {
            light: rgb(0x216e39).to_hsla(),
            dark: rgb(0x39d353).to_hsla(),
        },
        _ => t::theme().border,
    }
}

fn heatmap(
    owner: &ProfileView,
    ui: &super::view::ProfileUi,
    range: &ContributionRange,
    calendar: &ContributionCalendar,
) -> AnyElement {
    let Some(model) = heatmap_model(range, &SectionState::Ready(calendar.clone())) else {
        return view().into_element();
    };
    let selected = ui.selected_day;
    let mut columns: Vec<AnyElement> = Vec::new();
    let mut day_index = 0usize;
    for (week_index, week) in model.weeks.iter().enumerate() {
        let mut cells: Vec<AnyElement> = Vec::new();
        for day in &week.days {
            let index = day_index;
            let level = day.level();
            let is_selected = selected == Some(index);
            cells.push(
                view()
                    .test_id(&format!("profile-contribution-day-{index}"))
                    .size(px(11.0))
                    .rounded(px(2.0))
                    .bg(level_color(level))
                    .when(is_selected, |cell| {
                        cell.border_1().border_color(t::theme().foreground)
                    })
                    .label(day.detail())
                    .into_element(),
            );
            day_index += 1;
        }
        columns.push(
            view()
                .test_id(&format!("profile-contribution-week-{week_index}"))
                .flex_col()
                .gap(px(3.0))
                .children(cells)
                .into_element(),
        );
    }
    let detail = selected.and_then(|_| owner.selected_day_detail());
    let mut summary = format!(
        "{} contributions in the last year ({} days observed)",
        model.total_contributions, model.observed_days
    );
    if model.missing_days > 0 {
        summary.push_str(&format!("; {} days missing", model.missing_days));
    }
    if model.invalid_days > 0 {
        summary.push_str(&format!("; {} days invalid", model.invalid_days));
    }
    if model.has_field_errors {
        summary.push_str("; some contribution fields had errors");
    }
    view()
        .test_id("profile-heatmap")
        .w_full()
        .min_w(px(0.0))
        .flex_col()
        .gap_2()
        .child(
            scroll()
                .test_id("profile-heatmap-scroll")
                .horizontal()
                .scrollbar(false)
                .child(
                    view()
                        .test_id("profile-heatmap-grid")
                        .flex_row()
                        .gap(px(3.0))
                        .children(columns),
                ),
        )
        .child(
            view().test_id("profile-heatmap-summary").child(
                text(summary)
                    .text_size(px(12.0))
                    .text_color(t::theme().muted_foreground),
            ),
        )
        .children(detail.map(|detail| {
            view().test_id("profile-day-detail").child(
                text(detail)
                    .text_size(px(12.0))
                    .text_color(t::theme().foreground),
            )
        }))
        .into_element()
}

fn panel(child: AnyElement) -> AnyElement {
    view()
        .w_full()
        .min_w(px(0.0))
        .p_3()
        .rounded(t::theme().radius)
        .border_1()
        .border_color(t::theme().border)
        .child(child)
        .into_element()
}
