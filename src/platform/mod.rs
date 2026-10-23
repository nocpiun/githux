//! Platform integration: bundled fonts now; IME, credentials and OS services later.

pub mod fonts;

#[cfg(target_os = "windows")]
pub mod caption;

#[cfg(target_os = "windows")]
mod wasdk_bindings;
