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
pub const ISLAND_INSET: f32 = 8.0;

/// Gap between the sidebar and the island.
pub const ISLAND_GAP: f32 = 8.0;

/// The island's own corner radius.
pub const ISLAND_RADIUS: f32 = 12.0;

/// Height of the custom title bar. Ely's `titlebar_height()` token is consulted too; this keeps
/// the shell's own rows in step with it.
pub const TITLEBAR_HEIGHT: f32 = 38.0;

/// Width of the title bar's search entry.
pub const SEARCH_WIDTH: f32 = 300.0;

/// Height of one sidebar repository row.
pub const REPO_ROW_HEIGHT: f32 = 30.0;

/// Height of the account row at the sidebar's bottom.
pub const ACCOUNT_ROW_HEIGHT: f32 = 40.0;

/// Height of a Home activity row.
pub const ACTIVITY_ROW_HEIGHT: f32 = 52.0;
