//! Axis-aware wheel/trackpad motion on a real [`ScrollTarget`].
//!
//! Discrete wheel deltas ease for 130ms. Precise trackpad pixels and touch (including GPUI's own
//! momentum) apply immediately. Each region owns its `Motion`; no Shell or region name is part of
//! the contract. Programmatic writes cancel the old goal so reveal, keyboard navigation and
//! programmatic drags cannot spring back.

use std::{
    cell::{RefCell, RefMut},
    rc::Rc,
    time::Instant,
};

use ely_core::theme::ActiveTheme;
use gpui::{
    AnyElement, Axis, DispatchPhase, HitboxBehavior, IntoElement, Point, ScrollEvent, ScrollSource,
    Styled, Window, canvas, div, prelude::*, px,
};

use super::{Bars, ScrollTarget};

const EASE_MS: f32 = 130.0;
const ADOPT_EPSILON: f32 = 0.5;

#[derive(Clone, Copy, Default)]
struct AxisMotion {
    presented: f32,
    target: f32,
    from: f32,
    since: Option<Instant>,
    animating: bool,
}

impl AxisMotion {
    fn wheel(
        &mut self,
        now: Instant,
        current: f32,
        max: f32,
        delta: f32,
        precise: bool,
        reduced: bool,
    ) -> Option<f32> {
        self.adopt(current);
        if precise || reduced {
            let next = (current + delta).clamp(-max, 0.0);
            self.presented = next;
            self.target = next;
            self.animating = false;
            return Some(next);
        }
        self.from = self.presented.clamp(-max, 0.0);
        self.target = (self.target.clamp(-max, 0.0) + delta).clamp(-max, 0.0);
        self.since = Some(now);
        self.animating = self.target != self.from;
        None
    }

    fn tick(&mut self, now: Instant, current: f32, max: f32, reduced: bool) -> Option<f32> {
        self.adopt(current);
        if !self.animating {
            return None;
        }
        // The stored goal shrinks with the range, so reversing cannot use an unreachable target.
        self.target = self.target.clamp(-max, 0.0);
        if reduced {
            self.presented = self.target;
            self.animating = false;
            return Some(self.presented);
        }
        self.from = self.from.clamp(-max, 0.0);
        let since = self.since.unwrap_or(now);
        let elapsed = now.saturating_duration_since(since).as_secs_f32();
        let progress = (elapsed / (EASE_MS / 1000.0)).clamp(0.0, 1.0);
        let ease = ely_core::motion::ease_out_cubic(progress);
        self.presented = (self.from + (self.target - self.from) * ease).clamp(-max, 0.0);
        if progress >= 1.0 {
            self.animating = false;
        }
        Some(self.presented)
    }

    fn cancel(&mut self, current: f32) {
        self.presented = current;
        self.target = current;
        self.animating = false;
    }

    fn adopt(&mut self, current: f32) {
        if (current - self.presented).abs() > ADOPT_EPSILON {
            self.cancel(current);
        }
    }
}

#[derive(Default)]
struct Shared {
    vertical: AxisMotion,
    horizontal: AxisMotion,
    thumb_drag: Option<Axis>,
}

/// Wheel motion and thumb coordination for one scroll target. The target retains its native list
/// behavior; this wrapper never measures or recreates it.
#[derive(Clone)]
pub struct Motion<T: ScrollTarget + Clone + 'static> {
    target: T,
    shared: Rc<RefCell<Shared>>,
}

