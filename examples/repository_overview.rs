//! Standalone preview of the reusable repository TOP component.
//!
//! Run with `cargo run --example repository_overview`. It reuses the real component through
//! `#[path]` and never claims that the product shell has repository data wired in.

// Windows starts it without a console window, like the product binary; the test harness keeps its
// console so assertion output stays visible.
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/platform/fonts.rs"]
mod fonts;

#[path = "../src/bin_support/repository_overview_modules.rs"]
mod repository_overview_modules;
#[allow(unused_imports)]
use repository_overview_modules as ui;
#[allow(unused_imports)]
use repository_overview_modules::repository_overview;

use std::rc::Rc;

use ely_core::{
    Assets,
    buttons::{Button, ButtonVariant},
    theme::{ActiveTheme, Mode, TextSize, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    size,
};

// Tests exercise additional fixture variants beyond the interactive toolbar.
#[allow(dead_code)]
#[path = "../src/ui/repository_overview/fixtures.rs"]
mod fixtures;

use crate::repository_overview::{OverviewAction, RepositoryOverview, SectionState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Ready,
    LatestMissing,
    Partial,
    Error,
    EmptyRepository,
    ZeroLanguages,
    LongNames,
}

impl Scenario {
    const ALL: [Scenario; 7] = [
        Scenario::Ready,
        Scenario::LatestMissing,
        Scenario::Partial,
        Scenario::Error,
        Scenario::EmptyRepository,
        Scenario::ZeroLanguages,
        Scenario::LongNames,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::LatestMissing => "Latest 404",
            Self::Partial => "Partial",
            Self::Error => "Error",
            Self::EmptyRepository => "204 empty",
            Self::ZeroLanguages => "Zero languages",
            Self::LongNames => "Long names",
        }
    }
}

struct Preview {
    overview: Entity<RepositoryOverview>,
    scenario: Scenario,
    action: Option<String>,
    reader_scroll: ScrollHandle,
}

impl Preview {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let overview = cx.new(|cx| {
            let mut view = RepositoryOverview::new(fixtures::binding(1), window, cx);
            view.on_action(Rc::new(move |action, _window, cx| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let shown = describe(&action);
                this.update(cx, |preview, cx| {
                    preview.action = Some(shown);
                    cx.notify();
                });
            }));
            view
        });
        let mut preview = Self {
            overview,
            scenario: Scenario::Ready,
            action: None,
            reader_scroll: ScrollHandle::new(),
        };
        preview.apply(Scenario::Ready, window, cx);
        preview
    }

    fn apply(&mut self, scenario: Scenario, window: &mut Window, cx: &mut Context<Self>) {
        let binding = fixtures::binding(1);
        let overview = self.overview.clone();
        overview.update(cx, |view, cx| {
            view.set_binding(binding.clone(), window, cx);
            match scenario {
                Scenario::Ready => {
                    let _ = view.apply_releases(
                        &binding,
                        SectionState::Ready(fixtures::ready_releases()),
                        cx,
                    );
                    let _ = view.apply_contributors(
                        &binding,
                        SectionState::Ready(fixtures::ready_contributors()),
                        cx,
                    );
                    let _ = view.apply_languages(
                        &binding,
                        SectionState::Ready(fixtures::ready_languages()),
                        cx,
                    );
                }
                Scenario::LatestMissing => {
                    let _ = view.apply_releases(
                        &binding,
                        SectionState::Ready(fixtures::latest_none_releases()),
                        cx,
                    );
                    let _ = view.apply_contributors(
                        &binding,
                        SectionState::Ready(fixtures::ready_contributors()),
                        cx,
                    );
                    let _ = view.apply_languages(
                        &binding,
                        SectionState::Ready(fixtures::ready_languages()),
                        cx,
                    );
                }
                Scenario::Partial => {
                    let _ = view.apply_releases(&binding, fixtures::partial_releases_state(), cx);
                    let _ = view.apply_contributors(
                        &binding,
                        SectionState::Partial {
                            value: fixtures::partial_contributors(),
                            reason: vec![],
                        },
                        cx,
                    );
                    let _ = view.apply_languages(&binding, fixtures::partial_languages_state(), cx);
                }
                Scenario::Error => {
                    let _ = view.apply_releases(
                        &binding,
                        SectionState::Ready(fixtures::ready_releases()),
                        cx,
                    );
                    let _ = view.apply_contributors(
                        &binding,
                        SectionState::Error {
                            message: "403 forbidden".to_string(),
                        },
                        cx,
                    );
                    let _ = view.apply_languages(
                        &binding,
                        SectionState::Ready(fixtures::ready_languages()),
                        cx,
                    );
                }
                Scenario::EmptyRepository => {
                    let _ = view.apply_releases(
                        &binding,
                        SectionState::Ready(fixtures::ready_releases()),
                        cx,
                    );
                    let _ = view.apply_contributors(
                        &binding,
                        SectionState::Ready(fixtures::empty_repository_contributors()),
                        cx,
                    );
                    let _ = view.apply_languages(
                        &binding,
                        SectionState::Ready(fixtures::zero_languages()),
                        cx,
                    );
                }
                Scenario::ZeroLanguages => {
                    let _ = view.apply_releases(
                        &binding,
                        SectionState::Ready(fixtures::ready_releases()),
                        cx,
                    );
                    let _ = view.apply_contributors(
                        &binding,
                        SectionState::Ready(fixtures::ready_contributors()),
                        cx,
                    );
                    let _ = view.apply_languages(
                        &binding,
                        SectionState::Ready(fixtures::zero_languages()),
                        cx,
                    );
                }
                Scenario::LongNames => {
                    let _ = view.apply_releases(
                        &binding,
                        SectionState::Ready(fixtures::ready_releases()),
                        cx,
                    );
                    let _ = view.apply_contributors(
                        &binding,
                        SectionState::Ready(fixtures::large_contributors()),
                        cx,
                    );
                    let _ = view.apply_languages(
                        &binding,
                        SectionState::Ready({
                            let mut value = fixtures::many_languages();
                            value.languages[0].name = "An intentionally very long language name for compact-layout verification".into();
                            value
                        }),
                        cx,
                    );
                }
            }
        });
        self.overview.update(cx, |view, cx| {
            view.set_release_date_label(&binding, 4, "3 days ago".into(), cx);
        });
        self.scenario = scenario;
        self.reader_scroll.set_offset(gpui::Point::default());
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let buttons = Scenario::ALL.iter().enumerate().map(|(index, scenario)| {
            let this = cx.entity();
            let scenario = *scenario;
            let active = scenario == self.scenario;
            div()
                .debug_selector(move || format!("scenario-{}", scenario.label()))
                .child(
                    Button::new(("scenario", index), scenario.label())
                        .size(ely_core::theme::ControlSize::Sm)
                        .variant(match active {
                            true => ButtonVariant::Primary,
                            false => ButtonVariant::Secondary,
                        })
                        .on_click(move |_, window, cx| {
                            this.update(cx, |preview, cx| {
                                preview.apply(scenario, window, cx);
                            });
                        }),
                )
                .into_any_element()
        });
        div()
            .debug_selector(|| "overview-preview-toolbar".to_string())
            .flex()
            .flex_wrap()
            .w_full()
            .min_w_0()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.colors.border)
            .children(buttons)
    }
}

