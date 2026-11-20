//! The shell's one way to take a shadow from the theme's elevation vocabulary.
//!
//! A shadow is a statement about depth, so it belongs only to surfaces that really sit over other
//! content. The shell's inventory is short:
//!
//! - the command palette card, the sidebar overflow menu and every tooltip already ask Ely for a
//!   level (`Modal`/`Floating`); they keep doing so and must not gain a second layer here;
//! - the tab drag ghost is carried over the strip it came from;
//! - the round collapse/expand toggle rests on the seam between the sidebar and the island.
//!
//! Only the last two are drawn by this project, so both go through [`attach`] instead of writing
//! offsets and a color themselves. One role, one level, one attachment point:
//!
//! - the shadow color is the palette's `shadow` token per mode, so a light window never paints the
//!   dark mode's dense black (the tab ghost used to hard-code exactly that);
//! - offsets, blur and alpha come from Ely's `Elevation` table, so Geek Neutral and light stay in
//!   step with the component library instead of drifting into a project-local recipe;
//! - `Elevation::Floating` is the level the library describes as "for what floats over the page",
//!   which is what both surfaces do. The shallower `Raised` layer was measured first and rejected:
//!   on a 24px control it moved the surrounding pixels by one or two gray levels, i.e. it was not
//!   visibly a shadow at all (see [`docs/theme.md`](../../../docs/theme.md#阴影与浮层));
//! - `attach` is the only call site, so a caller cannot stack the same layer twice.
//!
//! Rows, headings, list items, the tab band and the page header deliberately stay flat: giving
//! each of them a shadow would turn a flat page into a stack of cards.

use ely_core::theme::{ActiveTheme, Elevation};
use gpui::{App, BoxShadow, Div, Styled};

/// The one layer for something that floats over other content: a carried tab, a control resting on
/// a seam between two surfaces.
pub(crate) fn floating(cx: &App) -> Vec<BoxShadow> {
    cx.theme().elevation(Elevation::Floating)
}

/// Applies [`floating`] to `div` once; the element that floats owns the shadow, not its children.
pub(crate) fn attach(div: Div, cx: &App) -> Div {
    div.shadow(floating(cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ely_core::theme::{Mode, Theme};
    use gpui::{Styled, TestAppContext, div, px};

    /// The role is the theme's own elevation for the mode currently installed. A hand-written
    /// offset, blur or color would break this equality; light is the half that used to be wrong
    /// for the tab ghost, which painted a full-strength black regardless of the palette.
    #[gpui::test]
    fn the_role_is_the_theme_floating_elevation_in_the_current_mode(cx: &mut TestAppContext) {
        cx.update(|cx| {
            ely_core::init_for_tests(cx);
            for mode in [Mode::Dark, Mode::Light] {
                Theme::set_mode_now(mode, cx);
                assert_eq!(
                    floating(cx),
                    cx.theme().elevation(Elevation::Floating),
                    "{mode:?}: the floating role must be the theme's floating layer"
                );
            }
        });
    }

    /// Both modes really paint, the layers are drop shadows rather than inset ones, and the two
    /// modes disagree. A literal black would make the last assertion fail, which is the regression
    /// this check exists for.
    #[gpui::test]
    fn both_modes_paint_drop_shadows_and_disagree_on_their_color(cx: &mut TestAppContext) {
        let (dark, light) = cx.update(|cx| {
            ely_core::init_for_tests(cx);
            Theme::set_mode_now(Mode::Dark, cx);
            let dark = floating(cx);
            Theme::set_mode_now(Mode::Light, cx);
            let light = floating(cx);
            (dark, light)
        });

        for (name, layers) in [("dark", &dark), ("light", &light)] {
            assert!(!layers.is_empty(), "{name} must paint at least one layer");
            assert!(
                layers.iter().all(|layer| !layer.inset),
                "{name} is a drop shadow, never an inset one"
            );
            assert!(
                layers
                    .iter()
                    .all(|layer| layer.color.a > 0.0 && layer.blur_radius > px(0.0)),
                "{name} keeps the theme's blurred, visible layers"
            );
        }
        assert_ne!(
            dark, light,
            "the shadow follows the palette's `shadow` token and mode"
        );
    }

    /// `attach` puts exactly the theme's layers on the element and nothing more, which is what
    /// "do not stack the same shadow twice" means at this seam.
    #[gpui::test]
    fn attach_applies_the_theme_layers_once(cx: &mut TestAppContext) {
        cx.update(|cx| {
            ely_core::init_for_tests(cx);
            for mode in [Mode::Dark, Mode::Light] {
                Theme::set_mode_now(mode, cx);
                let expected = cx.theme().elevation(Elevation::Floating);
                let mut element = attach(div(), cx);
                assert_eq!(
                    element.style().box_shadow,
                    Some(expected.clone()),
                    "{mode:?}: exactly the theme's layers"
                );
                assert_eq!(
                    element.style().box_shadow.as_ref().map(Vec::len),
                    Some(expected.len()),
                    "{mode:?}: no layer is added a second time"
                );
            }
        });
    }
}
