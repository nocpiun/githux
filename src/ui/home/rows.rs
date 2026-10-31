//! Content targets and activity cards; all navigation stays typed.
use super::{Column, HomeAction, HomeView, ProfileIdentity, RepositoryIdentity};
use crate::domain::home::*;
use crate::ui::tooltip::ui_tooltip;
use ely_core::{
    data_display::Avatar,
    primitives::{FocusRing, Icon, IconName},
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
        .tooltip(ui_tooltip(full))
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
        .tooltip(ui_tooltip(label.clone()))
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

/// Renders one already-validated Feed activity.
///
/// The host applies the product policy before publishing: only retained activity
/// kinds whose subject is a followed account reach this renderer, and a merged
/// contribution already carries its resolved author. This function therefore has
/// no fallback or placeholder path — there is nothing untruthful to draw.
pub(super) fn feed_row(
    activity: &FeedActivity,
    index: usize,
    selected: bool,
    now: SystemTime,
    real_avatar: Option<AnyElement>,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let event = &activity.event;
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let radius = theme.radius(Radius::Md);
    let body_size = theme.text_size(TextSize::Base);
    let tiny = theme.text_size(TextSize::Xs);
    // The credited subject is the pull-request author for a merge and the event
    // actor otherwise; the avatar renderer resolved the same identity.
    let actor = actor_name(&activity.subject);
    let profile = ProfileIdentity {
        id: Some(activity.subject.id),
        login: activity.subject.login.clone(),
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
    let repo = repository_links(feed_repository(event), &format!("feed-{index}"), cx);
    // The merged card reads exactly `<contributor> contributed to <owner>/<repo>`,
    // so its repository target sits on the header line; the inset then carries the
    // pull-request title. Star/create cards keep the repository in the inset. No
    // branch, ref, SHA or commit prose appears in either case.
    let merged = activity.kind == FeedActivityKind::PullRequestMerged;
    let sentence = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .min_w_0()
        .text_size(body_size)
        .child(actor_link)
        .child(div().min_w_0().child(action_sentence(activity)));
    let inset = div()
        .debug_selector(move || format!("feed-details-{index}"))
        .flex()
        .flex_col()
        .gap_2()
        .min_w_0()
        .p_3()
        .rounded(radius)
        .bg(colors.sunken);
    // Exactly one of the two cards owns the repository element.
    let (sentence, inset) = if merged {
        (sentence.child(div().min_w_0().child(repo)), inset)
    } else {
        (
            sentence,
            inset.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .text_size(tiny)
                    .text_color(colors.fg_muted)
                    .child(Icon::new(IconName::Folder).size(IconSize::Xs))
                    .child(div().min_w_0().child(repo)),
            ),
        )
    };
    let mut inset = inset;
    if let Some(target) = &activity.pull_request {
        let mut label = target
            .title
            .clone()
            .unwrap_or_else(|| "Pull request".to_string());
        if let Some(number) = target.number {
            label.push_str(&format!(" #{number}"));
        }
        inset = inset.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .text_size(body_size)
                .child(
                    Icon::new(IconName::GitPullRequest)
                        .size(IconSize::Sm)
                        .color(colors.accent),
                )
                .child(title(Column::Feed, index, label, cx)),
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
                                .child(sentence)
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

/// Details for a retained activity. No branch, ref, SHA or commit-count prose is
/// produced; only the pull-request target of a merged contribution is described.
pub(super) fn event_details(activity: &FeedActivity) -> EventDetails {
    let mut details = EventDetails {
        title: None,
        status: None,
        merged: false,
        lines: Vec::new(),
    };
    if let Some(target) = &activity.pull_request {
        details.merged = true;
        details.title = Some(match &target.title {
            Some(title) => format!("{title}{}", optional_number(target.number)),
            None => format!("Pull request{}", optional_number(target.number)),
        });
        details.status = Some("Merged".to_string());
    }
    details
}

/// The compact subject/action sentence of a retained activity. A merged
/// contribution reads `<author> contributed to <owner>/<repo>`.
pub(super) fn action_sentence(activity: &FeedActivity) -> String {
    match activity.kind {
        FeedActivityKind::PullRequestMerged => "contributed to".into(),
        FeedActivityKind::RepositoryStarred => "starred the repository".into(),
        FeedActivityKind::RepositoryCreated => "created the repository".into(),
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
/// Accessibility/one-line description of a retained activity. It credits the
/// activity subject (the pull-request author for a merge), never the actor who
/// merely clicked merge.
pub(super) fn event_description(activity: &FeedActivity) -> String {
    let details = event_details(activity);
    let repository = feed_repository(&activity.event);
    let mut parts = vec![actor_name(&activity.subject), action_sentence(activity)];
    // A merged contribution reads `<author> contributed to <owner>/<repo>`; the
    // other cards name their repository too, matching what is on screen.
    parts.push(repository.full_name());
    if let Some(title) = details.title {
        parts.push(title);
    }
    parts.join(" ").trim().to_string()
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
