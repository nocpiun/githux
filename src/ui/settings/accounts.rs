use ely_core::{
    buttons::{Button, ButtonVariant},
    forms::PasswordInput,
    theme::{ActiveTheme, AvatarSize, ControlSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, Styled, div, prelude::*, px,
};

use super::{SettingsBinding, binding::FormMode};
use crate::{
    domain::AccountSummary,
    domain::account::AuthorizationState,
    ui::{
        accounts::StatusTone,
        avatar::{AvatarSlot, AvatarView, avatar},
    },
};

pub(super) fn authorization(state: &AuthorizationState) -> (&'static str, bool) {
    use crate::domain::error::GitHubError;
    match state {
        AuthorizationState::Verified => ("Verified in this session", false),
        AuthorizationState::PendingVerification => {
            ("Verification pending · Revalidate to check", true)
        }
        AuthorizationState::MissingCredential => ("Credential missing · Replace PAT", true),
        AuthorizationState::CredentialUnavailable => {
            ("Credential unavailable · Revalidate or replace PAT", true)
        }
        AuthorizationState::VerificationFailed(GitHubError::Unauthorized) => {
            ("Authorization expired or rejected · Replace PAT", true)
        }
        AuthorizationState::VerificationFailed(
            GitHubError::Network { .. } | GitHubError::Timeout { .. },
        ) => ("Could not connect · Revalidate when connected", true),
        AuthorizationState::VerificationFailed(_) => {
            ("Verification failed · Revalidate or replace PAT", true)
        }
    }
}

fn name(summary: &AccountSummary) -> String {
    summary
        .identity
        .display_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&summary.identity.login)
        .to_owned()
}

fn identity(
    summary: &AccountSummary,
    current: bool,
    image: Option<gpui::ImageSource>,
    cx: &App,
) -> AnyElement {
    let id = summary.identity.key.id;
    let display = name(summary);
    div()
        .flex()
        .items_center()
        .gap_3()
        .min_w_0()
        .flex_1()
        .child(avatar(
            AvatarView {
                id: &format!("settings-face-{id}"),
                selector: &format!("settings-face-{id}"),
                image,
                name: &display,
                slot: AvatarSlot::Size(AvatarSize::Sm),
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
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .min_w_0()
                                .font_weight(FontWeight::MEDIUM)
                                .child(Ellipsis::new(display)),
                        )
                        .when(current, |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.0))
                                    .text_color(cx.theme().colors.fg_muted)
                                    .child("✓ Current"),
                            )
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(12.0))
                        .text_color(cx.theme().colors.fg_muted)
                        .child(Ellipsis::new(format!("@{}", summary.identity.login))),
                ),
        )
        .into_any_element()
}

