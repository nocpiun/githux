//! Home/Feed read models: a parameterized `involves:` search result and a
//! received-events feed.
//!
//! These types carry no credentials and perform no I/O. Home is confirmed as
//! `involves:<login> is:open` with no default time window; callers supply an
//! explicit login and, optionally, an explicit `updated` window. Feed is
//! confirmed as a **filtered** set of received activities
//! ([`classify_feed_activity`]). Neither list defines an "unread" meaning, a
//! review-requested relationship or a browse-history interpretation. Truncation
//! and missing data are represented by readiness, never by a silently
//! complete-looking empty list.

use crate::domain::error::GitHubError;
use thiserror::Error;

/// Hard errors from the Home query adapters. A genuinely empty result and a
/// truncated (partial) result are **not** errors; they are returned with
/// [`HomeReadiness`]/[`FeedReadiness`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HomeError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error("invalid {field}")]
    InvalidInput { field: &'static str },
    #[error("invalid response: {message}")]
    InvalidResponse { message: String },
    #[error("response exceeds configured {resource} limit ({limit})")]
    LimitExceeded {
        resource: &'static str,
        limit: usize,
    },
}

/// A GitHub login explicitly supplied by the caller.
///
/// Validation rejects anything that could inject additional search qualifiers
/// (spaces, `:`, quotes, operators) and the `@me` alias, so the resulting query
/// is always a fixed `involves:<login>` lookup rather than a caller-controlled
/// search expression. It is display identity, never a credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Login(String);

impl Login {
    pub fn new(value: impl Into<String>) -> Result<Self, HomeError> {
        let value = value.into();
        if !is_valid_login(&value) {
            return Err(HomeError::InvalidInput { field: "login" });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid_login(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=39).contains(&bytes.len())
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A validated `YYYY-MM-DD` UTC date used in an `updated:` qualifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateBound(String);

impl DateBound {
    pub fn new(value: impl Into<String>) -> Result<Self, HomeError> {
        let value = value.into();
        if !is_valid_date(&value) {
            return Err(HomeError::InvalidInput {
                field: "updated date",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let slice = value.get(range)?;
        if !slice.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        slice.parse().ok()
    };
    let Some(year) = digits(0..4) else {
        return false;
    };
    let Some(month) = digits(5..7) else {
        return false;
    };
    let Some(day) = digits(8..10) else {
        return false;
    };
    year >= 1 && (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// An explicit optional `updated` window. At least one bound must be present;
/// this type never invents a default. Both bounds are inclusive and expressed
/// as UTC dates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatedWindow {
    from: Option<DateBound>,
    to: Option<DateBound>,
}

impl UpdatedWindow {
    pub fn new(from: Option<DateBound>, to: Option<DateBound>) -> Result<Self, HomeError> {
        if from.is_none() && to.is_none() {
            return Err(HomeError::InvalidInput {
                field: "updated window",
            });
        }
        if let (Some(from), Some(to)) = (&from, &to)
            && from.as_str() > to.as_str()
        {
            return Err(HomeError::InvalidInput {
                field: "updated window",
            });
        }
        Ok(Self { from, to })
    }

    /// Inclusive lower bound (`updated:>=`), when supplied.
    pub fn from(&self) -> Option<&DateBound> {
        self.from.as_ref()
    }

    /// Inclusive upper bound (`updated:<=`), when supplied.
    pub fn to(&self) -> Option<&DateBound> {
        self.to.as_ref()
    }
}

/// A caller-supplied Home query. `login` is mandatory and the `updated` window
/// is optional; there is no hidden default window. The emitted query always
/// also carries the owner-confirmed `is:open` filter, so closed and merged
/// entries are not requested at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeQuery {
    pub login: Login,
    pub updated: Option<UpdatedWindow>,
}

impl HomeQuery {
    pub fn new(login: Login) -> Self {
        Self {
            login,
            updated: None,
        }
    }

    pub fn with_updated_window(mut self, updated: UpdatedWindow) -> Self {
        self.updated = Some(updated);
        self
    }
}

/// Issue vs pull request. GitHub's search returns both from the same endpoint;
/// the presence of the `pull_request` object is the only distinction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeItemKind {
    Issue,
    PullRequest,
}

/// Repository reference parsed from the search item's `repository_url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeRepositoryRef {
    pub owner: String,
    pub name: String,
}

/// One search hit. Only stable identity and display fields actually returned by
/// the API are kept; this is a read model, not an editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeItem {
    /// Stable GitHub issue/PR id (not the repository-local `number`).
    pub id: u64,
    pub kind: HomeItemKind,
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub state: String,
    pub updated_at: String,
    pub repository: HomeRepositoryRef,
}

impl HomeItem {
    /// GitHub returns `open` or `closed` for issues and pull requests; a merged
    /// pull request is `closed`. Home only presents open entries, so any other
    /// value (closed, merged, missing, unrecognised) must be treated as "not
    /// presentable" instead of being shown or counted as open.
    pub fn is_open(&self) -> bool {
        self.state.trim().eq_ignore_ascii_case("open")
    }
}

/// Why a Home search result is only partial. Any entry means the caller must
/// not present the list as a complete, lossless snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomePartialReason {
    /// A later page request failed after earlier pages succeeded.
    PageFailed { message: String },
    /// The local page budget was reached while a next page still existed.
    PageLimit,
    /// The local item budget was reached before all returned items were kept.
    EntryLimit,
    /// The search endpoint's 1,000-result ceiling was hit.
    ResultLimit,
    /// The server set `incomplete_results: true` (query timeout).
    IncompleteResults,
    /// A `Link: rel="next"` pointed outside the trusted origin.
    UntrustedPagination,
    /// A `Link: rel="next"` was malformed or not the expected search endpoint.
    InvalidPagination,
    /// A `Link: rel="next"` did not advance the page (loop).
    PaginationCycle,
    /// A later response exceeded the configured byte budget.
    ResponseLimit,
    /// A later page body could not be parsed.
    ParseFailed { message: String },
    /// `X-GitHub-SSO: partial-results` omitted some organizations.
    SsoPartial { organizations: Vec<String> },
}

/// Completeness of a Home search result. `Complete` means "no truncation was
/// detected", not "lossless sync": `sort=updated` is dynamic, so an item may
/// move between pages while a caller reads them. Callers must dedupe by id and
/// must not claim a guaranteed lossless set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeReadiness {
    Complete,
    Partial { reasons: Vec<HomePartialReason> },
}

impl HomeReadiness {
    pub(crate) fn from_reasons(reasons: Vec<HomePartialReason>) -> Self {
        if reasons.is_empty() {
            Self::Complete
        } else {
            Self::Partial { reasons }
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// Result of a parameterized `involves:` search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeSearchOutcome {
    /// Deduplicated by stable `id` across pages, in the order returned.
    pub items: Vec<HomeItem>,
    /// Server-reported total match count (may exceed what is reachable).
    pub total_count: u64,
    /// Server-reported `incomplete_results` flag.
    pub incomplete_results: bool,
    pub readiness: HomeReadiness,
}

/// Event actor summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedActor {
    pub id: u64,
    pub login: String,
    pub display_login: Option<String>,
    pub avatar_url: Option<String>,
}

/// Event repository summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRepository {
    pub id: u64,
    pub name: String,
    pub url: String,
}

/// Recognised event type, plus a passthrough for anything new. Unknown types
/// keep only the type name so a future payload shape cannot grow unbounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedEventType {
    Push,
    PullRequest,
    Issues,
    IssueComment,
    Watch,
    Create,
    Delete,
    Fork,
    Release,
    Public,
    Member,
    Unknown(String),
}

