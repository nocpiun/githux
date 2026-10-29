//! Runs the README picture tests in an integration target.
//!
//! `cargo test` builds example targets but does not run their tests, so the
//! picture fixture, the fixed-commit provider and the reader's picture state are
//! compiled here as well. This file only wires the module tree; the tests live
//! beside the code (`src/content/images/tests.rs`, `src/network/reader_images_tests.rs`,
//! `src/data/reader_images_tests.rs`, `src/bin_support/reader_images_view_tests.rs`).

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

#[path = "../src/bin_support/reader_images_modules.rs"]
mod reader_images_modules;
// The reader's own modules resolve `crate::ui::…`, and the tests and the
// fixture resolve `crate::reader::…`/`crate::fixture::…`; these aliases make the
// assembly match `tests/reader.rs` and `examples/reader_images.rs`.
#[allow(unused_imports)]
use reader_images_modules as ui;
#[allow(unused_imports)]
use reader_images_modules as fixture_root;
#[allow(unused_imports)]
use reader_images_modules::{fixture, reader};

#[path = "../src/bin_support/reader_images_view_tests.rs"]
mod view_tests;
