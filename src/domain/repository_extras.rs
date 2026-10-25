//! Bounded observations for the repository top information bar.
//!
//! These are GitHub observations, not live counts or cached authorization
//! proofs. Raw release/contributor fields are retained for forward-compatible
//! presentation; their URLs are data and must never be executed or downloaded
//! by this low-level reader.

use crate::domain::error::GitHubError;
use crate::domain::repository::{RepositoryError, RepositoryIdentity};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RepositoryExtrasError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error("invalid repository extras response: {message}")]
    InvalidResponse { message: &'static str },
    #[error("invalid repository extras limits: {0}")]
    InvalidLimits(&'static str),
    #[error("language byte total exceeds u64")]
    LanguageByteOverflow,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExtrasCompleteness {
    Complete,
    Partial {
        reasons: Vec<ExtrasIncompleteReason>,
    },
}

impl ExtrasCompleteness {
    pub(crate) fn from_reasons(reasons: Vec<ExtrasIncompleteReason>) -> Self {
        if reasons.is_empty() {
            Self::Complete
        } else {
            Self::Partial { reasons }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExtrasIncompleteReason {
    /// A later page failed. The original typed error is retained.
    Failed(RepositoryExtrasError),
    PageLimit {
        limit: usize,
    },
    EntryLimit {
        limit: usize,
    },
    ResponseLimit {
        limit: usize,
    },
    UntrustedPagination,
    InvalidPagination,
    PaginationCycle,
    SsoPartial {
        organizations: Vec<String>,
    },
}

/// HTTP cache/revalidation metadata for one response, not a freshness promise.
/// List readers keep one value per successfully received page: an ETag for one
/// page cannot validate an aggregate assembled from several responses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseMetadata {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cache_control: Option<String>,
    pub poll_interval_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub id: u64,
    pub node_id: Option<String>,
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub html_url: Option<String>,
    pub created_at: Option<String>,
    pub published_at: Option<String>,
    /// Server-provided visibility/selection fields, never inferred or filtered.
    pub draft: bool,
    pub prerelease: bool,
    /// Complete JSON object, including assets, author, reactions and fields not
    /// yet projected above. Asset URLs are not fetched by this service.
    pub raw: serde_json::Value,
}

impl Release {
    pub(crate) fn from_json(raw: serde_json::Value) -> Result<Self, RepositoryExtrasError> {
        #[derive(serde::Deserialize)]
        struct Fields {
            id: u64,
            node_id: Option<String>,
            tag_name: String,
            name: Option<String>,
            body: Option<String>,
            html_url: Option<String>,
            created_at: Option<String>,
            published_at: Option<String>,
            draft: bool,
            prerelease: bool,
        }
        let fields: Fields = serde_json::from_value(raw.clone()).map_err(|_| {
            RepositoryExtrasError::InvalidResponse {
                message: "invalid release object",
            }
        })?;
        Ok(Self {
            id: fields.id,
            node_id: fields.node_id,
            tag_name: fields.tag_name,
            name: fields.name,
            body: fields.body,
            html_url: fields.html_url,
            created_at: fields.created_at,
            published_at: fields.published_at,
            draft: fields.draft,
            prerelease: fields.prerelease,
            raw,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReleasesOutcome {
    pub repository: RepositoryIdentity,
    pub releases: Vec<Release>,
    pub completeness: ExtrasCompleteness,
    pub page_metadata: Vec<ResponseMetadata>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LatestReleaseOutcome {
    pub repository: RepositoryIdentity,
    /// `None` means GitHub returned 404: no qualifying release is visible, or
    /// the release/repository is not visible to this authorization. It does not
    /// prove that no release exists. Other errors are not converted to `None`.
    pub release: Option<Release>,
    pub metadata: ResponseMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContributorKind {
    GitHubUser,
    Anonymous,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contributor {
    pub login: Option<String>,
    pub id: Option<u64>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub html_url: Option<String>,
    pub contributions: u64,
    pub raw: serde_json::Value,
}

impl Contributor {
    pub fn kind(&self) -> ContributorKind {
        if self.id.is_some() || self.login.is_some() {
            ContributorKind::GitHubUser
        } else {
            ContributorKind::Anonymous
        }
    }

    pub(crate) fn from_json(raw: serde_json::Value) -> Result<Self, RepositoryExtrasError> {
        #[derive(serde::Deserialize)]
        struct Fields {
            login: Option<String>,
            id: Option<u64>,
            name: Option<String>,
            email: Option<String>,
            avatar_url: Option<String>,
            html_url: Option<String>,
            contributions: u64,
        }
        let fields: Fields = serde_json::from_value(raw.clone()).map_err(|_| {
            RepositoryExtrasError::InvalidResponse {
                message: "invalid contributor object",
            }
        })?;
        Ok(Self {
            login: fields.login,
            id: fields.id,
            name: fields.name,
            email: fields.email,
            avatar_url: fields.avatar_url,
            html_url: fields.html_url,
            contributions: fields.contributions,
            raw,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContributorsState {
    /// HTTP 200 with a JSON array, including a legitimately empty array.
    Content,
    /// HTTP 204: GitHub explicitly reports that the repository is empty.
    EmptyRepository,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContributorsOutcome {
    pub repository: RepositoryIdentity,
    pub contributors: Vec<Contributor>,
    pub state: ContributorsState,
    pub completeness: ExtrasCompleteness,
    pub page_metadata: Vec<ResponseMetadata>,
}

impl ContributorsOutcome {
    /// Number of observed entries only. A short first page, a partial result or
    /// GitHub's 500-email association cutoff is not a complete contributor count.
    pub fn observed_entries(&self) -> usize {
        self.contributors.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LanguageStat {
    pub name: String,
    /// GitHub's byte count, not lines of code.
    pub bytes: u64,
    /// `None` only when the total is zero; otherwise in the inclusive 0..=1 range.
    pub share: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LanguagesOutcome {
    pub repository: RepositoryIdentity,
    pub languages: Vec<LanguageStat>,
    pub total_bytes: u64,
    pub metadata: ResponseMetadata,
}

impl LanguagesOutcome {
    pub(crate) fn from_bytes(
        repository: RepositoryIdentity,
        bytes: serde_json::Map<String, serde_json::Value>,
        metadata: ResponseMetadata,
    ) -> Result<Self, RepositoryExtrasError> {
        let mut raw = Vec::with_capacity(bytes.len());
        for (name, value) in bytes {
            let count = value
                .as_u64()
                .ok_or(RepositoryExtrasError::InvalidResponse {
                    message: "language byte count is not an unsigned integer",
                })?;
            raw.push((name, count));
        }
        raw.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let total_bytes = raw.iter().try_fold(0u64, |total, (_, count)| {
            total
                .checked_add(*count)
                .ok_or(RepositoryExtrasError::LanguageByteOverflow)
        })?;
        let languages = raw
            .into_iter()
            .map(|(name, bytes)| LanguageStat {
                name,
                bytes,
                share: (total_bytes != 0).then_some(bytes as f64 / total_bytes as f64),
            })
            .collect();
        Ok(Self {
            repository,
            languages,
            total_bytes,
            metadata,
        })
    }
}
