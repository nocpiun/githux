//! Runs the reader module's tests in an integration target.
//!
//! `cargo test` builds example targets but does not run their tests, so the
//! reader's `#[cfg(test)]` suite is compiled here as well. This file only wires
//! the module tree the reader expects; the tests live beside the code.

#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/domain/mod.rs"]
mod domain;

// The module tree lives in one file so every `#[path]` inside it resolves
// against that file on Windows and Linux alike (and `rustfmt` can find it).
#[path = "../src/bin_support/reader_modules.rs"]
mod reader_modules;
#[allow(unused_imports)]
use reader_modules as ui;
#[allow(unused_imports)]
use reader_modules::reader;
