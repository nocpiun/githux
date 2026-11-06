//! Minimal standalone UI assembly; no Shell or duplicate whole-app test target.
#![allow(dead_code, unused_imports)]
#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/notifications/mod.rs"]
pub mod notifications;
#[path = "../ui/page_layout.rs"]
pub mod page_layout;
#[path = "../ui/relative_time.rs"]
pub mod relative_time;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
#[path = "../ui/tooltip.rs"]
pub mod tooltip;
