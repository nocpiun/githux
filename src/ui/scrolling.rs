//! Wheel-scroll smoothing shared by the shell's scroll regions.
//!
//! A discrete mouse-wheel notch is accumulated into a target offset and eased toward it in real
//! time; precise (trackpad) deltas apply directly so there is no double inertia. A capture-phase
//! wheel listener suppresses GPUI's built-in per-event jump for the region the pointer is over.
//! Programmatic jumps (keyboard reveal, route reset, scrollbar drag) change the handle offset and
//! the next tick adopts that position instead of fighting it. No timer or idle frame request is
//! left behind once motion settles, and Ely's reduced-motion preference is honoured.

use std::{cell::Cell, rc::Rc, time::Instant};

use ely_gpui_component::theme::ActiveTheme;
use gpui::{
    AnyElement, DispatchPhase, IntoElement, Point, ScrollHandle, ScrollWheelEvent, Styled, canvas,
    div, prelude::*, px,
};

/// How long one wheel notch takes to ease into place.
const WHEEL_EASE_MS: f32 = 130.0;
/// How far the handle may drift from the presented value before it is treated as an external
/// jump (keyboard reveal, route reset, scrollbar drag) that cancels the animation.
const ADOPT_EPSILON: f32 = 0.5;

/// The pure part of the smoothing: presented offset, accumulated target and easing clock. It is
/// separated from the handle so it can be exercised with a controlled clock in tests.
#[derive(Default)]
pub(super) struct SmoothState {
    presented: Cell<f32>,
    target: Cell<f32>,
    from: Cell<f32>,
    since: Cell<Option<Instant>>,
    animating: Cell<bool>,
}

impl SmoothState {
    /// Feed one wheel event. Precise and reduced-motion input applies at once and returns the
    /// offset to set; a discrete notch starts easing and returns `None` (the next tick applies).
    pub(super) fn wheel(
        &self,
        now: Instant,
        current: f32,
        max: f32,
        delta_y: f32,
        precise: bool,
        reduced: bool,
    ) -> Option<f32> {
        self.adopt(current);
        if precise || reduced {
            let next = (current + delta_y).clamp(-max, 0.0);
            self.presented.set(next);
            self.target.set(next);
            self.animating.set(false);
            return Some(next);
        }
        self.from.set(self.presented.get());
        self.target
            .set((self.target.get() + delta_y).clamp(-max, 0.0));
        self.since.set(Some(now));
        self.animating.set(true);
        None
    }

    /// Advance the easing clock. Returns the offset to apply when motion changed, and leaves
    /// `is_animating` true only while another frame is still needed.
    pub(super) fn tick(&self, now: Instant, current: f32, max: f32, reduced: bool) -> Option<f32> {
        self.adopt(current);
        if !self.animating.get() {
            return None;
        }
        if reduced {
            let next = self.target.get().clamp(-max, 0.0);
            self.presented.set(next);
            self.animating.set(false);
            return Some(next);
        }
        let since = self.since.get().unwrap_or(now);
        let elapsed = now.saturating_duration_since(since).as_secs_f32();
        let progress = (elapsed / (WHEEL_EASE_MS / 1000.0)).clamp(0.0, 1.0);
        let ease = ely_gpui_component::motion::ease_out_cubic(progress);
        let next =
            (self.from.get() + (self.target.get() - self.from.get()) * ease).clamp(-max, 0.0);
        self.presented.set(next);
        if progress >= 1.0 {
            self.animating.set(false);
        }
        Some(next)
    }

    pub(super) fn is_animating(&self) -> bool {
        self.animating.get()
    }

    /// Stop easing and adopt `current` as the presented value.
    pub(super) fn cancel(&self, current: f32) {
        self.presented.set(current);
        self.target.set(current);
        self.animating.set(false);
    }

    /// Adopt an external offset change so a reveal or reset wins over the easing.
    fn adopt(&self, current: f32) {
        if (current - self.presented.get()).abs() > ADOPT_EPSILON {
            self.cancel(current);
        }
    }
}

/// A scroll region with wheel smoothing attached to its handle.
#[derive(Clone)]
pub(super) struct SmoothScroll {
    scroll: ScrollHandle,
    state: Rc<SmoothState>,
}

impl SmoothScroll {
    pub(super) fn new(scroll: ScrollHandle) -> Self {
        Self {
            scroll,
            state: Rc::new(SmoothState::default()),
        }
    }

    /// Drop any pending motion; call before a programmatic jump so it is not pulled back.
    pub(super) fn cancel(&self) {
        self.state.cancel(f32::from(self.scroll.offset().y));
    }

    /// Apply one wheel event to the handle. Returns whether the region consumed it.
    pub(super) fn wheel(&self, now: Instant, delta_y: f32, precise: bool, reduced: bool) -> bool {
        let current = f32::from(self.scroll.offset().y);
        let max = f32::from(self.scroll.max_offset().y).max(0.0);
        if max <= 0.0 {
            return false;
        }
        if let Some(next) = self
            .state
            .wheel(now, current, max, delta_y, precise, reduced)
        {
            self.set_y(next);
        }
        true
    }

    /// Advance the handle toward the target. Returns true while another frame is needed.
    pub(super) fn tick(&self, now: Instant, reduced: bool) -> bool {
        let current = f32::from(self.scroll.offset().y);
        let max = f32::from(self.scroll.max_offset().y).max(0.0);
        let Some(next) = self.state.tick(now, current, max, reduced) else {
            return false;
        };
        self.set_y(next);
        self.state.is_animating()
    }

    fn set_y(&self, y: f32) {
        let x = self.scroll.offset().x;
        self.scroll.set_offset(Point::new(x, px(y)));
    }
}

/// Covers its parent with a transparent capture-phase wheel listener. It paints nothing and has
/// no hitbox, so it never blocks clicks; it only routes wheel deltas into `smooth`. Put it in a
/// `relative` parent as a sibling of the scrolling element.
pub(super) fn wheel_overlay(id: &'static str, smooth: SmoothScroll) -> AnyElement {
    let smooth = smooth.clone();
    div()
        .debug_selector(move || format!("wheel-overlay-{id}"))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _cx| {
                    let smooth = smooth.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture || !bounds.contains(&event.position) {
                            return;
                        }
                        let delta = event.delta.pixel_delta(window.line_height());
                        // A vertical-only region maps a horizontal (Shift+wheel) delta to
                        // vertical, the same fallback GPUI's built-in listener uses.
                        let delta_y = if delta.y != gpui::Pixels::ZERO {
                            f32::from(delta.y)
                        } else {
                            f32::from(delta.x)
                        };
                        let precise = event.delta.precise();
                        let reduced = cx.theme().reduced_motion || cx.reduce_motion();
                        let now = Instant::now();
                        if smooth.wheel(now, delta_y, precise, reduced) {
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
