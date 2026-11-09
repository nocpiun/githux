//! Content targets and activity cards; all navigation stays typed.
//!
//! Home's Involved rows are the shared [`crate::ui::work_item_row`] presentation
//! (one row for Home and the global Issue/PR lists). Feed cards own their own
//! geometry because their content structure differs, but they hand the host the
//! same typed actions.
use super::{HomeAction, ProfileIdentity, RepositoryIdentity};
use crate::domain::home::*;
use crate::ui::foundation::{self, theme as t};
use crate::ui::relative_time::relative;
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;
use std::rc::Rc;

/// The small glyph set this page draws. Lucide paths, under the repository's
/// existing ISC notice; a shared icon set may absorb them later.
#[derive(Clone, Copy, Debug)]
pub(super) enum Icon {
    Refresh,
    Folder,
    Clock,
    Star,
    Repository,
    PullRequest,
    CloudOff,
}

fn path(kind: Icon) -> &'static str {
    match kind {
        Icon::Refresh => {
            "<path d='M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-2l2 3M4 16l2 3a7 7 0 0 0 12-2'/>"
        }
        Icon::Folder => "<path d='M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z'/>",
        Icon::Clock => "<circle cx='12' cy='12' r='9'/><path d='M12 7v5l3 2'/>",
        Icon::Star => "<path d='m12 3 3 6 6 1-4.5 4 1 6-5.5-3-5.5 3 1-6L3 10l6-1z'/>",
        Icon::Repository => "<path d='M4 4h12a2 2 0 0 1 2 2v14H6a2 2 0 0 1-2-2z'/><path d='M18 16H6'/>",
        Icon::PullRequest => {
            "<circle cx='6' cy='5' r='3'/><circle cx='6' cy='19' r='3'/><circle cx='18' cy='19' r='3'/><path d='M6 8v8M18 16V9a4 4 0 0 0-4-4h-1m3-3-3 3 3 3'/>"
        }
        Icon::CloudOff => "<path d='M3 3l18 18M6 18a4 4 0 0 1 0-8 6 6 0 0 1 9-4M18 18h-1'/>",
    }
}

pub(super) fn icon(kind: Icon) -> AnyElement {
    svg(ImageSrc::svg_markup(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'>{}</svg>",
        path(kind)
    )))
    .size(px(16.0))
    .text_color(t::theme().muted_foreground)
    .into_element()
}

fn colored_icon(kind: Icon, color: AdaptiveColor) -> AnyElement {
    svg(ImageSrc::svg_markup(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'>{}</svg>",
        path(kind)
    )))
    .size(px(16.0))
    .text_color(color)
    .into_element()
}

/// One linking text cell. `on_press` runs on a pointer press and on Return/Space
/// once the cell holds keyboard focus (a Link-role focus stop).
fn link(label: String, aria: String, flex: bool, on_press: impl Fn() + 'static) -> AnyElement {
    link_with(None, label, aria, flex, on_press)
}

/// One linking text cell with an explicit test id on the pressable element
/// itself, so a test presses the node that really carries the action.
fn link_with(
    test_id: Option<String>,
    label: String,
    aria: String,
    flex: bool,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let mut cell = view();
    if let Some(test_id) = test_id {
        cell = cell.test_id(&test_id);
    }
    let mut cell = cell
        .role(AccessibilityRole::Link)
        .label(aria)
        .focusable(true)
        .min_w(px(0.0))
        .overflow_hidden()
        .cursor_pointer()
        .on_press(move |_: &()| on_press())
        .child(
            text(label)
                .truncate()
                .text_color(t::theme().foreground),
        );
    if flex {
        cell = cell.flex_1();
    }
    cell.into_element()
}

fn timestamp(value: Option<&str>, test_id: String, now: std::time::SystemTime) -> AnyElement {
    let label = relative(value, now);
    let full = value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Time not supplied")
        .to_string();
    view()
        .test_id(&test_id)
        .min_w(px(0.0))
        .tooltip(crate::ui::foundation::controls::tooltip(full))
        .child(
            text(label)
                .truncate()
                .text_size(px(12.0))
                .text_color(t::theme().muted_foreground),
        )
        .into_element()
}

fn nonempty(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.trim().to_string()
    }
}

fn profile_target(id: Option<u64>, login: &str) -> HomeAction {
    HomeAction::Profile(ProfileIdentity {
        id,
        login: login.to_string(),
    })
}

