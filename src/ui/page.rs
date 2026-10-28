//! Page bodies inside the island.
//!
//! This round draws exactly two page kinds: Home, with a small deterministic two-column density
//! check, and minimal resource identifiers for Repository and Profile so routing can be verified.
//! Everything else is intentionally empty: no "not implemented" cards, no fake issue detail, no
//! settings options, no repository reader. Leaving a page blank is the agreed behaviour until its
//! design exists.

use ely_core::{
    data_display::Avatar,
    primitives::{Icon, IconName},
    theme::{ActiveTheme, AvatarSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, FontWeight, IntoElement, ParentElement, Styled, div, prelude::*, px,
    transparent_black,
};

use crate::{
    domain::AccountSummary,
    ui::fixtures::{self, ActivityKind},
    workspace::{RepositorySession, Route, Session},
};

use super::geometry::ACTIVITY_ROW_HEIGHT;

/// One frame's page input. Everything here is view data: the real account identity when signed in
/// and the current account's repository rows. The page never reads a service, a credential or a
/// network path.
pub(super) struct PageData<'a> {
    /// The active account's identity, or `None` while logged out.
    pub account: Option<&'a AccountSummary>,
    /// The current account's repository rows, already fetched by the account controller.
    pub repositories: &'a [crate::domain::RepoSummary],
    /// Settings › Accounts. The shell builds it from the account controller, so this module never
    /// touches a service, a callback or a credential.
    pub settings: Option<AnyElement>,
}

/// The page for the active `session`.
pub(super) fn page(session: &Session, data: PageData<'_>, cx: &App) -> AnyElement {
    match session.route() {
        Route::Home => home(cx),
        Route::Repository { namespace, name } => {
            // The route itself is the resource identity, so a tab still draws correctly after the
            // list changed or the account switched. The row (description, avatar owner) is used
            // when the current account's list still contains it; nothing is invented when it does
            // not, and no repository browser is claimed here.
            let repo = data.repositories.iter().find(|repo| {
                repo.owner.login.eq_ignore_ascii_case(namespace)
                    && repo.name.eq_ignore_ascii_case(name)
            });
            repository(
                namespace,
                name,
                repo,
                session.state().repository.as_ref(),
                cx,
            )
        }
        Route::Profile => profile(data.account, cx),
        // Settings draws the Accounts section only; the remaining global settings design and the
        // cross-repository pages are still empty on purpose.
        Route::Settings => data.settings.unwrap_or_else(blank),
        Route::Notifications | Route::Issues | Route::PullRequests => blank(),
    }
}

fn blank() -> AnyElement {
    div().size_full().into_any_element()
}

/// A page heading at the density Ely's `Title`/`Caption` pair uses.
fn heading(text: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .text_size(theme.text_size(TextSize::Xl))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.colors.fg)
        .child(text.to_string())
        .into_any_element()
}

fn column_heading(text: &str, detail: Option<String>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_size(theme.text_size(TextSize::Lg))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.colors.fg)
        .child(text.to_string())
        .when_some(detail, |row, detail| {
            row.child(
                div()
                    .text_size(theme.text_size(TextSize::Sm))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(theme.colors.fg_subtle)
                    .child(detail),
            )
        })
        .into_any_element()
}

/// Home: the left column is titled exactly "Home" and holds recently involved issues/PRs; the
/// right column is titled exactly "Feed" and holds following activity. Those two names are the
/// confirmed product labels (2026-10-22), not working titles.
fn home(cx: &App) -> AnyElement {
    let involved = div()
        .debug_selector(|| "home-involved".to_string())
        .flex()
        .flex_col()
        .flex_1()
        .flex_basis(px(0.0))
        .min_w_0()
        .overflow_hidden()
        .gap_1()
        .child(column_heading(
            "Home",
            Some(format!("{}", fixtures::INVOLVED.len())),
            cx,
        ))
        .children(fixtures::INVOLVED.iter().map(|row| involved_row(row, cx)));
    let feed = div()
        .debug_selector(|| "home-feed".to_string())
        .flex()
        .flex_col()
        .flex_1()
        .flex_basis(px(0.0))
        .min_w_0()
        .overflow_hidden()
        .gap_1()
        .child(column_heading("Feed", None, cx))
        .children(fixtures::FEED.iter().map(|row| feed_row(row, cx)));
    div()
        .debug_selector(|| "page-home".to_string())
        .flex()
        .flex_col()
        .w_full()
        .px_6()
        .py_5()
        .gap_5()
        .child(heading("Home", cx))
        .child(
            div()
                .flex()
                .items_start()
                .gap_6()
                .w_full()
                .min_w_0()
                .child(involved)
                .child(feed),
        )
        .into_any_element()
}

fn activity_icon(kind: ActivityKind) -> IconName {
    match kind {
        ActivityKind::Issue => IconName::CircleDot,
        ActivityKind::PullRequest => IconName::GitPullRequest,
    }
}

