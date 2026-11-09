//! Githux native application library and core modules.

pub mod app;
pub mod auth;
pub mod content;
pub mod data;
pub mod diagnostics;
pub mod domain;
pub mod github;
pub mod network;
pub mod platform;
pub mod storage;
pub mod ui;
pub mod workspace;

#[cfg(test)]
#[path = "bin_support/repository_fixture.rs"]
pub mod repository_fixture;
#[cfg(test)]
pub mod repository_modules {
    pub use crate::repository_fixture as fixture;
}
