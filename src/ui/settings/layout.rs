use std::time::Instant;

use ely_core::{
    primitives::{Icon, IconName},
    theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, Axis, Context, Entity, FocusHandle, FontWeight, IntoElement, ParentElement, Render,
    ScrollHandle, Styled, Subscription, Window, div, prelude::*, px,
};

use super::{SettingsBinding, accounts};
use crate::ui::geometry::{
    SIDEBAR_INDICATOR_HEIGHT_RATIO, SIDEBAR_INDICATOR_MAX_HEIGHT, SIDEBAR_INDICATOR_MIN_HEIGHT,
    SIDEBAR_INDICATOR_RADIUS, SIDEBAR_INDICATOR_WIDTH, SIDEBAR_PADDING,
};
use crate::ui::scroll::{self, Bar, BarId, Bars, Motion, Placement, Style};

/// The actual production category list. Fixture categories are opt-in on the developer host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    Accounts,
    Fixture(usize),
}

pub(crate) struct SettingsView {
    pub binding: Entity<SettingsBinding>,
    pub category: Category,
    pub fixtures: usize,
    width: f32,
    last_form: Option<super::binding::FormMode>,
    scroll: ScrollHandle,
    motion: Motion<ScrollHandle>,
    bars: Bars,
    pub tabs: ScrollHandle,
    tab_motion: Motion<ScrollHandle>,
    tab_bars: Bars,
    tab_focus: Vec<FocusHandle>,
    focus_observers: Vec<Subscription>,
    clock: Instant,
    _observed: Subscription,
    _activation: Subscription,
}

impl SettingsView {
    /// Host width is measured on the actual element; it does not infer sidebar state.
    pub fn new(
        binding: Entity<SettingsBinding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroll = ScrollHandle::new();
        let tabs = ScrollHandle::new();
        let observed = cx.observe(&binding, |_, _, cx| cx.notify());
        let activation = cx.observe_window_activation(window, |this, window, _| {
            if !window.is_window_active() {
                this.cancel_interactions();
            }
        });
        Self {
            binding,
            category: Category::Accounts,
            fixtures: 0,
            width: 0.0,
            last_form: None,
            motion: Motion::new(scroll.clone()),
            scroll,
            bars: Bars::new(),
            tab_motion: Motion::new(tabs.clone()),
            tabs,
            tab_bars: Bars::new(),
            tab_focus: vec![],
            focus_observers: vec![],
            clock: Instant::now(),
            _observed: observed,
            _activation: activation,
        }
    }

    pub fn cancel_interactions(&mut self) {
        scroll::cancel_region(&mut self.bars, &self.motion);
        scroll::cancel_region(&mut self.tab_bars, &self.tab_motion);
    }

