//! Home/Feed read models: a parameterized `involves:` search result and a
//! received-events feed.
//!
//! These types carry no credentials and perform no I/O. Home is confirmed as
//! `involves:<login> is:open` with no default time window; callers supply an
//! explicit login and, optionally, an explicit `updated` window. Feed is a
//! **filtered** set of received activities whose subject is a followed account:
//! [`build_feed`] is the single policy function. Neither list defines an
//! "unread" meaning, a review-requested relationship or a browse-history
//! interpretation. Truncation, unknown membership and unresolved authorship are
//! represented by readiness, never by a silently complete-looking empty list.

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

/// Which received-event activities become Feed cards (owner decision,
/// 2026-10-30, refined after live-data review).
///
/// **Every** retained card's subject must be an account the active account
/// currently follows. Push, issue/comment, release, fork, membership, public,
/// branch/tag create-or-delete, non-merged pull requests and unknown types are
/// technical noise: they must not produce a card, a placeholder or a fallback
/// sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedActivityKind {
    /// A pull request was merged: `<author> contributed to <owner>/<repo>`, where
    /// the subject is the pull request's **author**, not the actor who merged it.
    PullRequestMerged,
    /// `WatchEvent` with `action=started` (starring a repository, not a watch
    /// subscription). Subject: the event actor.
    RepositoryStarred,
    /// `CreateEvent` with `ref_type=repository` (branch/tag creation excluded).
    /// Subject: the event actor.
    RepositoryCreated,
}

/// One retained Feed activity: the product decision plus the event it came from.
///
/// The original [`FeedEvent`] is kept whole so identity, timestamp, repository
/// and event type stay exactly as received; nothing is re-derived from display
/// strings. `subject` is already the validated, followed account, so the view
/// never needs to re-run membership logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedActivity {
    pub kind: FeedActivityKind,
    /// The followed account that the card credits and links.
    pub subject: FeedActor,
    /// Pull-request target, present only for [`FeedActivityKind::PullRequestMerged`].
    pub pull_request: Option<FeedActivityTarget>,
    /// The received event this activity was derived from (source order, real id).
    pub event: FeedEvent,
}

/// The pull request an activity refers to, when the event payload has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedActivityTarget {
    pub number: Option<u64>,
    pub title: Option<String>,
    pub html_url: Option<String>,
}

/// A merged pull request that still needs its author resolved.
///
/// A live `PullRequestEvent` with `action=merged` carries only a slim
/// `pull_request` object (url/id/number/head/base) without `user`, so the author
/// must come from the pull-request detail endpoint. This value carries the
/// trusted, locally-derived coordinates for that read; the payload's own `url`
/// is deliberately not stored for fetching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedMergeCandidate {
    /// Repository owner and name from the event's `repo.name`.
    pub owner: String,
    pub repository: String,
    pub pull_request: FeedActivityTarget,
    /// The payload's embedded pull-request author, when the source supplied one.
    pub embedded_author: Option<FeedAuthor>,
}

impl FeedMergeCandidate {
    /// Cross-event de-duplication key for author lookups.
    pub fn key(&self) -> String {
        format!(
            "{}/{}#{}",
            self.owner.to_ascii_lowercase(),
            self.repository.to_ascii_lowercase(),
            self.pull_request
                .number
                .map_or_else(|| "?".to_string(), |number| number.to_string())
        )
    }
}

