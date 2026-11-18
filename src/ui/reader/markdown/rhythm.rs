//! Shared Markdown presentation policy: vertical rhythm and GitHub alert tones.
//!
//! The numbers come from GitHub's Primer `markdown-body` styles, expressed in
//! multiples of the body font size so the reader keeps scaling with the theme's
//! font scale:
//!
//! * `$body-line-height: 1.5` — prose line height (kept at 1.6 for mixed CJK).
//! * `p, blockquote, ul, ol, table, pre` — 16px block gap at a 16px body.
//! * `h1…h6` — 24px above, 16px below, `line-height: 1.25`, weight 600, sizes
//!   2 / 1.5 / 1.25 / 1 / 0.875 / 0.85em; `h1`/`h2` add a hairline rule.
//! * `li + li` — 0.25em; `li > p` — 1em (loose items).
//! * `hr` — 1.5em above and below.
//! * GitHub alerts — 0.25em accent bar, 0.25em vertical / 1em horizontal
//!   padding, title in the accent colour with an Octicon-style icon.
//!
//! <https://raw.githubusercontent.com/primer/css/main/src/markdown/markdown-body.scss>
//! <https://raw.githubusercontent.com/primer/css/main/src/markdown/headings.scss>
//! <https://raw.githubusercontent.com/primer/css/main/src/markdown/lists.scss>

use ely_core::{
    primitives::IconName,
    theme::{Palette, contrast},
};
use gpui::{FontWeight, Hsla, Pixels, px};

use super::ast::{AlertKind, Block};

/// Prose line height as a multiple of the body size.
pub(crate) const LINE_HEIGHT: f32 = 1.6;
/// Gap between two prose blocks (Primer's 16px block margin).
const BLOCK: f32 = 0.85;
/// Extra room above a heading (Primer's 24px heading margin-top).
const HEADING_TOP: f32 = 1.6;
/// Primer's `$lh-condensed` heading line height.
const HEADING_LINE: f32 = 1.25;
/// Primer's `hr { margin: 24px 0 }`.
const RULE: f32 = 1.5;
/// Primer's `li + li { margin-top: 0.25em }`.
const LIST_ITEM: f32 = 0.25;
/// Primer's `blockquote { padding: 0 1em }`.
const QUOTE_INSET: f32 = 1.0;
/// GitHub alert padding: 8px vertical, 16px horizontal at a 16px body.
const QUOTE_PAD_Y: f32 = 0.5;
/// Primer's alert `border-left: .25em`.
const QUOTE_BAR: f32 = 0.25;

/// Vertical rhythm for one document, in real pixels for the body text size.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rhythm {
    body: f32,
}

impl Rhythm {
    pub(crate) fn new(body: f32) -> Self {
        Self { body }
    }

    /// The body font size these measurements belong to.
    pub(crate) fn body(self) -> f32 {
        self.body
    }

    /// One prose line box.
    pub(crate) fn line(self) -> Pixels {
        px(self.body * LINE_HEIGHT)
    }

    /// Gap between two adjacent prose blocks, already split between the two
    /// blocks' own paddings so margins never stack.
    pub(crate) fn block(self) -> Pixels {
        px(self.body * BLOCK)
    }

    /// Room above a heading, minus the previous block's half gap.
    pub(crate) fn heading_top(self) -> Pixels {
        px((self.body * HEADING_TOP - self.body * BLOCK / 2.0).max(0.0))
    }

    /// Heading line height (Primer's condensed scale).
    pub(crate) fn heading_line(self) -> f32 {
        HEADING_LINE
    }

    /// Primer heading sizes, relative to body text.
    pub(crate) fn heading_scale(self, level: u8) -> f32 {
        match level {
            1 => 2.0,
            2 => 1.5,
            3 => 1.25,
            4 => 1.0,
            5 => 0.875,
            _ => 0.85,
        }
    }

