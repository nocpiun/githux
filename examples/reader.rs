//! Standalone preview of the reusable read-only reader on the Native Shell.
//!
//! Run with `cargo run --example reader` on Windows. It reuses the real reader
//! module through `#[path]` rather than copying any backend logic; the native
//! title bar keeps it a preview, not a second product shell.
//!
//! The preview is an NSH application mounting the same `ReaderHandle`/`reader()`
//! a page mounts. The fixture and its host-action buttons live in
//! `reader::fixtures::Preview`; `tests/nsh_reader` re-runs it as the
//! compatibility probe.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

use githux::ui::reader;

#[cfg(windows)]
fn main() -> impl std::process::Termination {
    use nsh_platform_windows as platform;
    platform::set_ui_sets(vec![nsh_base::registry()]);
    let root =
        nsh_core::element::Root::of::<reader::fixtures::Preview>().tokens(reader::dark_tokens());
    platform::run_app(
        platform::AppSource::Rust(root),
        platform::AppOptions {
            title: "Githux reader preview".into(),
            app_id: "dev.githux.reader-preview".into(),
            size: (960, 760),
            ..Default::default()
        },
    )
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This native preview targets Windows; the reader tests run headlessly.");
}
