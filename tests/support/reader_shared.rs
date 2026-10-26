//! Shared source modules used by reader integration tests.
#![allow(dead_code, unused_imports)]

#[path = "../../src/ui/geometry.rs"]
pub mod geometry;
#[path = "../../src/ui/reader/mod.rs"]
#[allow(unused_imports)]
pub mod reader;
#[path = "../../src/ui/scrollbar.rs"]
pub mod scrollbar;
#[path = "../../src/ui/tab_scrollbar.rs"]
pub mod tab_scrollbar;
