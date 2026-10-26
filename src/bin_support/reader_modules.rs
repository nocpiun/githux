// Root of the standalone reader module tree, pulled in by `#[path]` from
// `examples/reader.rs` and `tests/reader.rs`. Each `#[path]` below is resolved
// against this file's directory, so it is identical on every platform and
// `rustfmt` can resolve it.
//
// The reader continues to use the real `crate::ui::...` paths, so no logic is
// duplicated here.
#![allow(dead_code, unused_imports)]

#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/reader/mod.rs"]
pub mod reader;
#[path = "../ui/scrollbar.rs"]
pub mod scrollbar;
#[path = "../ui/tab_scrollbar.rs"]
pub mod tab_scrollbar;
