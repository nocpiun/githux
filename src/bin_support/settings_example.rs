//! A standalone native host with an expanded-sidebar width reservation and fixture controls.
use crate::{
    app::accounts::Accounts,
    settings::{FormMode, SettingsBinding, SettingsView},
    settings_fixture::{Fixture, Scenario},
};
use ely_core::{
    buttons::Button,
    theme::{ActiveTheme, ControlSize},
};
use gpui::{
    App, Context, Entity, IntoElement, ParentElement, Render, Styled, Task, Window, div,
    prelude::*, px, size,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

pub struct SettingsPreview {
    pub view: Option<Entity<SettingsView>>,
    pub fixture: Option<Fixture>,
    inbox: Arc<Mutex<VecDeque<(u64, Fixture)>>>,
    generation: u64,
    scenario: Scenario,
    fixtures: bool,
    /// Opens a contextual form after the fixture mounts, so a native capture can inspect it
    /// without relying on synthetic window input. Production hosts leave this `None`.
    pub form: Option<FormMode>,
    initial_category: usize,
    _receiver: Task<()>,
}
impl SettingsPreview {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        initial: Option<FormMode>,
        initial_category: usize,
    ) -> Self {
        let mut this = Self {
            view: None,
            fixture: None,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            generation: 0,
            scenario: Scenario::Normal,
            fixtures: true,
            form: initial,
            initial_category,
            _receiver: cx.spawn_in(window, async |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                    if this
                        .update_in(cx, |this, window, cx| {
                            let replies = this
                                .inbox
                                .lock()
                                .expect("fixture inbox")
                                .drain(..)
                                .collect::<Vec<_>>();
                            for (generation, fixture) in replies {
                                if generation != this.generation {
                                    continue;
                                }
                                let controller = cx.new(|cx| {
                                    Accounts::new(Some(fixture.runtime.clone()), window, cx)
                                });
                                let binding =
                                    cx.new(|cx| SettingsBinding::new(controller, window, cx));
                                let view = cx.new(|cx| SettingsView::new(binding, window, cx));
                                view.update(cx, |view, _| {
                                    view.fixtures = if this.fixtures { 5 } else { 0 };
                                    view.category = if this.initial_category == 0 {
                                        crate::settings::Category::Accounts
                                    } else {
                                        crate::settings::Category::Fixture(this.initial_category)
                                    };
                                });
                                this.view = Some(view);
                                this.fixture = Some(fixture);
                                cx.notify();
                            }
                            // The fixture restores an account and starts an automatic read. A form
                            // cannot open while that is in flight, so retry once it is idle.
                            if let Some((view, mode)) = this.view.clone().zip(this.form.clone()) {
                                let busy =
                                    view.read(cx).binding.read(cx).controller.read(cx).busy();
                                if !busy {
                                    view.update(cx, |view, cx| {
                                        view.binding.update(cx, |binding, cx| {
                                            binding.open(mode, window, cx)
                                        });
                                    });
                                    this.form = None;
                                    cx.notify();
                                }
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }),
        };
        this.load(Scenario::Normal, cx);
        this
    }
    fn load(&mut self, scenario: Scenario, cx: &mut Context<Self>) {
        self.view = None;
        self.fixture = None;
        self.generation += 1;
        let generation = self.generation;
        self.scenario = scenario;
        let inbox = self.inbox.clone();
        std::thread::spawn(move || {
            let fixture = Fixture::open(scenario);
            inbox
                .lock()
                .expect("fixture inbox")
                .push_back((generation, fixture));
        });
        cx.notify();
    }
}
impl Render for SettingsPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut controls = div().flex().flex_wrap().gap_1().p_2();
        for scenario in Scenario::ALL {
            controls = controls.child(
                Button::new(format!("scenario-{scenario:?}"), scenario.label())
                    .size(ControlSize::Sm)
                    .on_click(cx.listener(move |this, _, _, cx| this.load(scenario, cx))),
            );
        }
        controls = controls.child(
            Button::new(
                "settings-fixtures",
                if self.fixtures {
                    "Fixture categories: on"
                } else {
                    "Accounts only"
                },
            )
            .size(ControlSize::Sm)
            .on_click(cx.listener(|this, _, _, cx| {
                this.fixtures = !this.fixtures;
                if let Some(view) = &this.view {
                    view.update(cx, |view, cx| {
                        view.fixtures = if this.fixtures { 5 } else { 0 };
                        view.category = crate::settings::Category::Accounts;
                        cx.notify();
                    });
                }
                cx.notify();
            })),
        );
        div().size_full().flex().flex_col().bg(cx.theme().colors.bg).text_color(cx.theme().colors.fg)
            .child(controls)
            .child(div().px_2().pb_2().text_size(px(12.0)).child("Offline fixture preview · Synthetic inputs: preview-alex / preview-sam / preview-jordan · Any other input returns 401"))
            .child(div().flex().flex_1().min_h_0().gap_2().p_2()
                .child(div().w(px(200.0)).flex_none().p_3().child("Expanded sidebar space").child(div().mt_4().child("Preview only")))
                .child(div().flex_1().min_w_0().h_full().overflow_hidden().rounded(px(10.0))
                    .when_some(self.view.clone(), |root, view| root.child(view))
                    .when(self.view.is_none(), |root| root.child("Preparing isolated fixture…"))))
    }
}

pub fn open_window(cx: &mut App) {
    open_window_with(cx, None, 0);
}

/// Also used by the native probe host to land directly in a contextual form or a fixture category.
pub fn open_window_with(cx: &mut App, initial: Option<FormMode>, initial_category: usize) {
    let bounds = gpui::Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(880.0), px(560.0))),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Githux · Settings Accounts preview".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| SettingsPreview::new(window, cx, initial.clone(), initial_category))
        },
    )
    .expect("settings preview window");
}
