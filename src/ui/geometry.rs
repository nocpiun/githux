//! Geometry shared by the shell, sidebar and island.
//!
//! These values are the implementation starting point for this round, not a signed-off visual
//! result. They are kept in one place so the outer gaps, radii, sidebar width and the island's
//! inner corners move together instead of drifting per page.

/// Width of the fixed sidebar; its column owns the horizontal padding, so rows line up inside it.
pub const SIDEBAR_WIDTH: f32 = 232.0;

/// Client-edge inset. The island gap supplies the matching right inset for sidebar rows.
pub const SIDEBAR_PADDING: f32 = 8.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 200.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 360.0;
/// The rail owns the left inset; ISLAND_GAP supplies the matching right inset.
pub const SIDEBAR_RAIL_ITEM_SIZE: f32 = 28.0;
pub const SIDEBAR_COLLAPSED_WIDTH: f32 = SIDEBAR_RAIL_ITEM_SIZE + SIDEBAR_PADDING;
pub const SIDEBAR_ANIMATION_MS: u64 = 200;
pub const SIDEBAR_INDICATOR_INSET: f32 = 2.0;
pub const SIDEBAR_INDICATOR_RADIUS: f32 = 2.0;
pub const SIDEBAR_CONTENT_MIN_WIDTH: f32 = 600.0;
pub const SIDEBAR_RESIZE_HIT_WIDTH: f32 = 8.0;
pub const SIDEBAR_TOGGLE_SIZE: f32 = 24.0;
pub const REPO_ACCOUNT_GAP: f32 = 12.0;
pub const REPO_LEFT_PADDING: f32 = 4.0;

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
pub const ACCOUNT_ROW_HEIGHT: f32 = 48.0;

/// Height of a Home activity row.
pub const ACTIVITY_ROW_HEIGHT: f32 = 52.0;
