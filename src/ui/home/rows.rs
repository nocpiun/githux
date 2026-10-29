//! Content targets and activity cards; all navigation stays typed.
use super::{Column, HomeAction, HomeView, ProfileIdentity, RepositoryIdentity};
use crate::domain::home::*;
use ely_core::{
    data_display::Avatar,
    primitives::{FocusRing, Icon, IconName, Tooltip},
    theme::{ActiveTheme, AvatarSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{AnyElement, App, Context, FontWeight, IntoElement, div, prelude::*, px};
use std::time::SystemTime;
pub(super) const HOME_ROW_HEIGHT: f32 = 68.0;

fn link(name: String, label: String, action: HomeAction, cx: &mut Context<HomeView>) -> AnyElement {
    let weak = cx.entity().downgrade();
    let debug = name.clone();
    div()
        .id(name)
        .debug_selector(move || debug.clone())
        .role(gpui::Role::Link)
        .aria_label(label.clone())
        .tab_index(0)
        .min_w_0()
        .overflow_hidden()
        .cursor_pointer()
        .focus_ring(cx)
        .hover(|style| style.underline())
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.prevent_default()
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            let action = action.clone();
            let _ = weak.update(cx, |view, cx| view.emit(action, window, cx));
        })
        .child(Ellipsis::new(label))
        .into_any_element()
}

fn repository_links(
    repo: RepositoryIdentity,
    prefix: &str,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let mut row = div().flex().items_center().min_w_0().overflow_hidden();
    if !repo.owner.trim().is_empty() {
        row = row
            .child(link(
                format!("{prefix}-owner"),
                repo.owner.clone(),
                HomeAction::Profile(ProfileIdentity {
                    id: None,
                    login: repo.owner.clone(),
                }),
                cx,
            ))
            .child(div().flex_none().child("/"));
    }
    if !repo.name.trim().is_empty() {
        row = row.child(link(
            format!("{prefix}-repository"),
            repo.name.clone(),
            HomeAction::Repository(repo),
            cx,
        ));
    } else {
        row = row.child("Repository unavailable");
    }
    row.into_any_element()
}

fn timestamp(value: Option<&str>, name: String, now: SystemTime, cx: &App) -> AnyElement {
    let label = super::time::relative(value, now);
    let full = value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Time not supplied")
        .to_string();
    let debug = name.clone();
    div()
        .id(name)
        .debug_selector(move || debug.clone())
        .min_w_0()
        .text_size(cx.theme().text_size(TextSize::Xs))
        .text_color(cx.theme().colors.fg_subtle)
        .tooltip(Tooltip::text(full))
        .child(label)
        .into_any_element()
}

fn title(column: Column, index: usize, label: String, cx: &mut Context<HomeView>) -> AnyElement {
    let name = format!("{}-title-{index}", super::column_name(column));
    let weak = cx.entity().downgrade();
    div()
        .id(name.clone())
        .debug_selector(move || name.clone())
        .role(gpui::Role::Link)
        .aria_label(label.clone())
        .tab_index(0)
        .flex_1()
        .min_w_0()
        .cursor_pointer()
        .focus_ring(cx)
        .font_weight(FontWeight::MEDIUM)
        .hover(|style| style.underline())
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.prevent_default()
        })
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            let pointer = !matches!(event, gpui::ClickEvent::Keyboard(_));
            let _ = weak.update(cx, |view, cx| {
                if pointer {
                    view.select(column, index, window, cx);
                }
                view.activate(column, index, window, cx);
            });
        })
        .tooltip(Tooltip::text(label.clone()))
        .child(Ellipsis::new(label))
        .into_any_element()
}

