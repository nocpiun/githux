//! Standalone developer preview of the reusable Profile Overview.
//!
//! `cargo run --example profile` uses only synthetic data. The normal product
//! binary has no scenario chooser, preview route or debug flag.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/bin_support/profile_modules.rs"]
mod profile_modules;
#[allow(unused_imports)]
use profile_modules as ui;
#[allow(unused_imports)]
use profile_modules::profile;

use ely_core::{
    Assets,
    theme::{ActiveTheme, Mode, TextSize, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use std::rc::Rc;

use crate::{
    domain::{AccountKey, AuthEpoch, HostId},
    profile::{ProfileAction, ProfileBinding, ProfileTarget, ProfileView, SectionState},
    profile_modules::reader::ResourceContext,
};

#[path = "../src/bin_support/profile_fixtures.rs"]
mod fixtures;
use fixtures::{SCENARIOS, Scenario, basic, contributions, readme, repositories, rolling_year};

struct Preview {
    profile: Entity<ProfileView>,
    scenario: Scenario,
    action: Option<String>,
}

impl Preview {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let target = ProfileTarget::new(HostId::github(), 42);
        let binding = ProfileBinding::new(AccountKey::new(7, HostId::github()), AuthEpoch(1));
        let profile = cx.new(|cx| ProfileView::new(target, binding, rolling_year(), window, cx));
        let mut preview = Self {
            profile,
            scenario: if std::env::args().any(|arg| arg == "--long") {
                Scenario::Partial
            } else {
                Scenario::Complete
            },
            action: None,
        };
        let sink = cx.entity().downgrade();
        preview.profile.update(cx, |profile, _| {
            profile.on_action(Rc::new(move |action, _, cx| {
                if let Some(preview) = sink.upgrade() {
                    preview.update(cx, |preview, cx| {
                        preview.action = Some(describe(&action));
                        cx.notify();
                    });
                }
            }));
        });
        preview.load(window, cx);
        preview
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = ProfileTarget::new(HostId::github(), 42);
        let binding = ProfileBinding::new(AccountKey::new(7, HostId::github()), AuthEpoch(1));
        let request = self.profile.update(cx, |profile, cx| {
            profile.bind(target, binding, rolling_year(), cx)
        });
        let scenario = self.scenario;
        self.action = None;
        self.profile.update(cx, |profile, cx| {
            let basic_state = SectionState::Ready(basic(scenario));
            profile.update_basic(&request, basic_state, cx);
            profile.update_repositories(&request, repositories(scenario), cx);
            profile.update_contributions(&request, &rolling_year(), contributions(scenario), cx);
            profile.update_readme_with_context(
                &request,
                readme(scenario),
                ResourceContext::new("lin-example", "lin-example", "a".repeat(40), "README.md"),
                window,
                cx,
            );
        });
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let selected = SCENARIOS
            .iter()
            .position(|(scenario, _)| *scenario == self.scenario)
            .unwrap_or(0);
        div()
            .debug_selector(|| "profile-preview-toolbar".into())
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .text_size(cx.theme().text_size(TextSize::Xs))
            .text_color(cx.theme().colors.fg_muted)
            .child("Profile preview")
            .child(
                div().w(px(190.0)).child(
                    ely_core::forms::Select::new(
                        "profile-scenario",
                        SCENARIOS.iter().enumerate().map(|(index, (_, label))| {
                            ely_core::forms::Choice::new(index.to_string(), *label)
                        }),
                    )
                    .selected(selected.to_string())
                    .size(ely_core::theme::ControlSize::Sm)
                    .on_change(move |value, window, cx| {
                        if let Ok(index) = value.parse::<usize>()
                            && let Some((scenario, _)) = SCENARIOS.get(index)
                        {
                            this.update(cx, |preview, cx| {
                                preview.scenario = *scenario;
                                preview.load(window, cx);
                            });
                        }
                    }),
                ),
            )
    }
}

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let action = self
            .action
            .clone()
            .unwrap_or_else(|| "Actions appear here; navigation is handled by the host.".into());
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors.bg)
            .text_color(cx.theme().colors.fg)
            .child(self.toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .min_h_0()
                    .min_w_0()
                    .child(self.profile.clone()),
            )
            .child(
                div()
                    .debug_selector(|| "profile-preview-action".to_string())
                    .p_2()
                    .border_t_1()
                    .border_color(cx.theme().colors.border)
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .text_color(cx.theme().colors.fg_subtle)
                    .min_w_0()
                    .child(ely_core::typography::Ellipsis::new(action)),
            )
    }
}

fn describe(action: &ProfileAction) -> String {
    format!("target {} · {:?}", action.request.target, action.kind)
}

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_core::init(cx).expect("Ely failed to start");
            fonts::register(cx).expect("the bundled fonts failed to register");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = fonts::UI_FAMILY.into();
                theme.mono_family = fonts::CODE_FAMILY.into();
            });
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1100.0), px(800.0)),
                        cx,
                    ))),
                    window_min_size: Some(size(px(880.0), px(560.0))),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("Githux · Profile Overview preview".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Preview::new(window, cx)),
            )
            .expect("the Profile preview window failed to open");
            cx.activate(true);
        });
}
