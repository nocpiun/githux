//! Page bodies not owned by a real page host.
//!
//! Home, Profile and Repository are mounted from `app::pages`; this module contains only the
//! transitional Settings surface and the intentionally blank, not-yet-designed routes. It never
//! supplies fixture activity or a second repository/profile implementation.

use ely_core::theme::{ActiveTheme, TextSize};
use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, div, prelude::*};

use crate::workspace::{Route, Session};

pub(super) struct PageData {
    pub settings: Option<AnyElement>,
}

pub(super) fn page(session: &Session, data: PageData, cx: &App) -> AnyElement {
    match session.route() {
        Route::Settings => data.settings.unwrap_or_else(blank),
        Route::Notifications | Route::Issues | Route::PullRequests => blank(),
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

fn blank() -> AnyElement {
    div().size_full().into_any_element()
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
