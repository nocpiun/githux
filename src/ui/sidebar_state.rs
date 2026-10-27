//! Window-local sidebar geometry and interruptible, frame-driven presentation.
use super::geometry::*;
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub(super) struct SidebarFrame {
    pub width: f32,
    /// Zero at the icon rail, one at the expanded layout.
    pub expansion: f32,
}

struct Transition {
    from: SidebarFrame,
    to: SidebarFrame,
    since: Instant,
}

pub(super) struct SidebarState {
    pub collapsed: bool,
    pub expanded_width: f32,
    pub dragging: bool,
    pub menu_open: Rc<Cell<bool>>,
    transition: Option<Transition>,
    pub frame: SidebarFrame,
    /// Whether the pointer is really inside the resize heat band. Tracked separately from the
    /// sash target so a drag never masquerades as hover: the line may pin on during a drag, but
    /// the next pointer movement must still be able to turn it off.
    sash_pointer_inside: bool,
    /// Sash opacity target and presentation, faded in real time while the pointer is on the
    /// resize band or a drag is running.
    sash_target: f32,
    sash_opacity: f32,
    sash_since: Instant,
    /// Where inside the sash hit band the gesture grabbed, in sidebar-container coordinates.
    /// Expanded drags bind absolutely to `pointer - grab_offset`; only a few pixels of real grab
    /// difference remain, never the jump caused by restoring a remembered width.
    drag_grab_offset: f32,
    /// Current pointer in the same container coordinates, and the total travel from press.
    drag_pointer_x: f32,
    drag_travel: f32,
    /// Press pointer captured at `begin_resize`; travel is measured from here for the entire
    /// gesture, so mode switches never rebase it.
    press_pointer_at_begin: f32,
    /// Whether this gesture started on the rail (rather than merely being mid-collapse).
    drag_from_rail: bool,
    /// Footer presentation latched for the current gesture, so transient widths cannot flip it.
    footer_latched_compact: bool,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            collapsed: false,
            expanded_width: SIDEBAR_WIDTH,
            dragging: false,
            menu_open: Default::default(),
            transition: None,
            frame: SidebarFrame {
                width: SIDEBAR_WIDTH,
                expansion: 1.0,
            },
            sash_pointer_inside: false,
            sash_target: 0.0,
            sash_opacity: 0.0,
            sash_since: Instant::now(),
            drag_grab_offset: SIDEBAR_RESIZE_HIT_WIDTH / 2.0,
            drag_pointer_x: 0.0,
            drag_travel: 0.0,
            press_pointer_at_begin: 0.0,
            drag_from_rail: false,
            footer_latched_compact: true,
        }
    }
}

impl SidebarState {
    pub fn limit(window_width: f32) -> f32 {
        (window_width - SIDEBAR_CONTENT_MIN_WIDTH - ISLAND_GAP - ISLAND_INSET)
            .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
    }
    pub fn width(&self, window_width: f32) -> f32 {
        if self.collapsed {
            SIDEBAR_COLLAPSED_WIDTH
        } else {
            self.expanded_width
                .clamp(SIDEBAR_MIN_WIDTH, Self::limit(window_width))
        }
    }
    fn target(&self, window_width: f32) -> SidebarFrame {
        SidebarFrame {
            width: self.width(window_width),
            expansion: if self.collapsed { 0.0 } else { 1.0 },
        }
    }
    /// Returns whether another animation frame is needed. No timer or retained callback.
    pub fn sample(&mut self, now: Instant, window_width: f32, reduced: bool) -> bool {
        let target = self.target(window_width);
        // A drag keeps its own transition while the pointer retargets it; only an external change
        // (window resize, toggle, settle) drops it. `retarget_transition` handles the drag case.
        if reduced
            || (!self.dragging
                && self
                    .transition
                    .as_ref()
                    .is_some_and(|t| t.to.width != target.width))
        {
            self.transition = None;
        }
        let Some(t) = &self.transition else {
            self.frame = target;
            return false;
        };
        let progress = now.saturating_duration_since(t.since).as_secs_f32()
            / Duration::from_millis(SIDEBAR_ANIMATION_MS).as_secs_f32();
        if progress >= 1.0 {
            self.frame = target;
            self.transition = None;
            return false;
        }
        let ease = ely_core::motion::ease_out_cubic(progress);
        self.frame = SidebarFrame {
            width: t.from.width + (t.to.width - t.from.width) * ease,
            expansion: t.from.expansion + (t.to.expansion - t.from.expansion) * ease,
        };
        true
    }
    pub fn toggle(&mut self, now: Instant, window_width: f32, reduced: bool) {
        self.sample(now, window_width, reduced);
        self.collapsed = !self.collapsed;
        self.dragging = false;
        self.menu_open.set(false);
        self.transition = Some(Transition {
            from: self.frame,
            to: self.target(window_width),
            since: now,
        });
        if reduced {
            self.sample(now, window_width, true);
        }
    }
    /// Records the pointer's real position relative to the resize band. The sash shows only while
    /// hovered (or while a drag is running), never because of selection or focus.
    pub fn set_sash_hover(&mut self, hovered: bool, now: Instant, reduced: bool) {
        self.sash_pointer_inside = hovered;
        self.retarget_sash(now, reduced);
    }

