//! Page bodies inside the island.
//!
//! This round draws exactly two page kinds: Home, with a small deterministic two-column density
//! check, and minimal resource identifiers for Repository and Profile so routing can be verified.
//! Everything else is intentionally empty: no "not implemented" cards, no fake issue detail, no
//! settings options, no repository reader. Leaving a page blank is the agreed behaviour until its
//! design exists.

use ely_gpui_component::{
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
    ui::fixtures::{self, ActivityKind, RepoFixture},
    workspace::Route,
};

use super::geometry::ACTIVITY_ROW_HEIGHT;

/// The page for `route`.
pub(super) fn page(route: &Route, repositories: &[RepoFixture], cx: &App) -> AnyElement {
    match route {
        Route::Home => home(cx),
        Route::Repository { namespace, name } => {
            let repo = repositories
                .iter()
                .find(|repo| repo.namespace == *namespace && repo.name == *name);
            match repo {
                Some(repo) => repository(repo, cx),
                None => blank(),
            }
        }
        Route::Profile => profile(cx),
        // Settings options, notifications, cross-repository issues/PRs and Issue/PR detail are not
        // designed yet, so their content area stays empty on purpose.
        Route::Settings | Route::Notifications | Route::Issues | Route::PullRequests => blank(),
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

/// Home: recently involved issues/PRs on the left, following feed on the right.
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
            "Recently involved",
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
        .child(column_heading("Following feed", None, cx))
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
/// header information and content tabs belong to later tasks.
fn repository(repo: &RepoFixture, cx: &App) -> AnyElement {
    let theme = cx.theme();
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
                .child(
                    Avatar::new("repository-avatar", repo.namespace.clone()).size(AvatarSize::Lg),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(theme.text_size(TextSize::Xl))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.colors.fg)
                        .child(Ellipsis::new(repo.full_name())),
                ),
        )
        .child(
            div()
                .text_size(theme.text_size(TextSize::Sm))
                .text_color(theme.colors.fg_muted)
                .child(repo.description),
        )
        .into_any_element()
}

/// A profile route's minimal identifier; Overview is a later task.
fn profile(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let account = fixtures::ACCOUNT;
    div()
        .debug_selector(|| "page-profile".to_string())
        .flex()
        .items_center()
        .w_full()
        .px_6()
        .py_5()
        .gap_3()
        .child(Avatar::new("profile-avatar", account.name).size(AvatarSize::Xl))
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
                        .child(Ellipsis::new(account.name)),
                )
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Sm))
                        .text_color(theme.colors.fg_subtle)
                        .child(Ellipsis::new(format!("@{}", account.login))),
                ),
        )
        .into_any_element()
}