/// Local merge evidence for one event.
///
/// Official `PullRequestEvent.action` values include `merged`, so a live merge
/// arrives as `action=merged`; an older/other source may instead report
/// `action=closed` with `pull_request.merged=true`. Both are accepted, and
/// `closed` **without** merge evidence is not a merge. `action=closed` with an
/// absent `merged` flag is treated as unknown rather than as a merge: it neither
/// qualifies nor triggers a detail read.
pub fn merge_candidate(event: &FeedEvent) -> Option<FeedMergeCandidate> {
    let (
        FeedEventType::PullRequest,
        FeedPayload::PullRequest {
            action,
            number,
            title,
            html_url,
            merged,
            author,
            ..
        },
    ) = (&event.event_type, &event.payload)
    else {
        return None;
    };
    let merge_evidence = action == "merged" || (action == "closed" && *merged == Some(true));
    if !merge_evidence {
        return None;
    }
    let (owner, repository) = event
        .repository
        .name
        .split_once('/')
        .map_or(("", event.repository.name.as_str()), |(owner, name)| {
            (owner, name)
        });
    if owner.trim().is_empty() || repository.trim().is_empty() {
        // Without a usable repository identity the author cannot be resolved and
        // the card target cannot be linked.
        return None;
    }
    Some(FeedMergeCandidate {
        owner: owner.to_string(),
        repository: repository.to_string(),
        pull_request: FeedActivityTarget {
            number: *number,
            title: title
                .as_ref()
                .map(|title| title.trim())
                .filter(|title| !title.is_empty())
                .map(str::to_string),
            html_url: html_url.clone(),
        },
        embedded_author: author.clone(),
    })
}

/// A non-merge retained activity plus its subject, decided locally.
///
/// Star and repository-create cards credit the event actor, so no extra read is
/// needed; membership is applied by the caller.
pub fn local_activity(event: &FeedEvent) -> Option<(FeedActivityKind, FeedActor)> {
    match (&event.event_type, &event.payload) {
        // Starring a repository, not watching it.
        (FeedEventType::Watch, FeedPayload::Watch { action }) if action == "started" => {
            Some((FeedActivityKind::RepositoryStarred, event.actor.clone()))
        }
        // Repository creation only; branch/tag creation is excluded.
        (FeedEventType::Create, FeedPayload::Create { ref_type, .. })
            if ref_type.as_deref() == Some("repository") =>
        {
            Some((FeedActivityKind::RepositoryCreated, event.actor.clone()))
        }
        _ => None,
    }
}

/// Whether the Feed product policy retains this event at all, ignoring
/// membership. Used to decide which events are worth resolving.
pub fn is_retained_activity(event: &FeedEvent) -> bool {
    merge_candidate(event).is_some() || local_activity(event).is_some()
}

/// One account the active account follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowedAccount {
    pub id: u64,
    pub login: String,
    pub avatar_url: Option<String>,
}

/// Why the followed-account list is only partial. Any entry means membership is
/// **unknown** for subjects that are absent from the list, so those subjects must
/// not be displayed and the Feed must not be reported as a complete empty page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowingPartialReason {
    PageFailed { message: String },
    PageLimit,
    AccountLimit,
    ResponseLimit,
    ParseFailed { message: String },
    UntrustedPagination,
    InvalidPagination,
    PaginationCycle,
    SsoPartial { organizations: Vec<String> },
}

/// Completeness of the followed-account list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowingReadiness {
    Complete,
    Partial {
        reasons: Vec<FollowingPartialReason>,
    },
}

impl FollowingReadiness {
    pub(crate) fn from_reasons(reasons: Vec<FollowingPartialReason>) -> Self {
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

/// The active account's followed accounts.
///
/// `accounts` is the set that was actually read; with a `Partial` readiness it is
/// only a subset, so [`Following::follows`] returning `false` means "not in the
/// list we could read", not "does not follow".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Following {
    pub accounts: Vec<FollowedAccount>,
    pub readiness: FollowingReadiness,
}

impl Following {
    /// Login comparison is ASCII case-insensitive because GitHub logins are.
    pub fn follows(&self, login: &str) -> bool {
        let login = login.trim();
        !login.is_empty()
            && self
                .accounts
                .iter()
                .any(|account| account.login.eq_ignore_ascii_case(login))
    }

    /// Match stable IDs when both endpoints supplied them, so renames cannot
    /// transfer membership to a different account that later takes the old login.
    pub fn follows_subject(&self, subject: &FeedActor) -> bool {
        if subject.id != 0 {
            self.accounts.iter().any(|account| account.id == subject.id)
        } else {
            self.follows(&subject.login)
        }
    }

    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }
}

/// Result of resolving one merged pull request's author.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullRequestDetailOutcome {
    /// The detail endpoint supplied the author (and possibly a title/URL).
    Attributed(ResolvedPullRequest),
    /// The read succeeded but identified no author. The merger must never be
    /// substituted, so this contribution is not shown.
    Unattributable,
}