pub(super) fn content(binding: Entity<SettingsBinding>, cx: &mut App) -> AnyElement {
    let controller = binding.read(cx).controller.clone();
    let snapshot = controller.read(cx).snapshot().clone();
    let busy = controller.read(cx).busy();
    let mode = binding.read(cx).mode.clone();
    let add = SettingsBinding::action(binding.clone(), |this, window, cx| {
        this.open(FormMode::Add, window, cx)
    });
    let add_focus = binding.read(cx).add_focus.clone();
    let mut content = div()
        .id("settings-accounts-content")
        .debug_selector(|| "settings-accounts-content".into())
        .w_full()
        .min_w_0()
        .max_w(px(720.0))
        .flex()
        .flex_col()
        .gap_6()
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_start()
                .justify_between()
                .gap_3()
                .child(
                    div().min_w_0().flex_1().child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Accounts"),
                    ),
                )
                .child(
                    Button::new("settings-add", "Add account")
                        .focus_handle(&add_focus)
                        .disabled(busy)
                        .primary()
                        .on_click(move |_, window, cx| add(window, cx)),
                ),
        )
        .child(
            div()
                .text_color(cx.theme().colors.fg_muted)
                .child("Choose the account used for GitHub reads, or add another identity."),
        );
    let mut list = div().w_full().min_w_0().flex().flex_col().child(
        div()
            .pb_2()
            .font_weight(FontWeight::SEMIBOLD)
            .child("Saved accounts"),
    );
    if snapshot.accounts.is_empty() {
        list = list.child(
            div()
                .py_3()
                .child("No saved accounts. Add an account to get started."),
        );
    }
    for summary in &snapshot.accounts {
        let key = summary.identity.key.clone();
        let id = key.id;
        let current = snapshot.active.as_ref() == Some(&key);
        let replace_focus = binding.update(cx, |this, cx| {
            this.replace_focus
                .entry(key.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone()
        });
        let replace_key = key.clone();
        let replace_binding = binding.clone();
        let select_binding = binding.clone();
        let select_key = key.clone();
        let revalidate_binding = binding.clone();
        let revalidate_key = key.clone();
        let image = crate::ui::avatar::account_key(&summary.identity).and_then(|key| {
            match controller.update(cx, |this, _| this.avatar(key)) {
                crate::app::avatar::AvatarState::Ready(image) => {
                    Some(gpui::ImageSource::Image(image))
                }
                _ => None,
            }
        });
        let (state, warning) = authorization(&summary.authorization);
        let identity = identity(summary, current, image, cx);
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .min_w_0()
            .max_w_full()
            .when(!current, |row| {
                row.child(
                    div()
                        .debug_selector(move || format!("settings-switch-{id}"))
                        .child(
                            Button::new(("settings-switch", id), "Switch")
                                .size(ControlSize::Sm)
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    select_binding
                                        .update(cx, |this, cx| this.select(select_key.clone(), cx))
                                }),
                        ),
                )
            })
            .child(
                Button::new(("settings-revalidate", id), "Revalidate")
                    .size(ControlSize::Sm)
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        revalidate_binding
                            .update(cx, |this, cx| this.revalidate(revalidate_key.clone(), cx))
                    }),
            )
            .child(
                Button::new(("settings-replace", id), "Replace PAT")
                    .size(ControlSize::Sm)
                    .disabled(busy)
                    .focus_handle(&replace_focus)
                    .on_click(move |_, window, cx| {
                        replace_binding.update(cx, |this, cx| {
                            this.open(FormMode::Replace(replace_key.clone()), window, cx)
                        })
                    }),
            );
        list = list.child(
            div()
                .id(("settings-account", id))
                .debug_selector(move || format!("settings-account-{id}"))
                .role(gpui::Role::Group)
                .aria_label(format!("{} (@{})", name(summary), summary.identity.login))
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .py_3()
                .border_b_1()
                .border_color(cx.theme().colors.border)
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_wrap()
                        .gap_3()
                        .items_center()
                        .justify_between()
                        .child(identity)
                        .child(actions),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(if warning {
                            cx.theme().colors.warning
                        } else {
                            cx.theme().colors.fg_muted
                        })
                        .child(state),
                ),
        );
    }
    content = content.child(list);
    // The one operation message lives next to the form when one is open, otherwise below the list.
    let status = |cx: &App| {
        if !binding.read(cx).activity {
            return None;
        }
        snapshot.status.as_ref().map(|status| {
            let color = match status.tone {
                StatusTone::Error => cx.theme().colors.danger,
                StatusTone::Warning => cx.theme().colors.warning,
                StatusTone::Success => cx.theme().colors.success,
                StatusTone::Info => cx.theme().colors.fg_muted,
            };
            div()
                .id("settings-status")
                .role(gpui::Role::Status)
                .text_size(px(12.0))
                .text_color(color)
                .child(status.text.clone())
        })
    };
    // One operation message only. The list shows it when no form is open; an open form shows it
    // below the input. It is rendered once and moved between those locations, never duplicated.
    if let Some(mode) = mode {
        let replacing = matches!(mode, FormMode::Replace(_));
        let input = binding.read(cx).input.clone();
        if input.read(cx).is_disabled() != busy {
            input.update(cx, |input, cx| input.set_disabled(busy, cx));
        }
        let has_value = !input.read(cx).text().trim().is_empty();
        let help = binding.read(cx).help;
        let help_binding = binding.clone();
        let cancel =
            SettingsBinding::action(binding.clone(), |this, window, cx| this.close(window, cx));
        let submit =
            SettingsBinding::action(binding.clone(), |this, window, cx| this.submit(window, cx));
        let help_controller = controller.clone();
        let mut form = div()
            .id("settings-pat-form")
            .debug_selector(|| "settings-pat-form".into())
            .flex()
            .flex_col()
            .gap_3()
            .min_w_0()
            .w_full()
            .pt_5()
            .border_t_1()
            .border_color(cx.theme().colors.border)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(if replacing {
                "Replace personal access token"
            } else {
                "Add account"
            }));
        if let FormMode::Replace(key) = &mode
            && let Some(target) = snapshot
                .accounts
                .iter()
                .find(|summary| &summary.identity.key == key)
        {
            form = form.child(identity(target, false, None, cx)).child(
                div()
                    .text_size(px(12.0))
                    .text_color(cx.theme().colors.fg_muted)
                    .child("Replacing this account only."),
            );
        }
        form = form.child("Classic personal access token")
            .child(div().id("settings-token-field").w_full().min_w_0().child(PasswordInput::new(&input)))
            .when_some(binding.read(cx).field_error, |form, error| form.child(div().text_size(px(12.0)).text_color(cx.theme().colors.danger).child(error)))
            .children(status(cx))
            .child(div().text_size(px(12.0)).text_color(cx.theme().colors.fg_muted).child("Verified before it is saved. The token stays masked and is not written to logs."))
            .child(Button::new("settings-help", "About Classic PAT and permissions").variant(ButtonVariant::Link)
                .on_click(move |_, _, cx| help_binding.update(cx, |this, cx| { this.help = !this.help; cx.notify(); })))
            .when(help, |form| form.child(div().min_w_0().flex().flex_col().gap_2().text_size(px(12.0))
                .child("Public repositories need no extra scope. Private repositories need repo, which also grants write access. Notifications and organization SSO may require additional authorization.")
                .child(Button::new("settings-create-pat", "Create a Classic PAT on GitHub").variant(ButtonVariant::Link)
                    .on_click(move |_, _, cx| help_controller.update(cx, |this, cx| this.open_pat_help(cx))))))
            .child(div().flex().flex_wrap().justify_end().gap_2()
                .child(Button::new("settings-cancel", if replacing { "Cancel replacement" } else { "Cancel" }).on_click(move |_, window, cx| cancel(window, cx)))
                .child(Button::new("settings-submit", if busy { "Verifying…" } else if replacing { "Verify and replace" } else { "Verify and add" })
                    .primary().disabled(busy || !has_value).on_click(move |_, window, cx| submit(window, cx))));
        content = content.child(form);
    } else {
        content = content.children(status(cx));
    }
    content.into_any_element()
}
