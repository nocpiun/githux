//! One selection background painted behind the sidebar's navigation and repository rows.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{
    AnyElement, App, BorderStyle, Bounds, ClickEvent, ContentMask, DispatchPhase, Div, Edges, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseExitEvent, MouseMoveEvent, Pixels,
    ScrollHandle, Stateful, StatefulInteractiveElement, Styled, Window, canvas, point, px, size,
    transparent_black,
};

use super::geometry::SIDEBAR_PADDING;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Item {
    Home,
    Settings,
    Repo(u64),
}

impl Item {
    fn is_repo(self) -> bool {
        matches!(self, Self::Repo(_))
    }
}

type Rect = Bounds<Pixels>;
type Measurement = (Item, Rect);

#[derive(Clone, Copy, PartialEq)]
struct Viewport {
    bounds: Rect,
    offset: Pixels,
}

#[derive(Clone, Copy)]
struct Press {
    item: Item,
    selected: Option<Item>,
    canceled: bool,
}

struct Frame {
    target: Measurement,
    viewport: Viewport,
    scale: f32,
    top: Spring,
    height: Spring,
    now: u64,
    crosses_heading: bool,
}

#[derive(Clone, Default)]
pub(super) struct SidebarActive {
    previous: Rc<RefCell<Option<Frame>>>,
    press: Rc<Cell<Option<Press>>>,
}

impl SidebarActive {
    pub fn reset(&self) {
        self.previous.borrow_mut().take();
        self.cancel_press();
    }

    pub fn cancel_press(&self) {
        if let Some(press) = self.press.get() {
            self.cancel(press.item);
        }
    }

    fn cancel(&self, item: Item) -> bool {
        let Some(mut press) = self
            .press
            .get()
            .filter(|press| press.item == item && !press.canceled)
        else {
            return false;
        };
        press.canceled = true;
        self.press.set(Some(press));
        true
    }

    pub fn measure(&self, selected: Option<Item>) -> Measurements {
        if self
            .press
            .get()
            .is_some_and(|press| press.selected != selected)
        {
            self.cancel_press();
        }
        Measurements {
            selected,
            preview: self
                .press
                .get()
                .filter(|press| !press.canceled)
                .map(|press| press.item),
            previous: self.previous.borrow().as_ref().map(|frame| frame.target.0),
            rows: Rc::default(),
        }
    }

    fn bind_row(
        &self,
        row: Stateful<Div>,
        item: Item,
        selected: Option<Item>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let down = self.press.clone();
        let up = self.press.clone();
        let click = self.press.clone();
        row.on_mouse_down(MouseButton::Left, move |_, window, _| {
            window.prevent_default();
            down.set(Some(Press {
                item,
                selected,
                canceled: false,
            }));
            window.refresh();
        })
        .on_mouse_up_out(MouseButton::Left, move |_, window, _| {
            if up.get().is_some_and(|press| press.item == item) {
                up.set(None);
                window.refresh();
            }
        })
        .on_click(move |event, window, cx| {
            let press = click.take();
            if matches!(event, ClickEvent::Mouse(_))
                && press.is_some_and(|press| press.item == item && press.canceled)
            {
                window.refresh();
                return;
            }
            window.refresh();
            run(window, cx);
        })
    }

    pub fn layer(&self, measurements: Measurements, style: LayerStyle) -> AnyElement {
        let previous = self.previous.clone();
        let motion = self.clone();
        canvas(
            |_, _, _| {},
            move |layer_bounds, _, window, _| {
                let rows = measurements.rows.borrow();
                let mut previous = previous.borrow_mut();
                let prior = previous.take();
                let viewport = Viewport {
                    bounds: style.scroll.bounds(),
                    offset: style.scroll.offset().y,
                };
                let mask = window.content_mask().bounds.intersect(&layer_bounds);
                let repo_clip = mask.intersect(&viewport.bounds);
                let clip_for = |item: Item| if item.is_repo() { repo_clip } else { mask };
                let visible =
                    |(item, bounds): &Measurement| has_area(clip_for(*item).intersect(bounds));
                let preview = rows.preview.filter(|(item, bounds)| {
                    let hit_bounds = clip_for(*item).intersect(bounds);
                    has_area(hit_bounds) && hit_bounds.contains(&window.mouse_position())
                });
                if let Some((item, bounds)) = preview {
                    let hit_bounds = clip_for(item).intersect(&bounds);
                    let leave = motion.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                        if phase == DispatchPhase::Capture
                            && (event.pressed_button != Some(MouseButton::Left)
                                || !hit_bounds.contains(&event.position))
                            && leave.cancel(item)
                        {
                            window.refresh();
                        }
                    });
                    let leave = motion.clone();
                    window.on_mouse_event(move |_: &MouseExitEvent, phase, window, _| {
                        if phase == DispatchPhase::Capture && leave.cancel(item) {
                            window.refresh();
                        }
                    });
                } else if let Some(item) = measurements.preview {
                    // A scroll or layout change can move the pressed row away from a stationary
                    // pointer. Retire the preview too, even if no mouse-move event arrives.
                    motion.cancel(item);
                }
                let Some(target @ (item, bounds)) =
                    preview.or_else(|| rows.selected.filter(visible))
                else {
                    return;
                };

                let scale = window.scale_factor();
                let mut frame = prior
                    .filter(|prior| {
                        !style.reduced
                            && prior.scale == scale
                            && rows.previous == Some(prior.target)
                            && (prior.viewport == viewport
                                || !(item.is_repo()
                                    || prior.target.0.is_repo()
                                    || prior.crosses_heading))
                    })
                    .unwrap_or_else(|| Frame {
                        target,
                        viewport,
                        scale,
                        top: Spring::at(bounds.top()),
                        height: Spring::at(bounds.size.height),
                        now: style.now,
                        crosses_heading: false,
                    });
                if frame.target != target {
                    // Redirect from the last painted sample, including its velocity. The new
                    // target takes effect on this frame; elapsed time belonged to the old target.
                    frame.crosses_heading |= frame.target.0.is_repo() != item.is_repo();
                } else {
                    let elapsed = style.now.saturating_sub(frame.now) as f32 / 1000.0;
                    frame.top.step(bounds.top(), elapsed);
                    frame.height.step(bounds.size.height, elapsed);
                }
                let moving =
                    !frame.top.settled(bounds.top()) || !frame.height.settled(bounds.size.height);
                let clip = if moving && frame.crosses_heading {
                    // Fixed navigation can glide across the heading, while the footer is outside
                    // the travel area. Repository-to-repository motion uses the full native clip.
                    mask.intersect(&Bounds::from_corners(
                        mask.origin,
                        point(mask.right(), viewport.bounds.bottom()),
                    ))
                } else {
                    clip_for(item)
                };
                let painted = Bounds::new(
                    point(bounds.left(), px(frame.top.position)),
                    size(bounds.size.width, px(frame.height.position)),
                );
                if has_area(clip.intersect(&painted)) {
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        window.paint_quad(gpui::quad(
                            painted,
                            style.radius,
                            style.color,
                            Edges::default(),
                            transparent_black(),
                            BorderStyle::Solid,
                        ));
                    });
                }
                if moving {
                    window.request_animation_frame();
                }
                frame.target = target;
                frame.viewport = viewport;
                frame.now = style.now;
                frame.crosses_heading &= moving;
                *previous = Some(frame);
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }
}

