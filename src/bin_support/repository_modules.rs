// Root of the standalone repository-page module tree, pulled in by `#[path]`
// from `examples/repository.rs`. Each `#[path]` below
// resolves against this file's directory, so Windows and Linux assemble the
// same real modules and `rustfmt` can resolve them.
//
// The page keeps using the real `crate::ui::...` paths, so no implementation is
// duplicated here. The scroll module declares its own children next to it, and
// the reader module declares its own submodules the same way.
//
// `fixture` is the one development fixture the preview and the tests both browse;
// it is compiled only through this development-only assembly.
#![allow(dead_code, unused_imports)]

#[path = "../ui/badge.rs"]
pub mod badge;
#[path = "../ui/compact_status.rs"]
pub mod compact_status;
#[path = "repository_fixture.rs"]
pub mod fixture;
#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/interaction.rs"]
pub mod interaction;
#[path = "../ui/island.rs"]
pub mod island;
#[path = "../ui/page_layout.rs"]
pub mod page_layout;
#[path = "../ui/reader/mod.rs"]
pub mod reader;
#[path = "../ui/relative_time.rs"]
pub mod relative_time;
#[path = "../ui/repository/mod.rs"]
pub mod repository;
#[path = "../ui/repository_overview/mod.rs"]
pub mod repository_overview;
#[path = "../ui/repository_pulls.rs"]
pub mod repository_pulls;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
#[path = "../ui/tab_underline.rs"]
pub mod tab_underline;
#[path = "../ui/tooltip.rs"]
pub mod tooltip;
#[path = "../ui/work_item_row.rs"]
pub mod work_item_row;
