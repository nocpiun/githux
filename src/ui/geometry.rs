//! Geometry shared by the shell, sidebar and island.
//!
//! These values are the implementation starting point for this round, not a signed-off visual
//! result. They are kept in one place so the outer gaps, radii, sidebar width and the island's
//! inner corners move together instead of drifting per page.

/// Width of the fixed sidebar; its column owns the horizontal padding, so rows line up inside it.
pub const SIDEBAR_WIDTH: f32 = 232.0;

/// Client-edge inset. The island gap supplies the matching right inset for sidebar rows.
pub const SIDEBAR_PADDING: f32 = 8.0;
/// Narrowest expanded width. Lowered from 200 for the narrower critical width the owner asked for
/// (Chrome vertical-tabs scale); it is still an implementation starting point, not a fixed rule.
pub const SIDEBAR_MIN_WIDTH: f32 = 160.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 360.0;
/// The rail owns the left inset; ISLAND_GAP supplies the matching right inset.
pub const SIDEBAR_RAIL_ITEM_SIZE: f32 = 28.0;
pub const SIDEBAR_COLLAPSED_WIDTH: f32 = SIDEBAR_RAIL_ITEM_SIZE + SIDEBAR_PADDING;
pub const SIDEBAR_ANIMATION_MS: u64 = 200;
/// Active-route marker: flush to the drawable left edge, square on the left, rounded on the
/// right, and about two thirds of the row height so it never reads as a full-height bar.
pub const SIDEBAR_INDICATOR_WIDTH: f32 = 2.0;
pub const SIDEBAR_INDICATOR_RADIUS: f32 = 4.0;
pub const SIDEBAR_INDICATOR_HEIGHT_RATIO: f32 = 2.0 / 3.0;
pub const SIDEBAR_INDICATOR_MIN_HEIGHT: f32 = 20.0;
pub const SIDEBAR_INDICATOR_MAX_HEIGHT: f32 = 24.0;
/// A thin, low-contrast rule separating the repository group from the fixed footer.
pub const FOOTER_DIVIDER_HEIGHT: f32 = 1.0;
/// The sidebar column's row gap.
pub const SIDEBAR_ROW_GAP: f32 = 4.0;
/// Space above and below the footer rule. The column gap adds below this band's own content, so
/// the band is sized to absorb it: the rule ends up equally spaced from the list and identity.
pub const FOOTER_DIVIDER_SPACE: f32 = 8.0;
/// Size of the account status button (expanded only).
pub const STATUS_BUTTON_SIZE: f32 = 24.0;
pub const SIDEBAR_CONTENT_MIN_WIDTH: f32 = 600.0;
pub const SIDEBAR_RESIZE_HIT_WIDTH: f32 = 8.0;
/// The visible sash line inside the resize hit band, and how long it takes to fade.
///
/// The left edge is the historical centre of the old 2px line and stays put; the right edge grows
/// out to the band's own end, which is the island's outer frame. Widening must not re-centre the
/// line, so the anchor and the painted width are separate values.
pub const SIDEBAR_SASH_LEFT: f32 = (SIDEBAR_RESIZE_HIT_WIDTH - 2.0) / 2.0;
pub const SIDEBAR_SASH_WIDTH: f32 = SIDEBAR_RESIZE_HIT_WIDTH - SIDEBAR_SASH_LEFT;
pub const SIDEBAR_SASH_FADE_MS: u64 = 140;
/// Extra pointer travel past the minimum width before an expanded drag collapses. It also acts as
/// the hysteresis that stops one gesture flapping between modes at the same point.
pub const SIDEBAR_DRAG_COLLAPSE_THRESHOLD: f32 = 32.0;
/// How far the pointer must move off the rail before an expansion starts. It is smaller than the
/// collapse overshoot because the target width itself must also be legal, so the two conditions
/// together keep the rail stable without a fake multi-hundred-pixel dead zone.
pub const SIDEBAR_DRAG_EXPAND_THRESHOLD: f32 = 12.0;
pub const SIDEBAR_TOGGLE_SIZE: f32 = 24.0;
pub const REPO_LEFT_PADDING: f32 = 4.0;

/// Gap between the window edge and the island.
pub const ISLAND_INSET: f32 = 7.0;

/// Gap between the sidebar and the island.
pub const ISLAND_GAP: f32 = 8.0;

/// The island's own corner radius.
pub const ISLAND_RADIUS: f32 = 6.0;

/// Width of the island's border. The divider toggle is centred on this line, so both derive
/// from one value instead of guessing pixels.
pub const ISLAND_BORDER_WIDTH: f32 = 1.0;

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

/// Height of the band the strip's scrollbar paints in. It is reserved whether or not the scrollbar
/// is visible, so fading it in or out never moves the tabs.
pub const TAB_SCROLLBAR_BAND: f32 = super::scroll::THICKNESS + 2.0;

/// The strip's total height: the inset, the tabs, then the scrollbar band.
pub const TAB_STRIP_HEIGHT: f32 = TAB_STRIP_INSET + TAB_HEIGHT + TAB_SCROLLBAR_BAND;

/// Width of the toolbar pinned to the strip's right edge, outside the scrolling viewport. It is one
/// square slot matching the tab height, so its centered icon has equal top and right gaps.
pub const TAB_TOOLBAR_WIDTH: f32 = TAB_HEIGHT;
