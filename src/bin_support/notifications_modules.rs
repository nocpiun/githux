//! Minimal standalone UI assembly; no Shell or duplicate whole-app test target.
#![allow(dead_code, unused_imports)]
#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/notifications/mod.rs"]
pub mod notifications;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
