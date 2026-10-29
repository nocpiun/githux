// Thin standalone assembly for the reusable Profile Overview and its existing
// ReaderView/scrollbar dependencies. No product Shell, app or routes are
// included; `examples/profile.rs` and `tests/profile.rs` share this real tree.
#![allow(dead_code, unused_imports)]

#[path = "../ui/avatar.rs"]
pub mod avatar;
#[path = "../ui/compact_status.rs"]
pub mod compact_status;
#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/profile/mod.rs"]
pub mod profile;
#[path = "../ui/reader/mod.rs"]
pub mod reader;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
