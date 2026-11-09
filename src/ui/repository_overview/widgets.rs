//! Presentation-only widgets shared by the repository page and its overview strip.
//!
//! They render host-supplied state and callbacks only: no services, no URLs, no cache. They keep
//! the confirmed compact vocabulary (a filled pill for a count, an outline pill for a provenance
//! mark, one warning icon with its reason in a tooltip, a neutral dot for a proven empty value).

use super::glyphs::{Glyph, colored_glyph};
use crate::ui::foundation::theme as t;
use nsh_core::prelude::*;

/// A small filled pill for a numeric or textual count. Zero and large values are shown as is.
pub fn badge(value: impl IntoReactive<SharedString>) -> View {
    view()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(t::theme().radius)
        .bg(t::theme().accent)
        .justify_center()
        .child(
            t::label(value)
                .text_size(px(11.0))
                .text_color(t::theme().muted_foreground),
        )
}

/// An outline pill: a provenance mark (`Cached`) or a verified semantic (`Latest`).
pub fn badge_outline(value: impl IntoReactive<SharedString>, color: AdaptiveColor) -> View {
    view()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(t::theme().radius)
        .border_1()
        .border_color(color)
        .justify_center()
        .child(t::label(value).text_size(px(11.0)).text_color(color))
}

/// A compact loading mark with an accessible label and tooltip. NSH has no spinner primitive, so
/// the mark is the shared refresh glyph rather than an invented one; it never implies completion.
pub fn loading(label: impl Into<SharedString>) -> View {
    let label = label.into();
    view()
        .size(px(18.0))
        .items_center()
        .justify_center()
        .role(AccessibilityRole::Progressbar)
        .label(label.clone())
        .tooltip(t::label(label).text_size(px(12.0)))
        .child(colored_glyph(
            Glyph::Refresh,
            t::theme().muted_foreground,
            14.0,
        ))
}

/// A real, actionable failure: one warning icon, its reason in the tooltip, and a retry only when
/// the host supplied one. Confirmed optional absence is the caller's job to omit entirely.
pub fn failed(
    label: impl Into<SharedString>,
    reason: impl Into<SharedString>,
    retry: Option<std::rc::Rc<dyn Fn()>>,
) -> View {
    let label = label.into();
    let reason = reason.into();
    let can_retry = retry.is_some();
    let message = SharedString::from(format!("{label}: {reason}"));
    view()
        .size(px(20.0))
        .items_center()
        .justify_center()
        .label(message.clone())
        .tooltip(t::label(message).text_size(px(12.0)))
        .role(if can_retry {
            AccessibilityRole::Button
        } else {
            AccessibilityRole::Image
        })
        .focusable(can_retry)
        .on_press(move || {
            if let Some(retry) = &retry {
                retry();
            }
        })
        .child(colored_glyph(Glyph::Warning, t::WARNING, 14.0))
}

/// A proven, successfully observed empty value: a muted dot, never a warning.
pub fn empty(label: impl Into<SharedString>) -> View {
    let label = label.into();
    view()
        .size(px(18.0))
        .items_center()
        .justify_center()
        .label(label.clone())
        .tooltip(t::label(label).text_size(px(12.0)))
        .child(
            view()
                .size(px(6.0))
                .rounded(px(3.0))
                .bg(t::theme().muted_foreground),
        )
}

/// One line of secondary text.
pub fn muted(value: impl IntoReactive<SharedString>) -> Text {
    t::label(value)
        .text_size(px(12.0))
        .text_color(t::theme().muted_foreground)
}