/// The authoritative pull-request author from `payload.pull_request.user`.
///
/// A merged-pull-request event is often generated by the maintainer who clicked
/// merge, so the event actor is **not** the contributor to credit. This is read
/// straight from the same event payload; it never needs an extra request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedAuthor {
    pub id: u64,
    pub login: String,
    pub avatar_url: Option<String>,
}

/// Minimal typed payload for the common event types, selected from the official
/// event schema. Unknown types keep no payload at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedPayload {
    Push {
        git_ref: Option<String>,
        before: Option<String>,
        head: Option<String>,
        size: Option<u64>,
    },
    PullRequest {
        action: String,
        number: Option<u64>,
        title: Option<String>,
        html_url: Option<String>,
        state: Option<String>,
        merged: Option<bool>,
        /// `pull_request.user`, when the payload supplies it.
        author: Option<FeedAuthor>,
    },
    Issues {
        action: String,
        number: Option<u64>,
        title: Option<String>,
        html_url: Option<String>,
        state: Option<String>,
    },
    IssueComment {
        action: String,
        issue_number: Option<u64>,
        comment_url: Option<String>,
    },
    Watch {
        action: String,
    },
    Create {
        ref_type: Option<String>,
        git_ref: Option<String>,
    },
    Delete {
        ref_type: Option<String>,
        git_ref: Option<String>,
    },
    Fork {
        forkee_full_name: Option<String>,
    },
    Release {
        action: String,
        tag_name: Option<String>,
        name: Option<String>,
        html_url: Option<String>,
    },
    Public,
    Member {
        action: String,
        member_login: Option<String>,
    },
    /// The payload was intentionally dropped because the type is not recognised.
    Unknown,
}

/// One received event. No field is a URL that the adapter will ever fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEvent {
    pub id: String,
    pub event_type: FeedEventType,
    pub actor: FeedActor,
    pub repository: FeedRepository,
    pub created_at: Option<String>,
    pub public: bool,
    pub payload: FeedPayload,
}

/// Why a Feed result is only partial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedPartialReason {
    PageFailed { message: String },
    PageLimit,
    EntryLimit,
    ResponseLimit,
    ParseFailed { message: String },
    UntrustedPagination,
    InvalidPagination,
    PaginationCycle,
    SsoPartial { organizations: Vec<String> },
}