fn describe(action: &OverviewAction) -> String {
    match action {
        OverviewAction::Refresh { section, .. } => {
            format!("refresh → {}", section.title())
        }
        OverviewAction::OpenRelease {
            release_id,
            tag_name,
            html_url,
            ..
        } => format!(
            "open release {release_id} {tag_name} → {}",
            html_url.as_deref().unwrap_or("no URL")
        ),
        OverviewAction::OpenContributor {
            login, html_url, ..
        } => format!(
            "open contributor {} → {}",
            login.as_deref().unwrap_or("anonymous"),
            html_url.as_deref().unwrap_or("no URL")
        ),
    }
}

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg, border, subtle) = (
            theme.colors.bg,
            theme.colors.fg,
            theme.colors.border,
            theme.colors.fg_subtle,
        );
        let reader_text = theme.text_size(TextSize::Sm);
        let action_text = theme.text_size(TextSize::Xs);
        let preview_width = (_window.viewport_size().width - px(48.0)).max(px(1.0));
        let status = self.action.clone().unwrap_or_else(|| {
            "Actions are shown here; the preview never opens a URL or downloads an asset."
                .to_string()
        });
        let toolbar = self.toolbar(cx);
        div()
            .debug_selector(|| "overview-preview".to_string())
            .size_full()
            .w(_window.viewport_size().width)
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .child(toolbar)
            .child(div().px_6().pt_5().pb_3().child(div().text_size(px(22.0)).child("nocpiun / githux"))
                .child(div().text_color(subtle).child("A calm desktop workspace for GitHub.")))
            .child(div().w_full().px_6().child(div().w(preview_width).border_t_1().border_color(border).child(self.overview.clone())))
            .child(div().px_6().py_2().border_b_1().border_color(border).child("Code"))
            .child(
                div()
                    .id("overview-preview-reader")
                    .debug_selector(|| "overview-preview-reader".to_string())
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .border_t_1()
                    .border_color(border)
                    .overflow_y_scroll()
                    .track_scroll(&self.reader_scroll)
                    .child(
                        div()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(div().text_size(px(22.0)).child("Githux"))
                            .child(div().text_size(reader_text).child("Your GitHub workspace, made for desktop."))
                            .child(div().pt_4().child("A quieter way to work"))
                            .child(div().text_size(reader_text).child("Browse repository files, read project documentation, and stay close to the work that matters."))
                            .child(div().pt_4().child("Getting started"))
                            .child(div().font_family(fonts::CODE_FAMILY).text_size(reader_text).child("cargo run --locked")),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "overview-preview-action".to_string())
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .text_size(action_text)
                    .text_color(subtle)
                    .child(status),
            )
            .child(self.overview.read(cx).language_bar(cx))
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_core::init(cx).expect("Ely failed to start");
            fonts::register(cx).expect("the bundled fonts failed to register");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = fonts::UI_FAMILY.into();
                theme.mono_family = fonts::CODE_FAMILY.into();
            });
            let bounds = Bounds::centered(None, size(px(1100.0), px(760.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(480.0), px(480.0))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Githux · Compact overview preview".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| cx.new(|cx| Preview::new(window, cx)))
                .expect("the repository overview preview window failed to open");
            cx.activate(true);
        });
}
