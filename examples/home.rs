//! Standalone, synthetic Home/Feed preview. This is not a product route or a
//! real-data integration; the host adapter and formal Shell mount come later.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/bin_support/home_modules.rs"]
mod home_modules;
#[allow(unused_imports)]
use home_modules as ui;

use std::cell::RefCell;
use std::rc::Rc;

use ely_core::{
    Assets,
    buttons::Button,
    theme::{ActiveTheme, Mode, TextSize, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::domain::error::GitHubError;
use crate::domain::home::{
    FeedActor, FeedEvent, FeedEventType, FeedOutcome, FeedPartialReason, FeedPayload,
    FeedReadiness, FeedRepository, HomeError, HomeItem, HomeItemKind, HomePartialReason,
    HomeReadiness, HomeRepositoryRef, HomeSearchOutcome,
};
use crate::ui::home::{Column, HomeView, RefreshAvailability, RequestBinding};

#[derive(Clone, Copy)]
struct Scenario {
    label: &'static str,
    build: fn() -> (HomeSearchOutcome, FeedOutcome),
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        label: "Mixed",
        build: mixed,
    },
    Scenario {
        label: "Loading / error",
        build: loading_error,
    },
    Scenario {
        label: "Partial",
        build: partial,
    },
    Scenario {
        label: "Empty / unavailable",
        build: empty_unavailable,
    },
    Scenario {
        label: "Large",
        build: large,
    },
];

struct Preview {
    home: Entity<HomeView>,
    scenario: usize,
    binding: RequestBinding,
    generation: u64,
    action: Rc<RefCell<Option<String>>>,
    cooldown: bool,
    _changed: gpui::Subscription,
}

impl Preview {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let binding = RequestBinding::new("preview-account/home");
        let home = cx.new(|cx| HomeView::new(binding.clone(), 1, window, cx));
        let action = Rc::new(RefCell::new(None));
        let sink = action.clone();
        home.update(cx, |home, cx| {
            // Fixture clock: 2026-10-28 12:00 UTC, independent of wall time.
            home.set_now(
                Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_793_188_800)),
                cx,
            );
            home.on_action(Rc::new(move |action, _, _| {
                *sink.borrow_mut() = Some(format!("{action:?}"));
            }));
        });
        let mut preview = Self {
            home: home.clone(),
            scenario: 0,
            binding,
            generation: 1,
            action,
            cooldown: false,
            _changed: cx.observe(&home, |_, _, cx| cx.notify()),
        };
        preview.apply(window, cx);
        preview
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.cooldown = false;
        self.home.update(cx, |home, cx| {
            home.bind(self.binding.clone(), self.generation, window, cx);
        });
        match SCENARIOS[self.scenario].label {
            "Loading / error" => {
                self.home.update(cx, |home, cx| {
                    home.set_loading(Column::Home, &self.binding, self.generation, cx);
                    home.apply_error(
                        Column::Feed,
                        &self.binding,
                        self.generation,
                        HomeError::GitHub(GitHubError::Network {
                            message: "synthetic offline fixture".to_string(),
                        }),
                        cx,
                    );
                });
            }
            "Empty / unavailable" => {
                let (_, feed) = empty_unavailable();
                self.home.update(cx, |home, cx| {
                    home.set_unavailable(
                        Column::Home,
                        &self.binding,
                        self.generation,
                        "Offline and no cached Home snapshot is available; this is not an empty result.",
                        cx,
                    );
                    home.apply_feed(&self.binding, self.generation, feed, cx);
                });
            }
            _ => {
                let (search, feed) = (SCENARIOS[self.scenario].build)();
                self.home.update(cx, |home, cx| {
                    home.apply_home(&self.binding, self.generation, search, cx);
                    home.apply_feed(&self.binding, self.generation, feed, cx);
                });
            }
        }
    }

    fn switch(&mut self, scenario: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.scenario = scenario;
        *self.action.borrow_mut() = None;
        self.apply(window, cx);
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .debug_selector(|| "home-preview-toolbar".to_string())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.colors.border)
            .children(SCENARIOS.iter().enumerate().map(|(index, scenario)| {
                let this = cx.entity();
                Button::new(("home-scenario", index), scenario.label)
                    .variant(if index == self.scenario {
                        ely_core::buttons::ButtonVariant::Primary
                    } else {
                        ely_core::buttons::ButtonVariant::Secondary
                    })
                    .on_click(move |_, window, cx| {
                        this.update(cx, |preview, cx| preview.switch(index, window, cx));
                    })
            }))
            .child(
                Button::new("home-preview-complete", "Finish refresh").on_click({
                    let this = cx.entity();
                    move |_, _, cx| {
                        this.update(cx, |preview, cx| {
                            let (search, feed) = (SCENARIOS[preview.scenario].build)();
                            preview.home.update(cx, |home, cx| {
                                home.apply_home(&preview.binding, preview.generation, search, cx);
                                home.apply_feed(&preview.binding, preview.generation, feed, cx);
                            });
                        });
                    }
                }),
            )
            .child(
                Button::new("home-preview-cooldown", "Toggle Home cooldown").on_click({
                    let home = self.home.clone();
                    let binding = self.binding.clone();
                    let generation = self.generation;
                    {
                        let this = cx.entity();
                        move |_, _, cx| {
                            this.update(cx, |preview, cx| {
                                preview.cooldown = !preview.cooldown;
                                let unavailable = preview.cooldown;
                                home.update(cx, |home, cx| {
                                    home.set_refresh_availability(
                                        Column::Home,
                                        &binding,
                                        generation,
                                        if unavailable {
                                            RefreshAvailability::unavailable("Preview cooldown")
                                        } else {
                                            RefreshAvailability::available()
                                        },
                                        cx,
                                    );
                                });
                                cx.notify();
                            });
                        }
                    }
                }),
            )
    }
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (bg, fg, border, subtle, tiny) = {
            let theme = cx.theme();
            (
                theme.colors.bg,
                theme.colors.fg,
                theme.colors.border,
                theme.colors.fg_subtle,
                theme.text_size(TextSize::Xs),
            )
        };
        let action = self.action.borrow().clone().unwrap_or_else(|| {
            "Synthetic fixture · actions are shown here; nothing is opened.".to_string()
        });
        div()
            .w(window.viewport_size().width)
            .h(window.viewport_size().height)
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .child(self.toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .w_full()
                    .child(self.home.clone()),
            )
            .child(
                div()
                    .debug_selector(|| "home-preview-action".to_string())
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .text_size(tiny)
                    .text_color(subtle)
                    .child(action),
            )
    }
}

