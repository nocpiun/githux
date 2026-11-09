//! Ely dark baseline at main@11f63e8. Colors stay light/dark aware (`AdaptiveColor`);
//! the values shown are the dark side, the same RGB as the legacy theme.
use nsh_core::prelude::*;

const fn tone(dark: Rgba) -> AdaptiveColor {
    AdaptiveColor {
        light: dark.to_hsla(),
        dark: dark.to_hsla(),
    }
}

pub const BG: AdaptiveColor = tone(rgb(0x252525));
pub const SURFACE: AdaptiveColor = tone(rgb(0x2c2c2c));
pub const HOVER: AdaptiveColor = tone(rgb(0x383838));
pub const ACTIVE: AdaptiveColor = tone(rgb(0x3f3f3f));
pub const BORDER: AdaptiveColor = tone(rgb(0x3f3f3f));
pub const FG: AdaptiveColor = tone(rgb(0xf3f1f0));
pub const MUTED: AdaptiveColor = tone(rgb(0xc4c2c0));
pub const SUBTLE: AdaptiveColor = tone(rgb(0xacaaa8));
pub const DISABLED: AdaptiveColor = tone(rgb(0x5c5a58));
pub const SUCCESS: AdaptiveColor = tone(rgb(0x6bbc89));
pub const WARNING: AdaptiveColor = tone(rgb(0xe8b45e));
pub const DANGER: AdaptiveColor = tone(rgb(0xe5756e));
pub const TOOLTIP_BG: AdaptiveColor = tone(rgb(0x4a4a4a));
pub const CLEAR: AdaptiveColor = tone(rgba(0x00000000));

pub const PAGE_GUTTER: f32 = 16.0;
pub const PAGE_TOP: f32 = 16.0;
pub const PAGE_BOTTOM: f32 = 8.0;
pub const PAGE_HEADER_HEIGHT: f32 = 40.0;
pub const PAGE_HEADER_INSET: f32 = 8.0;
pub const WORK_ITEM_ROW_HEIGHT: f32 = 72.0;
pub const SIDEBAR_WIDTH: f32 = 232.0;
pub const ISLAND_INSET: f32 = 7.0;
pub const ISLAND_RADIUS: f32 = 6.0;
pub const TITLEBAR_HEIGHT: f32 = 48.0;

pub fn label(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .font_family(super::fonts::UI_FAMILY)
        .text_size(px(14.0))
        .text_color(FG)
}

pub fn code(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .font_family(super::fonts::CODE_FAMILY)
        .text_size(px(13.0))
        .text_color(FG)
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

/// Match the application's baseline when NSH TextView reads its semantic tokens.
pub fn tokens() -> Tokens {
    let mut tokens = crate::reader::dark_tokens();
    for (name, color) in [
        ("background", BG),
        ("foreground", FG),
        ("card", SURFACE),
        ("card-foreground", FG),
        ("muted", SURFACE),
        ("muted-foreground", MUTED),
        ("border", BORDER),
        ("input", BORDER),
        ("accent", ACTIVE),
        ("accent-foreground", FG),
        ("primary", FG),
        ("primary-foreground", BG),
    ] {
        tokens.set_color(name, color);
    }
    tokens
}
