//! Home/Feed GitHub read adapters.
//!
//! - [`search::search_involving`]: `GET /search/issues` with a fixed
//!   `involves:<login>` query, `sort=updated&order=desc`, and an explicitly
//!   supplied optional `updated:` window. It never applies a product default
//!   window, never expands to review-requested, and is not browse history or
//!   unread state.
//! - [`events::fetch_received_events`]: `GET /users/{login}/received_events`
//!   with `ETag` / `X-Poll-Interval` / `304` surfaced for caller scheduling.
//!   This adapter does not poll in the background and never uses web cookies or
//!   internal endpoints.
//!
//! Both reuse the shared [`crate::github::client::GitHubClient`],
//! `Transport::execute_bounded`, cancellation, same-origin `Link` validation and
//! the existing error/rate-limit/SSO headers. Neither layer validates the active
//! account: they accept a `Pat` and an explicit login exactly like the other
//! low-level GitHub modules, so account-context checks stay with future data
//! wiring and the UI is not falsely reported as protected.

pub mod events;
pub mod search;

pub use events::{FeedLimits, fetch_received_events};
pub use search::{HomeSearchLimits, search_involving};
