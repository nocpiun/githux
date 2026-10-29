//! The account settings surface: saved accounts, the add/replace form and their status.
//!
//! Two callers use this file: the product Settings page and the developer-only account example.
//! Both hand over an `ely_core` text input they own and a precomputed map of decoded avatars, so
//! nothing here constructs a service, reads a credential, keeps the token or starts an image fetch.
//! Behaviour belongs to the account controller; this module is presentation only.
//!
//! Layout follows Ely's own settings vocabulary — [`SettingsSection`] groups and [`SettingsRow`]
//! rows — so the page reads as one compact list rather than a stack of large cards. A saved account
//! needs an avatar beside its name, which `SettingsRow` (text title + control) cannot express, so
//! each account uses one minimal custom row that keeps the section's theme, row height and divider.

use std::collections::HashMap;
use std::rc::Rc;

use ely_core::{
    buttons::Button,
    forms::{PasswordInput, TextInput},
    primitives::{Icon, IconName},
    theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use ely_settings::{SettingsRow, SettingsSection};
use gpui::{
    AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, Styled, Window, div,
    prelude::*, px,
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
    /// The account whose PAT is being replaced, when the user pressed "Replace PAT" on a saved
    /// account that may not be the active one. The form names this account, not the active one, so
    /// the instructions match which account `replace` will actually target.
    pub replacing_account: Option<AccountKey>,
    /// Decoded avatar sources keyed by account id. The caller requests the fetches; this only draws
    /// whatever is already available and falls back to initials for the rest.
    pub avatars: HashMap<AccountKey, gpui::ImageSource>,
    pub status: Option<StatusMessage>,
}

/// Settings › Accounts. A compact list of saved accounts, then the add/replace form.
pub(crate) fn accounts_section(section: AccountsSection<'_>, cx: &App) -> AnyElement {
    let AccountsSection {
        token,
        accounts,
        active,
        replacing_account,
        avatars,
        status,
    } = section;
    let TokenForm {
        token_input,
        replacing,
        busy,
        callbacks,
    } = token;

    // Saved accounts: one row each, the current one marked with a light status instead of a
    // redundant disabled button. The active account is not repeated above the list.
    let mut list = SettingsSection::new("Accounts");
    if accounts.is_empty() {
        list = list.description("No account is signed in yet.");
    }
    for summary in &accounts {
        let key = summary.identity.key.clone();
        let is_active = active.as_ref() == Some(&key);
        let avatar = avatars.get(&key).cloned();
        list = list.row(account_row(
            summary, &key, avatar, is_active, busy, callbacks, cx,
        ));
    }
    let list = list.into_any_element();

    // The form is its own group with an explicit mode, so "add" and "replace" never blur together.
    let form_title = if replacing {
        "Replace personal access token"
    } else {
        "Add account"
    };
    // The replacement form names the account being replaced, which need not be the active one.
    let target = replacing_account
        .as_ref()
        .and_then(|key| accounts.iter().find(|summary| &summary.identity.key == key));
    let mut form = SettingsSection::new(form_title);
    form = match (replacing, target) {
        (true, Some(summary)) => form.description(format!(
            "Enter a new Classic PAT for {} (@{}). A token for a different account is rejected and neither account changes.",
            display_name(summary),
            summary.identity.login
        )),
        _ => form.description(
            "A Classic PAT is verified before it is stored. Public repositories need no extra scope; private repositories need the repo scope, which also grants write access.",
        ),
    };
    form = form.row(
        SettingsRow::new("Classic personal access token")
            .description("Paste the token; it stays masked and is never logged.")
            .control(
                div()
                    .w_full()
                    .min_w_0()
                    .child(PasswordInput::new(&token_input)),
            ),
    );
    form = form.row(token_actions(token_input, replacing, busy, callbacks, cx));
    let form = form.into_any_element();

    div()
        .debug_selector(|| "settings-accounts".to_string())
        .flex()
        .flex_col()
        .gap_6()
        .w_full()
        .max_w(px(720.0))
        .child(list)
        .child(form)
        .children(status.map(|status| status_line(status, cx)))
        .into_any_element()
}

/// One saved account: avatar, name, `@login`, a short authorization state, then its actions. The
/// row follows the section's theme and spacing; only the avatar needs a bespoke arrangement, which
/// `SettingsRow`'s text-only title cannot express. Actions wrap under the name in a narrow window.
fn account_row(
    summary: &AccountSummary,
    key: &AccountKey,
    avatar: Option<gpui::ImageSource>,
    is_active: bool,
    busy: bool,
    callbacks: &AccountCallbacks,
    cx: &App,
) -> AnyElement {
    let name = display_name(summary);
    let login = summary.identity.login.clone();
    let state = authorization_label(&summary.authorization);
    let needs_attention = matches!(
        summary.authorization,
        AuthorizationState::PendingVerification
            | AuthorizationState::MissingCredential
            | AuthorizationState::CredentialUnavailable
            | AuthorizationState::VerificationFailed(_)
    );
    let select = callbacks.select.clone();
    let replace = callbacks.replace.clone();
    let revalidate = callbacks.revalidate.clone();
    let select_key = key.clone();
    let replace_key = key.clone();
    let revalidate_key = key.clone();
    let row_id = key.id;

    div()
        .id(("saved-account", row_id))
        .debug_selector(move || format!("saved-account-{row_id}"))
        .role(gpui::Role::Group)
        .aria_label(format!("{name} (@{login})"))
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_x_4()
        .gap_y_2()
        .py_3()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .flex_1()
                .min_w_0()
                .child(crate::ui::avatar::avatar(
                    crate::ui::avatar::AvatarView {
                        id: &format!("account-{row_id}"),
                        selector: &format!("settings-account-avatar-{row_id}"),
                        image: avatar,
                        name: &name,
                        slot: crate::ui::avatar::AvatarSlot::Size(ely_core::theme::AvatarSize::Sm),
                    },
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
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
                                        .min_w_0()
                                        .text_size(cx.theme().text_size(TextSize::Sm))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(Ellipsis::new(name.clone())),
                                )
                                .when(is_active, |line| line.child(current_badge(cx))),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(cx.theme().text_size(TextSize::Xs))
                                .text_color(if needs_attention {
                                    cx.theme().colors.warning
                                } else {
                                    cx.theme().colors.fg_subtle
                                })
                                .child(Ellipsis::new(format!("@{login} · {state}"))),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .flex_wrap()
                .items_center()
                .justify_end()
                .gap_1()
                // Only a non-current account offers "Use"; the current one is marked, not disabled
                // in the action slot.
                .when(!is_active, |actions| {
                    actions.child(
                        Button::new(("account-select", row_id), "Use")
                            .size(ControlSize::Sm)
                            .disabled(busy)
                            .on_click(move |_, window, cx| select(select_key.clone(), window, cx)),
                    )
                })
                .child(
                    Button::new(("account-revalidate", row_id), "Revalidate")
                        .size(ControlSize::Sm)
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            revalidate(revalidate_key.clone(), window, cx)
                        }),
                )
                .child(
                    Button::new(("account-replace", row_id), "Replace PAT")
                        .size(ControlSize::Sm)
                        .disabled(busy)
                        .on_click(move |_, window, cx| replace(replace_key.clone(), window, cx)),
                ),
        )
        .into_any_element()
}