impl<T: ScrollTarget + Clone + 'static> Motion<T> {
    pub fn new(target: T) -> Self {
        Self {
            target,
            shared: Rc::new(RefCell::new(Shared::default())),
        }
    }

    /// Feed one input after the region has selected its axis and mapped its device delta.
    pub fn wheel(
        &self,
        now: Instant,
        delta: f32,
        axis: Axis,
        precise: bool,
        reduced: bool,
    ) -> bool {
        if self
            .shared
            .borrow()
            .thumb_drag
            .is_some_and(|drag| drag == axis)
        {
            // The thumb owns this axis until release, even if another wheel arrives meanwhile.
            return true;
        }
        if !self.target.supports_axis(axis) {
            return false;
        }
        let max = Self::max(&self.target, axis);
        if delta == 0.0 || max <= 0.0 {
            return false;
        }
        let current = Self::offset(&self.target, axis);
        let next =
            Self::state(&self.shared, axis).wheel(now, current, max, delta, precise, reduced);
        if let Some(next) = next {
            self.direct(axis, next);
        }
        true
    }

    /// Advance presentation. A caller requests another frame only while this returns true.
    pub fn tick(&self, now: Instant, reduced: bool) -> bool {
        for axis in [Axis::Vertical, Axis::Horizontal] {
            if !self.target.supports_axis(axis) {
                continue;
            }
            let max = Self::max(&self.target, axis);
            let current = Self::offset(&self.target, axis);
            let next = Self::state(&self.shared, axis).tick(now, current, max, reduced);
            if let Some(next) = next {
                self.direct(axis, next);
            } else if current < -max || current > 0.0 {
                self.direct(axis, current.clamp(-max, 0.0));
            }
        }
        self.is_animating()
    }

    pub fn is_animating(&self) -> bool {
        [Axis::Vertical, Axis::Horizontal]
            .into_iter()
            .any(|axis| Self::state(&self.shared, axis).animating)
    }

    /// Hand control to a programmatic action immediately, discarding its pending wheel goal.
    pub fn cancel(&self) {
        for axis in [Axis::Vertical, Axis::Horizontal] {
            Self::state(&self.shared, axis).cancel(Self::offset(&self.target, axis));
        }
    }

    pub fn set_offset(&self, offset: Point<gpui::Pixels>) {
        self.cancel();
        for axis in [Axis::Vertical, Axis::Horizontal] {
            let value = if axis == Axis::Vertical {
                offset.y
            } else {
                offset.x
            };
            self.set_axis_offset(axis, value);
        }
    }

    pub fn set_axis_offset(&self, axis: Axis, offset: gpui::Pixels) {
        if !self.target.supports_axis(axis) {
            return;
        }
        self.cancel_axis(axis);
        // Programmatic reveal can precede the next layout's expanded range (a new tab). The
        // native target clamps against its fresh layout; clamping to the previous frame here
        // would leave that new item hidden. Wheel and Bar writes already constrain their range.
        self.target.set_axis_offset(axis, offset);
        self.cancel_axis(axis);
    }

    fn cancel_axis(&self, axis: Axis) {
        Self::state(&self.shared, axis).cancel(Self::offset(&self.target, axis));
    }

    fn offset(target: &T, axis: Axis) -> f32 {
        let offset = target.offset();
        f32::from(if axis == Axis::Vertical {
            offset.y
        } else {
            offset.x
        })
    }

    fn max(target: &T, axis: Axis) -> f32 {
        let max = target.max_offset();
        f32::from(if axis == Axis::Vertical { max.y } else { max.x }).max(0.0)
    }

    fn state(shared: &Rc<RefCell<Shared>>, axis: Axis) -> RefMut<'_, AxisMotion> {
        RefMut::map(shared.borrow_mut(), |shared| {
            if axis == Axis::Vertical {
                &mut shared.vertical
            } else {
                &mut shared.horizontal
            }
        })
    }

    fn direct(&self, axis: Axis, offset: f32) {
        self.target.set_axis_offset(axis, px(offset));
    }
}

/// Reads/writes the same native target while external writes and drag hooks cancel motion.
/// Native `ListState` drag hooks still reach the list through its target implementation.
impl<T: ScrollTarget + Clone + 'static> ScrollTarget for Motion<T> {
    fn supports_axis(&self, axis: Axis) -> bool {
        self.target.supports_axis(axis)
    }
    fn viewport_bounds(&self) -> gpui::Bounds<gpui::Pixels> {
        self.target.viewport_bounds()
    }
    fn offset(&self) -> Point<gpui::Pixels> {
        self.target.offset()
    }
    fn max_offset(&self) -> Point<gpui::Pixels> {
        self.target.max_offset()
    }
    fn set_axis_offset(&self, axis: Axis, offset: gpui::Pixels) {
        Motion::set_axis_offset(self, axis, offset);
    }
    fn drag_started(&self, axis: Axis) {
        if !self.target.supports_axis(axis) {
            return;
        }
        if self.shared.borrow().thumb_drag.is_some() {
            return;
        }
        self.shared.borrow_mut().thumb_drag = Some(axis);
        self.cancel_axis(axis);
        self.target.drag_started(axis);
    }
    fn drag_ended(&self, axis: Axis) {
        if self.shared.borrow().thumb_drag != Some(axis) {
            return;
        }
        self.shared.borrow_mut().thumb_drag = None;
        self.target.drag_ended(axis);
        self.cancel_axis(axis);
    }
    fn redraw(&self, window: &mut Window) {
        self.target.redraw(window)
    }
}