    /// Recomputes the sash target. Outside a drag only real pointer hover shows the line; a valid
    /// resize drag keeps it visible for the whole gesture, even when the pointer outruns the band
    /// that is moving under it. Reversing mid-fade continues from the presented opacity.
    fn retarget_sash(&mut self, now: Instant, reduced: bool) {
        let target = if self.dragging || self.sash_pointer_inside {
            1.0
        } else {
            0.0
        };
        self.sash_since = now;
        self.sash_target = target;
        if reduced {
            self.sash_opacity = target;
        }
    }

    /// Whether the bottom entries must render compact (the More menu). This is derived from the
    /// presented frame, not from the `collapsed` flag alone: the critical/docking band and any
    /// collapse/expand transition stay compact, and the four entries appear only once the sidebar
    /// is clearly wider than the minimum. It never forces `collapsed` itself, so the rest of the
    /// sidebar keeps animating by its real width.
    pub fn footer_compact(&self) -> bool {
        if self.dragging {
            // During a gesture the width follows the pointer, so transient widths must not flip
            // the footer. A latched decision, taken once per crossing, keeps one presentation.
            return self.footer_latched_compact;
        }
        if self.collapsed || self.transition.is_some() {
            return true;
        }
        self.frame.width < SIDEBAR_MIN_WIDTH + SIDEBAR_FOOTER_COMPACT_MARGIN
    }

    /// Updates the per-gesture footer presentation latch from the pointer-derived width. Outside a
    /// drag the value is irrelevant; inside it the footer changes only on a clear crossing.
    fn latch_footer(&mut self, requested_width: f32) {
        let roomy = requested_width >= SIDEBAR_MIN_WIDTH + SIDEBAR_FOOTER_COMPACT_MARGIN;
        if roomy {
            self.footer_latched_compact = false;
        } else if requested_width <= SIDEBAR_MIN_WIDTH {
            self.footer_latched_compact = true;
        }
    }

    pub fn sash(&self) -> f32 {
        self.sash_opacity
    }

    /// Whether the pointer is genuinely inside the heat band (independent of a drag pinning the
    /// line on). The hover lifecycle itself comes from the element, so this is only inspected by
    /// tests that guard the "a drag is not hover" distinction.
    #[cfg(test)]
    pub fn sash_hovered(&self) -> bool {
        self.sash_pointer_inside
    }

    /// Advances the sash fade; true while another frame is needed.
    pub fn sample_sash(&mut self, now: Instant, reduced: bool) -> bool {
        if reduced || self.sash_opacity == self.sash_target {
            if self.sash_opacity != self.sash_target {
                self.sash_opacity = self.sash_target;
                return true;
            }
            return false;
        }
        let progress = now.saturating_duration_since(self.sash_since).as_secs_f32()
            / (SIDEBAR_SASH_FADE_MS as f32 / 1000.0);
        let progress = progress.clamp(0.0, 1.0);
        let step = ely_core::motion::ease_out_cubic(progress);
        let from = self.sash_opacity;
        // Ease between the value at the last change and the target.
        self.sash_opacity = if self.sash_target > from {
            from.max(step)
        } else {
            from.min(1.0 - step)
        };
        if progress >= 1.0 {
            self.sash_opacity = self.sash_target;
            return false;
        }
        true
    }

    /// Finishes (or cancels) a drag: the sash now follows the real pointer position, not the
    /// stale drag flag. `hovered` is whether the pointer is still inside the resize band.
    pub fn end_resize(&mut self, now: Instant, hovered: bool, reduced: bool) {
        self.dragging = false;
        self.sash_pointer_inside = hovered;
        self.retarget_sash(now, reduced);
    }

