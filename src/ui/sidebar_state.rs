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
    /// Manual resize wins over motion, starting from the width actually on screen.
    pub fn begin_resize(&mut self) {
        self.transition = None;
        self.collapsed = false;
        self.expanded_width = self.frame.width.max(SIDEBAR_MIN_WIDTH);
        self.dragging = true;
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
