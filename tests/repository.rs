//! Runs the repository page's tests in an integration target.
//!
//! `cargo test` builds example targets but does not run their tests, so the
//! page's `#[cfg(test)]` suite is compiled here too. This file only wires the
//! module tree the page expects; the tests live beside the code. The same tree
//! (including `src/bin_support/repository_fixture.rs`) is used by
//! `examples/repository.rs`, so the preview and the suite browse one fixture.

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

#[path = "../src/bin_support/repository_modules.rs"]
mod repository_modules;
#[allow(unused_imports)]
use repository_modules as ui;
#[allow(unused_imports)]
use repository_modules::{fixture, repository};

#[path = "../src/platform/mod.rs"]
pub mod platform;
