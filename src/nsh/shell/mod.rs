//! Windows foundation probe: each block is a verifiable capability, not an unmigrated page.
use crate::foundation::{
    controls::{self, Icon},
    theme as t,
};
use nsh_core::prelude::*;
use std::rc::Rc;

#[component]
pub fn Foundation() -> impl IntoElement {
    // Counters prove closures run from input paths, not a mock controller.
    let actions = signal(0usize);
    let a1 = actions;
    let a2 = actions;
    let a3 = actions;
    let actions_inner = actions;
    on_key("ctrl+k", move |_| actions_inner.update(|n| *n += 1));

    let rows = 800usize;

    // A real, recycled virtual list. Exact 3px shared scrollbar styling is a recorded gap.
    let virtual_row = move |at: Position| {
        let number = at.index() + 1;
        view()
            .h(px(40.0))
            .px_2()
            .flex_row()
            .items_center()
            .bg(move || {
                if at.index().is_multiple_of(2) {
                    t::SURFACE
                } else {
                    t::BG
                }
            })
            .child(
                t::code(move || SharedString::from(format!("#{number:03}"))).text_color(t::MUTED),
            )
            .child(view().flex_1().px_2().child(t::label(move || {
                if number.is_multiple_of(5) {
                    SharedString::from("中文 · 原生窗口 · DirectWrite")
                } else {
                    SharedString::from(format!("row {number}"))
                }
            })))
    };

    let toolbar = view().h(px(48.0)).px(px(t::PAGE_GUTTER)).bg(t::BG).child(
        view()
            .flex_1()
            .items_center()
            .flex_row()
            .child(view().w(px(1.0)).window_drag(true))
            .child(controls::button("Act", move || a1.update(|n| *n += 1)))
            .child(controls::icon_button("Glyph", Icon::Home, move || {
                a2.update(|n| *n += 1)
            }))
            .child(view().w(px(8.0)))
            .child(
                controls::failed(
                    "Retry example failure",
                    Some(Rc::new(move || a3.update(|n| *n += 1))),
                )
                .test_id("foundation-retry"),
            )
            .child(view().w(px(16.0)))
            .child(
                view()
                    .flex_1()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(controls::badge(move || {
                        SharedString::from(format!("Actions: {}", actions.get()))
                    }))
                    .child(
                        view()
                            .flex_1()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(t::BORDER)
                            .h(px(28.0))
                            .child(
                                nsh_core::element::input()
                                    .auto_correct(false)
                                    .test_id("foundation-input"),
                            ),
                    ),
            ),
    );
    view()
        .flex_1()
        .bg(t::BG)
        .child(toolbar)
        .child(
            view()
                .h(px(t::PAGE_HEADER_HEIGHT))
                .px(px(t::PAGE_HEADER_INSET))
                .justify_center()
                .child(
                    t::label("Native Shell 基础")
                        .text_size(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .test_id("foundation-title"),
                ),
        )
        .child(
            view()
                .flex_1()
                .min_h_0()
                .p(px(t::PAGE_GUTTER))
                .pt_0()
                .child(
                    virtual_list(rows, virtual_row)
                        .flex_1()
                        .min_h_0()
                        .scrollbar(true)
                        .test_id("foundation-list"),
                ),
        )
}