fn fixture_time(index: usize) -> Option<String> {
    match index % 8 {
        0 => Some("2026-10-28T11:50:00Z".into()),
        1 => Some("2026-10-28T11:00:00Z".into()),
        2 => Some("2026-10-27T12:00:00Z".into()),
        3 => None,
        4 => Some("2026-10-26T12:00:00Z".into()),
        5 => Some("2026-10-28T12:01:00Z".into()),
        6 => Some("invalid timestamp".into()),
        _ => Some("2026-10-28T11:59:20Z".into()),
    }
}

fn item(index: usize) -> HomeItem {
    HomeItem {
        id: 9_000 + index as u64,
        kind: if index.is_multiple_of(2) {
            HomeItemKind::Issue
        } else {
            HomeItemKind::PullRequest
        },
        number: 100 + index as u64,
        title: if index == 0 {
            "A deliberately long synthetic Issue title used to verify bounded text truncation in a narrow Home panel without horizontal overflow".to_string()
        } else {
            format!("Synthetic work item {index}")
        },
        html_url: String::new(),
        state: if index.is_multiple_of(2) {
            "open"
        } else {
            "closed"
        }
        .to_string(),
        updated_at: fixture_time(index).unwrap_or_default(),
        repository: HomeRepositoryRef {
            owner: "synthetic-owner".to_string(),
            name: format!("repository-{}", index % 3),
        },
    }
}

