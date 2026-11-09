//! Native Shell Accounts surface. Owned by nsh-accounts.
//!
//! `AccountsView` renders the account list, PAT form, busy/cancel state and recovery notice, and
//! submits only through the shared `RuntimeHandle`. It is used for sign-in and Settings.

mod controller;
pub mod view;

pub use view::{AccountAvatar, AccountAvatarProvider, AccountsView};

#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod tests;
