//! Shared hairline scrollbar appearance and hover/drag fade.
//!
//! Geometry and thumb interaction live in `tab_scrollbar`; readers use those
//! same primitives with their real scrolling handles. A fully hidden bar is
//! not in the element tree, so it cannot intercept text selection or a link.

use ely_gpui_component::theme::ActiveTheme;
use gpui::App;

#[allow(unused_imports)]
pub use super::geometry::{TAB_SCROLLBAR_FADE_MS, TAB_SCROLLBAR_THICKNESS};

pub const MIN_READER_THUMB: f32 = 24.0;

/// One appearance shared by the session strip and content readers.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub thickness: f32,
    pub min_thumb: f32,
    pub idle_opacity: f32,
    pub busy_opacity: f32,
}

impl Style {
    pub const TABS: Self = Self {
        thickness: TAB_SCROLLBAR_THICKNESS,
        min_thumb: 0.0,
        idle_opacity: 0.20,
        busy_opacity: 0.38,
    };

    pub const READER: Self = Self {
        thickness: TAB_SCROLLBAR_THICKNESS,
        min_thumb: MIN_READER_THUMB,
        idle_opacity: 0.20,
        busy_opacity: 0.38,
    };

    pub fn colors(self, cx: &App) -> (gpui::Hsla, gpui::Hsla) {
        let fg = cx.theme().colors.fg;
        (fg.opacity(self.idle_opacity), fg.opacity(self.busy_opacity))
    }
}

/// The strip's existing fade, made public through one shared entry point.
pub use super::tab_scrollbar::Fade;