/// Completeness of a Feed result. Events are only available for the last 30
/// days and at most 300 entries; "Complete" never means a full history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedReadiness {
    Complete,
    Partial { reasons: Vec<FeedPartialReason> },
}

impl FeedReadiness {
    pub(crate) fn from_reasons(reasons: Vec<FeedPartialReason>) -> Self {
        if reasons.is_empty() {
            Self::Complete
        } else {
            Self::Partial { reasons }
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// The three received-event activities that become Feed cards (owner decision,
/// 2026-10-30: "先实现三类，关注两类暂缓").
///
/// Push, issue/comment, release, fork, membership, public, branch/tag
/// create-or-delete, non-merged pull requests and unknown types are technical
/// noise for this page: they must not produce a card, a placeholder or a
/// fallback sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedActivityKind {
    /// A pull request was merged: `<author> contributed to <owner>/<repo>`.
    PullRequestMerged,
    /// `WatchEvent` with `action=started` (starring a repository, not a watch
    /// subscription).
    RepositoryStarred,
    /// `CreateEvent` with `ref_type=repository` (branch/tag creation excluded).
    RepositoryCreated,
}

/// One retained Feed activity, derived from a received event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedActivity {
    pub kind: FeedActivityKind,
    /// Whom the card credits and links. This is the event actor, except for a
    /// merged pull request where it is the payload's pull-request author.
    pub subject: FeedActor,
    /// Pull-request target, present only for [`FeedActivityKind::PullRequestMerged`].
    pub pull_request: Option<FeedActivityTarget>,
}

/// The pull request an activity refers to, when the event payload has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedActivityTarget {
    pub number: Option<u64>,
    pub title: Option<String>,
}

/// Product policy for the Feed column: which received events become cards.
///
/// This is deliberately a pure function of the already-parsed event, so the
/// adapter, the account-scoped data facade, fixtures and the view apply exactly
/// one rule. Filtering is a product decision, not a data-quality problem: an
/// empty result after filtering is a normal empty Feed, never a partial page or
/// an error.
///
/// Merge evidence follows the official Events API `PullRequestEvent` shape: a
/// merge is reported as `action=closed` with `pull_request.merged=true`. There
/// is no `action=merged`, so `closed` alone never qualifies, and an
/// already-`merged` payload that does not say `closed` is contradictory.
pub fn classify_feed_activity(event: &FeedEvent) -> Option<FeedActivity> {
    match (&event.event_type, &event.payload) {
        (
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                action,
                number,
                title,
                merged: Some(true),
                author: Some(author),
                ..
            },
        ) if action == "closed" && !author.login.trim().is_empty() => Some(FeedActivity {
            kind: FeedActivityKind::PullRequestMerged,
            subject: FeedActor {
                id: author.id,
                login: author.login.clone(),
                display_login: None,
                avatar_url: author.avatar_url.clone(),
            },
            pull_request: Some(FeedActivityTarget {
                number: *number,
                title: title
                    .as_ref()
                    .map(|title| title.trim())
                    .filter(|title| !title.is_empty())
                    .map(str::to_string),
            }),
        }),
        (FeedEventType::Watch, FeedPayload::Watch { action }) if action == "started" => {
            Some(FeedActivity {
                kind: FeedActivityKind::RepositoryStarred,
                subject: event.actor.clone(),
                pull_request: None,
            })
        }
        (FeedEventType::Create, FeedPayload::Create { ref_type, .. })
            if ref_type.as_deref() == Some("repository") =>
        {
            Some(FeedActivity {
                kind: FeedActivityKind::RepositoryCreated,
                subject: event.actor.clone(),
                pull_request: None,
            })
        }
        _ => None,
    }
}

impl FeedOutcome {
    /// Keep only product activities, preserving source order and real event
    /// identity.
    ///
    /// Filtering is not truncation: `readiness` is left untouched, so a Feed whose
    /// whole page was technical noise stays `Complete` and reads as a normal empty
    /// Feed, while a genuinely truncated source read stays `Partial`. The retained
    /// list is what the page shows and what selection indices refer to.
    pub fn filtered(mut self) -> Self {
        self.events
            .retain(|event| classify_feed_activity(event).is_some());
        self
    }
}

/// A successful Feed fetch. `etag` and `poll_interval_seconds` are surfaced so
/// the caller can schedule its own refresh; this adapter never polls itself.
/// `events` contains retained activities only, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedOutcome {
    pub events: Vec<FeedEvent>,
    pub etag: Option<String>,
    pub poll_interval_seconds: Option<u64>,
    pub readiness: FeedReadiness,
}

/// Outcome of a Feed request. `NotModified` is a distinct, non-error answer to
/// a conditional request; it still carries the new poll hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedFetch {
    Events(FeedOutcome),
    NotModified {
        etag: Option<String>,
        poll_interval_seconds: Option<u64>,
    },
}