pub(super) struct LayerStyle {
    pub scroll: ScrollHandle,
    pub radius: Pixels,
    pub reduced: bool,
    pub now: u64,
    pub color: Hsla,
}

#[derive(Default)]
struct Rows {
    selected: Option<Measurement>,
    preview: Option<Measurement>,
    previous: Option<Measurement>,
}

pub(super) struct Measurements {
    selected: Option<Item>,
    preview: Option<Item>,
    previous: Option<Item>,
    rows: Rc<RefCell<Rows>>,
}

pub(super) struct RowSelection<'a> {
    pub motion: &'a SidebarActive,
    pub measurements: &'a Measurements,
    pub item: Item,
}

impl RowSelection<'_> {
    pub fn active(&self) -> bool {
        self.measurements.selected == Some(self.item)
    }

    pub fn with_hover(&self, row: Stateful<Div>, fill: Hsla, foreground: Hsla) -> Stateful<Div> {
        if self.active() {
            return row;
        }
        let preview = self.measurements.preview == Some(self.item);
        row.hover(move |style| {
            let style = style.text_color(foreground);
            if preview { style } else { style.bg(fill) }
        })
    }

    pub fn probe(&self) -> Option<AnyElement> {
        self.measurements.probe(self.item)
    }

    pub fn bind(
        &self,
        row: Stateful<Div>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        self.motion
            .bind_row(row, self.item, self.measurements.selected, run)
    }
}

impl Measurements {
    /// Probes are siblings of the rows and match their outer background box.
    fn probe(&self, item: Item) -> Option<AnyElement> {
        let selected = self.selected == Some(item);
        let preview = self.preview == Some(item);
        let previous = self.previous == Some(item);
        if !selected && !preview && !previous {
            return None;
        }
        let rows = self.rows.clone();
        Some(
            canvas(
                move |bounds, _, _| {
                    let mut rows = rows.borrow_mut();
                    let measurement = Some((item, bounds));
                    if selected {
                        rows.selected = measurement;
                    }
                    if preview {
                        rows.preview = measurement;
                    }
                    if previous {
                        rows.previous = measurement;
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
            .left(px(SIDEBAR_PADDING))
            .into_any_element(),
        )
    }
}

fn has_area(bounds: Rect) -> bool {
    bounds.size.width > Pixels::ZERO && bounds.size.height > Pixels::ZERO
}

struct Spring {
    position: f32,
    velocity: f32,
}

impl Spring {
    fn at(target: Pixels) -> Self {
        Self {
            position: target.into(),
            velocity: 0.0,
        }
    }

    fn settled(&self, target: Pixels) -> bool {
        self.position == f32::from(target) && self.velocity == 0.0
    }

    fn step(&mut self, target: Pixels, elapsed: f32) {
        if self.settled(target) || elapsed == 0.0 {
            return;
        }
        // Natural period and damping ratio. Evaluate the damped oscillator analytically so
        // cadence changes do not change the spring, including when a frame arrives late.
        let frequency = std::f32::consts::TAU / 0.42;
        let damping: f32 = 0.84;
        let decay = damping * frequency;
        let oscillation = frequency * (1.0 - damping * damping).sqrt();
        let offset = self.position - f32::from(target);
        let amplitude = (self.velocity + decay * offset) / oscillation;
        let (sin, cos) = (oscillation * elapsed).sin_cos();
        let fade = (-decay * elapsed).exp();
        let next = fade * (offset * cos + amplitude * sin);
        let speed = fade * oscillation * (amplitude * cos - offset * sin) - decay * next;
        // The first arrival is final. Especially on long moves, a full oscillation would
        // produce visible bounce or pass the last row. Wait for low speed on near-arrivals.
        if (offset != 0.0 && offset.signum() != next.signum())
            || (next.abs() <= 0.2 && speed.abs() <= 4.0)
        {
            *self = Self::at(target);
        } else {
            self.position = f32::from(target) + next;
            self.velocity = speed;
        }
    }
}