pub(super) fn home_row(
    item: &HomeItem,
    index: usize,
    selected: bool,
    now: std::time::SystemTime,
    selected_signal: Signal<Option<usize>>,
    on_action: super::OnAction,
) -> AnyElement {
    use crate::ui::work_item_row::{
        self, Glyph, RepositoryLink, Row, RowAction, Selectors, TitleAction, Tone,
    };
    let kind = match item.kind {
        HomeItemKind::Issue => Glyph::Issue,
        HomeItemKind::PullRequest => Glyph::PullRequest,
    };
    let selectors = Selectors {
        row: format!("home-item-row-{index}"),
        title_line: Some(format!("home-title-line-{index}")),
        icon: Some(format!("home-icon-{index}")),
        title: format!("home-title-{index}"),
        metadata: Some(format!("home-metadata-{index}")),
        number: Some(format!("home-{index}-number")),
        repository: Some(format!("home-{index}-repository")),
        owner: Some(format!("home-{index}-owner")),
        updated: Some(format!("home-time-{index}")),
    };
    let owner_action: RowAction = {
        let on_action = on_action.clone();
        let owner = item.repository.owner.clone();
        Rc::new(move || on_action(profile_target(None, &owner)))
    };
    let name_action: RowAction = {
        let on_action = on_action.clone();
        let repository = RepositoryIdentity {
            id: None,
            owner: item.repository.owner.clone(),
            name: item.repository.name.clone(),
        };
        Rc::new(move || on_action(HomeAction::Repository(repository.clone())))
    };
    let title = nonempty(&item.title, &format!("{} #{}", kind.label(), item.number));
    let title_action: TitleAction = {
        let on_action = on_action.clone();
        let identity = super::WorkItemIdentity {
            id: item.id,
            kind: item.kind,
            number: item.number,
            repository: RepositoryIdentity {
                id: None,
                owner: item.repository.owner.clone(),
                name: item.repository.name.clone(),
            },
        };
        Rc::new(move |pointer: bool| {
            if pointer {
                selected_signal.set(Some(index));
            }
            on_action(HomeAction::ActivateWorkItem(identity.clone()));
        })
    };
    let state = nonempty(&item.state, "state unavailable");
    work_item_row::render(Row {
        selectors,
        glyph: kind,
        tone: Tone::from_state(&item.state),
        number: item.number,
        title: title.clone(),
        title_tooltip: title,
        repository: RepositoryLink::Split {
            owner: item.repository.owner.clone(),
            owner_action,
            name: item.repository.name.clone(),
            name_action,
        },
        updated_label: relative(Some(&item.updated_at), now),
        updated_tooltip: item.updated_at.clone(),
        aria: format!("{} {}, {state}", kind.label(), item.title.trim(),),
        selected,
        on_row_click: None,
        on_title_click: Some(title_action),
    })
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

pub(super) fn feed_pull_request_action(
    event: &FeedEvent,
    number: Option<u64>,
) -> Option<HomeAction> {
    let number = number.filter(|number| *number > 0)?;
    let repository = feed_repository(event);
    if repository.owner.trim().is_empty() || repository.name.trim().is_empty() {
        return None;
    }
    crate::domain::repository::RepositoryLocator::new(
        repository.owner.clone(),
        repository.name.clone(),
    )
    .ok()?;
    Some(HomeAction::OpenPullRequest { repository, number })
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

fn initials(name: &str) -> String {
    let mut letters = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>();
    if letters.is_empty() {
        letters = name.chars().take(1).collect();
    }
    letters.to_uppercase()
}

fn avatar(picture: Option<ImageSrc>, name: String) -> AnyElement {
    match picture {
        Some(source) => image(source)
            .size(px(40.0))
            .rounded_full()
            .object_fit(ResizeMode::Cover)
            .into_element(),
        None => view()
            .size(px(40.0))
            .rounded_full()
            .bg(t::theme().muted)
            .items_center()
            .justify_center()
            .child(
                text(initials(&name))
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t::theme().muted_foreground),
            )
            .into_element(),
    }
}

pub(super) fn action_sentence(activity: &FeedActivity) -> String {
    match activity.kind {
        FeedActivityKind::PullRequestMerged => "contributed to".into(),
        FeedActivityKind::RepositoryStarred => "starred the repository".into(),
        FeedActivityKind::RepositoryCreated => "created the repository".into(),
    }
}

/// Renders one already-validated Feed activity.
///
/// The host applies the product policy before publishing: only retained activity
/// kinds whose subject is a followed account reach this renderer, and a merged
/// contribution already carries its resolved author.
#[allow(clippy::too_many_arguments)]
pub(super) fn feed_row(
    activity: &FeedActivity,
    index: usize,
    selected: bool,
    now: std::time::SystemTime,
    image: Option<ImageSrc>,
    repo_description: Option<&str>,
    selected_signal: Signal<Option<usize>>,
    on_action: super::OnAction,
) -> AnyElement {
    let event = &activity.event;
    let colors = t::theme();
    let radius = colors.radius;
    // The credited subject is the pull-request author for a merge and the event
    // actor otherwise; the avatar resolver resolved the same identity.
    let actor = actor_name(&activity.subject);
    let profile = ProfileIdentity {
        id: Some(activity.subject.id),
        login: activity.subject.login.clone(),
    };
    let actor_link = if profile.login.trim().is_empty() {
        view().child(text(actor.clone())).into_element()
    } else {
        let action = profile_target(Some(activity.subject.id), &activity.subject.login);
        let on_action = on_action.clone();
        link(actor.clone(), actor.clone(), false, move || {
            on_action(action.clone())
        })
    };
    let avatar_element = {
        let action = profile_target(Some(activity.subject.id), &activity.subject.login);
        let on_action = on_action.clone();
        let face = avatar(image, actor.clone());
        if profile.login.trim().is_empty() {
            view()
                .test_id(&format!("feed-avatar-{index}"))
                .flex_none()
                .child(face)
                .into_element()
        } else {
            view()
                .test_id(&format!("feed-avatar-{index}"))
                .flex_none()
                .role(AccessibilityRole::Link)
                .label(format!("Open {} profile", profile.login))
                .focusable(true)
                .cursor_pointer()
                .on_press(move |_: &()| on_action(action.clone()))
                .child(face)
                .into_element()
        }
    };
    let when = timestamp(
        event.created_at.as_deref(),
        format!("feed-time-{index}"),
        now,
    );
    let repo = {
        let repository = feed_repository(event);
        let owner_login = repository.owner.clone();
        let name = repository.name.clone();
        let mut row = view().flex_row().items_center().min_w(px(0.0)).overflow_hidden();
        if !owner_login.trim().is_empty() {
            let action = profile_target(None, &owner_login);
            let on_action = on_action.clone();
            let label = owner_login.clone();
            row = row
                .child(link_with(
                    Some(format!("feed-{index}-owner")),
                    owner_login.clone(),
                    label,
                    false,
                    move || on_action(action.clone()),
                ))
                .child(text("/").text_color(t::theme().muted_foreground));
        }
        let action = HomeAction::Repository(repository);
        let on_action = on_action.clone();
        let aria = format!("Open repository {owner_login}/{name}");
        row.child(link_with(
            Some(format!("feed-{index}-repository")),
            name,
            aria,
            true,
            move || on_action(action.clone()),
        ))
        .into_element()
    };
    // The merged card reads exactly `<contributor> contributed to <owner>/<repo>`,
    // so its repository target sits on the header line; the inset then carries the
    // pull-request title. Star/create cards keep the repository in the inset.
    let merged = activity.kind == FeedActivityKind::PullRequestMerged;
    let mut sentence = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_1()
        .min_w(px(0.0))
        .child(actor_link)
        .child(
            view()
                .min_w(px(0.0))
                .child(text(action_sentence(activity))),
        );
    let mut inset = view()
        .test_id(&format!("feed-details-{index}"))
        .flex_col()
        .gap_2()
        .min_w(px(0.0))
        .p_3()
        .rounded(radius)
        .bg(t::theme().muted);
    if merged {
        sentence = sentence.child(view().min_w(px(0.0)).child(repo));
    } else {
        inset = inset.child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.0))
                .child(icon(match activity.kind {
                    FeedActivityKind::RepositoryStarred => Icon::Star,
                    FeedActivityKind::RepositoryCreated => Icon::Repository,
                    FeedActivityKind::PullRequestMerged => Icon::PullRequest,
                }))
                .child(view().min_w(px(0.0)).child(repo)),
        );
    }
    if activity.kind == FeedActivityKind::RepositoryStarred
        && let Some(description) = repo_description.filter(|d| !d.trim().is_empty())
    {
        inset = inset.child(
            view()
                .test_id(&format!("feed-repo-description-{index}"))
                .min_w(px(0.0))
                .child(
                    text(description.trim().to_string())
                        .number_of_lines(3u32)
                        .text_color(t::theme().muted_foreground),
                ),
        );
    }
    if let Some(target) = &activity.pull_request {
        let mut label = target
            .title
            .clone()
            .unwrap_or_else(|| "Pull request".to_string());
        if let Some(number) = target.number {
            label.push_str(&format!(" #{number}"));
        }
        let title_action = {
            let on_action = on_action.clone();
            let number = target.number;
            let event = event.clone();
            move || {
                selected_signal.set(Some(index));
                if let Some(action) = feed_pull_request_action(&event, number) {
                    on_action(action);
                }
            }
        };
        inset = inset.child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.0))
                .child(colored_icon(Icon::PullRequest, colors.primary))
                .child(link(
                    label.clone(),
                    label.clone(),
                    true,
                    title_action,
                )),
        );
        if let Some(preview) = &target.body_preview
            && !preview.excerpt.trim().is_empty()
        {
            let navigation = feed_pull_request_action(event, target.number);
            let body = view()
                .test_id(&format!("feed-pr-body-{index}"))
                .min_w(px(0.0))
                .child(
                    text(preview.excerpt.clone())
                        .text_size(px(12.0))
                        .text_color(t::theme().muted_foreground),
                )
                .into_element();
            if preview.is_truncated {
                if let Some(action) = navigation {
                    let on_action = on_action.clone();
                    let read_more = link_with(
                        Some(format!("feed-pr-read-more-{index}")),
                        "Read more".to_string(),
                        "Open pull request in Githux".to_string(),
                        false,
                        move || {
                            selected_signal.set(Some(index));
                            on_action(action.clone())
                        },
                    );
                    inset = inset.child(
                        view()
                            .flex_col()
                            .gap_1()
                            .min_w(px(0.0))
                            .child(body)
                            .child(read_more),
                    );
                } else {
                    inset = inset.child(body);
                }
            } else {
                inset = inset.child(body);
            }
        }
    }
    view()
        .test_id(&format!("feed-event-row-{index}"))
        .w_full()
        .min_w(px(0.0))
        .pb_4()
        .px_1()
        .child(
            view()
                .test_id(&format!("feed-card-{index}"))
                .flex_col()
                .gap_3()
                .min_w(px(0.0))
                .p_3()
                .rounded(radius)
                .border_1()
                .border_color(t::theme().border)
                .bg(if selected {
                    t::theme().accent
                } else {
                    t::theme().background
                })
                .child(
                    view()
                        .test_id(&format!("feed-header-{index}"))
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .min_w(px(0.0))
                        .child(avatar_element)
                        .child(
                            view()
                                .flex_col()
                                .gap_1()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(sentence)
                                .child(when),
                        ),
                )
                .child(inset),
        )
        .into_element()
}

