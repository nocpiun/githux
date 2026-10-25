//! Profile read types (R4).
//!
//! These types carry facts only: they hold no credentials, perform no I/O and
//! do not decide presentation. In particular they preserve the *uncertainty*
//! GitHub reports for the Pinned/Popular section (gist-only pins, mixed pins,
//! truncated lists) so a later UI task can choose a fallback policy. This layer
//! deliberately does not define one.

use crate::domain::error::GitHubError;
use thiserror::Error;

/// Typed errors from the profile read layer.
///
/// A GraphQL response is never assumed successful just because its HTTP status
/// is 200: `data` and `errors` may coexist, and `errors` are reduced to a safe
/// classification plus the query path. Raw server messages are not retained
/// because they may echo caller-supplied values (for example a login).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProfileError {
    /// Transport or HTTP-status failure, reusing the existing typed errors.
    #[error(transparent)]
    GitHub(#[from] GitHubError),

    /// Caller-supplied input (login, contribution range) is not valid.
    #[error("invalid {field}")]
    InvalidInput { field: &'static str },

    /// A 2xx response could not be parsed into the expected shape.
    #[error("invalid profile response: {message}")]
    InvalidResponse { message: String },

    /// A GraphQL response carried an error that left no usable data. The path
    /// and a safe category are preserved; the server message is not.
    #[error("GraphQL error at {path:?}: {kind:?}")]
    Graphql {
        kind: GraphqlErrorKind,
        path: Vec<String>,
    },

    /// A GraphQL POST answered with a redirect. Redirects are never followed for
    /// POST: re-sending the query body is not safe to automate, and the caller
    /// must treat it as an error rather than silently losing or forwarding it.
    #[error("GraphQL POST redirect was not followed (HTTP {status})")]
    RedirectNotFollowed { status: u16 },

    /// A response exceeded a configured engineering limit.
    #[error("response exceeds configured {resource} limit ({limit})")]
    LimitExceeded {
        resource: &'static str,
        limit: usize,
    },
}

/// Safe classification of a single GraphQL error. Mirrors the REST semantics in
/// [`GitHubError`] where possible (rate limit, permission, not found), and keeps
/// unknown error types as [`GraphqlErrorKind::Unknown`] without echoing the
/// server message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphqlErrorKind {
    NotFound,
    Unauthorized,
    Forbidden,
    /// Primary GraphQL rate limit. GitHub signals this with HTTP 200 and
    /// `x-ratelimit-remaining: 0`, not with a 4xx status.
    RateLimitPrimary,
    /// Secondary/abuse rate limit (no reliable header; may be 200 or 403).
    RateLimitSecondary,
    /// Organization SAML SSO authorization is required.
    SsoRequired {
        authorization_url: Option<String>,
    },
    /// An error type this client does not recognise (schema evolution). The
    /// message is intentionally discarded.
    Unknown,
}

/// A GraphQL error that accompanied otherwise-usable data (a partial result).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGraphqlError {
    pub kind: GraphqlErrorKind,
    /// Field path reported by GitHub (safe: field names, not values).
    pub path: Vec<String>,
}

/// Public profile information. `id` is the stable numeric identity; `login` is
/// display-only metadata and may change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileBasic {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    /// Profile HTML URL (`html_url` / `url`).
    pub profile_url: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub website_url: Option<String>,
    pub followers: Option<u64>,
    pub following: Option<u64>,
    pub public_repos: Option<u64>,
    pub public_gists: Option<u64>,
    pub created_at: Option<String>,
}

/// A repository entry in the Pinned/Popular section. Only fields needed for a
/// display summary are captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRepo {
    /// Numeric database id when the API provides one (GraphQL `databaseId`).
    pub id: Option<u64>,
    pub name: String,
    pub name_with_owner: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub is_private: Option<bool>,
    pub is_fork: Option<bool>,
    pub stargazer_count: Option<u64>,
    pub fork_count: Option<u64>,
    pub primary_language: Option<String>,
    pub owner_login: Option<String>,
}

/// Classification of the `itemShowcase.items` union.
///
/// `itemShowcase` returns the user's pins when any exist, otherwise GitHub's
/// "Popular" repository fallback. Its items are a `PinnableItem` union
/// (`Repository` / `Gist`), and `hasPinnedItems` counts both. This type keeps
/// that distinction instead of silently converting gists into repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Showcase {
    /// No pins at all: the server returned its "Popular repositories" fallback.
    PopularRepositories(Vec<ProfileRepo>),
    /// Pins exist and are all repositories.
    PinnedRepositories(Vec<ProfileRepo>),
    /// Pins exist and mix repositories and gists.
    PinnedWithGists {
        repositories: Vec<ProfileRepo>,
        gists: usize,
    },
    /// Pins exist but are all gists (no repository pin to show).
    PinnedGistsOnly { gists: usize },
    /// The showcase could not be classified (unknown union member, parse drift
    /// or a missing field). Kept explicit rather than treated as "no repos".
    Unclassified,
}

