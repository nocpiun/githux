//! Integration entry for the real account preview's module tests.
//!
//! Examples are built but their `#[cfg(test)]` tests are not executed by Cargo,
//! so `cargo test` runs the existing bridge and account tests through this
//! thin module assembly. The test bodies remain beside the code they exercise.

#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;

// The production-composition tests exercise the ordinary Shell as well, so they use the real
// product module tree. Only the external boundaries (database, credentials, transport) are
// injected by the test bodies.
#[path = "../src/app/mod.rs"]
#[allow(dead_code)]
mod app;

// The example-only preview window is a separate developer target. These tests exercise the
// product composition (`app::run`'s entity graph), so the example module tree is not included
// here; the example keeps its own scroll-region coverage in its own target.

#[path = "../src/platform/mod.rs"]
mod platform;
#[path = "../src/ui/mod.rs"]
mod ui;
#[path = "../src/workspace/mod.rs"]
mod workspace;

// The production Shell mounts `ui::repository`, whose own suite shares the development fixture.
// Product binaries never declare this module.
#[path = "../src/bin_support/test_repository_modules.rs"]
mod repository_modules;
