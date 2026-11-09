//! Pure Rust NSH entry and shared foundation. The GPUI application remains a separate package.
pub mod foundation;
pub mod preview;
pub mod reader;
pub mod runtime;
pub mod shell;

// Reuse the same framework-independent sources as the legacy application.
#[path = "../auth/mod.rs"]
pub(crate) mod auth;
#[path = "../content/mod.rs"]
pub mod content;
#[path = "../data/mod.rs"]
pub(crate) mod data;
#[path = "../domain/mod.rs"]
pub mod domain;
#[path = "../github/mod.rs"]
pub(crate) mod github;
#[path = "../network/mod.rs"]
pub(crate) mod network;
#[path = "../storage/mod.rs"]
pub(crate) mod storage;

/// A root with the application's embedded fonts. No installed fonts or current directory required.
pub fn root() -> nsh_engine::Root {
    foundation::fonts::install(nsh_engine::Root::of::<preview::MigrationPreview>())
        .tokens(foundation::theme::tokens())
        .app_id("dev.githux.native-shell")
}

#[cfg(test)]
mod tests;
