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
        if reduced
            || self
                .transition
                .as_ref()
                .is_some_and(|t| t.to.width != target.width)
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
        let ease = ely_gpui_component::motion::ease_out_cubic(progress);
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

    /// Recomputes the sash target from the real pointer position alone. Reversing mid-fade
    /// continues from the presented opacity, so a quick in/out never jumps.
    ///
    /// A drag does not pin the line on: the agreed behaviour is "only while hovered", so holding
    /// the button and moving onto the sidebar or the island must fade it out like any other leave.
    fn retarget_sash(&mut self, now: Instant, reduced: bool) {
        let target = if self.sash_pointer_inside { 1.0 } else { 0.0 };
        self.sash_since = now;
        self.sash_target = target;
        if reduced {
            self.sash_opacity = target;
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
        let step = ely_gpui_component::motion::ease_out_cubic(progress);
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

    pub fn begin_resize(&mut self) {
        self.transition = None;
        self.collapsed = false;
        self.expanded_width = self.frame.width.max(SIDEBAR_MIN_WIDTH);
        self.dragging = true;
    }
    /// Starts judging the line from the pointer during a drag, where the element's hover lifecycle
    /// is suppressed. The pointer is still inside the band at drag start.
    pub fn note_drag(&mut self, now: Instant, reduced: bool) {
        self.sash_pointer_inside = true;
        self.retarget_sash(now, reduced);
    }

    pub fn settle(&mut self, window_width: f32) {
        self.transition = None;
        self.frame = self.target(window_width);
    }
    pub fn resize(&mut self, requested: f32, window_width: f32) {
        if !self.collapsed && self.dragging {
            self.expanded_width = requested.clamp(SIDEBAR_MIN_WIDTH, Self::limit(window_width));
            self.settle(window_width);
        }
    }
}

pub(super) struct SidebarDrag {
    pub owner: gpui::EntityId,
    pub start_width: f32,
    pub start_x: Cell<gpui::Pixels>,
}
