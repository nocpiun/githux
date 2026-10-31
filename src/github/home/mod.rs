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
//! - [`following::fetch_following`]: `GET /user/following`, the followed-account
//!   set the Feed membership decision depends on, with explicit
//!   complete/partial semantics.
//! - [`pull_request::fetch_pull_request_detail`][] reads `GET
//!   /repos/{owner}/{repo}/pulls/{number}`, used only to resolve a merged
//!   contribution's author, because a live `action=merged` event carries no
//!   author.
//!
//! Both reuse the shared [`crate::github::client::GitHubClient`],
//! `Transport::execute_bounded`, cancellation, same-origin `Link` validation and
//! the existing error/rate-limit/SSO headers. Neither layer validates the active
//! account: they accept a `Pat` and an explicit login exactly like the other
//! low-level GitHub modules, so account-context checks stay with future data
//! wiring and the UI is not falsely reported as protected.

pub mod events;
pub mod following;
pub mod pull_request;
pub mod search;

pub use events::{FeedLimits, fetch_received_events, fetch_received_events_page};
pub use following::{FollowingLimits, fetch_following};
pub use pull_request::{PullRequestDetailLimits, fetch_pull_request_detail};
pub use search::{HomeSearchLimits, search_involving};
