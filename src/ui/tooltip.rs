//! Native Shell tooltip with bundled UI font.
use nsh_core::prelude::*;
use crate::ui::foundation::theme as t;

pub fn ui_tooltip(message: impl Into<SharedString>) -> impl IntoTooltip {
    nsh_base::tooltip()
        .bg(t::theme().popover)
        .rounded(px(4.0))
        .px_2()
        .py_1()
        .child(
            t::label(message.into())
                .text_size(px(12.0))
                .text_color(t::theme().popover_foreground),
        )
}
