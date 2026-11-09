//! Interactive offline preview of Native Shell global Issues / Pull Requests.
//!
//! Uses the real runtime, the real search controller and the real view against
//! an offline fixture. Does not open the product account directory or any
//! platform credentials.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

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

#[path = "../src/nsh/foundation/mod.rs"]
pub mod foundation;
#[path = "../src/ui/global_work_items/mod.rs"]
pub mod global_work_items;
#[path = "../src/ui/notifications/mod.rs"]
pub mod notifications;
#[path = "../src/bin_support/nsh_pages_pages.rs"]
pub mod pages;
#[path = "../src/ui/relative_time.rs"]
pub mod relative_time;
#[path = "../src/nsh/runtime/mod.rs"]
pub mod runtime;

#[path = "../src/bin_support/work_items_fixture.rs"]
mod fixture;
#[path = "../src/bin_support/nsh_pages_harness.rs"]
mod harness;

pub mod app {
    pub use crate::pages;
    pub use crate::runtime;
}
pub mod ui {
    pub use crate::foundation;
    pub use crate::global_work_items;
    pub use crate::notifications;
    pub use crate::relative_time;
}
pub mod workspace {
    pub use crate::runtime::sessions::{
        PageIdentity, RepositoryNavigation, Route, RouteKey, Session, Sessions, TabId, Workspace,
    };
}

use std::sync::Arc;

use fixture::{FixtureTransport, Scene};
use harness::{FixtureBackend, Kind};

const NOW: u64 = 1_793_620_800;

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "windows")]
    {
        use nsh_platform_windows::{AppOptions, AppSource, run_app};
        let kind = if std::env::args().any(|arg| arg == "--pr") {
            Kind::PullRequests
        } else {
            Kind::Issues
        };
        let scene = if std::env::args().any(|arg| arg == "--empty") {
            Scene::Empty
        } else if std::env::args().any(|arg| arg == "--initial-failure") {
            Scene::InitialFailure
        } else {
            Scene::Mixed
        };
        let transport = Arc::new(FixtureTransport::new(scene));
        let mut backend = FixtureBackend::build(
            transport.clone() as Arc<dyn network::transport::Transport>,
            "ghp_synthetic-work-items-preview",
            kind,
        );
        backend.mount.now = Some(NOW);
        let mount = harness::Mount {
            backend: std::sync::Mutex::new(backend.mount.backend.lock().unwrap().take()),
            kind,
            now: Some(NOW),
            start_inactive: false,
        };
        let root = harness::root();
        let mut config = nsh_engine::EngineConfig::new(root);
        config.context = nsh_core::element::EngineContext::default().with(mount);

        nsh_platform_windows::set_ui_sets(vec![nsh_base::registry()]);
        run_app(
            AppSource::Rust(harness::root()),
            AppOptions {
                title: "Githux - Global work items preview".into(),
                app_id: "dev.githux.work-items-preview".into(),
                size: (1080, 720),
                ..Default::default()
            },
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("Global work items preview targets Windows.");
        std::process::ExitCode::FAILURE
    }
}
