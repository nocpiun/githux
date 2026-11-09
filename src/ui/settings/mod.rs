//! Native Shell Settings View.
//!
//! Provides the primary application Settings interface in Native Shell.
//! Houses category navigation (Accounts as the authoritative category) and
//! delegates account management to `ui::accounts::AccountsView`.

pub mod layout;

pub use layout::{Category, SettingsView};

#[cfg(test)]
mod tests;
