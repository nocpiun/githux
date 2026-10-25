//! Geometry shared by the shell, sidebar and island.
//!
//! These values are the implementation starting point for this round, not a signed-off visual
//! result. They are kept in one place so the outer gaps, radii, sidebar width and the island's
//! inner corners move together instead of drifting per page.

/// Width of the fixed sidebar; its column owns the horizontal padding, so rows line up inside it.
pub const SIDEBAR_WIDTH: f32 = 232.0;

/// Sidebar inner horizontal padding.
pub const SIDEBAR_PADDING: f32 = 10.0;

/// Gap between the window edge and the island.
pub const ISLAND_INSET: f32 = 7.0;

/// Gap between the sidebar and the island.
pub const ISLAND_GAP: f32 = 8.0;

/// The island's own corner radius.
pub const ISLAND_RADIUS: f32 = 6.0;

/// Content row and symmetric vertical inset (spacing 2 / 0.5rem).
pub const TITLEBAR_CONTENT_HEIGHT: f32 = 32.0;
pub const TITLEBAR_PADDING: f32 = 8.0;
pub const TITLEBAR_HEIGHT: f32 = TITLEBAR_CONTENT_HEIGHT + 2.0 * TITLEBAR_PADDING;

/// Search-entry height, independent of the title bar and native caption buttons.
pub const SEARCH_HEIGHT: f32 = 28.0;

/// Width of the title bar's search entry.
pub const SEARCH_WIDTH: f32 = 300.0;

/// Height of one sidebar repository row.
pub const REPO_ROW_HEIGHT: f32 = 30.0;

/// Height of the account row at the sidebar's bottom.
pub const ACCOUNT_ROW_HEIGHT: f32 = 40.0;

/// Height of a Home activity row.
pub const ACTIVITY_ROW_HEIGHT: f32 = 52.0;

/// The application session tab strip at the top of the island.
///
/// The strip's inset is one value on every in-scroll edge, so the first tab's top and left gaps
/// (and the last tab's right gap) match the island's inner boundary regardless of DPI rounding.
/// The strip clips horizontally and scrolls; it never widens the island, and its height is derived
/// so that the horizontal scrollbar, when it appears, does not move the tabs.
pub const TAB_STRIP_INSET: f32 = 8.0;
pub const TAB_HEIGHT: f32 = 26.0;
/// One fixed, readable tab width. Tabs never compress: the strip scrolls instead.
pub const TAB_WIDTH: f32 = 168.0;
pub const TAB_CLOSE_SIZE: f32 = 16.0;
/// The gap between two tabs in the strip.
pub const TAB_GAP: f32 = 4.0;
/// Reserved room under the tabs for Ely's overlay horizontal scrollbar. Ely's horizontal track is
/// two thicknesses tall and its thumb paints in the upper half, so the band is exactly that tall
/// and the thumb sits inside it. The band is always reserved, so the scrollbar appearing or
/// disappearing never moves the tabs.
pub const TAB_SCROLLBAR_BAND: f32 = 12.0;
/// The strip's total height: one inset, the tabs, then the scrollbar band.
pub const TAB_STRIP_HEIGHT: f32 = TAB_STRIP_INSET + TAB_HEIGHT + TAB_SCROLLBAR_BAND;
