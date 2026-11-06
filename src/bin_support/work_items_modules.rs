// Standalone preview assembly. No second whole-app integration test target.
#![allow(dead_code, unused_imports)]
#[path = "work_items_fixture.rs"]
pub mod fixture;
#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/global_work_items/mod.rs"]
pub mod global_work_items;
#[path = "../ui/page_layout.rs"]
pub mod page_layout;
#[path = "../ui/relative_time.rs"]
pub mod relative_time;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
#[path = "../ui/tooltip.rs"]
pub mod tooltip;
#[path = "../ui/work_item_row.rs"]
pub mod work_item_row;
