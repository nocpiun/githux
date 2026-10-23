//! Repository summary and list outcome types produced by `/user/repos`.

/// Visibility classification for a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoVisibility {
    Public,
    Internal,
    Private,
}

/// Minimal repository owner needed for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoOwner {
    pub id: u64,
    pub login: String,
    pub avatar_url: Option<String>,
}

/// Stable repository summary parsed from `/user/repos`. Only the fields needed
/// for the first-release repository list are captured; future pages can extend
/// parsing without breaking the type contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    pub id: u64,
    pub full_name: String,
    pub name: String,
    pub owner: RepoOwner,
    pub visibility: RepoVisibility,
    pub fork: bool,
    pub archived: bool,
    pub description: Option<String>,
    pub default_branch: Option<String>,
}

/// Whether a repository list represents the full visible set, a partial set
/// (mid-pagination failure or SSO partial-results), or failed outright.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReposReadiness {
    /// All pages returned successfully and the server indicated the list is
    /// complete (no remaining `next` link).
    Complete,
    /// Some pages returned data but a later page failed, or the server returned
    /// `X-GitHub-SSO: partial-results`. Already-fetched items are still usable,
    /// but must never be presented as the complete set.
    Partial { reason: PartialReason },
}

/// Why a repository list is only partial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartialReason {
    /// A page request failed after earlier pages succeeded.
    PageFailed { message: String },
    /// `X-GitHub-SSO: partial-results; organizations=...` — one or more
    /// organizations require SSO authorization and were silently omitted.
    SsoPartial { organizations: Vec<String> },
}

/// Outcome of listing repositories for the active account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReposOutcome {
    pub repositories: Vec<RepoSummary>,
    pub readiness: ReposReadiness,
}
