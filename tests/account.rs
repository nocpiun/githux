//! Integration entry for the real account preview's module tests.
//!
//! Examples are built but their `#[cfg(test)]` tests are not executed by Cargo,
//! so `cargo test` runs the existing bridge and account tests through this
//! thin module assembly. The test bodies remain beside the code they exercise.

#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;

#[path = "../src/bin_support/account_modules.rs"]
mod account_modules;
#[allow(unused_imports)]
use account_modules as ui;
#[allow(unused_imports)]
use account_modules::bridge as app;

#[path = "../src/platform/mod.rs"]
pub mod platform;
