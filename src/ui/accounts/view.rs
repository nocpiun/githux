use std::rc::Rc;

use ely_gpui_component::{
    buttons::Button,
    data_display::Avatar,
    forms::{PasswordInput, TextInput},
    primitives::{Icon, IconName},
    theme::{ActiveTheme, AvatarSize, ControlSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, Styled, Window, div,
    prelude::*, px,
};

use crate::domain::account::AuthorizationState;
use crate::domain::repo::{PartialReason, ReposReadiness};
use crate::domain::{AccountKey, AccountSummary};

use super::{RepositoryPreview, StatusMessage, StatusTone};

pub(crate) type AccountAction = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
pub(crate) type SelectAction = Rc<dyn Fn(AccountKey, &mut Window, &mut App) + 'static>;

pub(crate) struct AccountCallbacks {
    pub submit: AccountAction,
    pub cancel: AccountAction,
    pub select: SelectAction,
    pub replace: SelectAction,
    pub revalidate: SelectAction,
    pub refresh: AccountAction,
    pub cancel_replace: AccountAction,
    pub open_pat_help: AccountAction,
}

pub(crate) struct AccountsView {
    pub token_input: Entity<TextInput>,
    pub replacing: bool,
    pub busy: bool,
    pub status: Option<StatusMessage>,
    pub accounts: Vec<AccountSummary>,
    pub active: Option<AccountKey>,
    pub repositories: RepositoryPreview,
    pub callbacks: AccountCallbacks,
}

pub(crate) fn render(view: AccountsView, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let AccountsView {
        token_input,
        replacing,
        busy,
        status,
        accounts,
        active,
        repositories,
        callbacks,
    } = view;
    let can_submit = !busy && !token_input.read(cx).text().trim().is_empty();
    let submit_label = if replacing {
        "Verify and replace PAT"
    } else {
        "Verify and add account"
    };

    let intro = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(theme.text_size(TextSize::Xs))
                .text_color(colors.fg_subtle)
                .child("Use a Classic personal access token. Public repositories need no extra scope; private repository visibility requires repo. The repo scope also grants write access, so it is not a read-only token. No future feature permissions are requested here."),
        )
        .child(
            Button::new("pat-help", "Create a Classic PAT on GitHub")
                .variant(ely_gpui_component::buttons::ButtonVariant::Link)
                .on_click(move |_, window, cx| (callbacks.open_pat_help)(window, cx))
        );

    let form = div()
        .debug_selector(|| "account-token-form".to_string())
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded(theme.radius(Radius::Lg))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .child(intro)
        .child(PasswordInput::new(&token_input))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("account-token-submit", submit_label)
                        .primary()
                        .disabled(!can_submit)
                        .loading(busy)
                        .on_click(move |_, window, cx| (callbacks.submit)(window, cx)),
                )
                .when(busy, |row| {
                    row.child(Button::new("account-operation-cancel", "Cancel").on_click({
                        let cancel = callbacks.cancel.clone();
                        move |_, window, cx| cancel(window, cx)
                    }))
                })
                .when(replacing && !busy, |row| {
                    row.child(
                        Button::new("account-replace-cancel", "Stop replacing").on_click({
                            let cancel = callbacks.cancel_replace.clone();
                            move |_, window, cx| cancel(window, cx)
                        }),
                    )
                }),
        );

    let account_rows = if accounts.is_empty() {
        vec![
            div()
                .text_color(colors.fg_subtle)
                .child("No saved accounts yet. A restored directory entry is never presented as verified access.")
                .into_any_element(),
        ]
    } else {
        accounts
            .into_iter()
            .map(|summary| {
                let key = summary.identity.key.clone();
                let is_active = active.as_ref() == Some(&key);
                let name = summary
                    .identity
                    .display_name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&summary.identity.login)
                    .to_string();
                let select = callbacks.select.clone();
                let replace = callbacks.replace.clone();
                let revalidate = callbacks.revalidate.clone();
                let select_key = key.clone();
                let replace_key = key.clone();
                let revalidate_key = key.clone();
                div()
                    .debug_selector(move || format!("saved-account-{}", key.id))
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_3()
                    .rounded(theme.radius(Radius::Md))
                    .border_1()
                    .border_color(if is_active {
                        colors.focus
                    } else {
                        colors.border
                    })
                    .child(
                        Avatar::new(("account-name-avatar", key.id), name.clone())
                            .size(AvatarSize::Md),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(Ellipsis::new(name)),
                            )
                            .child(
                                div()
                                    .text_size(theme.text_size(TextSize::Xs))
                                    .text_color(colors.fg_subtle)
                                    .child(Ellipsis::new(format!(
                                        "@{} · {}",
                                        summary.identity.login,
                                        authorization_label(&summary.authorization)
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new(
                                    ("account-select", key.id),
                                    if is_active { "Current" } else { "Use" },
                                )
                                .size(ControlSize::Sm)
                                .disabled(busy || is_active)
                                .on_click(move |_, window, cx| {
                                    select(select_key.clone(), window, cx)
                                }),
                            )
                            .child(
                                Button::new(("account-revalidate", key.id), "Revalidate")
                                    .size(ControlSize::Sm)
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        revalidate(revalidate_key.clone(), window, cx)
                                    }),
                            )
                            .child(
                                Button::new(("account-replace", key.id), "Replace PAT")
                                    .size(ControlSize::Sm)
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        replace(replace_key.clone(), window, cx)
                                    }),
                            ),
                    )
                    .into_any_element()
            })
            .collect()
    };

    let repository_section =
        render_repositories(repositories, callbacks.refresh, busy, active.is_none(), cx);
    let status = status.map(|status| {
        let color = match status.tone {
            StatusTone::Info => colors.fg_subtle,
            StatusTone::Success => colors.success,
            StatusTone::Warning => colors.warning,
            StatusTone::Error => colors.danger,
        };
        div()
            .debug_selector(|| "account-operation-status".to_string())
            .p_3()
            .rounded(theme.radius(Radius::Md))
            .bg(colors.surface)
            .text_color(color)
            .child(status.text)
            .into_any_element()
    });

    div()
        .debug_selector(|| "account-preview".to_string())
        .flex()
        .flex_col()
        .w_full()
        .max_w(px(1120.0))
        .mx_auto()
        .p_6()
        .gap_4()
        .text_size(theme.text_size(TextSize::Sm))
        .text_color(colors.fg)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Xl))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Accounts & repository access"),
                )
                .child(
                    div()
                        .text_color(colors.warning)
                        .child("Wiring acceptance preview · not the final Shell account UI"),
                ),
        )
        .children(status)
        .child(form)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(heading("Saved accounts", cx))
                .children(account_rows),
        )
        .child(repository_section)
        .into_any_element()
}

