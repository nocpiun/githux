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
/// match the island's inner boundary regardless of DPI rounding. It deliberately matches the gap
/// between the island and the window edge, so the strip's own padding reads as the same rhythm as
/// the frame instead of a second, larger margin.
pub const TAB_STRIP_INSET: f32 = ISLAND_INSET;

/// One tab's height. The strip is a little taller than the tab, which leaves room for the thin
/// scrollbar without another inset.
pub const TAB_HEIGHT: f32 = 26.0;

/// One fixed, readable tab width. Tabs never compress: the strip scrolls instead.
pub const TAB_WIDTH: f32 = 168.0;

/// The icons' touch target inside a tab.
pub const TAB_CLOSE_SIZE: f32 = 16.0;

/// Gap between a tab's icon and its title. The icon-to-close spacing stays tighter, so the label
/// keeps most of the width.
pub const TAB_ICON_GAP: f32 = 7.0;

/// The gap between two tabs in the strip.
pub const TAB_GAP: f32 = 4.0;

/// Visible thickness of the strip's own scrollbar thumb. It is a hairline compared with the
/// theme's default thickness, and it is the implementation starting point rather than a signed-off
/// value.
pub const TAB_SCROLLBAR_THICKNESS: f32 = 3.0;

/// Height of the band the scrollbar paints in. It is reserved whether or not the scrollbar is
/// visible, so fading it in or out never moves the tabs.
pub const TAB_SCROLLBAR_BAND: f32 = TAB_SCROLLBAR_THICKNESS + 2.0;

/// The strip's total height: the inset, the tabs, then the scrollbar band.
pub const TAB_STRIP_HEIGHT: f32 = TAB_STRIP_INSET + TAB_HEIGHT + TAB_SCROLLBAR_BAND;

/// Width of the toolbar pinned to the strip's right edge, outside the scrolling viewport. It is one
/// small icon button wide; the strip's inset already provides the gap to the island's edge.
pub const TAB_TOOLBAR_WIDTH: f32 = TAB_HEIGHT - 2.0;

/// How long the strip's scrollbar takes to fade in or out.
pub const TAB_SCROLLBAR_FADE_MS: u64 = 140;
