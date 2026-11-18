//! The product's dark palette: the owner-selected `Geek Neutral` carbon ramp.
//!
//! Ely still owns the light palette, the semantic token names and the mode transition. Only the
//! dark target is replaced, through Ely's public `Theme::set_palette`, so the startup paint and a
//! later light → dark toggle read the same colors. The values below are the owner's Round-2 choice
//! from the local comparison (`cb2d350`, `Candidate::GeekNeutral`), not a product candidate list:
//! there is no switcher, persistence or new setting in this path.
//!
//! ## What is replaced, and what is inherited
//!
//! Every surface, text, hairline, semantic, chart, ANSI and syntax token is set explicitly so a
//! missing assignment cannot silently fall back to Ely's warm dark pastels. The tokens the
//! comparison left alone stay Ely's dark values: `on_media`, `backdrop`, `media_backdrop`,
//! `shimmer`, `shadow`, `paper` and `ink`. `glass` is the new ground with Ely's existing alpha, so
//! a blurred pane keeps its weight instead of showing the old grey through.

use ely_core::theme::{Mode, Palette, Syntax, Theme};
use gpui::{App, Hsla, rgb, rgba};

/// Ely's dark `glass` alpha, `0xb8`: keep the pane weight and follow the new ground.
const GLASS_ALPHA: f32 = 0xb8 as f32 / 255.0;

/// Installs the dark palette, then shows it at once so the first window paints the final colors.
///
/// `set_palette` also starts a fade; `set_mode_now` stops it and assigns the target directly. A
/// later `Shell::toggle_theme`/`Theme::set_mode(Dark)` resolves the same installed palette.
pub(crate) fn install(cx: &mut App) {
    Theme::set_palette(Mode::Dark, Some(geek_neutral()), cx);
    Theme::set_mode_now(Mode::Dark, cx);
}