    /// Primer gives `h1`/`h2` a bottom hairline with `padding-bottom: 0.3em`.
    pub(crate) fn heading_rule(self, level: u8) -> bool {
        level <= 2
    }

    pub(crate) fn heading_weight(self) -> FontWeight {
        FontWeight::SEMIBOLD
    }

    /// Space around a horizontal rule.
    pub(crate) fn rule(self) -> Pixels {
        px((self.body * RULE - self.body * BLOCK / 2.0).max(0.0))
    }

    /// Gap between tight list items.
    pub(crate) fn list_item(self) -> Pixels {
        px(self.body * LIST_ITEM)
    }

    /// Left/right inset of a quote or callout.
    pub(crate) fn quote_inset(self) -> Pixels {
        px(self.body * QUOTE_INSET)
    }

    /// Vertical padding inside a callout.
    pub(crate) fn quote_padding_y(self) -> Pixels {
        px(self.body * QUOTE_PAD_Y)
    }

    /// Width of the quote bar and alert accent border.
    pub(crate) fn quote_bar(self) -> Pixels {
        px((self.body * QUOTE_BAR).max(2.0))
    }

    /// Padding inside a fenced code block.
    pub(crate) fn code_padding(self) -> Pixels {
        px(self.body * 0.9)
    }

    /// Table cell padding: Primer's `padding: 6px 13px`.
    pub(crate) fn cell_padding(self) -> (Pixels, Pixels) {
        (px(self.body * 0.95), px(self.body * 0.45))
    }

    /// Padding applied to one top-level block wrapper: `(top, bottom)`.
    ///
    /// Adjacent blocks each contribute half of [`Self::block`], so the real gap
    /// is exactly the policy value; a heading adds its larger lead-in. Nothing
    /// is applied above the document's first block.
    pub(crate) fn block_padding(self, block: &Block, first: bool) -> (Pixels, Pixels) {
        let half = self.block() * 0.5;
        let top = if first {
            Pixels::ZERO
        } else {
            match block {
                Block::Heading(..) => self.heading_top(),
                Block::Rule => self.rule(),
                _ => half,
            }
        };
        (top, half)
    }
}

/// One GitHub alert's presentation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AlertStyle {
    /// Icon, title text and the left bar.
    pub(crate) accent: Hsla,
    /// The callout's tinted background.
    pub(crate) fill: Hsla,
    pub(crate) icon: IconName,
}

/// The shared alert policy used by top-level and nested quotes alike.
///
/// GitHub's alert colours map onto the theme's semantic tones (blue note,
/// green tip, amber warning, red caution). The palette has no purple semantic,
/// so "important" uses the palette's violet chart hue, which the theme defines
/// for both modes (and for the colour-blind-safe palette).
pub(crate) fn alert_style(kind: AlertKind, colors: &Palette) -> AlertStyle {
    let (accent, fill) = match kind {
        AlertKind::Note => (colors.info, colors.info_subtle),
        AlertKind::Tip => (colors.success, colors.success_subtle),
        AlertKind::Important => {
            let violet = colors.chart.get(4).copied().unwrap_or(colors.accent_hover);
            // No violet `*_subtle` token exists; a low-alpha tint over the
            // reader surface reads the same in light and dark modes.
            (violet, violet.opacity(0.12))
        }
        AlertKind::Warning => (colors.warning, colors.warning_subtle),
        AlertKind::Caution => (colors.danger, colors.danger_subtle),
    };
    AlertStyle {
        accent: readable(accent, colors),
        fill,
        icon: alert_icon(kind),
    }
}

