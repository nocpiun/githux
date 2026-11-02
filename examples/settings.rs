//! Offline native Settings + Accounts preview. No Shell routes or real saved accounts are opened.
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

#[path = "../src/app/mod.rs"]
#[allow(dead_code)]
mod app;
#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/platform/mod.rs"]
#[allow(dead_code)]
mod platform;
#[path = "../src/ui/settings/mod.rs"]
mod settings;
#[path = "../src/bin_support/settings_example.rs"]
mod settings_example;
#[path = "../src/bin_support/settings_fixture.rs"]
mod settings_fixture;
// The product `ui` tree now contains repository modules whose own test suites reference
// `crate::repository_modules::fixture`. Alias the single real fixture instead of compiling a second
// copy of the repository module tree, which would duplicate its types.
#[cfg(test)]
#[path = "../src/bin_support/repository_fixture.rs"]
#[allow(dead_code)]
pub mod repository_fixture;
#[cfg(test)]
mod repository_modules {
    pub use crate::repository_fixture as fixture;
}
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/ui/mod.rs"]
#[allow(dead_code)]
mod ui;
#[path = "../src/workspace/mod.rs"]
#[allow(dead_code)]
mod workspace;

use ely_core::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::App;
fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_core::init(cx).expect("Ely initialization");
            platform::fonts::register(cx).expect("bundled fonts");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = platform::fonts::UI_FAMILY.into();
                theme.mono_family = platform::fonts::CODE_FAMILY.into();
            });
            let initial = match std::env::var("GITHUX_SETTINGS_PREVIEW_FORM").as_deref() {
                Ok("add") => Some(settings::FormMode::Add),
                Ok("replace") => Some(settings::FormMode::Replace(domain::AccountKey::new(
                    2,
                    domain::HostId::github(),
                ))),
                _ => None,
            };
            let initial_category = std::env::var("GITHUX_SETTINGS_PREVIEW_CATEGORY")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0)
                .min(5);
            settings_example::open_window_with(cx, initial, initial_category);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    app::accounts::wait_for_shutdown();
}
