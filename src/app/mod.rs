//! Application startup and composition.
//!
//! Startup installs the account controller and opens the ordinary Githux Shell window. The
//! controller restores the saved account directory on its own background thread, so nothing here
//! blocks the UI thread on SQLite or the platform credential store, and the window shows an honest
//! logged-out or restoring state first.
//!
//! The developer-only account example is a separate Cargo target. This product path has no
//! preview flag, fixture scenario selector or preview window hook.

use ely_core::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

#[cfg(target_os = "windows")]
use ely_core::theme::ActiveTheme;

pub mod accounts;

use crate::{app::accounts::Accounts, platform, ui::Shell};

pub fn run() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_core::init(cx).expect("failed to initialize Ely");
            platform::fonts::register(cx).expect("failed to register bundled fonts");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = platform::fonts::UI_FAMILY.into();
                theme.mono_family = platform::fonts::CODE_FAMILY.into();
            });
            Shell::bind_keys(cx);

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
                    let accounts = cx.new(|cx| Accounts::new(None, window, cx));
                    cx.set_global(AccountsHandle(accounts));
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
    // Once the event loop returns, let any submitted SQLite/credential commit and the executor
    // finish, bounded by the controller's existing exit gate.
    crate::app::accounts::wait_for_shutdown();
}

/// The app-global account controller. The shell and any future page read it from here instead of
/// constructing a second service stack.
pub struct AccountsHandle(pub gpui::Entity<Accounts>);

impl gpui::Global for AccountsHandle {}

impl AccountsHandle {
    /// The installed controller, once a window has created it.
    pub fn get(cx: &App) -> Option<gpui::Entity<Accounts>> {
        cx.try_global::<Self>().map(|handle| handle.0.clone())
    }

    /// The narrow production access point for the next integration step: the active account's
    /// account service, repository service and executor. `None` while logged out or unavailable.
    #[allow(dead_code)]
    pub fn services(cx: &App) -> Option<crate::app::accounts::AccountServices> {
        let accounts = Self::get(cx)?;
        accounts.read(cx).services()
    }
}