/// The identity a merged contribution needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPullRequest {
    pub author: FeedAuthor,
    /// `true` only when the detail response itself says `merged: true`.
    pub merged: bool,
    pub title: Option<String>,
    pub html_url: Option<String>,
}

/// One pull-request detail read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestDetail {
    pub id: u64,
    pub number: u64,
    pub state: Option<String>,
    pub merged: bool,
    pub outcome: PullRequestDetailOutcome,
}

/// Why the Feed is only partial. Beyond the event-source reasons this includes
/// the followed-relationship read and author enrichment, because either can leave
/// real contributions unresolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedPartialReason {
    PageFailed {
        message: String,
    },
    PageLimit,
    EntryLimit,
    ResponseLimit,
    ParseFailed {
        message: String,
    },
    UntrustedPagination,
    InvalidPagination,
    PaginationCycle,
    SsoPartial {
        organizations: Vec<String>,
    },
    /// The followed-account list could not be read completely, so some subjects'
    /// membership is unknown and those cards were withheld.
    FollowingIncomplete {
        details: Vec<String>,
    },
    /// One or more merged pull requests could not have their author resolved.
    AuthorUnresolved {
        count: usize,
    },
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

    pub fn reasons(&self) -> &[FeedPartialReason] {
        match self {
            Self::Complete => &[],
            Self::Partial { reasons } => reasons,
        }
    }
}

/// Author lookups for merged pull requests, keyed by
/// [`FeedMergeCandidate::key`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorResolutions {
    entries: Vec<(String, Option<ResolvedPullRequest>)>,
}

impl AuthorResolutions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a resolved author. A later resolution for the same key replaces an
    /// earlier one so a refresh cannot keep a stale author.
    pub fn insert(&mut self, key: impl Into<String>, value: ResolvedPullRequest) {
        self.set(key, Some(value));
    }

    /// Record that the author could not be resolved (read failure, missing user,
    /// or budget exhaustion).
    pub fn mark_unresolved(&mut self, key: impl Into<String>) {
        self.set(key, None);
    }

    fn set(&mut self, key: impl Into<String>, value: Option<ResolvedPullRequest>) {
        let key = key.into();
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == &key)
        {
            entry.1 = value;
            return;
        }
        self.entries.push((key, value));
    }

    pub fn get(&self, key: &str) -> Option<&Option<ResolvedPullRequest>> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Build the visible Feed from the received events, the followed-account list and
/// the resolved pull-request authors.
///
/// This is the single place the product policy is applied, so every data path —
/// first load, refresh, 304 replay and retained snapshots — produces the same
/// result:
///
/// 1. keep only the three retained activity kinds;
/// 2. for a merged pull request take the **author** (embedded or resolved), never
///    the merging actor; without a reliable author the contribution is withheld;
/// 3. keep a card only when its subject is in the followed-account list. When that
///    list is incomplete, absence is unknown rather than "not followed", so the
///    card is withheld and the outcome is reported partial.
///
/// Source order, real event identity and the source's own readiness reasons are
/// preserved. A policy-only empty result is `Complete`; unknown membership or an
/// unresolved author is never reported as a complete empty Feed.
pub fn build_feed(
    events: Vec<FeedEvent>,
    source_reasons: Vec<FeedPartialReason>,
    following: &Following,
    authors: &AuthorResolutions,
) -> FeedOutcome {
    let mut activities: Vec<FeedActivity> = Vec::new();
    let mut unresolved = 0usize;
    for event in events {
        if let Some(candidate) = merge_candidate(&event) {
            let resolved = candidate
                .embedded_author
                .clone()
                .map(|author| ResolvedPullRequest {
                    author,
                    // Local merge evidence already established the merge.
                    merged: true,
                    title: candidate.pull_request.title.clone(),
                    html_url: candidate.pull_request.html_url.clone(),
                })
                .or_else(|| authors.get(&candidate.key()).cloned().flatten());
            let Some(resolved) = resolved else {
                unresolved += 1;
                continue;
            };
            if !resolved.merged || resolved.author.login.trim().is_empty() {
                // Contradictory/missing detail evidence is unresolved, not proof
                // of a complete empty Feed.
                unresolved += 1;
                continue;
            }
            let subject = FeedActor {
                id: resolved.author.id,
                login: resolved.author.login.clone(),
                display_login: None,
                avatar_url: resolved.author.avatar_url.clone(),
            };
            if !following.follows_subject(&subject) {
                continue;
            }
            activities.push(FeedActivity {
                kind: FeedActivityKind::PullRequestMerged,
                subject,
                pull_request: Some(FeedActivityTarget {
                    number: candidate.pull_request.number,
                    title: resolved.title.or(candidate.pull_request.title),
                    html_url: resolved.html_url.or(candidate.pull_request.html_url),
                }),
                event,
            });
            continue;
        }
        let Some((kind, subject)) = local_activity(&event) else {
            continue;
        };
        if !following.follows_subject(&subject) {
            continue;
        }
        activities.push(FeedActivity {
            kind,
            subject,
            pull_request: None,
            event,
        });
    }

    let mut reasons = source_reasons;
    if let FollowingReadiness::Partial {
        reasons: following_reasons,
    } = &following.readiness
    {
        reasons.push(FeedPartialReason::FollowingIncomplete {
            details: following_reasons.iter().map(following_reason).collect(),
        });
    }
    if unresolved > 0 {
        reasons.push(FeedPartialReason::AuthorUnresolved { count: unresolved });
    }

    FeedOutcome {
        activities,
        etag: None,
        poll_interval_seconds: None,
        readiness: FeedReadiness::from_reasons(reasons),
    }
}

