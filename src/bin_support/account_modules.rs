// Module tree for the standalone account preview and its integration tests.
// Every path is relative to this file, so Windows and Linux resolve the same
// real account modules without copying their implementations.
// The example and test entry use different subsets of the shared modules.
#![allow(dead_code, unused_imports)]

#[path = "../ui/accounts/mod.rs"]
pub mod accounts;
#[path = "../app/accounts/mod.rs"]
pub mod bridge;
#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
