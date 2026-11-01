//! Native settings components. The public configuration contains Accounts only.
//! The host supplies the existing account controller through `SettingsBinding`.
// The coordinator mounts this module in Shell; the standalone example compiles it meanwhile.
#![allow(dead_code)]

mod accounts;
mod binding;
mod layout;

pub(crate) use binding::{FormMode, SettingsBinding};
pub(crate) use layout::{Category, SettingsView};

#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod tests;
