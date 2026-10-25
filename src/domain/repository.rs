//! Repository observations and immutable Git object identities. No UI or I/O.

use crate::domain::error::GitHubError;
use crate::domain::repo::{RepoOwner, RepoVisibility};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RepositoryError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error("invalid {field}")]
    InvalidInput { field: &'static str },
    #[error("invalid repository response: {message}")]
    InvalidResponse { message: String },
    #[error("Git data unavailable or repository empty (HTTP 409)")]
    EmptyOrUnavailable,
    #[error("response exceeds configured {resource} limit ({limit})")]
    LimitExceeded {
        resource: &'static str,
        limit: usize,
    },
    #[error("SSO returned only partial results")]
    SsoPartial { organizations: Vec<String> },
}

/// A validated full SHA-1, never a dynamic ref or arbitrary URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectSha(String);

impl ObjectSha {
    pub fn new(value: impl Into<String>) -> Result<Self, RepositoryError> {
        let value = value.into();
        if value.len() != 40 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(RepositoryError::InvalidInput { field: "SHA-1" });
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// API owner/name locator; segments are validated and encoded independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryLocator {
    owner: String,
    name: String,
}

impl RepositoryLocator {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Result<Self, RepositoryError> {
        let owner = owner.into();
        let name = name.into();
        for part in [&owner, &name] {
            if part.is_empty()
                || part == "."
                || part == ".."
                || part.contains(['/', '\\'])
                || part.chars().any(char::is_control)
            {
                return Err(RepositoryError::InvalidInput {
                    field: "owner/name",
                });
            }
        }
        Ok(Self { owner, name })
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryIdentity {
    pub(crate) id: u64,
    pub(crate) locator: RepositoryLocator,
}

impl RepositoryIdentity {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn locator(&self) -> &RepositoryLocator {
        &self.locator
    }
}

/// Values observed in this request, without a freshness/cache guarantee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryDetails {
    pub identity: RepositoryIdentity,
    pub owner: RepoOwner,
    pub description: Option<String>,
    pub visibility: RepoVisibility,
    pub default_branch: Option<DynamicRef>,
    pub fork: bool,
    pub archived: bool,
    pub stars: u64,
    pub forks: u64,
    /// REST open_issues_count includes pull requests.
    pub open_issues_and_prs: u64,
    pub subscribers: Option<u64>,
    pub size_kb: u64,
    pub html_url: String,
    pub clone_url: Option<String>,
    pub pushed_at: Option<String>,
}

/// Mutable reference. Slashes, spaces and Unicode are encoded as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicRef(String);

impl DynamicRef {
    pub fn new(value: impl Into<String>) -> Result<Self, RepositoryError> {
        let value = value.into();
        if value.is_empty()
            || value.chars().any(char::is_control)
            || value.split('/').any(|part| part == "." || part == "..")
        {
            return Err(RepositoryError::InvalidInput { field: "ref" });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionSpec {
    Ref(DynamicRef),
    Commit(ObjectSha),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: DynamicRef,
    /// Observed pointer only; use RevisionSpec::Commit to open this exact value.
    pub observed_commit: ObjectSha,
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncompleteReason {
    Failed(RepositoryError),
    PageLimit,
    EntryLimit,
    ResponseLimit,
    UntrustedPagination,
    InvalidPagination,
    PaginationCycle,
    SsoPartial { organizations: Vec<String> },
    ServerTruncated,
    UnknownEntryType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completeness {
    Complete,
    Partial { reasons: Vec<IncompleteReason> },
}

impl Completeness {
    pub(crate) fn from_reasons(reasons: Vec<IncompleteReason>) -> Self {
        if reasons.is_empty() {
            Self::Complete
        } else {
            Self::Partial { reasons }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchesOutcome {
    pub repository: RepositoryIdentity,
    pub branches: Vec<Branch>,
    pub completeness: Completeness,
}

/// Raw Git signature, not GitHub's mutable association with a user account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSignature {
    pub name: Option<String>,
    pub email: Option<String>,
    pub date: Option<String>,
}

/// Constructed only by revision resolution; tree/README readers accept this
/// instead of a branch name. Account/authorization checks still belong to data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSnapshot {
    pub(crate) repository: RepositoryIdentity,
    pub(crate) requested: RevisionSpec,
    pub(crate) commit: ObjectSha,
    pub(crate) root_tree: ObjectSha,
    pub(crate) message: String,
    pub(crate) author: Option<GitSignature>,
    pub(crate) committer: Option<GitSignature>,
}

impl RepoSnapshot {
    pub fn repository(&self) -> &RepositoryIdentity {
        &self.repository
    }
    pub fn requested(&self) -> &RevisionSpec {
        &self.requested
    }
    pub fn commit(&self) -> &ObjectSha {
        &self.commit
    }
    pub fn root_tree(&self) -> &ObjectSha {
        &self.root_tree
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn author(&self) -> Option<&GitSignature> {
        self.author.as_ref()
    }
    pub fn committer(&self) -> Option<&GitSignature> {
        self.committer.as_ref()
    }
}

/// Validated repository-relative path; not a filesystem path or a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPath(String);

impl RepositoryPath {
    pub fn new(value: impl Into<String>) -> Result<Self, RepositoryError> {
        let value = value.into();
        if value.is_empty()
            || value.contains('\\')
            || value.chars().any(char::is_control)
            || value
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(RepositoryError::InvalidInput {
                field: "repository path",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeEntryKind {
    File { executable: bool },
    Directory,
    Symlink,
    Submodule,
    Unknown { git_type: String, mode: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: RepositoryPath,
    pub sha: ObjectSha,
    pub size: Option<u64>,
    pub kind: TreeEntryKind,
}

/// Opaque directory handle derived from the snapshot root or a returned tree.
/// Keeps the repository, commit, path and tree bound together for expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDirectory {
    pub(crate) repository: RepositoryIdentity,
    pub(crate) commit: ObjectSha,
    pub(crate) tree: ObjectSha,
    pub(crate) path: Option<RepositoryPath>,
}

impl SnapshotDirectory {
    pub fn root(snapshot: &RepoSnapshot) -> Self {
        Self {
            repository: snapshot.repository.clone(),
            commit: snapshot.commit.clone(),
            tree: snapshot.root_tree.clone(),
            path: None,
        }
    }
    pub fn repository(&self) -> &RepositoryIdentity {
        &self.repository
    }
    pub fn commit(&self) -> &ObjectSha {
        &self.commit
    }
    pub fn tree(&self) -> &ObjectSha {
        &self.tree
    }
    pub fn path(&self) -> Option<&RepositoryPath> {
        self.path.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryTree {
    pub directory: SnapshotDirectory,
    pub entries: Vec<TreeEntry>,
    /// None means the server did not provide a truncation assertion.
    pub truncated: Option<bool>,
    pub completeness: Completeness,
}

impl DirectoryTree {
    pub fn child_directory(&self, index: usize) -> Result<SnapshotDirectory, RepositoryError> {
        let entry = self
            .entries
            .get(index)
            .ok_or(RepositoryError::InvalidInput {
                field: "tree index",
            })?;
        if entry.kind != TreeEntryKind::Directory {
            return Err(RepositoryError::InvalidInput {
                field: "directory entry",
            });
        }
        let path = match &self.directory.path {
            Some(parent) => {
                RepositoryPath::new(format!("{}/{}", parent.as_str(), entry.path.as_str()))?
            }
            None => entry.path.clone(),
        };
        Ok(SnapshotDirectory {
            repository: self.directory.repository.clone(),
            commit: self.directory.commit.clone(),
            tree: entry.sha.clone(),
            path: Some(path),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadmeDescriptor {
    pub repository: RepositoryIdentity,
    pub commit: ObjectSha,
    pub path: RepositoryPath,
    pub blob: ObjectSha,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadmeDiscovery {
    Found(ReadmeDescriptor),
    /// A 404 alone cannot distinguish absence, lost access or a missing ref.
    NotFoundOrInaccessible {
        repository: RepositoryIdentity,
        commit: ObjectSha,
    },
    /// Only emitted when a successfully read, complete empty root tree proves
    /// there are no files in this commit (not inferred from a 404).
    AbsentInEmptyTree {
        repository: RepositoryIdentity,
        commit: ObjectSha,
    },
}