fn involved_row(row: &fixtures::InvolvedFixture, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let radius = theme.radius(Radius::Md);
    let (fg, subtle, muted, hover) = (colors.fg, colors.fg_subtle, colors.fg_muted, colors.hover);
    div()
        .id(format!("involved-{}", row.number))
        .flex()
        .items_center()
        .gap_3()
        .h(px(ACTIVITY_ROW_HEIGHT))
        .px_2()
        .rounded(radius)
        .border_1()
        .border_color(transparent_black())
        .hover(move |style| style.bg(hover))
        .child(
            Icon::new(activity_icon(row.kind))
                .size(IconSize::Sm)
                .color(muted),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .min_w_0()
                        .text_size(theme.text_size(TextSize::Base))
                        .text_color(fg)
                        .child(Ellipsis::new(row.title)),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(theme.text_size(TextSize::Xs))
                        .text_color(subtle)
                        .child(Ellipsis::new(format!(
                            "{} #{} · {}",
                            row.repo,
                            row.number,
                            row.kind.label()
                        ))),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_size(theme.text_size(TextSize::Xs))
                .text_color(subtle)
                .child(row.updated),
        )
        .into_any_element()
}

fn feed_row(row: &fixtures::FeedFixture, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let radius = theme.radius(Radius::Md);
    let (fg, subtle, muted, hover, border) = (
        colors.fg,
        colors.fg_subtle,
        colors.fg_muted,
        colors.hover,
        colors.border,
    );
    div()
        .id(row.when)
        .flex()
        .items_start()
        .gap_3()
        .px_2()
        .py_1p5()
        .rounded(radius)
        .hover(move |style| style.bg(hover))
        .child(Avatar::new(row.when, row.actor).size(AvatarSize::Sm))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .text_size(theme.text_size(TextSize::Base))
                        .text_color(fg)
                        .child(Ellipsis::new(format!(
                            "{} {} {}",
                            row.actor, row.action, row.target
                        )))
                        .child(
                            div()
                                .flex_none()
                                .text_size(theme.text_size(TextSize::Xs))
                                .text_color(subtle)
                                .child(row.when),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .min_w_0()
                        .p_2()
                        .gap_1()
                        .rounded(radius)
                        .border_1()
                        .border_color(border)
                        .bg(colors.surface)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(muted)
                                .child(Icon::new(IconName::Folder).size(IconSize::Xs).color(muted))
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(theme.text_size(TextSize::Xs))
                                        .child(Ellipsis::new(row.target)),
                                ),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(theme.text_size(TextSize::Sm))
                                .text_color(subtle)
                                .child(Ellipsis::new(row.detail)),
                        ),
                ),
        )
        .into_any_element()
}

/// A repository route's minimal identifier: enough to confirm the right page opened. The reader,
/// header information and content tabs belong to later tasks. When the session has a browsing
/// selection set (the R3 reader will do that), it is shown here as session state.
///
/// The remembered branch/file of a tab that comes back after an account switch is navigation only:
/// this page shows it and claims no current authorization for the path.
fn repository(
    namespace: &str,
    name: &str,
    repo: Option<&crate::domain::RepoSummary>,
    session: Option<&RepositorySession>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let full_name = format!("{namespace}/{name}");
    let description = repo.and_then(|repo| repo.description.clone());
    let branch = session.and_then(|session| session.branch.clone());
    let file = session.and_then(|session| session.file.clone());
    div()
        .debug_selector(|| "page-repository".to_string())
        .flex()
        .flex_col()
        .w_full()
        .px_6()
        .py_5()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(Avatar::new("repository-avatar", namespace.to_string()).size(AvatarSize::Lg))
                .child(
                    div()
                        .min_w_0()
                        .text_size(theme.text_size(TextSize::Xl))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.colors.fg)
                        .child(Ellipsis::new(full_name)),
                ),
        )
        .when_some(description, |page, description| {
            page.child(
                div()
                    .text_size(theme.text_size(TextSize::Sm))
                    .text_color(theme.colors.fg_muted)
                    .child(description),
            )
        })
        .when_some(branch, |page, branch| {
            let file = file.clone();
            page.child(
                div()
                    .debug_selector(|| "repository-session".to_string())
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Icon::new(IconName::GitBranch)
                            .size(IconSize::Xs)
                            .color(theme.colors.fg_subtle),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(theme.text_size(TextSize::Xs))
                            .text_color(theme.colors.fg_subtle)
                            .child(branch),
                    )
                    .when_some(file, |row, file| {
                        row.child(
                            div()
                                .min_w_0()
                                .text_size(theme.text_size(TextSize::Xs))
                                .text_color(theme.colors.fg_muted)
                                .child(Ellipsis::new(file)),
                        )
                    }),
            )
        })
        .into_any_element()
}

/// A profile route's minimal identifier; Overview is a later task. It draws the real active
/// account, and nothing at all while logged out.
fn profile(account: Option<&AccountSummary>, cx: &App) -> AnyElement {
    let Some(account) = account else {
        return blank();
    };
    let theme = cx.theme();
    let name = account
        .identity
        .display_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&account.identity.login)
        .to_string();
    let login = account.identity.login.clone();
    div()
        .debug_selector(|| "page-profile".to_string())
        .flex()
        .items_center()
        .w_full()
        .px_6()
        .py_5()
        .gap_3()
        .child(Avatar::new("profile-avatar", name.clone()).size(AvatarSize::Xl))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Xl))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.colors.fg)
                        .child(Ellipsis::new(name)),
                )
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Sm))
                        .text_color(theme.colors.fg_subtle)
                        .child(Ellipsis::new(format!("@{login}"))),
                ),
        )
        .into_any_element()
}