pub(super) fn home_row(
    item: &HomeItem,
    index: usize,
    selected: bool,
    now: SystemTime,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let radius = theme.radius(Radius::Md);
    let body = theme.text_size(TextSize::Base);
    let tiny = theme.text_size(TextSize::Xs);
    let (kind, icon) = match item.kind {
        HomeItemKind::Issue => ("Issue", IconName::CircleDot),
        HomeItemKind::PullRequest => ("PR", IconName::GitPullRequest),
    };
    let heading = title(
        Column::Home,
        index,
        nonempty(&item.title, &format!("{kind} #{}", item.number)),
        cx,
    );
    let repository = repository_links(
        RepositoryIdentity {
            id: None,
            owner: item.repository.owner.clone(),
            name: item.repository.name.clone(),
        },
        &format!("home-{index}"),
        cx,
    );
    let when = timestamp(
        Some(&item.updated_at),
        format!("home-time-{index}"),
        now,
        cx,
    );
    // The icon and title share a single flex row; metadata is below that row.
    div()
        .id(("home-item", item.id))
        .debug_selector(move || format!("home-item-row-{index}"))
        .h(px(HOME_ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .px_2()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .rounded(radius)
        .bg(if selected {
            colors.active
        } else {
            gpui::transparent_black()
        })
        .child(
            div()
                .debug_selector(move || format!("home-title-line-{index}"))
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .text_size(body)
                .child(
                    div()
                        .debug_selector(move || format!("home-icon-{index}"))
                        .flex_none()
                        .flex()
                        .items_center()
                        .child(Icon::new(icon).size(IconSize::Sm).color(colors.fg_muted)),
                )
                .child(heading),
        )
        .child(
            div()
                .debug_selector(move || format!("home-metadata-{index}"))
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .text_size(tiny)
                .text_color(colors.fg_subtle)
                .child(div().flex_1().min_w_0().child(repository))
                .child(div().flex_none().child(format!(
                    "#{} · {}",
                    item.number,
                    nonempty(&item.state, "state unavailable")
                )))
                .child(when),
        )
        .into_any_element()
}

pub(super) fn feed_repository(event: &FeedEvent) -> RepositoryIdentity {
    let (owner, name) = event
        .repository
        .name
        .split_once('/')
        .unwrap_or(("", &event.repository.name));
    RepositoryIdentity {
        id: Some(event.repository.id),
        owner: owner.to_string(),
        name: name.to_string(),
    }
}

pub(super) fn feed_row(
    event: &FeedEvent,
    index: usize,
    selected: bool,
    now: SystemTime,
    real_avatar: Option<AnyElement>,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let radius = theme.radius(Radius::Md);
    let body_size = theme.text_size(TextSize::Base);
    let tiny = theme.text_size(TextSize::Xs);
    let actor = actor_name(&event.actor);
    let profile = ProfileIdentity {
        id: Some(event.actor.id),
        login: event.actor.login.clone(),
    };
    let actor_link = if profile.login.trim().is_empty() {
        div().child(actor.clone()).into_any_element()
    } else {
        link(
            format!("feed-actor-{index}"),
            actor.clone(),
            HomeAction::Profile(profile.clone()),
            cx,
        )
    };
    let weak = cx.entity().downgrade();
    let avatar = div()
        .id(("feed-avatar", index))
        .debug_selector(move || format!("feed-avatar-{index}"))
        .flex_none()
        .child(real_avatar.unwrap_or_else(|| {
            Avatar::new(("feed-avatar-face", index), actor)
                .size(AvatarSize::Lg)
                .into_any_element()
        }))
        .when(!profile.login.trim().is_empty(), |avatar| {
            avatar
                .role(gpui::Role::Link)
                .aria_label(format!("Open {} profile", profile.login))
                .tab_index(0)
                .cursor_pointer()
                .focus_ring(cx)
                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let profile = profile.clone();
                    let _ = weak.update(cx, |view, cx| view.open_profile(profile, window, cx));
                })
        });
    let when = timestamp(
        event.created_at.as_deref(),
        format!("feed-time-{index}"),
        now,
        cx,
    );
    let details = event_details(event);
    let repo = repository_links(feed_repository(event), &format!("feed-{index}"), cx);
    let mut inset = div()
        .debug_selector(move || format!("feed-details-{index}"))
        .flex()
        .flex_col()
        .gap_2()
        .min_w_0()
        .p_3()
        .rounded(radius)
        .bg(colors.sunken)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .text_size(tiny)
                .text_color(colors.fg_muted)
                .child(Icon::new(IconName::Folder).size(IconSize::Xs))
                .child(div().min_w_0().child(repo)),
        );
    if let Some(label) = details.title {
        let icon = if matches!(event.event_type, FeedEventType::PullRequest) {
            IconName::GitPullRequest
        } else {
            IconName::CircleDot
        };
        inset = inset.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .text_size(body_size)
                .child(Icon::new(icon).size(IconSize::Sm).color(if details.merged {
                    colors.accent
                } else {
                    colors.fg_muted
                }))
                .child(title(Column::Feed, index, label, cx)),
        );
    }
    if let Some(status) = details.status {
        inset = inset.child(
            div()
                .debug_selector(move || format!("feed-item-status-{index}"))
                .text_size(tiny)
                .text_color(if details.merged {
                    colors.accent
                } else {
                    colors.fg_subtle
                })
                .child(status),
        );
    }
    for detail in details.lines {
        inset = inset.child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_size(tiny)
                .text_color(colors.fg_subtle)
                .child(detail),
        );
    }
    div()
        .debug_selector(move || format!("feed-event-row-{index}"))
        .w_full()
        .min_w_0()
        .pb_4()
        .px_1()
        .child(
            div()
                .id(format!("feed-event-{}", event.id))
                .debug_selector(move || format!("feed-card-{index}"))
                .flex()
                .flex_col()
                .gap_3()
                .min_w_0()
                .p_3()
                .rounded(radius)
                .border_1()
                .border_color(colors.border)
                .bg(if selected {
                    colors.active
                } else {
                    colors.surface
                })
                .child(
                    div()
                        .debug_selector(move || format!("feed-header-{index}"))
                        .flex()
                        .items_center()
                        .gap_3()
                        .min_w_0()
                        .child(avatar)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap_1()
                                        .min_w_0()
                                        .text_size(body_size)
                                        .child(actor_link)
                                        .child(div().min_w_0().child(action_sentence(event))),
                                )
                                .child(when),
                        ),
                )
                .child(inset),
        )
        .into_any_element()
}

