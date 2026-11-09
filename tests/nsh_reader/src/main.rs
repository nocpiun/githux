//! Native Windows probe for the in-place Native Shell reader.
//!
//! It mounts the same preview application `examples/reader.rs` runs, through the
//! real `src/ui/reader` module tree, so the compatibility test's headless
//! evidence has a matching native window. It is a probe, not the product Shell.

#[path = "../../../src/content/mod.rs"]
mod content;
#[path = "../../../src/domain/mod.rs"]
mod domain;
#[path = "../../../src/ui/reader/mod.rs"]
mod reader;

#[cfg(windows)]
fn main() -> impl std::process::Termination {
    use nsh_platform_windows as platform;
    platform::set_ui_sets(vec![nsh_base::registry()]);
    let root =
        nsh_core::element::Root::of::<reader::fixtures::Preview>().tokens(reader::dark_tokens());
    platform::run_app(
        platform::AppSource::Rust(root),
        platform::AppOptions {
            title: "Githux NSH reader compatibility probe".into(),
            app_id: "dev.githux.reader-probe".into(),
            size: (960, 760),
            ..Default::default()
        },
    )
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This native probe targets Windows; run the compatibility tests headlessly.");
}
