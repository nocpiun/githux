//! Small static pill badge using NSH primitives and shared theme.
use nsh_core::prelude::*;
use crate::ui::foundation::theme as t;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum BadgeVariant {
    #[default]
    Filled,
    Outline,
}

pub struct Badge {
    label: SharedString,
    tone: Tone,
    variant: BadgeVariant,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: Tone::Neutral,
            variant: BadgeVariant::Filled,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn outline(mut self) -> Self {
        self.variant = BadgeVariant::Outline;
        self
    }
}

impl IntoElement for Badge {
    fn into_element(self) -> AnyElement {
        let theme = t::theme();
        let (bg, border, fg) = match (self.tone, self.variant) {
            (Tone::Neutral, BadgeVariant::Filled) => (theme.secondary, theme.border, theme.secondary_foreground),
            (Tone::Neutral, BadgeVariant::Outline) => (AdaptiveColor::TRANSPARENT, theme.border, theme.foreground),
            (Tone::Info, BadgeVariant::Filled) => (theme.accent, theme.border, theme.accent_foreground),
            (Tone::Info, BadgeVariant::Outline) => (AdaptiveColor::TRANSPARENT, theme.accent, theme.accent),
            (Tone::Success, BadgeVariant::Filled) => (theme.accent, theme.border, theme.accent_foreground),
            (Tone::Success, BadgeVariant::Outline) => (AdaptiveColor::TRANSPARENT, theme.accent, theme.accent),
            (Tone::Warning, BadgeVariant::Filled) => (theme.destructive, theme.border, theme.background),
            (Tone::Warning, BadgeVariant::Outline) => (AdaptiveColor::TRANSPARENT, theme.destructive, theme.destructive),
            (Tone::Danger, BadgeVariant::Filled) => (theme.destructive, theme.border, theme.background),
            (Tone::Danger, BadgeVariant::Outline) => (AdaptiveColor::TRANSPARENT, theme.destructive, theme.destructive),
        };
        view()
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(border)
            .bg(bg)
            .child(
                text(self.label)
                    .font_family(crate::platform::fonts::UI_FAMILY)
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg),
            )
            .into_element()
    }
}
