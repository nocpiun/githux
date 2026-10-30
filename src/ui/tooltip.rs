//! Tooltips that carry the application's UI face themselves.
//!
//! GPUI prepaints a tooltip as its own root. The style stack of the element the tip hangs from is
//! not active while the tip's content is built, so a plain `Tooltip::text` falls back to GPUI's
//! default `.SystemUIFont` even though every visible ancestor sets Noto Sans. Wrapping the content
//! here is the one place that keeps menu, sidebar, tab and repository tips on the UI face.

use ely_core::{primitives::Tooltip, theme::ActiveTheme};
use gpui::{AnyView, App, Div, IntoElement, ParentElement, SharedString, Styled, Window, div};

/// Tooltip content on the theme's UI family, for callers that need custom children.
///
/// The product theme selects the bundled Noto Sans / Noto Sans SC composite; a standalone component assembly sets whatever family it
/// renders with, so reading the theme keeps the tip on the same face without importing the
/// application's font registration into every preview tree.
pub(crate) fn ui_tooltip_content(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .font_family(cx.theme().font_family.clone())
        .child(text.into())
}

/// A tooltip builder for `.tooltip(...)` whose content sets the UI family itself.
pub(crate) fn ui_tooltip(
    text: impl Into<SharedString> + 'static,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    Tooltip::rich(move |_, cx| ui_tooltip_content(text.clone(), cx).into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ely_core::theme::{ActiveTheme, Theme};
    use gpui::{Styled, TestAppContext};

    /// A tooltip is prepainted as its own root, so its content has to carry the UI family itself.
    /// The assertion is on the style the content actually sets: a bare tip has no family of its
    /// own and would fall back to `.SystemUIFont`.
    #[gpui::test]
    fn tooltip_content_sets_the_theme_ui_family(cx: &mut TestAppContext) {
        cx.update(|cx| {
            ely_core::init_for_tests(cx);
            Theme::update(cx, |theme| theme.font_family = "Noto Sans".into());
        });
        cx.update(|cx| {
            let expected = cx.theme().font_family.clone();
            let mut bare = div();
            assert_eq!(bare.style().text.font_family, None);
            let mut content = ui_tooltip_content("Refresh", cx);
            assert_eq!(content.style().text.font_family.as_ref(), Some(&expected));
        });
    }
}