fn following_reason(reason: &FollowingPartialReason) -> String {
    match reason {
        FollowingPartialReason::PageFailed { .. } => {
            "a later followed-accounts page failed".to_string()
        }
        FollowingPartialReason::PageLimit => "the followed-accounts page budget was reached".into(),
        FollowingPartialReason::AccountLimit => {
            "the followed-accounts budget was reached".to_string()
        }
        FollowingPartialReason::ResponseLimit => {
            "a followed-accounts response exceeded its size budget".to_string()
        }
        FollowingPartialReason::ParseFailed { .. } => {
            "a followed-accounts page could not be parsed".to_string()
        }
        FollowingPartialReason::UntrustedPagination => {
            "an untrusted followed-accounts pagination link was rejected".to_string()
        }
        FollowingPartialReason::InvalidPagination => {
            "a followed-accounts pagination link was invalid".to_string()
        }
        FollowingPartialReason::PaginationCycle => {
            "a followed-accounts pagination cycle was stopped".to_string()
        }
        FollowingPartialReason::SsoPartial { organizations } => format!(
            "SSO authorization is missing for {}",
            if organizations.is_empty() {
                "some organizations".to_string()
            } else {
                organizations.join(", ")
            }
        ),
    }
}

/// A successful Feed fetch: the visible, followed-subject activities plus the
/// scheduling metadata of the underlying events read. `activities` is in source
/// order and already policy-filtered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedOutcome {
    pub activities: Vec<FeedActivity>,
    pub etag: Option<String>,
    pub poll_interval_seconds: Option<u64>,
    pub readiness: FeedReadiness,
}

/// One raw events read, before the product policy is applied.
///
/// Keeping this shape separate from [`FeedOutcome`] makes it impossible to
/// publish an unfiltered source list as Feed content: only
/// [`build_feed`] can produce a [`FeedOutcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEventsRead {
    /// Every parsed source event in server order, deduplicated by event id.
    pub events: Vec<FeedEvent>,
    pub etag: Option<String>,
    pub poll_interval_seconds: Option<u64>,
    /// Readiness of the **source read** only; it says nothing about membership or
    /// author resolution.
    pub readiness: FeedReadiness,
}

impl FeedEventsRead {
    /// Source-level partial reasons, to be carried into the built outcome.
    pub fn source_reasons(&self) -> Vec<FeedPartialReason> {
        self.readiness.reasons().to_vec()
    }
}

/// Outcome of a raw Feed request. `NotModified` is a distinct, non-error answer
/// to a conditional request; it still carries the new poll hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedFetch {
    Events(FeedEventsRead),
    NotModified {
        etag: Option<String>,
        poll_interval_seconds: Option<u64>,
    },
}

#[cfg(test)]
mod feed_policy_tests;