/// Capture-phase input with the same occlusion check as GPUI's normal scroll hitbox. It neither
/// blocks clicks nor bypasses a modal. The region places it in its own relative viewport.
pub fn wheel_overlay<T: ScrollTarget + Clone + 'static>(
    id: &'static str,
    axis: Axis,
    motion: Motion<T>,
) -> AnyElement {
    wheel_overlay_with(id, WheelAxes::Mapped(axis), motion, Instant::now)
}

/// Input ownership for a region. Existing one-axis consumers retain their mapping; readers
/// can preserve native x/y and Shift without converting a horizontal gesture into vertical motion.
#[derive(Clone, Copy)]
pub enum WheelAxes {
    Mapped(Axis),
    Both,
    Vertical,
}

/// The same input overlay with explicit axis ownership and the owner's monotonic clock.
/// The clock also lets event tests advance presentation without sleeping.
pub fn wheel_overlay_with<T: ScrollTarget + Clone + 'static>(
    id: &'static str,
    axes: WheelAxes,
    motion: Motion<T>,
    now: impl Fn() -> Instant + 'static,
) -> AnyElement {
    let now = Rc::new(now);
    div()
        .debug_selector(move || format!("wheel-overlay-{id}"))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(
            canvas(
                |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                move |_, hitbox, window, _| {
                    let motion = motion.clone();
                    let now = now.clone();
                    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                            return;
                        }
                        let delta = event.delta.pixel_delta(window.line_height());
                        let precise = event.delta.precise() || event.source == ScrollSource::Touch;
                        let reduced = cx.theme().reduced_motion || cx.reduce_motion();
                        let now = now();
                        let (x, y) = (f32::from(delta.x), f32::from(delta.y));
                        let consumed = match axes {
                            WheelAxes::Mapped(axis) => {
                                let (primary, secondary) = if axis == Axis::Vertical {
                                    (y, x)
                                } else {
                                    (x, y)
                                };
                                motion.wheel(
                                    now,
                                    if primary != 0.0 { primary } else { secondary },
                                    axis,
                                    precise,
                                    reduced,
                                )
                            }
                            WheelAxes::Both => {
                                // Windows already emits x for Shift+wheel; remap y only when
                                // there is no native x. A diagonal precise gesture keeps both axes.
                                let (x, y) = if event.modifiers.shift {
                                    (if x != 0.0 { x } else { y }, 0.0)
                                } else {
                                    (x, y)
                                };
                                let horizontal =
                                    motion.wheel(now, x, Axis::Horizontal, precise, reduced);
                                let vertical =
                                    motion.wheel(now, y, Axis::Vertical, precise, reduced);
                                horizontal || vertical
                            }
                            WheelAxes::Vertical => {
                                if event.modifiers.shift && y != 0.0 {
                                    // A vertical native list otherwise consumes Shift+y in its
                                    // bubble handler. Reserve that gesture for horizontal owners.
                                    true
                                } else {
                                    motion.wheel(now, y, Axis::Vertical, precise, reduced)
                                }
                            }
                        };
                        if consumed {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// Settle one region's motion and thumb drag when it closes or its window is deactivated.
pub fn cancel_region<T: ScrollTarget + Clone + 'static>(bars: &mut Bars, motion: &Motion<T>) {
    bars.cancel(motion);
    motion.cancel();
}

#[cfg(test)]
mod tests;
