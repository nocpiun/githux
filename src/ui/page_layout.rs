//! Shared page-content geometry for the production pages.
//!
//! The Shell island already gives every page the same viewport and tab strip; what drifted between
//! pages was each page's own content padding and heading sizes. These values are the single
//! reference the mounted pages use, so a page cannot invent a second rhythm:
//!
//! - `PAGE_GUTTER` matches Home's `px_4` body padding, so titles and list edges line up across
//!   Home, Notifications, Issues and Pull Requests.
//! - `PAGE_TOP` matches Home's `pt_4`.
//! - `PAGE_BOTTOM` is the required content-end breathing room, not an outer viewport mask.
//! - `PAGE_HEADER_HEIGHT` matches Home's column header row and is the height Notifications uses for
//!   its title line.
//!
//! Values live here rather than in `geometry.rs` so this task's page work does not rewrite the
//! shared island/sidebar geometry module that unrelated work also reads.

/// Horizontal page insert, the same 16 logical px Home already uses (`px_4`).
pub(crate) const PAGE_GUTTER: f32 = 16.0;

/// Extra inset a *heading row* adds inside the page gutter, matching Home's panel header
/// (`px_2`, 8 logical px). Home's title and its rows therefore start at different x: the title at
/// gutter + 8, the rows at the gutter. Only the title line uses this inset — shifting the list or
/// the empty state by it as well would push the content edges past Home's.
pub(crate) const PAGE_HEADER_INSET: f32 = 8.0;

/// Space above a page's first row of content.
pub(crate) const PAGE_TOP: f32 = 16.0;

/// Required bottom breathing room, placed inside scroll content (or real footer space).
pub(crate) const PAGE_BOTTOM: f32 = 8.0;

/// Title row height, shared with Home's column header so headings sit on one line.
pub(crate) const PAGE_HEADER_HEIGHT: f32 = 40.0;
