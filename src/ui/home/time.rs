//! Compatibility shim for views that `#[path]`-include this file.
//!
//! The single relative-time implementation lives in
//! [`crate::ui::relative_time`] (shared by Home and Notifications); this module
//! only forwards so an existing `#[path = "../home/time.rs"] mod time;` keeps
//! resolving one formatter instead of a second copy.
pub(super) use crate::ui::relative_time::relative;
