//! Minimal production-test assembly for `ui::repository`'s fixture-dependent tests.
//!
//! Product binaries never declare this module. The repository page was previously compiled only
//! through the standalone preview/test assembly, which intentionally exposes small inspection
//! helpers for its own suite. Mounting the same page in the product crate brings those helpers into
//! this test-only build too; the allowance below stays confined to this test assembly.
#![allow(dead_code)]

#[path = "repository_fixture.rs"]
pub mod fixture;
