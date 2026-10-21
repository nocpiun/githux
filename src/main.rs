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

fn main() {
    app::run();
}
