//! Standalone account and repository-read preview.
//!
//! Run with `cargo run --example account`. The example reuses the real account service, the
//! product account controller, the product account presentation and the shared scrollbar; the
//! product binary never declares this preview's modules or opens its account database.
//!
//! Every module below is declared at this file's top level with `#[path]`, exactly like
//! `tests/account.rs`, so `rustfmt` and the Rust module resolver agree on every platform. An inline
//! `mod { #[path] … }` block would make `#[path]` resolve against a virtual directory that does not
//! exist on Linux.

// Keep the preview console-free on Windows, like the product. Tests keep their console output in a
// separate integration target.
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

// The product modules this developer target reuses. They are declared at the top level so
// `crate::app::accounts::Accounts` and `crate::ui::accounts::…` are the very product types, with no
// second, incompatible copy.
#[path = "../src/app/mod.rs"]
#[allow(dead_code)]
mod app;
#[path = "../src/platform/mod.rs"]
#[allow(dead_code)]
mod platform;
#[path = "../src/ui/mod.rs"]
#[allow(dead_code)]
mod ui;
#[path = "../src/workspace/mod.rs"]
#[allow(dead_code)]
mod workspace;

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
            platform::fonts::register(cx).expect("the bundled fonts failed to register");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = platform::fonts::UI_FAMILY.into();
                theme.mono_family = platform::fonts::CODE_FAMILY.into();
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
