#[cfg(windows)]
fn main() -> impl std::process::Termination {
    use nsh_platform_windows as platform;
    platform::set_ui_sets(vec![nsh_base::registry()]);
    let ui = include_bytes!("../../../assets/fonts/githux-ui/GithuxUI-Regular.ttf");
    let mono = include_bytes!("../../../assets/fonts/google-sans-code/GoogleSansCode.ttf");
    let root = platform::Root::of::<githux_nsh_reader_probe::fixture::Preview>()
        .font(ui.to_vec())
        .font(mono.to_vec())
        .tokens(githux_nsh_reader_probe::reader::dark_tokens());
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