/// Details for a retained activity. No branch, ref, SHA or commit-count prose is
/// produced; only the pull-request target of a merged contribution is described.
pub(super) struct EventDetails {
    pub title: Option<String>,
    pub status: Option<String>,
    pub merged: bool,
}

pub(super) fn event_details(activity: &FeedActivity) -> EventDetails {
    let mut details = EventDetails {
        title: None,
        status: None,
        merged: false,
    };
    if let Some(target) = &activity.pull_request {
        details.merged = true;
        details.title = Some(match &target.title {
            Some(title) => format!(
                "{title}{}",
                target
                    .number
                    .map_or_else(String::new, |n| format!(" #{n}"))
            ),
            None => format!(
                "Pull request{}",
                target
                    .number
                    .map_or_else(String::new, |n| format!(" #{n}"))
            ),
        });
        details.status = Some("Merged".to_string());
    }
    details
}

/// Accessibility/one-line description of a retained activity. It credits the
/// activity subject (the pull-request author for a merge), never the actor who
/// merely clicked merge.
pub(super) fn event_description(activity: &FeedActivity) -> String {
    let details = event_details(activity);
    let repository = feed_repository(&activity.event);
    let mut parts = vec![actor_name(&activity.subject), action_sentence(activity)];
    parts.push(repository.full_name());
    if let Some(title) = details.title {
        parts.push(title);
    }
    parts.join(" ").trim().to_string()
}

/// Relative time for a card's exact timestamp, used by tests and hosts.
pub(super) fn relative_label(value: Option<&str>, now: std::time::SystemTime) -> String {
    relative(value, now)
}