pub(super) struct EventDetails {
    pub title: Option<String>,
    pub status: Option<String>,
    pub merged: bool,
    pub lines: Vec<String>,
}

pub(super) fn event_details(event: &FeedEvent) -> EventDetails {
    let mut details = EventDetails {
        title: None,
        status: None,
        merged: false,
        lines: Vec::new(),
    };
    match (&event.event_type, &event.payload) {
        (
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                number,
                title,
                state,
                merged,
                ..
            },
        ) => {
            details.title = Some(format!(
                "{}{}",
                nonempty(
                    title.as_deref().unwrap_or(""),
                    "Pull request title unavailable"
                ),
                optional_number(*number)
            ));
            details.merged = *merged == Some(true);
            details.status = Some(if details.merged {
                "Merged".to_string()
            } else {
                nonempty(state.as_deref().unwrap_or(""), "Status unavailable")
            });
        }
        (
            FeedEventType::Issues,
            FeedPayload::Issues {
                number,
                title,
                state,
                ..
            },
        ) => {
            details.title = Some(format!(
                "{}{}",
                nonempty(title.as_deref().unwrap_or(""), "Issue title unavailable"),
                optional_number(*number)
            ));
            details.status = Some(nonempty(
                state.as_deref().unwrap_or(""),
                "Status unavailable",
            ));
        }
        (FeedEventType::IssueComment, FeedPayload::IssueComment { issue_number, .. }) => {
            details.title = Some(format!("Issue{}", optional_number(*issue_number)));
            details
                .lines
                .push("Comment body is not supplied by this event.".into());
        }
        (
            FeedEventType::Push,
            FeedPayload::Push {
                git_ref,
                before,
                head,
                size,
            },
        ) => {
            details.lines.push(size.map_or_else(
                || "Commit count unavailable".into(),
                |count| {
                    format!(
                        "{count} {} pushed",
                        if count == 1 { "commit" } else { "commits" }
                    )
                },
            ));
            details.lines.push(format!(
                "Ref: {}",
                nonempty(git_ref.as_deref().unwrap_or(""), "unavailable")
            ));
            if let Some(before) = before.as_deref().filter(|v| !v.trim().is_empty()) {
                details.lines.push(format!("Before: {before}"));
            }
            if let Some(head) = head.as_deref().filter(|v| !v.trim().is_empty()) {
                details.lines.push(format!("Head: {head}"));
            }
        }
        (FeedEventType::Create, FeedPayload::Create { ref_type, git_ref })
        | (FeedEventType::Delete, FeedPayload::Delete { ref_type, git_ref }) => {
            details.lines.push(format!(
                "{}{}",
                nonempty(
                    ref_type.as_deref().unwrap_or(""),
                    "Resource type unavailable"
                ),
                optional_suffix(": ", git_ref.as_deref())
            ));
        }
        (FeedEventType::Fork, FeedPayload::Fork { forkee_full_name }) => {
            details.lines.push(format!(
                "Fork: {}",
                nonempty(
                    forkee_full_name.as_deref().unwrap_or(""),
                    "name unavailable"
                )
            ))
        }
        (FeedEventType::Release, FeedPayload::Release { tag_name, name, .. }) => {
            details.lines.push(format!(
                "Release: {}",
                nonempty(name.as_deref().unwrap_or(""), "name unavailable")
            ));
            details.lines.push(format!(
                "Tag: {}",
                nonempty(tag_name.as_deref().unwrap_or(""), "unavailable")
            ));
        }
        (FeedEventType::Member, FeedPayload::Member { member_login, .. }) => {
            details.lines.push(format!(
                "Member: {}",
                nonempty(member_login.as_deref().unwrap_or(""), "login unavailable")
            ))
        }
        (FeedEventType::Unknown(name), _) => details.lines.push(format!(
            "{} · Event details unavailable",
            nonempty(name, "Unknown event")
        )),
        (FeedEventType::Watch, _) | (FeedEventType::Public, _) => {}
        _ => details.lines.push("Event details unavailable".into()),
    }
    details
}

