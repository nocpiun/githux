//! Native title-bar adapter. AppWindow owns the Tall path; legacy DWM is a fallback.

use gpui::Window;
use windows::core::Result;

#[path = "caption_legacy.rs"]
mod legacy;
#[path = "caption_tall.rs"]
mod tall;

pub use legacy::CaptionArea;

pub fn install(window: &Window, background: gpui::Hsla, foreground: gpui::Hsla) -> Result<()> {
    match tall::install(window, background, foreground) {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("Tall caption unavailable; trying legacy DWM: {error}");
            legacy::install(window, background, foreground)
        }
    }
}

pub fn area(window: &Window) -> Option<CaptionArea> {
    tall::area(window).or_else(|| legacy::area(window))
}

pub fn height(window: &Window) -> f32 {
    tall::area(window).map_or(crate::ui::geometry::TITLEBAR_HEIGHT, |area| area.bottom)
}

pub fn content_top_inset(window: &Window) -> f32 {
    if tall::area(window).is_some() {
        0.0
    } else {
        legacy::content_top_inset(window)
    }
}

pub fn shutdown() {
    tall::shutdown();
}