/// Pinned/Popular facts for a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePinned {
    /// Repository pins from `pinnedItems(types: [REPOSITORY])`, in the user's
    /// order. Order is preserved.
    pub repos: Vec<ProfileRepo>,
    /// Total repository pins reported by the server. May exceed `repos.len()`
    /// when the node list is truncated.
    pub total_repos: u32,
    /// `itemShowcase.hasPinnedItems`: true when the user pinned any item
    /// (repository *or* gist).
    pub has_pinned_items: bool,
    pub showcase: Showcase,
    /// GraphQL errors that accompanied otherwise-usable data.
    pub errors: Vec<ProfileGraphqlError>,
}

/// One contribution calendar day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionDay {
    /// ISO `YYYY-MM-DD`.
    pub date: String,
    pub count: u64,
    /// Server-provided level name (`NONE`, `FIRST_QUARTILE`, ...). Kept as a
    /// string so unknown levels from a schema change survive.
    pub level: String,
    /// Server-provided colour (theme-dependent; never hard-coded).
    pub color: String,
}

/// One week of contribution days.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionWeek {
    pub days: Vec<ContributionDay>,
}

/// Month header metadata for the calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionMonth {
    pub name: String,
    pub year: u32,
    /// ISO `YYYY-MM-DD` of the month's first displayed day.
    pub first_day: String,
    pub total_weeks: u32,
}

/// Contribution heatmap for a requested range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionCalendar {
    pub weeks: Vec<ContributionWeek>,
    pub months: Vec<ContributionMonth>,
    pub total_contributions: u64,
    /// GraphQL errors that accompanied otherwise-usable data.
    pub errors: Vec<ProfileGraphqlError>,
}

/// A validated inclusive date range for the contribution calendar.
///
/// Dates are stored in `YYYY-MM-DD` form. Validation rejects malformed or
/// non-existent dates, reversed ranges, and ranges wider than
/// [`ContributionRange::MAX_DAYS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionRange {
    from: String,
    to: String,
}

impl ContributionRange {
    /// Defensive span cap. GitHub itself limits the contribution window; this
    /// is an engineering bound so one request cannot request an unbounded
    /// number of weeks.
    pub const MAX_DAYS: i64 = 366;

    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Result<Self, ProfileError> {
        let from = from.into();
        let to = to.into();
        let from_days = parse_iso_date(&from)?;
        let to_days = parse_iso_date(&to)?;
        if to_days < from_days {
            return Err(ProfileError::InvalidInput {
                field: "contribution range (end before start)",
            });
        }
        if to_days - from_days > Self::MAX_DAYS {
            return Err(ProfileError::InvalidInput {
                field: "contribution range (span too wide)",
            });
        }
        Ok(Self { from, to })
    }

    pub fn from(&self) -> &str {
        &self.from
    }

    pub fn to(&self) -> &str {
        &self.to
    }

    /// Inclusive start as a GraphQL `DateTime` scalar value.
    pub fn start_datetime(&self) -> String {
        format!("{}T00:00:00Z", self.from)
    }

    /// Inclusive end as a GraphQL `DateTime` scalar value.
    pub fn end_datetime(&self) -> String {
        format!("{}T23:59:59Z", self.to)
    }
}

/// Parse `YYYY-MM-DD` into a day number, validating the calendar date.
fn parse_iso_date(value: &str) -> Result<i64, ProfileError> {
    let invalid = || ProfileError::InvalidInput {
        field: "contribution date",
    };
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(invalid());
    }
    let year: i64 = value[0..4].parse().map_err(|_| invalid())?;
    let month: u32 = value[5..7].parse().map_err(|_| invalid())?;
    let day: u32 = value[8..10].parse().map_err(|_| invalid())?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Err(invalid());
    }
    Ok(days_from_civil(year, month, day))
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since the Unix epoch (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let mp = (month + 9) % 12;
    let doy = (153 * mp as u64 + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_valid_inclusive_range() {
        let range = ContributionRange::new("2024-01-01", "2024-12-31").expect("valid");
        assert_eq!(range.from(), "2024-01-01");
        assert_eq!(range.to(), "2024-12-31");
        assert_eq!(range.start_datetime(), "2024-01-01T00:00:00Z");
        assert_eq!(range.end_datetime(), "2024-12-31T23:59:59Z");
    }

    #[test]
    fn rejects_reversed_range() {
        assert!(matches!(
            ContributionRange::new("2024-02-01", "2024-01-31"),
            Err(ProfileError::InvalidInput { .. })
        ));
    }

    #[test]
    fn rejects_malformed_or_nonexistent_dates() {
        for bad in [
            "2024-1-01",
            "2024-13-01",
            "2024-02-30",
            "2023-02-29",
            "2024-00-10",
            "2024-01-00",
            "not-a-date",
            "2024/01/01",
        ] {
            assert!(
                ContributionRange::new(bad, "2024-12-31").is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_leap_day_in_a_leap_year() {
        assert!(ContributionRange::new("2024-02-29", "2024-03-01").is_ok());
    }

    #[test]
    fn rejects_ranges_wider_than_the_cap() {
        assert!(matches!(
            ContributionRange::new("2020-01-01", "2024-01-01"),
            Err(ProfileError::InvalidInput { .. })
        ));
    }
}
