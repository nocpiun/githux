// Module tree for the standalone Home/Feed preview and integration tests.
// Every path is relative to this file; the real view and shared scroll code
// are reused without Shell, routes, network or application services.
#![allow(dead_code, unused_imports)]

#[path = "../ui/avatar.rs"]
pub mod avatar;
#[path = "../ui/compact_status.rs"]
pub mod compact_status;
#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/home/mod.rs"]
pub mod home;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
#[path = "../ui/tooltip.rs"]
pub mod tooltip;
