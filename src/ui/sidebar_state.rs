//! Window-local sidebar geometry and GPUI drag ownership; no route or persistent state.
use super::geometry::*;

pub(super) struct SidebarState {
    pub collapsed: bool,
    pub expanded_width: f32,
    pub dragging: bool,
    pub menu_open: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            collapsed: false,
            expanded_width: SIDEBAR_WIDTH,
            dragging: false,
            menu_open: Default::default(),
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

    pub fn resize(&mut self, requested: f32, window_width: f32) {
        if !self.collapsed && self.dragging {
            self.expanded_width = requested.clamp(SIDEBAR_MIN_WIDTH, Self::limit(window_width));
        }
    }
}

/// Payload is local to this shell entity, so another window cannot resize this sidebar.
pub(super) struct SidebarDrag {
    pub owner: gpui::EntityId,
    pub start_width: f32,
    pub start_x: std::cell::Cell<gpui::Pixels>,
}