    pub fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index > self.fixtures {
            return;
        }
        self.category = if index == 0 {
            Category::Accounts
        } else {
            Category::Fixture(index)
        };
        self.tab_motion.cancel();
        self.tabs.scroll_to_item(index);
        if let Some(focus) = self.tab_focus.get(index) {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn navigation(
        &mut self,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        while self.tab_focus.len() <= self.fixtures {
            let index = self.tab_focus.len();
            let focus = cx.focus_handle().tab_stop(true);
            self.focus_observers
                .push(cx.on_focus(&focus, window, move |this, _, cx| {
                    this.tab_motion.cancel();
                    this.tabs.scroll_to_item(index);
                    cx.notify();
                }));
            self.tab_focus.push(focus);
        }
        let theme = cx.theme();
        let row_height = f32::from(
            theme
                .control_height(ControlSize::Md)
                .to_pixels(window.rem_size()),
        );
        let indicator_height = (row_height * SIDEBAR_INDICATOR_HEIGHT_RATIO)
            .clamp(SIDEBAR_INDICATOR_MIN_HEIGHT, SIDEBAR_INDICATOR_MAX_HEIGHT);
        let indicator_inset = ((row_height - indicator_height) / 2.0).max(0.0);
        let (fg, hover, muted) = (theme.colors.fg, theme.colors.hover, theme.colors.fg_muted);
        let mut items = div()
            .id("settings-category-list")
            .flex()
            .when(!narrow, |row| row.flex_col())
            .gap_1()
            .min_w_0();
        for index in 0..=self.fixtures {
            let selected = match self.category {
                Category::Accounts => index == 0,
                Category::Fixture(i) => index == i,
            };
            let label = if index == 0 {
                "Accounts".to_string()
            } else {
                format!("Fixture section {index}")
            };
            let icon = if index == 0 {
                IconName::User
            } else {
                IconName::Layers
            };
            items = items.child(if narrow {
                div()
                    .id(("settings-category", index))
                    .debug_selector(move || format!("settings-category-{index}"))
                    .role(gpui::Role::Tab)
                    .aria_label(label.clone())
                    .aria_selected(selected)
                    .track_focus(&self.tab_focus[index])
                    .cursor_pointer()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(7.0))
                    .h(px(38.0))
                    .px_3()
                    .whitespace_nowrap()
                    .text_color(theme.colors.fg)
                    .text_size(px(14.0))
                    .when(selected, |item| item.font_weight(FontWeight::SEMIBOLD))
                    .border_b_2()
                    .border_color(if selected {
                        theme.colors.fg
                    } else {
                        gpui::transparent_black()
                    })
                    .focus_visible(|item| item.border_1().border_color(theme.colors.focus))
                    .child(Icon::new(icon).size(IconSize::Sm))
                    .child(label)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select(index, window, cx)),
                    )
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            let next = match event.keystroke.key.as_str() {
                                "enter" | "space" => Some(index),
                                "right" | "down" => Some((index + 1) % (this.fixtures + 1)),
                                "left" | "up" => {
                                    Some((index + this.fixtures) % (this.fixtures + 1))
                                }
                                "home" => Some(0),
                                "end" => Some(this.fixtures),
                                _ => None,
                            };
                            if let Some(next) = next {
                                cx.stop_propagation();
                                this.select(next, window, cx);
                            }
                        },
                    ))
                    .into_any_element()
            } else {
                div()
                    .id(("settings-category", index))
                    .debug_selector(move || format!("settings-category-{index}"))
                    .relative()
                    .flex_none()
                    .w_full()
                    .min_w_0()
                    .h(px(row_height))
                    .pl(px(SIDEBAR_PADDING))
                    .child(
                        div()
                            .id(("settings-category-row", index))
                            .role(gpui::Role::Tab)
                            .aria_label(label.clone())
                            .aria_selected(selected)
                            .track_focus(&self.tab_focus[index])
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .h(px(row_height))
                            .w_full()
                            .min_w_0()
                            .px(px(8.0))
                            .overflow_hidden()
                            .rounded(theme.radius(Radius::Md))
                            .border_1()
                            .border_color(gpui::transparent_black())
                            .text_size(theme.text_size(TextSize::Sm))
                            .text_color(if selected { fg } else { muted })
                            .when(selected, |row| {
                                row.bg(hover).font_weight(FontWeight::MEDIUM)
                            })
                            .hover(move |style| style.bg(hover).text_color(fg))
                            .focus(move |style| style.border_color(muted))
                            .cursor_pointer()
                            .child(Icon::new(icon).size(IconSize::Sm))
                            .child(div().flex_1().min_w_0().child(Ellipsis::new(label)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(index, window, cx)
                            }))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    let next = match event.keystroke.key.as_str() {
                                        "enter" | "space" => Some(index),
                                        "right" | "down" => Some((index + 1) % (this.fixtures + 1)),
                                        "left" | "up" => {
                                            Some((index + this.fixtures) % (this.fixtures + 1))
                                        }
                                        "home" => Some(0),
                                        "end" => Some(this.fixtures),
                                        _ => None,
                                    };
                                    if let Some(next) = next {
                                        cx.stop_propagation();
                                        this.select(next, window, cx);
                                    }
                                },
                            )),
                    )
                    .when(selected, |lane| {
                        lane.child(
                            div()
                                .debug_selector(|| "settings-category-indicator".to_string())
                                .absolute()
                                .left_0()
                                .top(px(indicator_inset))
                                .w(px(SIDEBAR_INDICATOR_WIDTH))
                                .h(px(indicator_height))
                                .rounded_tl(px(0.0))
                                .rounded_bl(px(0.0))
                                .rounded_tr(px(SIDEBAR_INDICATOR_RADIUS))
                                .rounded_br(px(SIDEBAR_INDICATOR_RADIUS))
                                .bg(muted),
                        )
                    })
                    .into_any_element()
            });
        }
        if !narrow {
            return items.w(px(168.0)).flex_none().into_any_element();
        }
        let now = self.clock.elapsed().as_millis() as u64;
        let opacity = self.tab_bars.opacity(now);
        if self.tab_motion.tick(Instant::now(), cx.reduce_motion()) || self.tab_bars.is_animating()
        {
            window.request_animation_frame();
        }
        self.tab_bars
            .settle(&self.tab_motion, now, cx.reduce_motion());
        let owner = cx.entity();
        let drag: scroll::OnDragging = std::rc::Rc::new(move |axis, active, _, cx| {
            owner.update(cx, |this, cx| {
                this.tab_bars.drag(
                    &this.tab_motion,
                    axis,
                    active,
                    this.clock.elapsed().as_millis() as u64,
                    cx.reduce_motion(),
                );
                cx.notify();
            })
        });
        let bar = Bar {
            target: &self.tab_motion,
            id: BarId {
                owner: cx.entity_id(),
                name: "settings-tab-bar",
                axis: Axis::Horizontal,
            },
            placement: Placement::overlay(0.0, 6.0),
            style: Style::DEFAULT,
            opacity,
            on_drag: drag,
        }
        .render(cx);
        div()
            .id("settings-tabs-region")
            .debug_selector(|| "settings-tabs-region".into())
            .relative()
            .w_full()
            .min_w_0()
            .h(px(44.0))
            .flex_none()
            .on_hover(cx.listener(|this, hovered, _, cx| {
                this.tab_bars.hover(
                    *hovered,
                    this.clock.elapsed().as_millis() as u64,
                    cx.reduce_motion(),
                );
                cx.notify();
            }))
            .child(
                items
                    .w_full()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.tabs),
            )
            .child(scroll::wheel_overlay(
                "settings-tabs-wheel",
                Axis::Horizontal,
                self.tab_motion.clone(),
            ))
            .child(bar)
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.binding.update(cx, |this, cx| this.settle(window, cx));
        let form = self.binding.read(cx).mode.clone();
        if self.last_form != form {
            self.last_form = form.clone();
            if form.is_some() {
                let scroll = self.scroll.clone();
                self.motion.cancel();
                window.defer(cx, move |_, _| scroll.scroll_to_bottom());
            }
        }
        let width = if self.width > 0.0 {
            self.width
        } else {
            f32::from(window.viewport_size().width)
        };
        let narrow = width < 760.0;
        let nav = self.navigation(narrow, window, cx);
        let content = match self.category {
            Category::Accounts => accounts::content(self.binding.clone(), cx),
            Category::Fixture(index) => div()
                .w_full()
                .min_w_0()
                .flex_1()
                .child(format!("Fixture section {index}"))
                .child(
                    div()
                        .mt_3()
                        .child("Layout fixture only. This is not a product setting."),
                )
                .into_any_element(),
        };
        let body = div()
            .w_full()
            .min_w_0()
            .flex()
            .gap(px(32.0))
            .when(narrow, |row| row.flex_col().gap_6())
            .child(nav)
            .child(div().flex_1().min_w_0().child(content));
        let now = self.clock.elapsed().as_millis() as u64;
        self.bars.settle(&self.motion, now, cx.reduce_motion());
        if self.motion.tick(Instant::now(), cx.reduce_motion()) || self.bars.is_animating() {
            window.request_animation_frame();
        }
        let opacity = self.bars.opacity(now);
        let owner = cx.entity();
        let drag: scroll::OnDragging = std::rc::Rc::new(move |axis, active, _, cx| {
            owner.update(cx, |this, cx| {
                this.bars.drag(
                    &this.motion,
                    axis,
                    active,
                    this.clock.elapsed().as_millis() as u64,
                    cx.reduce_motion(),
                );
                cx.notify();
            })
        });
        let bar = Bar {
            target: &self.motion,
            id: BarId {
                owner: cx.entity_id(),
                name: "settings-page-bar",
                axis: Axis::Vertical,
            },
            placement: Placement::overlay(4.0, 8.0),
            style: Style::DEFAULT,
            opacity,
            on_drag: drag,
        }
        .render(cx);
        let weak = cx.weak_entity();
        let measure = gpui::canvas(
            move |bounds, window, cx| {
                let width = f32::from(bounds.size.width);
                let weak = weak.clone();
                window.defer(cx, move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if (this.width - width).abs() > 0.5 {
                            this.width = width;
                            cx.notify();
                        }
                    });
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .id("settings-root")
            .debug_selector(|| "settings-root".into())
            .relative()
            .size_full()
            .min_w_0()
            .text_size(px(14.0))
            .text_color(cx.theme().colors.fg)
            .bg(cx.theme().colors.surface)
            .overflow_hidden()
            .on_hover(cx.listener(|this, hovered, _, cx| {
                this.bars.hover(
                    *hovered,
                    this.clock.elapsed().as_millis() as u64,
                    cx.reduce_motion(),
                );
                cx.notify();
            }))
            .child(measure)
            .child(
                div()
                    .id("settings-page-scroll")
                    .size_full()
                    .overflow_x_hidden()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .px(px(28.0))
                            .pt_6()
                            .pb_8()
                            .flex()
                            .flex_col()
                            .gap_6()
                            .child(
                                div()
                                    .text_size(px(24.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Settings"),
                            )
                            .child(body),
                    ),
            )
            // Common wheel input honours the nested Tabs hitbox, so only that strip scrolls
            // horizontally. Bar and wheel share the same Motion instance.
            .child(scroll::wheel_overlay_with(
                "settings-page",
                scroll::WheelAxes::Vertical,
                self.motion.clone(),
                Instant::now,
            ))
            .child(bar)
    }
}

impl Drop for SettingsView {
    fn drop(&mut self) {
        self.cancel_interactions();
    }
}