pub(super) fn action_sentence(event: &FeedEvent) -> String {
    match (&event.event_type, &event.payload) {
        (FeedEventType::Push, _) => "pushed to the repository".into(),
        (FeedEventType::PullRequest, FeedPayload::PullRequest { action, merged, .. }) => format!(
            "{} a pull request",
            if *merged == Some(true) {
                "merged".into()
            } else {
                nonempty(action, "updated")
            }
        ),
        (FeedEventType::Issues, FeedPayload::Issues { action, .. }) => {
            format!("{} an issue", nonempty(action, "updated"))
        }
        (FeedEventType::IssueComment, FeedPayload::IssueComment { action, .. }) => {
            format!("{} a comment", nonempty(action, "updated"))
        }
        (FeedEventType::Watch, _) => "starred the repository".into(),
        (FeedEventType::Create, _) => "created a repository resource".into(),
        (FeedEventType::Delete, _) => "deleted a repository resource".into(),
        (FeedEventType::Fork, _) => "forked the repository".into(),
        (FeedEventType::Release, FeedPayload::Release { action, .. }) => {
            format!("{} a release", nonempty(action, "updated"))
        }
        (FeedEventType::Public, _) => "made the repository public".into(),
        (FeedEventType::Member, FeedPayload::Member { action, .. }) => {
            format!("{} a repository member", nonempty(action, "updated"))
        }
        (FeedEventType::Unknown(name), _) => {
            format!("shared {} activity", nonempty(name, "unknown"))
        }
        _ => "updated repository activity".into(),
    }
}
fn actor_name(actor: &FeedActor) -> String {
    actor
        .display_login
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| (!actor.login.trim().is_empty()).then_some(actor.login.as_str()))
        .unwrap_or("Unknown actor")
        .to_string()
}
pub(super) fn event_description(event: &FeedEvent) -> String {
    let details = event_details(event);
    format!(
        "{} {} {}",
        actor_name(&event.actor),
        action_sentence(event),
        details.title.unwrap_or_default()
    )
}
fn nonempty(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.trim().to_string()
    }
}
fn optional_number(number: Option<u64>) -> String {
    number.map_or_else(String::new, |number| format!(" #{number}"))
}
fn optional_suffix(prefix: &str, value: Option<&str>) -> String {
    value
        .filter(|v| !v.trim().is_empty())
        .map_or_else(String::new, |value| format!("{prefix}{value}"))
}
