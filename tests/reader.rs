//! Runs the reader module's tests in an integration target.
//!
//! `cargo test` builds example targets but does not run their tests, so the
//! reader's `#[cfg(test)]` suite is compiled here as well. This file only wires
//! the module tree the reader expects; the tests live beside the code.

#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/ui/reader/mod.rs"]
#[allow(unused_imports)]
mod reader;
