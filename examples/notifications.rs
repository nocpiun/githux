//! Interactive offline preview of the real service/controller/view. Does not
//! open the product account directory or any platform credentials.
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]
#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/bin_support/notifications_fixture.rs"]
mod fixture;
#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/bin_support/notifications_modules.rs"]
mod ui;

use ely_core::{
    Assets,
    buttons::Button,
    theme::{ActiveTheme, Mode, TextSize, Theme},
};
use fixture::{Fixture, Scene};
use gpui::{
    App, AppContext, Bounds, Context, Entity, Render, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use network::executor::BackgroundExecutor;
use std::{rc::Rc, sync::Arc};
use ui::notifications::NotificationsView;

struct Preview {
    accounts: Arc<data::AccountService>,
    executor: Arc<BackgroundExecutor>,
    fixture: Arc<Fixture>,
    view: Entity<NotificationsView>,
    scene: Scene,
    action: String,
}
impl Preview {
    fn new(
        accounts: Arc<data::AccountService>,
        executor: Arc<BackgroundExecutor>,
        fixture: Arc<Fixture>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let callback = Self::callback(cx);
        let view = cx.new(|cx| {
            NotificationsView::new(
                accounts.repo_service().notifications().unwrap(),
                executor.clone(),
                callback,
                window,
                cx,
            )
        });
        Self {
            accounts,
            executor,
            fixture,
            view,
            scene: Scene::Mixed,
            action: "Offline fixture · repository actions appear here".into(),
        }
    }
    fn callback(cx: &Context<Self>) -> ui::notifications::OnRepository {
        let weak = cx.entity().downgrade();
        Rc::new(move |action, _, cx| {
            let _ = weak.update(cx, |preview, cx| {
                if preview.accounts.repo_service().is_current(&action.context) {
                    preview.action = format!(
                        "Repository id={} {}/{}",
                        action.repository.id(),
                        action.repository.locator().owner(),
                        action.repository.locator().name()
                    );
                    cx.notify();
                }
            });
        })
    }
    fn select(&mut self, scene: Scene, window: &mut Window, cx: &mut Context<Self>) {
        self.view
            .update(cx, |view, cx| view.cancel_interactions(cx));
        self.fixture.set_scene(scene);
        self.scene = scene;
        let callback = Self::callback(cx);
        self.view = cx.new(|cx| {
            NotificationsView::new(
                self.accounts.repo_service().notifications().unwrap(),
                self.executor.clone(),
                callback,
                window,
                cx,
            )
        });
        self.action = format!("Offline fixture · {}", scene.label());
        cx.notify();
    }
}
impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        div()
            .w(window.viewport_size().width)
            .h(window.viewport_size().height)
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.fg)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .children(Scene::ALL[..4].iter().map(|scene| {
                                let scene = *scene;
                                let weak = cx.entity().downgrade();
                                Button::new(format!("scene-{scene:?}"), scene.label()).on_click(
                                    move |_, window, cx| {
                                        let _ = weak
                                            .update(cx, |view, cx| view.select(scene, window, cx));
                                    },
                                )
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .children(Scene::ALL[4..].iter().map(|scene| {
                                let scene = *scene;
                                let weak = cx.entity().downgrade();
                                Button::new(format!("scene-{scene:?}"), scene.label()).on_click(
                                    move |_, window, cx| {
                                        let _ = weak
                                            .update(cx, |view, cx| view.select(scene, window, cx));
                                    },
                                )
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(self.view.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .p_2()
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .child(self.action.clone()),
            )
    }
}
fn main() {
    let executor = Arc::new(BackgroundExecutor::new(4));
    let fixture = Arc::new(Fixture::new(Scene::Mixed));
    let client = Arc::new(github::client::GitHubClient::new(
        github::client::GitHubConfig::default(),
        fixture.clone(),
    ));
    // Throwaway SQLite account metadata only; the synthetic PAT stays in the
    // existing in-memory store. This is the real AccountService login/read path.
    let directory = tempfile::tempdir().expect("temporary preview directory");
    let config =
        storage::accounts::AccountDatabaseConfig::at(directory.path().join("preview.sqlite3"));
    let accounts = executor.raw_handle().block_on(async {
        let accounts = data::AccountService::open(
            config,
            Arc::new(auth::memory::InMemoryCredentialStore::new()),
            client,
            executor.clone(),
        )
        .await
        .expect("preview account service");
        accounts
            .login(auth::token::Pat::from_str("synthetic-not-a-real-pat"))
            .await
            .expect("fixture login");
        Arc::new(accounts)
    });
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_core::init(cx).unwrap();
            fonts::register(cx).unwrap();
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = fonts::UI_FAMILY.into();
                theme.mono_family = fonts::CODE_FAMILY.into();
            });
            let bounds = Bounds::centered(None, size(px(960.0), px(740.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(360.0), px(360.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Githux · Notifications preview".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Preview::new(accounts, executor, fixture, window, cx)),
            )
            .unwrap();
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    drop(directory);
}
