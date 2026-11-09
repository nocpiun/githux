//! Lucide glyphs used by the repository page and its overview strip.
//!
//! The NSH foundation owns the shared icon vocabulary; these are the repository-specific marks
//! it does not carry yet. Paths are Lucide's, under the repository's existing ISC notice.

use crate::ui::foundation::theme as t;
use nsh_core::{element::ImageSrc, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Code,
    Folder,
    File,
    ChevronRight,
    ChevronDown,
    Star,
    Fork,
    Tag,
    BookOpen,
    Link,
    Package,
    Warning,
    Refresh,
    Issue,
    PullRequest,
    Settings,
}

fn path(glyph: Glyph) -> &'static str {
    match glyph {
        Glyph::Code => "<path d='m8 6-6 6 6 6M16 6l6 6-6 6'/>",
        Glyph::Folder => {
            "<path d='M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z'/>"
        }
        Glyph::File => {
            "<path d='M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z'/><path d='M14 3v6h6'/>"
        }
        Glyph::ChevronRight => "<path d='m9 6 6 6-6 6'/>",
        Glyph::ChevronDown => "<path d='m6 9 6 6 6-6'/>",
        Glyph::Star => {
            "<path d='m12 3 2.9 5.9 6.5.9-4.7 4.6 1.1 6.5L12 17.8 6.2 20.9l1.1-6.5L2.6 9.8l6.5-.9z'/>"
        }
        Glyph::Fork => {
            "<circle cx='6' cy='5' r='3'/><circle cx='6' cy='19' r='3'/><circle cx='18' cy='19' r='3'/><path d='M6 8v8M18 16V9a4 4 0 0 0-4-4h-1m3-3-3 3 3 3'/>"
        }
        Glyph::Tag => "<path d='M20 12 12 20 3 11V3h8l9 9z'/><circle cx='7.5' cy='7.5' r='1'/>",
        Glyph::BookOpen => {
            "<path d='M12 6c-2-1.5-4-2-7-2v14c3 0 5 .5 7 2 2-1.5 4-2 7-2V4c-3 0-5 .5-7 2zM12 6v14'/>"
        }
        Glyph::Link => {
            "<path d='M10 13a4 4 0 0 0 6 .5l2-2a4 4 0 0 0-6-6l-1 1'/><path d='M14 11a4 4 0 0 0-6-.5l-2 2a4 4 0 0 0 6 6l1-1'/>"
        }
        Glyph::Package => "<path d='m12 3 9 5v8l-9 5-9-5V8z'/><path d='m3 8 9 5 9-5M12 13v8'/>",
        Glyph::Warning => "<path d='m12 3 10 18H2zM12 9v4M12 17h.01'/>",
        Glyph::Refresh => {
            "<path d='M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-2l2 3M4 16l2 3a7 7 0 0 0 12-2'/>"
        }
        Glyph::Issue => "<circle cx='12' cy='12' r='9'/><circle cx='12' cy='12' r='1'/>",
        Glyph::PullRequest => {
            "<circle cx='6' cy='5' r='3'/><circle cx='6' cy='19' r='3'/><circle cx='18' cy='19' r='3'/><path d='M6 8v8M18 16V9a4 4 0 0 0-4-4h-1m3-3-3 3 3 3'/>"
        }
        Glyph::Settings => {
            "<circle cx='12' cy='12' r='3'/><path d='m9 3-1 3-3 1-2 3 2 2-1 3 3 2 2-1 3 2 3-2 3 1 2-3-1-3 2-2-2-3-3-1-1-3z'/>"
        }
    }
}

pub fn glyph(kind: Glyph) -> impl IntoElement {
    colored_glyph(kind, t::theme().muted_foreground, 14.0)
}

pub fn colored_glyph(kind: Glyph, color: AdaptiveColor, size: f32) -> impl IntoElement {
    svg(ImageSrc::svg_markup(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'>{}</svg>",
        path(kind)
    )))
    .size(px(size))
    .text_color(color)
}
