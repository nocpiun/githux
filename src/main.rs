//! Desktop executable. Application startup owns framework initialization.

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
