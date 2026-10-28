//! The reusable account controls: a masked PAT form, the saved-account list, and the status line.
//!
//! Two callers use this file: the product Settings page and the developer-only account example.
//! Both hand over an `ely_core` text input they own, so nothing here constructs a service, reads a
//! credential or keeps the token. Behaviour differences belong to the callers; appearance and the
//! accessible names of the controls live here once.

use std::rc::Rc;

use ely_core::{
    buttons::Button,
    data_display::Avatar,
    forms::{PasswordInput, TextInput},
    theme::{ActiveTheme, AvatarSize, ControlSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, Styled, Window, div,
    prelude::*,
};

use crate::domain::account::AuthorizationState;
use crate::domain::{AccountKey, AccountSummary};

use super::{StatusMessage, StatusTone};

pub(crate) type AccountAction = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
pub(crate) type SelectAction = Rc<dyn Fn(AccountKey, &mut Window, &mut App) + 'static>;

/// The account operations the accepted PR28 preview already defined. Adding, selecting,
/// replacing and revalidating are the whole scope; there is no delete or logout path here.
#[derive(Clone)]
pub(crate) struct AccountCallbacks {
    pub submit: AccountAction,
    pub cancel: AccountAction,
    pub select: SelectAction,
    pub replace: SelectAction,
    pub revalidate: SelectAction,
    pub cancel_replace: AccountAction,
    pub open_pat_help: AccountAction,
}

/// The masked Classic PAT field plus its submit state.
pub(crate) struct TokenForm<'a> {
    pub token_input: Entity<TextInput>,
    /// The form is waiting for a replacement PAT for an already saved account.
    pub replacing: bool,
    /// An account operation is in flight; the form is inert while it is.
    pub busy: bool,
    pub callbacks: &'a AccountCallbacks,
}

/// One frame's Settings › Accounts block.
pub(crate) struct AccountsSection<'a> {
    pub token: TokenForm<'a>,
    pub accounts: Vec<AccountSummary>,
    pub active: Option<AccountKey>,
    /// Restored directories are not authorization: `PendingVerification` is called out.
    pub restored: bool,
    pub status: Option<StatusMessage>,
}

/// The masked input, its help link and the submit/cancel pair.
pub(crate) fn token_form(form: TokenForm<'_>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let TokenForm {
        token_input,
        replacing,
        busy,
        callbacks,
    } = form;
    let can_submit = !busy && !token_input.read(cx).text().trim().is_empty();
    let submit_label = if replacing {
        "Verify and replace PAT"
    } else {
        "Verify and add account"
    };
    let help = callbacks.open_pat_help.clone();
    let submit = callbacks.submit.clone();
    let cancel = callbacks.cancel.clone();
    let cancel_replace = callbacks.cancel_replace.clone();
    div()
        .debug_selector(|| "account-token-form".to_string())
        .flex()
        .flex_col()
        .gap_2()
        .child(
            Button::new("pat-help", "Create a Classic PAT on GitHub")
                .variant(ely_core::buttons::ButtonVariant::Link)
                .on_click(move |_, window, cx| help(window, cx)),
        )
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
                        .on_click(move |_, window, cx| submit(window, cx)),
                )
                .when(busy, |row| {
                    row.child(
                        Button::new("account-operation-cancel", "Cancel")
                            .on_click(move |_, window, cx| cancel(window, cx)),
                    )
                })
                .when(replacing && !busy, |row| {
                    row.child(
                        Button::new("account-replace-cancel", "Stop replacing")
                            .on_click(move |_, window, cx| cancel_replace(window, cx)),
                    )
                }),
        )
        .text_size(theme.text_size(TextSize::Xs))
        .text_color(colors.fg_subtle)
        .into_any_element()
}