fn event(index: usize) -> FeedEvent {
    let kind = index % 12;
    let (event_type, payload) = match kind {
        0 => (
            FeedEventType::Push,
            FeedPayload::Push {
                git_ref: Some("refs/heads/main".into()),
                before: None,
                head: None,
                size: Some(3),
            },
        ),
        1 => (
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                action: "opened".into(),
                number: Some(42),
                title: Some("Synthetic pull request".into()),
                html_url: None,
                state: Some("open".into()),
                merged: None,
            },
        ),
        2 => (
            FeedEventType::Issues,
            FeedPayload::Issues {
                action: "closed".into(),
                number: Some(7),
                title: Some("Synthetic issue".into()),
                html_url: None,
                state: Some("closed".into()),
            },
        ),
        3 => (
            FeedEventType::IssueComment,
            FeedPayload::IssueComment {
                action: "created".into(),
                issue_number: Some(7),
                comment_url: None,
            },
        ),
        4 => (
            FeedEventType::Watch,
            FeedPayload::Watch {
                action: "started".into(),
            },
        ),
        5 => (
            FeedEventType::Create,
            FeedPayload::Create {
                ref_type: Some("branch".into()),
                git_ref: Some("preview".into()),
            },
        ),
        6 => (
            FeedEventType::Delete,
            FeedPayload::Delete {
                ref_type: Some("branch".into()),
                git_ref: Some("preview".into()),
            },
        ),
        7 => (
            FeedEventType::Fork,
            FeedPayload::Fork {
                forkee_full_name: Some("synthetic-user/fork".into()),
            },
        ),
        8 => (
            FeedEventType::Release,
            FeedPayload::Release {
                action: "published".into(),
                tag_name: Some("v1.0.0".into()),
                name: None,
                html_url: None,
            },
        ),
        9 => (FeedEventType::Public, FeedPayload::Public),
        10 => (
            FeedEventType::Member,
            FeedPayload::Member {
                action: "added".into(),
                member_login: Some("synthetic-member".into()),
            },
        ),
        _ => (
            FeedEventType::Unknown("FutureSyntheticEvent".into()),
            FeedPayload::Unknown,
        ),
    };
    FeedEvent {
        id: format!("synthetic-event-{index}"),
        event_type,
        actor: FeedActor {
            id: 50 + index as u64,
            login: "synthetic-user".into(),
            display_login: None,
            avatar_url: None,
        },
        repository: FeedRepository {
            id: 70 + index as u64,
            name: "synthetic-owner/repository".into(),
            url: String::new(),
        },
        created_at: fixture_time(index),
        public: true,
        payload,
    }
}

fn search(items: Vec<HomeItem>, readiness: HomeReadiness) -> HomeSearchOutcome {
    let total_count = items.len() as u64;
    HomeSearchOutcome {
        items,
        total_count,
        incomplete_results: !matches!(readiness, HomeReadiness::Complete),
        readiness,
    }
}

fn feed(events: Vec<FeedEvent>, readiness: FeedReadiness) -> FeedOutcome {
    FeedOutcome {
        events,
        etag: None,
        poll_interval_seconds: Some(60),
        readiness,
    }
}

fn mixed() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search((0..6).map(item).collect(), HomeReadiness::Complete),
        feed((0..12).map(event).collect(), FeedReadiness::Complete),
    )
}

fn loading_error() -> (HomeSearchOutcome, FeedOutcome) {
    mixed()
}

fn partial() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search(
            (0..4).map(item).collect(),
            HomeReadiness::Partial {
                reasons: vec![
                    HomePartialReason::ResultLimit,
                    HomePartialReason::IncompleteResults,
                ],
            },
        ),
        feed(
            (0..5).map(event).collect(),
            FeedReadiness::Partial {
                reasons: vec![FeedPartialReason::PageLimit],
            },
        ),
    )
}

fn empty_unavailable() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search(Vec::new(), HomeReadiness::Complete),
        feed(Vec::new(), FeedReadiness::Complete),
    )
}

fn large() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search(
            (0..1000).map(item).collect(),
            HomeReadiness::Partial {
                reasons: vec![HomePartialReason::ResultLimit],
            },
        ),
        feed((0..300).map(event).collect(), FeedReadiness::Complete),
    )
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
            let bounds = Bounds::centered(None, size(px(1180.0), px(800.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(620.0), px(520.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Githux · Home view preview".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Preview::new(window, cx)),
            )
            .expect("the Home preview window failed to open");
            cx.activate(true);
        });
}
