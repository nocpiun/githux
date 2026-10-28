use ely_core::{
    data_display::Avatar,
    primitives::{Icon, IconName},
    theme::{ActiveTheme, AvatarSize, ControlSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{AnyElement, App, Context, FontWeight, IntoElement, Window, div, prelude::*, px};

use crate::domain::error::GitHubError;
use crate::domain::home::{
    FeedActor, FeedEvent, FeedEventType, FeedOutcome, FeedPartialReason, FeedPayload,
    FeedReadiness, HomeError, HomeItem, HomeItemKind, HomePartialReason, HomeReadiness,
    HomeSearchOutcome,
};

use super::{Column, HomeView, ProfileIdentity, RepositoryIdentity};

pub(super) const HOME_ROW_HEIGHT: f32 = 66.0;
pub(super) const FEED_ROW_HEIGHT: f32 = 84.0;

pub(super) struct Summary {
    pub count: String,
    pub notice: String,
}

pub(super) fn home_summary(outcome: &HomeSearchOutcome) -> Summary {
    let count = format!(
        "{} shown · {} matched",
        outcome.items.len(),
        outcome.total_count
    );
    let notice = match &outcome.readiness {
        HomeReadiness::Complete if outcome.incomplete_results => {
            "GitHub marked the search results incomplete. involves:current login · updated descending · no time window; this is not an unread or complete-history list.".to_string()
        }
        HomeReadiness::Complete => {
            "involves:current login · updated descending · no time window. Search order is supplied by the host; this is not an unread or complete-history list.".to_string()
        }
        HomeReadiness::Partial { reasons } => {
            let details = reasons.iter().map(home_partial).collect::<Vec<_>>().join(" ");
            format!(
                "Partial result · {details} The visible entries keep the supplied updated-descending order; this is not a complete history or unread list."
            )
        }
    };
    Summary { count, notice }
}

fn home_partial(reason: &HomePartialReason) -> String {
    match reason {
        HomePartialReason::PageFailed { .. } => {
            "A later page failed; earlier entries remain available.".to_string()
        }
        HomePartialReason::PageLimit => "The local page budget stopped the search.".to_string(),
        HomePartialReason::EntryLimit => "The local entry budget truncated the search.".to_string(),
        HomePartialReason::ResultLimit => {
            "GitHub Search exposes at most 1,000 results for this query.".to_string()
        }
        HomePartialReason::IncompleteResults => {
            "GitHub marked the search results incomplete.".to_string()
        }
        HomePartialReason::UntrustedPagination => {
            "An untrusted pagination link was rejected.".to_string()
        }
        HomePartialReason::InvalidPagination => {
            "A pagination link was invalid for this search.".to_string()
        }
        HomePartialReason::PaginationCycle => "A pagination cycle was stopped.".to_string(),
        HomePartialReason::ResponseLimit => {
            "A later response exceeded its size budget.".to_string()
        }
        HomePartialReason::ParseFailed { .. } => {
            "A later page could not be parsed; earlier entries remain available.".to_string()
        }
        HomePartialReason::SsoPartial { organizations } => {
            let organizations = safe_list(organizations);
            format!("Some organizations require SSO authorization: {organizations}.")
        }
    }
}

pub(super) fn feed_summary(outcome: &FeedOutcome) -> Summary {
    let count = format!("{} events · supplied server order", outcome.events.len());
    let notice = match &outcome.readiness {
        FeedReadiness::Complete => {
            "Received Events covers at most the last 30 days and 300 events, and may be delayed. This is not a complete history."
                .to_string()
        }
        FeedReadiness::Partial { reasons } => {
            let details = reasons.iter().map(feed_partial).collect::<Vec<_>>().join(" ");
            format!(
                "Partial result · {details} Received Events is limited to at most 30 days / 300 events and may be delayed."
            )
        }
    };
    Summary { count, notice }
}

fn feed_partial(reason: &FeedPartialReason) -> String {
    match reason {
        FeedPartialReason::PageFailed { .. } => {
            "A later page failed; earlier events remain available.".to_string()
        }
        FeedPartialReason::PageLimit => "The local page budget stopped the read.".to_string(),
        FeedPartialReason::EntryLimit => "The local event budget truncated the read.".to_string(),
        FeedPartialReason::ResponseLimit => {
            "A later response exceeded its size budget.".to_string()
        }
        FeedPartialReason::ParseFailed { .. } => {
            "A later page could not be parsed; earlier events remain available.".to_string()
        }
        FeedPartialReason::UntrustedPagination => {
            "An untrusted pagination link was rejected.".to_string()
        }
        FeedPartialReason::InvalidPagination => {
            "A pagination link was invalid for this event stream.".to_string()
        }
        FeedPartialReason::PaginationCycle => "A pagination cycle was stopped.".to_string(),
        FeedPartialReason::SsoPartial { organizations } => format!(
            "Some organizations require SSO authorization: {}.",
            safe_list(organizations)
        ),
    }
}

fn safe_list(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .take(5)
        .collect::<Vec<_>>();
    if values.is_empty() {
        "names unavailable".to_string()
    } else {
        values.join(", ")
    }
}

pub(super) fn error_message(error: &HomeError) -> String {
    match error {
        HomeError::GitHub(GitHubError::Unauthorized) => {
            "Authentication failed. The host should verify or replace this account's authorization."
                .to_string()
        }
        HomeError::GitHub(GitHubError::RateLimited { retry }) => match retry.seconds {
            Some(seconds) => format!(
                "GitHub rate limit reached. Retry after the host's cooldown (server hint: {seconds}s)."
            ),
            None => {
                "GitHub rate limit reached. Refresh when the host makes it available.".to_string()
            }
        },
        HomeError::GitHub(GitHubError::SsoRequired { .. }) => {
            "Organization SSO authorization is required.".to_string()
        }
        HomeError::GitHub(GitHubError::Forbidden) => {
            "The current authorization cannot access this resource.".to_string()
        }
        HomeError::GitHub(GitHubError::NotFound) => {
            "The resource was not found or is not visible to this account.".to_string()
        }
        HomeError::GitHub(GitHubError::Network { .. }) => {
            "The network request failed. This is not an empty activity result.".to_string()
        }
        HomeError::GitHub(GitHubError::Timeout { .. }) => {
            "The request timed out. No new result was verified.".to_string()
        }
        HomeError::GitHub(GitHubError::Cancelled) => "The request was cancelled.".to_string(),
        HomeError::GitHub(GitHubError::StaleContext) => {
            "The account authorization changed; this result was discarded.".to_string()
        }
        HomeError::GitHub(GitHubError::UnexpectedStatus { status }) => {
            format!("GitHub returned unexpected HTTP status {status}.")
        }
        HomeError::GitHub(GitHubError::Parse { .. }) | HomeError::InvalidResponse { .. } => {
            "The response could not be understood.".to_string()
        }
        HomeError::GitHub(GitHubError::ResponseTooLarge { .. })
        | HomeError::LimitExceeded { .. } => {
            "The response exceeded its configured limit.".to_string()
        }
        HomeError::GitHub(GitHubError::CredentialStore { .. }) => {
            "The host could not access the account credential.".to_string()
        }
        HomeError::InvalidInput { .. } => "The host supplied an invalid query.".to_string(),
    }
}

pub(super) fn home_row(
    item: &HomeItem,
    index: usize,
    selected: bool,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let kind = match item.kind {
        HomeItemKind::Issue => "Issue",
        HomeItemKind::PullRequest => "PR",
    };
    let icon = match item.kind {
        HomeItemKind::Issue => IconName::CircleDot,
        HomeItemKind::PullRequest => IconName::GitPullRequest,
    };
    let title = if item.title.trim().is_empty() {
        format!("{kind} #{}", item.number)
    } else {
        item.title.clone()
    };
    let repository = RepositoryIdentity {
        id: None,
        owner: item.repository.owner.clone(),
        name: item.repository.name.clone(),
    };
    let repo_label = repository.full_name();
    let state = nonempty(&item.state, "state unavailable");
    let updated = nonempty(&item.updated_at, "update time unavailable");
    let meta = format!(
        "{repo_label} #{number} · {kind} · {state}",
        number = item.number
    );
    let weak = cx.entity().downgrade();
    let repository_weak = weak.clone();
    let row = div()
        .id(("home-item", item.id))
        .debug_selector(move || format!("home-item-row-{index}"))
        .flex()
        .items_center()
        .gap_3()
        .h(px(HOME_ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .px_3()
        .rounded(theme.radius(Radius::Md))
        .cursor_pointer()
        .bg(if selected {
            colors.active
        } else {
            gpui::transparent_black()
        })
        .hover(|style| style.bg(colors.hover))
        .on_click(move |_, window, cx| {
            let _ = weak.update(cx, |view, cx| {
                view.select(Column::Home, index, window, cx);
                view.activate(Column::Home, index, window, cx);
            });
        })
        .child(Icon::new(icon).size(IconSize::Sm).color(colors.fg_muted))
        .child(
            div()
                .flex()
                .flex_col()
                .justify_center()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .text_size(theme.text_size(TextSize::Base))
                        .font_weight(FontWeight::MEDIUM)
                        .child(Ellipsis::new(title)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .text_size(theme.text_size(TextSize::Xs))
                        .text_color(colors.fg_subtle)
                        .child(div().flex_1().min_w_0().child(Ellipsis::new(meta)))
                        .child(div().flex_none().child(Ellipsis::new(updated))),
                ),
        )
        .child(
            div()
                .debug_selector(move || format!("home-repository-{index}"))
                .child(
                    ely_core::buttons::Button::new(("home-repository", item.id), "Repository")
                        .size(ControlSize::Sm)
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let repository = repository.clone();
                            let _ = repository_weak.update(cx, |view, cx| {
                                view.open_repository(repository, window, cx);
                            });
                        }),
                ),
        );
    row.into_any_element()
}

pub(super) fn feed_row(
    event: &FeedEvent,
    index: usize,
    selected: bool,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let actor = actor_name(&event.actor);
    let description = event_description(event);
    let repository = repository_name(&event.repository.name);
    let when = event
        .created_at
        .as_deref()
        .map(|value| nonempty(value, "time unavailable"))
        .unwrap_or_else(|| "time unavailable".to_string());
    // The typed actor name is the only source for the avatar. The model's
    // optional avatar URL is intentionally not loaded: this preview and the
    // component show initials with no network image path.
    let avatar_name = actor_name(&event.actor);
    let weak = cx.entity().downgrade();
    let profile = ProfileIdentity {
        id: event.actor.id,
        login: actor.clone(),
    };
    let repo = RepositoryIdentity {
        id: Some(event.repository.id),
        owner: event
            .repository
            .name
            .split_once('/')
            .map_or("", |(owner, _)| owner)
            .to_string(),
        name: event
            .repository
            .name
            .split_once('/')
            .map_or(event.repository.name.clone(), |(_, name)| name.to_string()),
    };
    let profile_weak = weak.clone();
    let repository_weak = weak.clone();
    let row = div()
        .id(gpui::ElementId::Name(
            format!("feed-event-{}", event.id).into(),
        ))
        .debug_selector(move || format!("feed-event-row-{index}"))
        .flex()
        .items_center()
        .gap_3()
        .h(px(FEED_ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .px_3()
        .rounded(theme.radius(Radius::Md))
        .bg(if selected {
            colors.active
        } else {
            gpui::transparent_black()
        })
        .cursor_pointer()
        .hover(|style| style.bg(colors.hover))
        .on_click(move |_, window, cx| {
            let _ = weak.update(cx, |view, cx| {
                view.select(Column::Feed, index, window, cx);
                view.activate(Column::Feed, index, window, cx);
            });
        })
        .child(Avatar::new(("feed-avatar", event.actor.id), avatar_name).size(AvatarSize::Sm))
        .child(
            div()
                .flex()
                .flex_col()
                .justify_center()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(theme.text_size(TextSize::Base))
                                .child(Ellipsis::new(description)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(theme.text_size(TextSize::Xs))
                                .text_color(colors.fg_subtle)
                                .child(Ellipsis::new(when)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .text_size(theme.text_size(TextSize::Xs))
                        .text_color(colors.fg_subtle)
                        .child(Icon::new(IconName::Folder).size(IconSize::Xs))
                        .child(div().flex_1().min_w_0().child(Ellipsis::new(repository))),
                ),
        )
        .child(
            div()
                .debug_selector(move || format!("feed-profile-{index}"))
                .child(
                    ely_core::buttons::Button::new(("feed-profile", event.actor.id), actor)
                        .size(ControlSize::Sm)
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let profile = profile.clone();
                            let _ = profile_weak.update(cx, |view, cx| {
                                view.open_profile(profile, window, cx);
                            });
                        }),
                ),
        )
        .child(
            div()
                .debug_selector(move || format!("feed-repository-{index}"))
                .child(
                    ely_core::buttons::Button::new(
                        ("feed-repository", event.repository.id),
                        "Repository",
                    )
                    .size(ControlSize::Sm)
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let repo = repo.clone();
                        let _ = repository_weak.update(cx, |view, cx| {
                            view.open_repository(repo, window, cx);
                        });
                    }),
                ),
        );
    row.into_any_element()
}

fn actor_name(actor: &FeedActor) -> String {
    actor
        .display_login
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let login = actor.login.trim();
            (!login.is_empty()).then_some(login)
        })
        .unwrap_or("Unknown actor")
        .to_string()
}

