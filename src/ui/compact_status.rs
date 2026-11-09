//! Compact status vocabulary for inline states: loading, failed, empty mark.
use crate::ui::foundation::{
    controls::{self, Icon},
    theme as t,
};
use nsh_core::prelude::*;
use std::rc::Rc;

pub fn loading(label: impl Into<SharedString>) -> impl IntoElement {
    let label = label.into();
    view()
        .size(px(16.0))
        .items_center()
        .justify_center()
        .role(AccessibilityRole::Progressbar)
        .label(label.clone())
        .tooltip(controls::tooltip(label))
        .child(controls::icon(Icon::Refresh))
}

pub fn failed(
    label: impl Into<SharedString>,
    reason: impl Into<SharedString>,
    on_retry: Option<Rc<dyn Fn()>>,
) -> impl IntoElement {
    let label = label.into();
    let reason = reason.into();
    let message = format!("{label}: {reason}");
    controls::failed(message, on_retry)
}

pub fn empty_dot() -> impl IntoElement {
    view()
        .size(px(4.0))
        .rounded(px(2.0))
        .bg(t::theme().muted_foreground)
}
