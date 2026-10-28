//! Runs the repository overview module's tests in an integration target.
//!
//! `cargo test` builds example targets but does not run their tests, so the component's
//! `#[cfg(test)]` suite is compiled here as well. This file only wires the module tree the
//! component expects; the tests live beside the code.

#[path = "../src/domain/mod.rs"]
mod domain;

// The module tree lives in one file so every `#[path]` inside it resolves against that file on
// Windows and Linux alike (and `rustfmt` can find it).
#[path = "../src/bin_support/repository_overview_modules.rs"]
mod repository_overview_modules;
#[allow(unused_imports)]
use repository_overview_modules as ui;
#[allow(unused_imports)]
use repository_overview_modules::repository_overview;