/// A light "Current" marker. It is a status, not a disabled action button.
fn current_badge(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let accent = theme.colors.accent;
    div()
        .debug_selector(|| "account-current-badge".to_string())
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .rounded(theme.radius(Radius::Sm))
        .px_1()
        .text_size(theme.text_size(TextSize::Xs))
        .text_color(accent)
        .child(Icon::new(IconName::Check).size(IconSize::Xs).color(accent))
        .child("Current")
        .into_any_element()
}

/// The submit/cancel row for the token form, with its help link.
fn token_actions(
    token_input: Entity<TextInput>,
    replacing: bool,
    busy: bool,
    callbacks: &AccountCallbacks,
    cx: &App,
) -> AnyElement {
    let help = callbacks.open_pat_help.clone();
    let submit = callbacks.submit.clone();
    let cancel = callbacks.cancel.clone();
    let cancel_replace = callbacks.cancel_replace.clone();
    let submit_label = if replacing {
        "Verify and replace"
    } else {
        "Verify and add"
    };
    div()
        .debug_selector(|| "account-token-form".to_string())
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_2()
        .py_2()
        .child(
            Button::new("pat-help", "Create a Classic PAT")
                .variant(ely_core::buttons::ButtonVariant::Link)
                .on_click(move |_, window, cx| help(window, cx)),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child({
                    let ready = !busy && !token_input.read(cx).text().trim().is_empty();
                    Button::new("account-token-submit", submit_label)
                        .primary()
                        .disabled(!ready)
                        .loading(busy)
                        .on_click(move |_, window, cx| submit(window, cx))
                })
                .when(busy, |row| {
                    row.child(
                        Button::new("account-operation-cancel", "Cancel")
                            .on_click(move |_, window, cx| cancel(window, cx)),
                    )
                })
                .when(replacing && !busy, |row| {
                    row.child(
                        Button::new("account-replace-cancel", "Cancel replacement")
                            .on_click(move |_, window, cx| cancel_replace(window, cx)),
                    )
                }),
        )
        .into_any_element()
}

/// The one status message for the whole account surface, shown once. Errors are not repeated as
/// separate yellow lines elsewhere; per-account state already lives on each account's row.
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
        AuthorizationState::PendingVerification => "verification pending",
        AuthorizationState::Verified => "verified",
        AuthorizationState::MissingCredential => "credential missing",
        AuthorizationState::CredentialUnavailable => "credential unavailable",
        AuthorizationState::VerificationFailed(_) => "verification failed",
    }
}
