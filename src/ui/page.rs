//! Page bodies not owned by a real page host.
//!
//! Home, Profile, Repository, Notifications, Issues and Pull Requests are mounted from
//! `app::pages`. This module contains only the transitional Settings surface, the fallback for
//! unmounted pages. It never supplies fixture activity or a second page implementation.
use ely_core::theme::{ActiveTheme, TextSize};
use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, div, prelude::*};

use crate::workspace::{Route, Session};

pub(super) struct PageData {
    pub settings: Option<AnyElement>,
}

pub(super) fn page(session: &Session, data: PageData, cx: &App) -> AnyElement {
    match session.route() {
        Route::Settings => data.settings.unwrap_or_else(|| {
            unavailable("Account services are not available in this build.", cx)
        }),
        Route::Notifications | Route::Issues | Route::PullRequests => unavailable(
            "This page needs an active account and verified online access. No saved page content is available.",
            cx,
        ),
        Route::Issue {
            namespace,
            name,
            number,
        }
        | Route::PullRequest {
            namespace,
            name,
            number,
        } => unavailable(
            &format!(
                "{namespace}/{name} #{number} cannot be read until an active account's online service is available."
            ),
            cx,
        ),
        Route::Home | Route::Profile | Route::UserProfile { .. } => unavailable(
            "This page needs an active account and verified online access. No saved page content is available.",
            cx,
        ),
        Route::Repository { namespace, name } => unavailable(
            &format!(
                "{namespace}/{name} cannot be read until an active account's online repository service is available."
            ),
            cx,
        ),
    }
}

fn unavailable(message: &str, cx: &App) -> AnyElement {
    div()
        .debug_selector(|| "page-service-unavailable".to_string())
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_6()
        .text_align(gpui::TextAlign::Center)
        .text_size(cx.theme().text_size(TextSize::Sm))
        .text_color(cx.theme().colors.fg_subtle)
        .child(message.to_string())
        .into_any_element()
}
