//! Native settings components. The public configuration contains Accounts only.
//! The host supplies the existing account controller through `SettingsBinding`.

mod accounts;
mod binding;
mod layout;

pub(crate) use binding::{FormMode, SettingsBinding};
// `Category` is used by `examples/settings.rs` (`bin_support/settings_example.rs`) and tests.
#[allow(unused_imports)]
pub(crate) use layout::Category;
pub(crate) use layout::SettingsView;

#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod tests;
