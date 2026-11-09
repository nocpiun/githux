//! Application startup and composition for the native Shell.
//!
//! `run` opens the single application runtime (which restores the saved account directory on its
//! own background thread), installs the bundled fonts and the default NSH theme, and runs the
//! native window. Nothing here blocks the UI thread on SQLite or the platform credential store.

pub mod avatar;
pub mod pages;
pub mod runtime;

/// Runs the native Githux application. The event loop returns before durable background work is
/// awaited, so shutdown can settle the account/cache commits without blocking a frame.
#[cfg(target_os = "windows")]
pub fn run() -> std::process::ExitCode {
    use nsh_core::platform::Appearance;
    use nsh_engine::Root;
    use nsh_platform_windows::{AppOptions, AppSource, run_app};

    let gate = runtime::ExitGate::default();
    let runtime = runtime::RuntimeHandle::start(gate.clone(), None);
    crate::ui::install_runtime(runtime);

    let root = crate::platform::fonts::install(Root::of::<crate::ui::Shell>())
        .tokens(crate::ui::foundation::theme::tokens())
        .app_id("dev.githux.githux")
        .appearance(Appearance::System);
    let result = run_app(
        AppSource::Rust(root),
        AppOptions {
            title: "Githux".into(),
            app_id: "dev.githux.githux".into(),
            size: (1280, 820),
            appearance: Appearance::System,
            ..Default::default()
        },
    );
    if !gate.wait(std::time::Duration::from_secs(15)) {
        eprintln!("Background account cleanup did not finish before shutdown timeout.");
        return std::process::ExitCode::FAILURE;
    }
    result
}

#[cfg(not(target_os = "windows"))]
pub fn run() -> std::process::ExitCode {
    eprintln!("Githux Native Shell currently targets Windows.");
    std::process::ExitCode::FAILURE
}
