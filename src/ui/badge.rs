//! Small static pill badge using Ely's public typography, sizing and semantic palette.
//!
//! Adapted narrowly from `ely-core/src/data_display/badge.rs` at upstream revision
//! 30c66fe1d5d23db769a6c834b771d054d7f288cf:
//! https://github.com/ELY-Open-Source/Ely-GPUI-Components
//! Only the ordinary Badge rendering is adapted; no Tag, CountBadge or animation code.
//!
//! MIT License
//!
//! Copyright (c) 2026 Ely GPUI Component contributors
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

// Only the repository development assembly consumes this shared UI primitive until Shell mounts it.
#![allow(dead_code)]

use ely_core::{
    data_display::Tone,
    theme::{ActiveTheme, ControlSize, TextSize},
};
use gpui::{
    App, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
    div, transparent_black,
};

/// The compact pill always retains its label verbatim, including "0" and large counts.
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: Tone,
    variant: BadgeVariant,
}
#[derive(Clone, Copy, Default)]
pub enum BadgeVariant {
    #[default]
    Filled,
    Outline,
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
impl RenderOnce for Badge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        // Tone::colors is private upstream; only public semantic palette fields are used.
        let ink: Hsla = match self.tone {
            Tone::Neutral => colors.fg_muted,
            Tone::Accent => colors.accent,
            Tone::Info => colors.info,
            Tone::Success => colors.success,
            Tone::Warning => colors.warning,
            Tone::Danger => colors.danger,
        };
        let (fill, border, ink) = match self.variant {
            BadgeVariant::Outline => (transparent_black(), ink, ink),
            BadgeVariant::Filled => match self.tone {
                Tone::Neutral => (colors.hover, transparent_black(), ink),
                Tone::Accent => (colors.accent, transparent_black(), colors.on_accent),
                _ => (ink.opacity(0.12), transparent_black(), ink),
            },
        };
        let height = theme.control_height(ControlSize::Sm) * 0.75;
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(height)
            .min_w(height)
            .px_1p5()
            .rounded_full()
            .border_1()
            .border_color(border)
            .bg(fill)
            .text_size(theme.text_size(TextSize::Xs))
            .line_height(height)
            .font_weight(FontWeight::MEDIUM)
            .text_color(ink)
            .child(self.label)
    }
}
