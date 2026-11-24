//! Offline preview of the Files changed pane with the real account-bound reader.
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]
#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/bin_support/pr_files_data.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[cfg(not(test))]
#[path = "../src/bin_support/pr_files_fixture.rs"]
mod fixture;
#[cfg(test)]
use ui::pr_files::tests::fixture;
#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/bin_support/pr_files_github.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/bin_support/pr_files_ui.rs"]
mod ui;

use ely_core::{
    Assets,
    buttons::Button,
    theme::{ControlSize, Mode, Theme},
};
use fixture::{EMPTY, FIRST, Fixture};
use gpui::{
    App, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    size,
};
use ui::pr_files::FilesView;

struct Preview {
    fixture: Fixture,
    files: Entity<FilesView>,
}

impl Render for Preview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .p_2()
                    .child(
                        Button::new("preview-conversation", "Conversation")
                            .size(ControlSize::Sm)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.files
                                    .update(cx, |files, cx| files.set_active(false, cx))
                            })),
                    )
                    .children(
                        [
                            (FIRST, "Files changed · PR #1"),
                            (EMPTY, "Files changed · PR #2"),
                        ]
                        .map(|(number, title)| {
                            Button::new(("preview-pr", number as usize), title)
                                .size(ControlSize::Sm)
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    let session = view.fixture.session(number);
                                    view.files.update(cx, |files, cx| {
                                        files.rebind(session, cx);
                                        files.set_active(true, cx);
                                    });
                                }))
                        }),
                    )
                    .child(
                        Button::new("preview-refresh", "Refresh")
                            .size(ControlSize::Sm)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.files.update(cx, |files, cx| files.refresh(cx));
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.files.clone()))
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fixture = runtime.block_on(Fixture::new());
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
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1000.), px(720.)),
                    cx,
                ))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Githux · Files changed preview".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            cx.open_window(options, move |_, cx| {
                let files =
                    cx.new(|_| FilesView::new(fixture.session(FIRST), fixture.executor.clone()));
                cx.new(|_| Preview { fixture, files })
            })
            .unwrap();
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
