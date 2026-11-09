#[cfg(windows)]
fn main() -> impl std::process::Termination {
    use nsh_platform_windows as platform;
    platform::set_ui_sets(vec![nsh_base::registry()]);
    let root = githux_nsh::foundation::fonts::install(platform::Root::of::<
        githux_nsh::preview::reader::Preview,
    >())
    .tokens(githux_nsh::foundation::theme::tokens());
    platform::run_app(
        platform::AppSource::Rust(root),
        platform::AppOptions {
            title: "Githux NSH Reader compatibility probe".into(),
            app_id: "dev.githux.reader-probe".into(),
            size: (960, 760),
            ..Default::default()
        },
    )
}
#[cfg(not(windows))]
fn main() {
    eprintln!("This native probe targets Windows; run compatibility tests headlessly.");
}
