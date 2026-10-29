//! Flat profile identity header. Public metadata is secondary to the person.
use super::{ProfileActionKind, ProfileView, SectionState};
use crate::ui::tooltip::{ui_tooltip, ui_tooltip_content};
use ely_core::{
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, IconSize, TextSize},
};
use gpui::{
    AnyElement, App, Context, FontWeight, IntoElement, ParentElement, Styled, div, prelude::*,
};

impl ProfileView {
    pub(super) fn identity_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let SectionState::Ready(basic) = &self.basic else {
            return match &self.basic {
                SectionState::Error(error) => super::render::state_panel(error, cx),
                _ => super::render::loading("Loading profile…", cx),
            };
        };
        let theme = cx.theme();
        let display = super::render::clean(&basic.name)
            .unwrap_or(&basic.login)
            .to_string();
        let mut metadata = Vec::new();
        if let Some(location) = super::render::clean(&basic.location) {
            metadata.push(meta(IconName::MapPin, location.to_string(), cx));
        }
        if let Some(url) = super::render::clean(&basic.website_url) {
            let url = url.to_string();
            let label = website_label(&url);
            let request = self.request();
            let keyboard_request = request.clone();
            let keyboard_url = url.clone();
            let link = div()
                .id("profile-website")
                .debug_selector(|| "profile-website".into())
                .flex()
                .items_center()
                .gap_1p5()
                .min_w_0()
                .max_w_full()
                .cursor_pointer()
                .focusable()
                .tab_stop(true)
                .text_color(theme.colors.link)
                .on_key_down(
                    cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && view.accepts(&keyboard_request)
                        {
                            view.emit(
                                ProfileActionKind::ExternalLink {
                                    url: keyboard_url.clone(),
                                },
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }
                    }),
                )
                .child(
                    Icon::new(IconName::Link)
                        .size(IconSize::Xs)
                        .color(theme.colors.fg_muted),
                )
                .child(div().hover(|s| s.underline()).child(label))
                .tooltip(ui_tooltip(url.clone()))
                .on_click(cx.listener(move |view, _, window, cx| {
                    if view.accepts(&request) {
                        view.emit(
                            ProfileActionKind::ExternalLink { url: url.clone() },
                            window,
                            cx,
                        );
                    }
                }));
            metadata.push(link.into_any_element());
        }
        let follow = match (basic.followers, basic.following) {
            (Some(a), Some(b)) => Some(format!("{a} followers · {b} following")),
            (Some(a), None) => Some(format!("{a} followers")),
            (None, Some(b)) => Some(format!("{b} following")),
            _ => None,
        };
        if let Some(follow) = follow {
            metadata.push(meta(IconName::Users, follow, cx));
        }
        if let Some(company) = super::render::clean(&basic.company) {
            metadata.push(meta(IconName::House, company.to_string(), cx));
        }
        div()
            .debug_selector(|| "profile-basic".into())
            .w_full()
            .min_w_0()
            .flex()
            .items_start()
            .gap_5()
            .child(crate::ui::avatar::avatar(
                crate::ui::avatar::AvatarView {
                    id: "profile-header-avatar",
                    selector: "profile-avatar-initials",
                    image: self.avatar.clone(),
                    name: &display,
                    slot: crate::ui::avatar::AvatarSlot::Rems(gpui::rems(84.0 / 16.0)),
                },
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .debug_selector(|| "profile-name".into())
                            .text_size(theme.text_size(TextSize::Xl))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(display),
                    )
                    .child(
                        div()
                            .debug_selector(|| "profile-login".into())
                            .text_size(theme.text_size(TextSize::Base))
                            .text_color(theme.colors.fg_muted)
                            .child(format!("@{}", basic.login)),
                    )
                    .children(super::render::clean(&basic.bio).map(|bio| {
                        div()
                            .debug_selector(|| "profile-bio".into())
                            .mt_1()
                            .w_full()
                            .text_size(theme.text_size(TextSize::Sm))
                            .text_color(theme.colors.fg_muted)
                            .child(bio.to_string())
                    }))
                    .when(!metadata.is_empty(), |column| {
                        column.child(
                            div()
                                .debug_selector(|| "profile-public-metadata".into())
                                .w_full()
                                .min_w_0()
                                .mt_1()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_x_4()
                                .gap_y_1()
                                .text_size(theme.text_size(TextSize::Sm))
                                .text_color(theme.colors.fg_muted)
                                .children(metadata),
                        )
                    }),
            )
            .into_any_element()
    }
}

fn meta(icon: IconName, text: String, cx: &App) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .min_w_0()
        .max_w_full()
        .child(
            Icon::new(icon)
                .size(IconSize::Xs)
                .color(cx.theme().colors.fg_muted),
        )
        .child(text)
        .into_any_element()
}

pub(super) fn website_label(url: &str) -> String {
    // Display text only. The original supplied address is preserved in the action.
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_string()
}
