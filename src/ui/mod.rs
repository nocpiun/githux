//! Ely component composition and view-local state. No business I/O during rendering.

use ely_gpui_component::{buttons::Button, primitives::FocusScope, theme::ActiveTheme};
use gpui::{Context, FocusHandle, IntoElement, ParentElement, Render, Styled, Window, div, px};

use crate::platform::fonts;

#[cfg(test)]
mod tests;

pub struct Hello {
    focus: FocusHandle,
    press_focus: FocusHandle,
    presses: u64,
}

impl Hello {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            // Keep the button's tab stop alive across renders and key events.
            press_focus: cx.focus_handle().tab_stop(true),
            presses: 0,
        }
    }
}

impl Render for Hello {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let this = cx.entity();
        FocusScope::new(&self.focus)
            .root()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(theme.colors.bg)
            .text_color(theme.colors.fg)
            .font_family(theme.font_family.clone())
            .text_size(px(16.0))
            .child(div().text_size(px(28.0)).child("Hello, world!"))
            .child(
                div()
                    .font_family(fonts::CODE_FAMILY)
                    .child(format!("Pressed {} times", self.presses)),
            )
            .child(
                Button::new("hello-press", "Press")
                    .focus_handle(&self.press_focus)
                    .primary()
                    .on_click(move |_, _, cx| {
                        this.update(cx, |hello, cx| {
                            hello.presses = hello.presses.saturating_add(1);
                            cx.notify();
                        });
                    }),
            )
            .child("Tab to focus · Enter / Space to press")
    }
}
