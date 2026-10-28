// Root of the standalone repository-page module tree, pulled in by `#[path]`
// from `examples/repository.rs` and `tests/repository.rs`. Each `#[path]` below
// resolves against this file's directory, so Windows and Linux assemble the
// same real modules and `rustfmt` can resolve them.
//
// The page keeps using the real `crate::ui::...` paths, so no implementation is
// duplicated here. The scroll module declares its own children next to it, and
// the reader module declares its own submodules the same way.
#![allow(dead_code, unused_imports)]

#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/reader/mod.rs"]
pub mod reader;
#[path = "../ui/repository/mod.rs"]
pub mod repository;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
