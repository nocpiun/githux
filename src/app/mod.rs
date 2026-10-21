//! Application startup and composition. Business services are not implemented yet.

use ely_gpui_component::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use crate::{platform, ui::Hello};

pub fn run() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_gpui_component::init(cx).expect("failed to initialize Ely");
            platform::fonts::register(cx).expect("failed to register bundled fonts");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = platform::fonts::UI_FAMILY.into();
                theme.mono_family = platform::fonts::CODE_FAMILY.into();
            });

            let bounds = Bounds::centered(None, size(px(720.0), px(480.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(420.0), px(300.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Githux".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Hello::new(window, cx)),
            )
            .expect("failed to open Githux window");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
