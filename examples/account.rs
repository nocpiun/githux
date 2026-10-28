//! Standalone account and repository-read preview.
//!
//! Run with `cargo run --example account`. The example reuses the real account
//! service, GPUI bridge and shared scrollbar; the product binary never declares
//! this preview's modules or opens its account database.

// Keep the preview console-free on Windows, like the product. Tests keep their
// console output in a separate integration target.
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

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
#[path = "../src/storage/mod.rs"]
mod storage;

#[path = "../src/platform/fonts.rs"]
mod fonts;

// The example and the integration test entry both declare the product modules at these paths, so
// `crate::app::accounts::Accounts` and `crate::ui::accounts::…` are the very product types.
// The product modules below are compiled into this developer target where only part of their
// surface is exercised, so the narrow allowances name that reason instead of widening the product
// API or masking real warnings elsewhere.
#[allow(dead_code)]
mod app {
    #[path = "../../src/app/accounts/mod.rs"]
    pub mod accounts;
}
#[allow(dead_code)]
mod ui {
    #[path = "../../src/ui/accounts/mod.rs"]
    pub mod accounts;
    #[path = "../../src/ui/geometry.rs"]
    pub mod geometry;
    #[path = "../../src/ui/scroll/mod.rs"]
    pub mod scroll;
}

#[path = "../src/bin_support/account_example.rs"]
mod example;

use ely_core::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::App;

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
            example::open_window(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    // Once the event loop returns, let any submitted SQLite/credential commit and the executor
    // finish, bounded by the product controller's existing exit gate.
    app::accounts::wait_for_shutdown();
}
