//! Repository-scoped pull request list: query identity, row identity and one page.
//!
//! This is the repository's own list (`GET /repos/{owner}/{repo}/pulls`), not the
//! global personal search. A pull request number is only unique inside one
//! repository, so a page always carries the [`RepositoryLocator`] it was read
//! from and nothing here merges two repositories or treats an issue number as a
//! pull request.
//!
//! State is derived from the three fields GitHub actually reports on the list
//! endpoint: `state` (`open`/`closed`), `draft` and `merged_at`. A closed pull
//! request may be merged or closed without merging; the list never guesses from
//! the state name alone.
//!
//! No I/O and no credentials live in this module.

use crate::domain::repository::{Completeness, DynamicRef, RepositoryError, RepositoryLocator};

/// The only two states the list itself can be filtered by.
///
/// GitHub's `state=closed` result set contains both merged and closed-unmerged
/// pull requests, so the row state must be derived instead of inferred from the
/// filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PullFilter {
    Open,
    Closed,
}

impl PullFilter {
    /// The filter buttons in their product order.
    pub const ALL: [Self; 2] = [Self::Open, Self::Closed];

    /// Query value sent to the REST endpoint.
    pub const fn api_state(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    /// Visible filter label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Closed => "Closed",
        }
    }
}

/// What one list row communicates about the change.
///
/// `Draft` is an open pull request that is not ready for review; a merged pull
/// request is always `Merged` even if the closing action also closed it. `Closed`
/// therefore means "closed without merging", which is exactly the distinction
/// the repository tab has to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PullState {
    Draft,
    Open,
    Merged,
    Closed,
}

impl PullState {
    /// Derive the row state from the list endpoint's own fields.
    ///
    /// Unknown state strings and an open pull request that also reports
    /// `merged_at` are invalid responses instead of a silently "open" row.
    pub fn classify(
        state: &str,
        draft: bool,
        merged_at: Option<&str>,
    ) -> Result<Self, RepositoryError> {
        let merged = merged_at.is_some_and(|value| !value.trim().is_empty());
        match state.trim().to_ascii_lowercase().as_str() {
            "open" if merged => Err(RepositoryError::InvalidResponse {
                message: "open pull request reports merged_at".into(),
            }),
            "open" => Ok(if draft { Self::Draft } else { Self::Open }),
            "closed" if merged => Ok(Self::Merged),
            "closed" => Ok(Self::Closed),
            _ => Err(RepositoryError::InvalidResponse {
                message: "unknown pull request state".into(),
            }),
        }
    }

    /// Visible state label, matching GitHub's wording.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Open => "Open",
            Self::Merged => "Merged",
            Self::Closed => "Closed",
        }
    }

    /// Whether this row belongs to the open set the user can filter by.
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::Draft)
    }
}

/// One pull request as the list needs it.
///
/// `id` is GitHub's globally unique pull request id and exists only so the list
/// can dedupe rows when new pull requests shift the server-side pages. `number`
/// is what the user sees and what a `Route::PullRequest` target uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullSummary {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub state: PullState,
    /// Author login; an author GitHub cannot report stays absent, not empty.
    pub author: Option<String>,
    pub head: DynamicRef,
    pub base: DynamicRef,
    pub created_at: String,
    pub updated_at: String,
}

/// The immutable identity of one repository pull request list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullListQuery {
    filter: PullFilter,
    locator: RepositoryLocator,
}

impl PullListQuery {
    pub fn new(locator: RepositoryLocator, filter: PullFilter) -> Self {
        Self { filter, locator }
    }

    pub fn filter(&self) -> PullFilter {
        self.filter
    }

    pub fn locator(&self) -> &RepositoryLocator {
        &self.locator
    }
}

/// A validated next page. The URL is never adopted from the response: only the
/// page number survives, and the adapter rebuilds the request for the same
/// repository identity, filter and page size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullPageCursor {
    repository_id: u64,
    query: PullListQuery,
    page: u32,
    per_page: u32,
}

impl PullPageCursor {
    pub(crate) fn new(repository_id: u64, query: PullListQuery, page: u32, per_page: u32) -> Self {
        Self {
            repository_id,
            query,
            page,
            per_page,
        }
    }

    /// The numeric repository identity this continuation belongs to. Names are
    /// renameable, so an id is what keeps two repositories apart.
    pub fn repository_id(&self) -> u64 {
        self.repository_id
    }

    pub fn query(&self) -> &PullListQuery {
        &self.query
    }

    pub fn page(&self) -> u32 {
        self.page
    }

    /// The page size the cursor was issued with; a caller cannot mix sizes.
    pub fn per_page(&self) -> u32 {
        self.per_page
    }
}

/// One page of the repository list. A truly empty page is a successful result,
/// not an error, and a partial page still carries its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullListPage {
    pub query: PullListQuery,
    pub page: u32,
    pub pulls: Vec<PullSummary>,
    pub next: Option<PullPageCursor>,
    /// SSO partial results and rejected pagination links are completeness
    /// problems, not row failures.
    pub completeness: Completeness,
}
