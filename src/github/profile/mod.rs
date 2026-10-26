//! Profile data adaptation (R4).
//!
//! Reads a user's public profile (REST), the Pinned/Popular section and the
//! contribution calendar (read-only GraphQL). Boundaries kept explicit:
//! - Only reads. No mutation is ever sent, and no write queue exists here.
//! - The Pinned/Popular section preserves GitHub's uncertainty (gist-only or
//!   mixed pins, truncated lists). It does not pick a fallback policy.
//! - A GraphQL `200` with `errors` is never treated as full success.
//! - No account/authorization wrapper or UI: a later task wires this behind an
//!   authorization-epoch check in the data layer.
//!
//! Deliberately excluded from this task: the personal-README heuristic, the
//! pre-2020 "share to profile" marker, and the final gist-only presentation.
//! Those remain product decisions and no README body is fetched here.

pub mod basic;
pub mod contributions;
pub mod graphql;
pub mod pinned;

pub use basic::fetch_profile;
pub use contributions::fetch_contribution_calendar;
pub use graphql::ProfileLimits;
pub use pinned::fetch_profile_pinned;

use crate::domain::profile::ProfileError;

/// Validate a GitHub login before it is placed in a request path or a GraphQL
/// variable. This is a safety check (no path/query separators or control
/// characters), not a claim that the login exists.
pub(crate) fn validate_login(login: &str) -> Result<(), ProfileError> {
    if login.is_empty()
        || login.len() > 100
        || login
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | '?' | '#' | ' '))
    {
        return Err(ProfileError::InvalidInput { field: "login" });
    }
    Ok(())
}
