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

/// The application session tab strip at the top of the island. The strip never widens the
/// island: it clips, and each tab shrinks between the two width bounds below.
pub const TAB_STRIP_HEIGHT: f32 = 38.0;
pub const TAB_HEIGHT: f32 = 26.0;
pub const TAB_MIN_WIDTH: f32 = 56.0;
pub const TAB_MAX_WIDTH: f32 = 180.0;
pub const TAB_CLOSE_SIZE: f32 = 16.0;
