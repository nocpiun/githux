//! Interactive offline preview of the Native Shell Notifications inbox.
//!
//! Uses the real runtime, the real inbox controller and the real view against
//! an offline fixture. Does not open the product account directory or any
//! platform credentials.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

pub use githux::{
    app, auth, content, data, diagnostics, domain, github, network, storage, ui, workspace,
};

#[path = "../src/bin_support/notifications_fixture.rs"]
mod fixture;
#[path = "../src/bin_support/nsh_pages_harness.rs"]
mod harness;

use std::sync::Arc;

use fixture::{Fixture, Scene};
use harness::{FixtureBackend, Kind};

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "windows")]
    {
        use nsh_platform_windows::{AppOptions, AppSource, run_app};
        let scene = if std::env::args().any(|arg| arg == "--empty") {
            Scene::Empty
        } else if std::env::args().any(|arg| arg == "--first-failure") {
            Scene::FirstFailure
        } else {
            Scene::Mixed
        };
        let fixture = Arc::new(Fixture::new(scene));
        let mut backend = FixtureBackend::build(
            fixture.clone() as Arc<dyn network::transport::Transport>,
            "ghp_synthetic-notifications-preview",
            Kind::Notifications,
        );
        backend.mount.now = Some(fixture::PREVIEW_NOW);
        let mount = harness::Mount {
            backend: std::sync::Mutex::new(backend.mount.backend.lock().unwrap().take()),
            kind: Kind::Notifications,
            now: Some(fixture::PREVIEW_NOW),
            start_inactive: false,
        };
        let root = harness::root();
        let mut config = nsh_engine::EngineConfig::new(root);
        config.context = nsh_core::element::EngineContext::default().with(mount);

        nsh_platform_windows::set_ui_sets(vec![nsh_base::registry()]);
        run_app(
            AppSource::Rust(harness::root()),
            AppOptions {
                title: "Githux - Notifications preview".into(),
                app_id: "dev.githux.notifications-preview".into(),
                size: (1080, 720),
                ..Default::default()
            },
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("Notifications preview targets Windows.");
        std::process::ExitCode::FAILURE
    }
}
