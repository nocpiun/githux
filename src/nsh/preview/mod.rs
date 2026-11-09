//! Integration preview only; real business pages are migrated in the next phase.
pub mod reader;
use crate::{
    foundation::{controls, theme as t},
    shell,
};
use nsh_core::prelude::*;

#[component]
pub fn MigrationPreview() -> impl IntoElement {
    let selected = signal(false);
    view()
        .size_full()
        .flex_col()
        .bg(t::BG)
        .child(
            view()
                .h(px(48.0))
                .px(px(16.0))
                .flex_row()
                .items_center()
                .gap_2()
                .child(t::label("Githux").font_weight(FontWeight::SEMIBOLD))
                .child(view().flex_1().window_drag(true))
                .child(controls::button("Foundation", move || selected.set(false)))
                .child(controls::button("Reader", move || selected.set(true))),
        )
        .child(view().flex_1().min_h_0().child(dynamic(move || {
            if selected.get() {
                reader::preview().into_element()
            } else {
                shell::foundation().into_element()
            }
        })))
}
