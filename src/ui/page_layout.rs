//! Shared page-content geometry for the production pages.
//!
//! The Shell island already gives every page the same viewport and tab strip; what drifted between
//! pages was each page's own content padding and heading sizes. These values are the single
//! reference the mounted pages use, so a page cannot invent a second rhythm:
//!
//! - `PAGE_GUTTER` matches Home's `px_4` body padding, so titles and list edges line up across
//!   Home, Notifications, Issues and Pull Requests.
//! - `PAGE_TOP` matches Home's `pt_4`.
//! - `PAGE_BOTTOM` matches Home's `pb_2`; the list itself owns the remaining scroll slack.
//! - `PAGE_HEADER_HEIGHT` matches Home's column header row and is the height Notifications uses for
//!   its title line.
//!
//! Values live here rather than in `geometry.rs` so this task's page work does not rewrite the
//! shared island/sidebar geometry module that unrelated work also reads.

/// Horizontal page insert, the same 16 logical px Home already uses.
pub(crate) const PAGE_GUTTER: f32 = 16.0;

/// Space above a page's first row of content.
pub(crate) const PAGE_TOP: f32 = 16.0;

/// Space below the last row of content before the island edge.
pub(crate) const PAGE_BOTTOM: f32 = 8.0;

/// Title row height, shared with Home's column header so headings sit on one line.
pub(crate) const PAGE_HEADER_HEIGHT: f32 = 40.0;