fn repository_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        "Repository unavailable".to_string()
    } else {
        name.to_string()
    }
}

pub(super) fn event_description(event: &FeedEvent) -> String {
    let actor = actor_name(&event.actor);
    match (&event.event_type, &event.payload) {
        (FeedEventType::Push, FeedPayload::Push { git_ref, size, .. }) => {
            let commits =
                size.map_or_else(|| "commits".to_string(), |size| format!("{size} commit(s)"));
            format!(
                "{actor} pushed {commits}{}",
                optional_suffix(" to ", git_ref.as_deref())
            )
        }
        (
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                action,
                number,
                title,
                merged,
                ..
            },
        ) => {
            let action = if *merged == Some(true) && action == "closed" {
                "merged".to_string()
            } else {
                nonempty(action, "updated")
            };
            let title = title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty());
            format!(
                "{actor} {action} pull request{}{}",
                optional_number(*number),
                optional_suffix(": ", title)
            )
        }
        (
            FeedEventType::Issues,
            FeedPayload::Issues {
                action,
                number,
                title,
                ..
            },
        ) => {
            let title = title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty());
            format!(
                "{actor} {} issue{}{}",
                nonempty(action, "updated"),
                optional_number(*number),
                optional_suffix(": ", title)
            )
        }
        (
            FeedEventType::IssueComment,
            FeedPayload::IssueComment {
                action,
                issue_number,
                ..
            },
        ) => {
            format!(
                "{actor} {} a comment on issue{}",
                nonempty(action, "updated"),
                optional_number(*issue_number)
            )
        }
        (FeedEventType::Watch, _) => format!("{actor} starred the repository"),
        (FeedEventType::Create, FeedPayload::Create { ref_type, git_ref }) => {
            format!(
                "{actor} created {}{}",
                ref_type.as_deref().unwrap_or("a resource"),
                optional_suffix(" ", git_ref.as_deref())
            )
        }
        (FeedEventType::Delete, FeedPayload::Delete { ref_type, git_ref }) => {
            format!(
                "{actor} deleted {}{}",
                ref_type.as_deref().unwrap_or("a resource"),
                optional_suffix(" ", git_ref.as_deref())
            )
        }
        (FeedEventType::Fork, FeedPayload::Fork { forkee_full_name }) => {
            format!(
                "{actor} forked the repository{}",
                optional_suffix(" to ", forkee_full_name.as_deref())
            )
        }
        (
            FeedEventType::Release,
            FeedPayload::Release {
                action,
                tag_name,
                name,
                ..
            },
        ) => {
            let release = name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .or(tag_name.as_deref());
            format!(
                "{actor} {} release{}",
                nonempty(action, "updated"),
                optional_suffix(" ", release)
            )
        }
        (FeedEventType::Public, _) => format!("{actor} made the repository public"),
        (
            FeedEventType::Member,
            FeedPayload::Member {
                action,
                member_login,
            },
        ) => {
            format!(
                "{actor} {} member {}",
                nonempty(action, "updated"),
                member_login.as_deref().unwrap_or("(name unavailable)")
            )
        }
        (FeedEventType::Unknown(name), _) => {
            format!("{actor} · {} activity", nonempty(name, "Unknown event"))
        }
        // A typed event with a missing/mismatched payload remains a concise
        // known-type fallback, never a raw payload dump.
        (kind, _) => format!("{actor} · {} activity", event_kind_name(kind)),
    }
}

fn event_kind_name(kind: &FeedEventType) -> &'static str {
    match kind {
        FeedEventType::Push => "Push",
        FeedEventType::PullRequest => "Pull request",
        FeedEventType::Issues => "Issue",
        FeedEventType::IssueComment => "Issue comment",
        FeedEventType::Watch => "Watch",
        FeedEventType::Create => "Create",
        FeedEventType::Delete => "Delete",
        FeedEventType::Fork => "Fork",
        FeedEventType::Release => "Release",
        FeedEventType::Public => "Public",
        FeedEventType::Member => "Member",
        FeedEventType::Unknown(_) => "Unknown event",
    }
}

fn nonempty(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn optional_number(number: Option<u64>) -> String {
    number.map_or_else(String::new, |number| format!(" #{number}"))
}

fn optional_suffix(prefix: &str, value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(String::new, |value| format!("{prefix}{value}"))
}
