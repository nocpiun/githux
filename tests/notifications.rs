//! Runs the reusable Notifications component tests without a Shell or a second
//! whole-app target. Services, adapter, view and fixture are the real modules.
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

#[path = "../src/bin_support/notifications_modules.rs"]
mod notifications_modules;
#[allow(unused_imports)]
use notifications_modules as ui;
