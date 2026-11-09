//! Native Windows foundation executable, independent of the legacy GPUI binary.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    use nsh_platform_windows::{AppOptions, AppSource, run_app};
    nsh_platform_windows::set_ui_sets(vec![nsh_base::registry()]);
    run_app(
        AppSource::Rust(githux_nsh::root()),
        AppOptions {
            title: "Githux - Native Shell migration preview".into(),
            app_id: "dev.githux.native-shell".into(),
            size: (1280, 820),
            appearance: nsh_core::platform::Appearance::Dark,
            ..Default::default()
        },
    )
}

#[cfg(not(target_os = "windows"))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "The NSH migration window currently targets Windows; the legacy application is retained."
    );
    std::process::ExitCode::FAILURE
}