/// The saved-account rows. Each row names the stable identity state instead of implying that a
/// restored directory is a verified authorization.
pub(crate) fn account_list(
    accounts: Vec<AccountSummary>,
    active: Option<AccountKey>,
    busy: bool,
    callbacks: &AccountCallbacks,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let mut rows = Vec::new();
    if accounts.is_empty() {
        rows.push(
            div()
                .debug_selector(|| "account-list-empty".to_string())
                .text_color(colors.fg_subtle)
                .child("No saved account yet. A restored directory entry is never presented as verified access.")
                .into_any_element(),
        );
    }
    for summary in accounts {
        let key = summary.identity.key.clone();
        let is_active = active.as_ref() == Some(&key);
        let name = display_name(&summary);
        let select = callbacks.select.clone();
        let replace = callbacks.replace.clone();
        let revalidate = callbacks.revalidate.clone();
        let select_key = key.clone();
        let replace_key = key.clone();
        let revalidate_key = key.clone();
        rows.push(
            div()
                .debug_selector(move || format!("saved-account-{}", key.id))
                .flex()
                .items_center()
                .gap_3()
                .p_2()
                .rounded(theme.radius(Radius::Md))
                .border_1()
                .border_color(if is_active {
                    colors.focus
                } else {
                    colors.border
                })
                .child(
                    Avatar::new(("account-list-avatar", key.id), name.clone()).size(AvatarSize::Sm),
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
                            .on_click(move |_, window, cx| select(select_key.clone(), window, cx)),
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
                .into_any_element(),
        );
    }
    div()
        .debug_selector(|| "account-list".to_string())
        .flex()
        .flex_col()
        .gap_2()
        .children(rows)
        .into_any_element()
}

/// Settings › Accounts: the minimal account management section, and the only product surface that
/// changes accounts this round.
pub(crate) fn accounts_section(section: AccountsSection<'_>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let AccountsSection {
        token,
        accounts,
        active,
        restored,
        status,
    } = section;
    let TokenForm {
        token_input,
        replacing,
        busy,
        callbacks,
    } = token;
    let empty = accounts.is_empty();
    let active_summary = active.as_ref().and_then(|key| {
        accounts
            .iter()
            .find(|summary| &summary.identity.key == key)
            .cloned()
    });

    div()
        .debug_selector(|| "settings-accounts".to_string())
        .flex()
        .flex_col()
        .gap_3()
        .w_full()
        .max_w(gpui::px(720.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Xl))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Accounts"),
                )
                .child(
                    div()
                        .text_color(colors.fg_subtle)
                        .child(if empty {
                            "Sign in with a Classic personal access token. Public repositories need no extra scope; private repository visibility requires the repo scope, which also grants write access."
                        } else {
                            "One account at a time is active. Each account keeps its own open tabs, so switching back restores them."
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded(theme.radius(Radius::Lg))
                .border_1()
                .border_color(colors.border)
                .bg(colors.surface)
                .when_some(active_summary.map(|summary| {
                    (
                        display_name(&summary),
                        summary.identity.login.clone(),
                        authorization_label(&summary.authorization).to_string(),
                    )
                }), |card, (name, login, state)| {
                    card.child(
                        div()
                            .debug_selector(|| "settings-current-account".to_string())
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                Avatar::new("settings-account-avatar", name.clone())
                                    .size(AvatarSize::Md),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
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
                                            .child(Ellipsis::new(format!("@{login} · {state}"))),
                                    ),
                            ),
                    )
                })
                .when(!empty, |card| {
                    card.child(account_list(accounts, active, busy, callbacks, cx))
                }),
        )
        .when(restored, |section| {
            section.child(
                div()
                    .debug_selector(|| "settings-restored-notice".to_string())
                    .text_color(colors.warning)
                    .child("Restored accounts are saved identity records, not verified access. Revalidate before relying on them."),
            )
        })
        .children(status.map(|status| status_line(status, cx)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded(theme.radius(Radius::Lg))
                .border_1()
                .border_color(colors.border)
                .bg(colors.surface)
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(if replacing {
                            "Replace the saved PAT for this account"
                        } else {
                            "Add or replace an account"
                        }),
                )
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Xs))
                        .text_color(colors.fg_subtle)
                        .child(if replacing {
                            "The replacement PAT must belong to the same account, or it is rejected and neither account changes."
                        } else {
                            "The PAT is verified with GET /user before anything is stored. Tokens live in the platform credential store, never in the account database."
                        }),
                )
                .child(token_form(
                    TokenForm {
                        token_input,
                        replacing,
                        busy,
                        callbacks,
                    },
                    cx,
                )),
        )
        .into_any_element()
}

/// One status message, coloured by tone. Also used by the example, so its selector stays stable.
/// It never contains a token: messages are built from typed service outcomes only.
pub(crate) fn status_line(status: StatusMessage, cx: &App) -> AnyElement {
    let colors = &cx.theme().colors;
    let color = match status.tone {
        StatusTone::Info => colors.fg_subtle,
        StatusTone::Success => colors.success,
        StatusTone::Warning => colors.warning,
        StatusTone::Error => colors.danger,
    };
    div()
        .debug_selector(|| "account-operation-status".to_string())
        .p_3()
        .rounded(cx.theme().radius(Radius::Md))
        .bg(colors.surface)
        .text_color(color)
        .child(status.text)
        .into_any_element()
}

/// The account's display name, falling back to the login when GitHub returned no name.
pub(crate) fn display_name(summary: &AccountSummary) -> String {
    summary
        .identity
        .display_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&summary.identity.login)
        .to_string()
}

/// What the saved record currently proves. Only `Verified` is evidence of this run's access.
pub(crate) fn authorization_label(state: &AuthorizationState) -> &'static str {
    match state {
        AuthorizationState::PendingVerification => "saved · verification pending",
        AuthorizationState::Verified => "verified this run",
        AuthorizationState::MissingCredential => "saved · credential missing",
        AuthorizationState::CredentialUnavailable => "saved · credential unavailable",
        AuthorizationState::VerificationFailed(_) => "saved · verification failed",
    }
}
