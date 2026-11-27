//! Desktop executable. Application startup owns framework initialization.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

mod app;
mod auth;
mod content;
mod data;
mod diagnostics;
mod domain;
mod github;
mod network;
mod platform;
mod storage;
mod ui;
mod workspace;

#[cfg(test)]
#[path = "bin_support/test_repository_modules.rs"]
mod repository_modules;
#[cfg(test)]
use ui::{profile, reader};
#[cfg(test)]
#[path = "ui/profile_fixture_tests.rs"]
mod profile_fixture_tests;

// Developer preview scenarios reused by the profile fixture geometry test; preview-only helpers
// stay unused in this test assembly while `examples/profile.rs` still compiles them.
#[cfg(test)]
#[allow(dead_code)]
#[path = "bin_support/profile_fixtures.rs"]
mod profile_fixtures;

#[cfg(test)]
#[allow(dead_code)]
#[path = "bin_support/settings_fixture.rs"]
mod settings_fixture;

// Shared development fixture for README picture tests; preview-only helpers stay unused in this
// test assembly while `examples/reader_images.rs` still compiles them.
#[cfg(test)]
#[allow(dead_code)]
#[path = "bin_support/reader_images_fixture.rs"]
mod fixture;
#[cfg(test)]
#[path = "bin_support/reader_images_performance_fixture.rs"]
mod performance_fixture;
#[cfg(test)]
#[path = "bin_support/reader_images_view_tests.rs"]
mod reader_images_view_tests;
#[cfg(test)]
#[path = "ui/scroll_viewports_tests.rs"]
mod scroll_viewports_tests;

fn main() {
    app::run();
}
