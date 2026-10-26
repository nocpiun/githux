//! Application startup and composition. Business services are not implemented yet.

use ely_gpui_component::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

#[cfg(target_os = "windows")]
use ely_gpui_component::theme::ActiveTheme;

use crate::{platform, ui::Shell};

pub mod accounts;

pub fn run() {
    let account_preview = std::env::args_os().any(|arg| arg == "--account-preview");
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_gpui_component::init(cx).expect("failed to initialize Ely");
            platform::fonts::register(cx).expect("failed to register bundled fonts");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = platform::fonts::UI_FAMILY.into();
                theme.mono_family = platform::fonts::CODE_FAMILY.into();
            });
            Shell::bind_keys(cx);

            if account_preview {
                accounts::open_window(cx);
                cx.on_window_closed(|cx, _| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
                .detach();
                cx.activate(true);
                return;
            }

            let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(880.0), px(560.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Githux".into()),
                        // Windows draws the custom bar in the client area; the other platforms keep
                        // their system frame, matching the fact that only Windows is accepted here.
                        appears_transparent: cfg!(target_os = "windows"),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    #[cfg(target_os = "windows")]
                    if let Err(error) = platform::caption::install(
                        window,
                        cx.theme().colors.bg,
                        cx.theme().colors.fg,
                    ) {
                        eprintln!("Native caption unavailable; using custom controls: {error}");
                    }
                    cx.new(|cx| Shell::new(window, cx))
                },
            )
            .expect("failed to open Githux window");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    #[cfg(target_os = "windows")]
                    platform::caption::shutdown();
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