/// `Geek Neutral`: strictly achromatic carbon surfaces with terminal-grade signal colors.
pub(crate) fn geek_neutral() -> Palette {
    let mut palette = Palette::dark(false);
    // A monotonic elevation chain: sunken < bg < surface < overlay < hover < active.
    palette.sunken = c(0x141414);
    palette.bg = c(0x191919);
    palette.surface = c(0x202020);
    palette.overlay = c(0x262626);
    palette.hover = c(0x2c2c2c);
    palette.active = c(0x333333);
    palette.border = c(0x303030);
    palette.border_strong = c(0x474747);
    // Text weight: fg > fg_muted > fg_subtle > fg_disabled.
    palette.fg = c(0xededed);
    palette.fg_muted = c(0xb8b8b8);
    palette.fg_subtle = c(0x9e9e9e);
    palette.fg_disabled = c(0x525252);
    palette.tooltip_bg = c(0x3a3a3a);
    palette.tooltip_fg = c(0xededed);
    // Blue link/focus, violet accent and terminal emerald/amber/coral/blue feedback.
    palette.link = c(0x6cb6ff);
    palette.focus = c(0x4d9fff);
    palette.selection = rgba(0x4d9fff4d).into();
    palette.accent = c(0xa371f7);
    palette.accent_hover = c(0xb98cff);
    palette.on_accent = c(0x0d0d0d);
    palette.success = c(0x3fb950);
    palette.success_subtle = c(0x1b2f21);
    palette.warning = c(0xe3b341);
    palette.warning_subtle = c(0x382f1c);
    palette.danger = c(0xff6b6b);
    palette.danger_subtle = c(0x3a2321);
    palette.info = c(0x58a6ff);
    palette.info_subtle = c(0x1a2b3d);
    palette.chart = [
        0x6cb6ff, 0x56d4dd, 0xe3b341, 0xf2788f, 0xbc8cff, 0x3fb950, 0xff9e64, 0x7ee0ff,
    ]
    .map(c);
    palette.ansi = [
        0x333333, 0xff7b72, 0x3fb950, 0xe3b341, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb8b8b8, 0x525252,
        0xff8b85, 0x56d364, 0xf0cc6a, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xededed,
    ]
    .map(c);
    palette.syntax = Syntax {
        keyword: c(0xff7b72),
        string: c(0xa5d6ff),
        number: c(0x79c0ff),
        comment: c(0x8b949e),
        function: c(0xd2a8ff),
        type_name: c(0x7ee787),
        constant: c(0xffa657),
        property: c(0x79c0ff),
        tag: c(0x7ee787),
        attribute: c(0xffa657),
        operator: c(0xc9d1d9),
        punctuation: c(0x8b949e),
        variable: c(0xededed),
    };
    palette.glass = palette.bg.opacity(GLASS_ALPHA);
    palette
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ely_core::theme::ActiveTheme;
    use gpui::{Rgba, TestAppContext};
    use std::time::Duration;

    /// The hex a palette token currently holds, for exact-value checks.
    fn hex_of(color: Hsla) -> u32 {
        let rgba = Rgba::from(color);
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        (channel(rgba.r) << 16) | (channel(rgba.g) << 8) | channel(rgba.b)
    }

    /// Every token the owner's palette replaces, in the same order as the comparison's table.
    const OWNER_TOKENS: [(&str, u32); 28] = [
        ("sunken", 0x141414),
        ("bg", 0x191919),
        ("surface", 0x202020),
        ("overlay", 0x262626),
        ("hover", 0x2c2c2c),
        ("active", 0x333333),
        ("border", 0x303030),
        ("border_strong", 0x474747),
        ("fg", 0xededed),
        ("fg_muted", 0xb8b8b8),
        ("fg_subtle", 0x9e9e9e),
        ("fg_disabled", 0x525252),
        ("tooltip_bg", 0x3a3a3a),
        ("tooltip_fg", 0xededed),
        ("link", 0x6cb6ff),
        ("focus", 0x4d9fff),
        ("accent", 0xa371f7),
        ("accent_hover", 0xb98cff),
        ("on_accent", 0x0d0d0d),
        ("success", 0x3fb950),
        ("success_subtle", 0x1b2f21),
        ("warning", 0xe3b341),
        ("warning_subtle", 0x382f1c),
        ("danger", 0xff6b6b),
        ("danger_subtle", 0x3a2321),
        ("info", 0x58a6ff),
        ("info_subtle", 0x1a2b3d),
        ("selection", 0x4d9fff),
        // `glass` is checked separately because it carries the inherited alpha.
    ];

    #[test]
    fn the_dark_palette_is_exactly_the_owner_selected_geek_neutral() {
        let palette = geek_neutral();
        let ely = Palette::dark(false);

        for (token, expected) in OWNER_TOKENS {
            assert_eq!(
                hex_of(palette.token(token)),
                expected,
                "{token} must be the Geek Neutral value"
            );
        }

        // The selection wash keeps the documented `4d` alpha, not just its RGB.
        assert_eq!(hex_of(palette.selection), 0x4d9fff);
        let selection = Rgba::from(palette.selection);
        assert!((selection.a - 0x4d as f32 / 255.0).abs() < 0.005);

        assert_eq!(
            palette.chart.map(hex_of),
            [
                0x6cb6ff, 0x56d4dd, 0xe3b341, 0xf2788f, 0xbc8cff, 0x3fb950, 0xff9e64, 0x7ee0ff,
            ]
        );
        assert_eq!(
            palette.ansi.map(hex_of),
            [
                0x333333, 0xff7b72, 0x3fb950, 0xe3b341, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb8b8b8,
                0x525252, 0xff8b85, 0x56d364, 0xf0cc6a, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xededed,
            ]
        );
        for (name, expected) in [
            ("keyword", 0xff7b72),
            ("string", 0xa5d6ff),
            ("number", 0x79c0ff),
            ("comment", 0x8b949e),
            ("function", 0xd2a8ff),
            ("type_name", 0x7ee787),
            ("constant", 0xffa657),
            ("property", 0x79c0ff),
            ("tag", 0x7ee787),
            ("attribute", 0xffa657),
            ("operator", 0xc9d1d9),
            ("punctuation", 0x8b949e),
            ("variable", 0xededed),
        ] {
            assert_eq!(
                hex_of(palette.syntax.token(name)),
                expected,
                "syntax token {name}"
            );
        }

        // Tokens the comparison deliberately left to Ely's dark palette.
        for token in [
            "on_media",
            "backdrop",
            "media_backdrop",
            "shimmer",
            "shadow",
            "paper",
            "ink",
        ] {
            assert_eq!(
                palette.token(token),
                ely.token(token),
                "{token} must keep Ely's dark value"
            );
        }

        // The pane follows the new ground with Ely's existing alpha.
        assert_eq!(palette.glass, palette.bg.opacity(GLASS_ALPHA));
        assert!((Rgba::from(palette.glass).a - GLASS_ALPHA).abs() < 0.005);
    }

    #[test]
    fn the_surfaces_are_achromatic_and_step_cleanly() {
        let palette = geek_neutral();
        let ramp = [
            ("sunken", palette.sunken),
            ("bg", palette.bg),
            ("surface", palette.surface),
            ("overlay", palette.overlay),
            ("hover", palette.hover),
            ("active", palette.active),
        ];
        for (name, color) in ramp {
            let rgb = hex_of(color);
            let (r, g, b) = (rgb >> 16, (rgb >> 8) & 0xff, rgb & 0xff);
            assert!(
                r == g && g == b,
                "{name} must be gray, got {r:#04x}{g:#04x}{b:#04x}"
            );
        }
        for pair in ramp.windows(2) {
            let (a_name, a) = pair[0];
            let (b_name, b) = pair[1];
            assert!(
                hex_of(a) < hex_of(b),
                "{a_name} must be darker than {b_name}"
            );
            assert!(
                (hex_of(b) >> 16) - (hex_of(a) >> 16) >= 4,
                "{a_name} -> {b_name} must be a visible step"
            );
        }
        assert!(hex_of(palette.fg) > hex_of(palette.fg_muted));
        assert!(hex_of(palette.fg_muted) > hex_of(palette.fg_subtle));
        assert!(hex_of(palette.fg_subtle) > hex_of(palette.fg_disabled));
    }

    /// The product installs the palette before the first window and a light round trip resolves
    /// the same dark target, matching the sidebar's `Theme::set_mode` toggle.
    #[gpui::test]
    fn startup_and_a_light_round_trip_show_the_same_geek_neutral(cx: &mut TestAppContext) {
        cx.update(|cx| {
            ely_core::init_for_tests(cx);
            Theme::update(cx, |theme| theme.reduced_motion = true);
            install(cx);
        });
        assert_eq!(
            cx.read(|cx| cx.theme().colors.clone()),
            geek_neutral(),
            "the first paint must already be Geek Neutral"
        );

        cx.update(|cx| Theme::set_mode(Mode::Light, cx));
        settle(cx);
        assert_eq!(
            cx.read(|cx| cx.theme().colors.clone()),
            Palette::light(false),
            "the light palette stays Ely's"
        );

        cx.update(|cx| Theme::set_mode(Mode::Dark, cx));
        settle(cx);
        assert_eq!(
            cx.read(|cx| cx.theme().colors.clone()),
            geek_neutral(),
            "returning to dark must restore the same values"
        );
    }

    /// Lets the theme fade finish under the test executor.
    fn settle(cx: &mut TestAppContext) {
        std::thread::sleep(Duration::from_millis(2));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
    }
}
