//! Module root shared by `tests/reader.rs` and `examples/reader.rs`.
//!
//! The reader continues to use the real `crate::ui::reader`-style paths inside
//! its own tree; declaring it here keeps the test/example assembly identical to
//! the application module without copying any logic.

#![allow(dead_code, unused_imports)]

#[path = "../../src/ui/reader/mod.rs"]
pub mod reader;