    /// Starts a resize gesture in whichever mode is currently shown, anchored at the pointer's
    /// press position. Entering a drag on the rail must not silently expand it.
    pub fn begin_resize(&mut self, press_pointer: f32, press_sash_left: f32) {
        self.drag_grab_offset =
            (press_pointer - press_sash_left).clamp(0.0, SIDEBAR_RESIZE_HIT_WIDTH);
        self.drag_pointer_x = press_pointer;
        self.press_pointer_at_begin = press_pointer;
        self.drag_travel = 0.0;
        self.footer_latched_compact = self.footer_compact();
        // A drag that interrupts a collapse is still an expanded-mode drag: the sidebar is wider
        // than the rail on screen, so dropping straight to the rail branch would be wrong.
        self.drag_from_rail = self.collapsed && self.frame.width <= SIDEBAR_COLLAPSED_WIDTH + 0.5;
        // A mid-animation collapse is frozen as expanded so the threshold logic starts from the
        // presented width rather than the already-flipped mode flag.
        self.collapsed = self.drag_from_rail;
        self.dragging = true;
    }

    /// Records the real pointer position during a gesture without taking visibility away from the
    /// drag; after release it is the only input to the hover fade.
    pub fn note_pointer(&mut self, inside: bool, now: Instant, reduced: bool) {
        self.sash_pointer_inside = inside;
        if !self.dragging {
            self.retarget_sash(now, reduced);
        }
    }

    /// Starts judging the line from the pointer during a drag, where the element's hover lifecycle
    /// is suppressed. The pointer is still inside the band at drag start.
    /// Applies the pointer position at which the platform drag actually started.
    pub fn start_at(&mut self, pointer_x: f32, now: Instant, window_width: f32, reduced: bool) {
        self.resize_pointer_at(pointer_x, now, window_width, reduced);
    }

    pub fn note_drag(&mut self, now: Instant, reduced: bool) {
        self.sash_pointer_inside = true;
        self.retarget_sash(now, reduced);
    }

    pub fn settle(&mut self, window_width: f32) {
        self.transition = None;
        self.frame = self.target(window_width);
    }

    /// Retargets an in-flight transition from the frame currently presented, so a moving pointer
    /// bends the same curve instead of restarting or snapping.
    fn retarget_transition(&mut self, now: Instant, window_width: f32, reduced: bool) {
        let to = self.target(window_width);
        if reduced {
            self.frame = to;
            self.transition = None;
            return;
        }
        self.transition = Some(Transition {
            from: self.frame,
            to,
            since: now,
        });
    }
    /// Absolute pointer tracking for the current gesture. `pointer_x` is in sidebar-container
    /// coordinates, so width comes from where the pointer is rather than from a remembered width
    /// or an accumulated relative baseline.
    pub fn resize_pointer_at(
        &mut self,
        pointer_x: f32,
        now: Instant,
        window_width: f32,
        reduced: bool,
    ) {
        if !self.dragging {
            return;
        }
        self.drag_pointer_x = pointer_x;
        self.drag_travel = pointer_x - self.press_pointer_at_begin;
        let requested = pointer_x - self.drag_grab_offset;
        self.latch_footer(requested);
        let min = SIDEBAR_MIN_WIDTH;
        let limit = Self::limit(window_width);

        // Hysteresis is expressed on the same pointer-derived width for both directions, so the
        // two entry conditions can never both hold at one pointer position:
        //   rail  -> expanded : requested >= min            (pointer reached the sash position)
        //   expanded -> rail  : requested <= min - 32       (full overshoot past the minimum)
        // Between them (min - 32 .. min) the current mode is docked and kept, so repeated or
        // slightly jittering pointer events are idempotent instead of alternating.
        if self.drag_from_rail {
            if requested >= min {
                self.drag_from_rail = false;
                self.collapsed = false;
                // The expanded width is the pointer's own position, bounded only by the legal
                // range and the window budget. It must not snap to a remembered or minimum width.
                self.expanded_width = requested.clamp(min, limit);
                self.retarget_transition(now, window_width, reduced);
            } else {
                // Still inside the rail zone: stay on the rail and keep the animation quiescent.
                self.transition = None;
                self.frame = self.target(window_width);
            }
            return;
        }

        if requested >= min {
            self.expanded_width = requested.clamp(min, limit);
            self.transition = None;
            self.frame = self.target(window_width);
            return;
        }
        // Docked at the minimum while the pointer is in the hysteresis band.
        self.expanded_width = min;
        if requested <= min - SIDEBAR_DRAG_COLLAPSE_THRESHOLD {
            self.collapsed = true;
            self.drag_from_rail = true;
            self.retarget_transition(now, window_width, reduced);
        } else {
            self.transition = None;
            self.frame = self.target(window_width);
        }
    }
}

pub(super) struct SidebarDrag {
    pub owner: gpui::EntityId,
    pub start_x: Cell<gpui::Pixels>,
}