fn heading(text: &'static str, cx: &App) -> AnyElement {
    div()
        .text_size(cx.theme().text_size(TextSize::Lg))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
        .into_any_element()
}

fn authorization_label(state: &AuthorizationState) -> String {
    match state {
        AuthorizationState::PendingVerification => "saved · verification pending".into(),
        AuthorizationState::Verified => "verified this run".into(),
        AuthorizationState::MissingCredential => "saved · credential missing".into(),
        AuthorizationState::CredentialUnavailable => "saved · credential unavailable".into(),
        AuthorizationState::VerificationFailed(_) => "saved · verification failed".into(),
    }
}

fn render_repositories(
    preview: RepositoryPreview,
    refresh: AccountAction,
    busy: bool,
    no_account: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let mut body = Vec::new();
    match preview {
        RepositoryPreview::Idle(message) => {
            body.push(
                div()
                    .text_color(colors.fg_subtle)
                    .child(message)
                    .into_any_element(),
            );
        }
        RepositoryPreview::Loading => {
            body.push(
                div()
                    .debug_selector(|| "repository-preview-loading".to_string())
                    .child("Loading the visible repository set…")
                    .into_any_element(),
            );
        }
        RepositoryPreview::Failed(message) => {
            body.push(
                div()
                    .debug_selector(|| "repository-preview-error".to_string())
                    .text_color(colors.danger)
                    .child(message)
                    .into_any_element(),
            );
        }
        RepositoryPreview::Ready(outcome) => {
            let completeness = match &outcome.readiness {
                ReposReadiness::Complete => "Complete".to_string(),
                ReposReadiness::Partial { reason } => match reason {
                    PartialReason::PageFailed { .. } => "Partial · a later page failed".to_string(),
                    PartialReason::SsoPartial { organizations } => format!(
                        "Partial · organization SSO authorization required: {}",
                        organizations.join(", ")
                    ),
                },
            };
            body.push(
                div()
                    .debug_selector(|| "repository-preview-readiness".to_string())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if outcome.readiness == ReposReadiness::Complete {
                        colors.success
                    } else {
                        colors.warning
                    })
                    .child(format!(
                        "{} repositories · {completeness}",
                        outcome.repositories.len()
                    ))
                    .into_any_element(),
            );
            if outcome.repositories.is_empty() {
                body.push(
                    div()
                        .text_color(colors.fg_subtle)
                        .child(if outcome.readiness == ReposReadiness::Complete {
                            "The current token returned a complete empty repository set."
                        } else {
                            "No repositories were returned in the available partial result; this is not a complete empty set."
                        })
                        .into_any_element(),
                );
            }
            body.extend(
                outcome
                    .repositories
                    .into_iter()
                    .enumerate()
                    .map(|(index, repo)| {
                        div()
                            .debug_selector(move || format!("repository-preview-row-{index}"))
                            .flex()
                            .items_center()
                            .gap_2()
                            .py_1()
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(IconSize::Xs)
                                    .color(colors.fg_muted),
                            )
                            .child(div().min_w_0().child(Ellipsis::new(repo.full_name)))
                            .into_any_element()
                    }),
            );
        }
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(heading("Repository read preview", cx))
                .child(
                    Button::new("repository-preview-refresh", "Refresh repositories")
                        .size(ControlSize::Sm)
                        .disabled(busy || no_account)
                        .on_click(move |_, window, cx| refresh(window, cx)),
                ),
        )
        .child(
            div()
                .p_4()
                .rounded(theme.radius(Radius::Lg))
                .border_1()
                .border_color(colors.border)
                .bg(colors.surface)
                .children(body),
        )
        .into_any_element()
}
