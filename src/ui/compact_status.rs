//! One compact status vocabulary for page overlays and inline states.
//!
//! These helpers only render host-supplied state. They do not read services, schedule retries or
//! invent empty results. A failure is a warning mark with its readable reason in a tooltip; a busy
//! state uses Ely's own animated spinner; true empty and partial outcomes keep their own wording.

use ely_core::{
    motion::Spinner,
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, IconSize},
};
use gpui::{
    AnyElement, App, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div,
    prelude::*, px,
};

/// A compact loading mark. `label` is the accessible name and tooltip, so an icon-only row remains
/// understandable by hover and assistive technology.
pub fn loading(id: impl Into<gpui::ElementId>, label: impl Into<String>, cx: &App) -> AnyElement {
    let label = label.into();
    let selector = format!("compact-loading-{label}");
    div()
        .id(id.into())
        .debug_selector(move || selector.clone())
        .role(gpui::Role::ProgressIndicator)
        .aria_label(label.clone())
        .tooltip(Tooltip::text(label))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .child(
            Spinner::new("compact-status-spinner")
                .size(IconSize::Xs)
                .color(cx.theme().colors.fg_muted),
        )
        .into_any_element()
}

/// A compact failed read. It is a real button when a retry is offered, so mouse, Enter and Space
/// all follow the same single action. The warning icon carries the short label; the tooltip keeps
/// the full reason available without expanding the row.
/// `selector` is an optional stable debug selector (for example an existing row id). The visible
/// tooltip and accessibility label always carry the readable label and reason.
pub fn failed(
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
    reason: impl Into<String>,
    on_retry: Option<impl Fn(&mut gpui::Window, &mut App) + 'static>,
    selector: Option<&'static str>,
    cx: &App,
) -> AnyElement {
    let label = label.into();
    let reason = reason.into();
    let aria = format!("{label}: {reason}");
    let retry = on_retry.map(std::rc::Rc::new);
    let colors = &cx.theme().colors;
    let fallback = format!("compact-failed-{label}");
    let selector = selector.unwrap_or_else(|| Box::leak(fallback.into_boxed_str()));
    let base = div()
        .id(id.into())
        .debug_selector(move || selector.to_string())
        .role(if retry.is_some() {
            gpui::Role::Button
        } else {
            gpui::Role::Image
        })
        .aria_label(aria.clone())
        .tooltip(Tooltip::text(aria))
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .child(
            Icon::new(IconName::TriangleAlert)
                .size(IconSize::Xs)
                .color(colors.warning),
        );
    match retry {
        Some(retry) => {
            let key_retry = retry.clone();
            base.tab_index(0)
                .cursor_pointer()
                .hover(|style| style.opacity(0.8))
                .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                    if !event.is_held
                        && !event.keystroke.modifiers.modified()
                        && matches!(event.keystroke.key.as_str(), "enter" | "space")
                    {
                        cx.stop_propagation();
                        key_retry(window, cx);
                    }
                })
                .on_click(move |_, window, cx| {
                    // The mark often lives inside a clickable row (for example a directory).
                    // Consume the press here so one retry never also toggles that ancestor.
                    cx.stop_propagation();
                    retry(window, cx);
                })
                .child(
                    Icon::new(IconName::RotateCw)
                        .size(IconSize::Xs)
                        .color(colors.fg_muted),
                )
                .into_any_element()
        }
        None => base.into_any_element(),
    }
}

/// A true, successfully observed empty state belongs to the data, not to an error icon. Callers
/// that need a compact mark use a muted circle rather than a warning triangle.
pub fn empty(label: impl Into<String>, cx: &App) -> AnyElement {
    let label = label.into();
    let color = cx.theme().colors.fg_subtle;
    let selector = format!("compact-empty-{label}");
    div()
        .id(gpui::ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .aria_label(label.clone())
        .tooltip(Tooltip::text(label))
        .flex_none()
        .child(Icon::new(IconName::Circle).size(IconSize::Xs).color(color))
        .into_any_element()
}
