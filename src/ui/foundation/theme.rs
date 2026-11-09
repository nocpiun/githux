//! NSH's default theme. Keep only application layout and font choices here.
pub use nsh_base::theme;
use nsh_core::prelude::*;

// The app's confirmed warning semantic; NSH's default theme has no warning token.
pub const WARNING: AdaptiveColor = AdaptiveColor {
    light: rgb(0xb45309).to_hsla(),
    dark: rgb(0xfbbf24).to_hsla(),
};

pub use crate::ui::page_layout::PAGE_BOTTOM;
pub use crate::ui::page_layout::PAGE_GUTTER;
pub use crate::ui::page_layout::PAGE_HEADER_HEIGHT;
pub use crate::ui::page_layout::PAGE_HEADER_INSET;
pub use crate::ui::page_layout::PAGE_TOP;
pub const WORK_ITEM_ROW_HEIGHT: f32 = 72.0;
pub const SIDEBAR_WIDTH: f32 = 232.0;
pub const ISLAND_INSET: f32 = 7.0;
pub const TITLEBAR_HEIGHT: f32 = 48.0;

pub fn label(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .font_family(super::fonts::UI_FAMILY)
        .text_size(px(14.0))
        .text_color(theme().foreground)
}

pub fn code(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .font_family(super::fonts::CODE_FAMILY)
        .text_size(px(13.0))
        .text_color(theme().foreground)
}

pub fn page_header(title: impl IntoReactive<SharedString>) -> View {
    view()
        .h(px(PAGE_HEADER_HEIGHT))
        .px(px(PAGE_HEADER_INSET))
        .justify_center()
        .child(
            label(title)
                .text_size(px(18.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
}

/// Use upstream defaults verbatim, with only the existing application font families.
pub fn tokens() -> Tokens {
    let mut tokens = nsh_base::tokens_of(None).expect("NSH default theme is valid");
    tokens.set_font(nsh_core::element::FONT_SANS, super::fonts::UI_FAMILY);
    tokens.set_font(nsh_core::element::FONT_MONO, super::fonts::CODE_FAMILY);
    tokens
}
