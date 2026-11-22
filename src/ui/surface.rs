//! The surface ladder shared by the Shell and Home/Feed.
//!
//! - [`window`] — the title bar and the sidebar, below the island.
//! - [`page`] — the island and the page ground.
//! - [`raised`] — content that sits above its page: unselected Feed cards.
//! - [`well`] — content recessed below its page: the Feed detail inset.
//!
//! The tokens are the existing Ely palette fields (`bg`, `surface`, `overlay`, `sunken`), so the
//! ladder follows the theme in both dark and light modes.

use ely_core::theme::Palette;
use gpui::Hsla;

/// The window ground behind the island: title bar and sidebar.
#[allow(dead_code)] // Not every standalone example mounts the outer Shell chrome.
pub(crate) fn window(colors: &Palette) -> Hsla {
    colors.bg
}

/// The island and the page ground.
pub(crate) fn page(colors: &Palette) -> Hsla {
    colors.surface
}

/// Content raised above the page: unselected Feed cards.
pub(crate) fn raised(colors: &Palette) -> Hsla {
    colors.overlay
}

/// Content recessed below the page: the Feed detail inset.
pub(crate) fn well(colors: &Palette) -> Hsla {
    colors.sunken
}

#[cfg(test)]
mod tests {
    use super::*;
    use ely_core::theme::Palette;

    #[test]
    fn the_surface_ladder_has_four_distinct_steps_in_dark() {
        let colors = Palette::dark(false);
        let levels = [
            window(&colors),
            page(&colors),
            raised(&colors),
            well(&colors),
        ];
        for (index, level) in levels.iter().enumerate() {
            for other in &levels[index + 1..] {
                assert_ne!(level, other, "surface levels must not collapse");
            }
        }
    }

    #[test]
    fn the_surface_ladder_maps_to_semantic_tokens_in_light() {
        let colors = Palette::light(false);
        assert_eq!(window(&colors), colors.bg);
        assert_eq!(page(&colors), colors.surface);
        assert_eq!(raised(&colors), colors.overlay);
        assert_eq!(well(&colors), colors.sunken);
    }
}
