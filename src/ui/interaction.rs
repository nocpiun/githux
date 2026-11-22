//! The four interaction treatments shared by every clickable row in the shell.
//!
//! Selection, hover, press and keyboard focus are different states and must not share a fill:
//!
//! - hover — the pointer is over the row: `colors.hover`.
//! - press — the pointer is down on the row: `colors.active`.
//! - selected — the row is the current item: `colors.active` (the same fill, held).
//! - focus — the row is the keyboard stop: a 1px `colors.focus` ring.
//!
//! Callers keep a transparent 1px border in their base style so the focus ring never changes the
//! box size.

use ely_core::theme::ActiveTheme;
use gpui::{App, StyleRefinement, Styled};

/// How far an unavailable control fades. One value so disabled rows fade together.
pub(crate) const DISABLED_OPACITY: f32 = 0.45;

/// A 1px keyboard-focus ring in the theme's focus colour. The element must already reserve a
/// transparent border, or focusing it would shift its content by one pixel.
pub(crate) fn focus_ring(cx: &App) -> impl FnOnce(StyleRefinement) -> StyleRefinement {
    let color = cx.theme().colors.focus;
    move |style| style.border_1().border_color(color)
}

/// The fill under the pointer.
pub(crate) fn hover_fill(cx: &App) -> impl FnOnce(StyleRefinement) -> StyleRefinement {
    let color = cx.theme().colors.hover;
    move |style| style.bg(color)
}

/// The fill while the pointer is held down, and the held fill of a selected row.
pub(crate) fn pressed_fill(cx: &App) -> impl FnOnce(StyleRefinement) -> StyleRefinement {
    let color = cx.theme().colors.active;
    move |style| style.bg(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Background, StyleRefinement, TestAppContext};

    fn solid(style: StyleRefinement) -> Option<Background> {
        style.background.and_then(|fill| fill.color())
    }

    #[gpui::test]
    fn hover_and_press_are_distinct_theme_tokens(cx: &mut TestAppContext) {
        cx.update(ely_core::init_for_tests);
        cx.update(|cx| {
            let colors = cx.theme().colors.clone();
            let hover = solid(hover_fill(cx)(StyleRefinement::default()));
            let pressed = solid(pressed_fill(cx)(StyleRefinement::default()));
            assert_eq!(hover, Some(Background::from(colors.hover)));
            assert_eq!(pressed, Some(Background::from(colors.active)));
            assert_ne!(hover, pressed, "hover and press must not share a fill");
        });
    }

    #[gpui::test]
    fn focus_ring_is_a_one_pixel_focus_colour_ring(cx: &mut TestAppContext) {
        cx.update(ely_core::init_for_tests);
        cx.update(|cx| {
            let expected = cx.theme().colors.focus;
            let style = focus_ring(cx)(StyleRefinement::default());
            assert_eq!(style.border_color, Some(expected));
            assert!(
                style.border_widths.top.is_some_and(|w| !w.is_zero()),
                "the ring must reserve a visible border"
            );
        });
    }
}