/// Whether `color` stays readable on the page; if not, mix it towards `fg`.
///
/// The palette's `fg` already holds the mode's maximum-contrast text colour,
/// so walking lightness and saturation towards it keeps the alert's hue while
/// fixing contrast deterministically for light, dark and high-contrast
/// palettes alike.
fn readable(color: Hsla, colors: &Palette) -> Hsla {
    let ground = colors.surface;
    if contrast(color, ground) >= 4.5 {
        return color;
    }
    let to = colors.fg;
    let mut best = to;
    for step in 0..=20 {
        let t = step as f32 / 20.0;
        let mut candidate = color;
        candidate.l = color.l + (to.l - color.l) * t;
        candidate.s = color.s + (to.s - color.s) * t;
        candidate.a = 1.0;
        best = candidate;
        if contrast(candidate, ground) >= 4.5 {
            break;
        }
    }
    best
}

/// GitHub's octicon analogue for each alert kind.
fn alert_icon(kind: AlertKind) -> IconName {
    match kind {
        AlertKind::Note => IconName::Info,
        AlertKind::Tip => IconName::Lightbulb,
        AlertKind::Important => IconName::MessageSquareWarning,
        AlertKind::Warning => IconName::TriangleAlert,
        AlertKind::Caution => IconName::OctagonAlert,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> [AlertKind; 5] {
        [
            AlertKind::Note,
            AlertKind::Tip,
            AlertKind::Important,
            AlertKind::Warning,
            AlertKind::Caution,
        ]
    }

    #[test]
    fn every_alert_tone_is_distinct_and_readable_in_both_modes() {
        for palette in [
            Palette::light(false),
            Palette::light(true),
            Palette::dark(false),
            Palette::dark(true),
        ] {
            let mut accents: Vec<(u32, f32, f32)> = Vec::new();
            for kind in kinds() {
                let style = alert_style(kind, &palette);
                let ratio = contrast(style.accent, palette.surface);
                assert!(
                    ratio >= 4.5,
                    "{kind:?} title must be readable on its surface: {ratio}"
                );
                let hue = style.accent.to_rgb();
                accents.push((kind as u32, ratio, hue.r + hue.g));
            }
            // Distinct kinds keep distinct hues, so the five callouts never
            // collapse into one grey box.
            let mut hues: Vec<f32> = accents.iter().map(|(_, _, hue)| *hue).collect();
            hues.sort_by(f32::total_cmp);
            for pair in hues.windows(2) {
                assert!(
                    (pair[1] - pair[0]).abs() > 0.001,
                    "alert hues must stay distinct: {hues:?}"
                );
            }
        }
    }

    #[test]
    fn alert_fills_differ_from_the_page_and_icons_are_distinct() {
        for palette in [Palette::light(false), Palette::dark(false)] {
            for kind in kinds() {
                let style = alert_style(kind, &palette);
                assert_ne!(
                    style.fill, palette.surface,
                    "{kind:?} fill must be tinted in both modes"
                );
            }
            let icons: Vec<&str> = kinds()
                .iter()
                .map(|kind| alert_style(*kind, &palette).icon.name())
                .collect();
            let unique: std::collections::HashSet<&&str> = icons.iter().collect();
            assert_eq!(unique.len(), icons.len(), "one icon per kind: {icons:?}");
        }
    }

    #[test]
    fn rhythm_matches_the_primer_scale() {
        let rhythm = Rhythm::new(13.0);
        assert!((f32::from(rhythm.line()) - 20.8).abs() < 0.001);
        assert!((f32::from(rhythm.block()) - 11.05).abs() < 0.001);
        assert!((rhythm.heading_scale(1) - 2.0).abs() < 0.001);
        assert!((rhythm.heading_scale(3) - 1.25).abs() < 0.001);
        assert!((rhythm.heading_scale(6) - 0.85).abs() < 0.001);
        assert!(rhythm.heading_rule(1) && rhythm.heading_rule(2) && !rhythm.heading_rule(3));
        // Headings lead in more than prose, and nothing is applied before the
        // document's first block.
        let heading = Block::Heading(2, Default::default());
        assert!(f32::from(rhythm.block_padding(&heading, false).0) > f32::from(rhythm.block()));
        assert_eq!(rhythm.block_padding(&heading, true).0, Pixels::ZERO);
    }
}
