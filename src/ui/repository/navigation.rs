//! Pure browsing state for the repository page.
//!
//! Nothing here performs I/O, touches GPUI or holds a credential. The page
//! controller owns authorization and request generations; this module owns the
//! navigation intent a host may save (branch, selected path, expanded
//! directories), the lazily loaded directory model, and the honest presentation
//! of read failures. Keeping it separate is what lets a rebind restore intent
//! without restoring an old `RepositorySession` or its data.
//!
//! Failure descriptions deliberately carry no scope: the *page* decides where a
//! failure belongs (page, one directory, the reader) and stores it there. A 404
//! therefore never claims "this repository is not visible" when it was only one
//! blob that could not be read.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::error::GitHubError;
use crate::domain::repository::{
    BranchesOutcome, Completeness, DirectoryTree, DynamicRef, IncompleteReason, ReadmeDescriptor,
    ReadmeDiscovery, RepositoryError, SnapshotDirectory, TreeEntryKind,
};

/// How many loaded rows may be flattened into the tree viewport at once.
///
/// This bounds the row vector, not the network work: a directory is still only
/// fetched when the user opens it. A repository larger than this keeps its
/// loaded data but the visible list stops at the cap, which the page states.
pub const MAX_TREE_ROWS: usize = 20_000;

/// Deepest directory nesting the flat row builder will walk.
pub const MAX_TREE_DEPTH: usize = 64;

/// What a host should save to restore this page later.
///
/// It deliberately has no commit, repository id, service instance or
/// authorization field: a restored page must re-resolve fresh authorization and
/// then move the saved branch to a fixed commit again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryNavigation {
    /// Branch name, not a commit. A restored page re-resolves it.
    pub branch: Option<String>,
    /// Repository-relative file path, if a file was selected.
    pub path: Option<String>,
    /// Expanded directory paths (`""` is not stored; the root is always shown).
    pub expanded: BTreeSet<String>,
}

impl RepositoryNavigation {
    /// The paths from the repository root down to `path`'s parent.
    pub fn ancestors(path: &str) -> Vec<String> {
        let segments: Vec<&str> = path.split('/').collect();
        let mut out = Vec::new();
        let mut prefix = String::new();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            out.push(prefix.clone());
        }
        out
    }

    /// The directory holding `path`, or `""` for a root-level file.
    pub fn parent(path: &str) -> &str {
        path.rsplit_once('/').map_or("", |(dir, _)| dir)
    }

    /// The last segment of `path`.
    pub fn name(path: &str) -> &str {
        path.rsplit_once('/').map_or(path, |(_, name)| name)
    }
}

/// A failure worth showing the user, with the reason it happened.
///
/// It describes *one resource*. The page decides whether that resource is the
/// whole page, one directory, or the reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageError {
    pub title: String,
    pub detail: Option<String>,
    /// Whether trying the same request again could help. Never drives an
    /// automatic retry; the page only offers a button.
    pub retryable: bool,
}

impl PageError {
    pub fn new(title: impl Into<String>, detail: Option<String>, retryable: bool) -> Self {
        Self {
            title: title.into(),
            detail,
            retryable,
        }
    }

    /// Classifies a repository-layer error without claiming an empty result and
    /// without over-claiming why a resource is missing.
    pub fn from_repository(error: &RepositoryError) -> Self {
        match error {
            RepositoryError::GitHub(inner) => Self::from_github(inner),
            RepositoryError::EmptyOrUnavailable => Self::new(
                "This repository has no readable Git data.",
                Some(
                    "GitHub answered 409 for this commit. The repository may be empty, or its Git data may be unavailable to this account. This is not an empty directory listing."
                        .to_string(),
                ),
                true,
            ),
            RepositoryError::SsoPartial { organizations } => Self::new(
                "Organization SSO authorization is required.",
                Some(format!(
                    "GitHub could only return part of this response until the token is authorized for: {}.",
                    organizations.join(", ")
                )),
                true,
            ),
            RepositoryError::LimitExceeded { resource, limit } => Self::new(
                "This response is larger than the read limit.",
                Some(format!(
                    "GitHub's {resource} exceeded {limit} bytes; the listing was not treated as complete."
                )),
                false,
            ),
            RepositoryError::InvalidInput { field } => Self::new(
                "The request could not be built.",
                Some(format!(
                    "The {field} value was rejected before any request was sent."
                )),
                false,
            ),
            RepositoryError::InvalidResponse { message } => Self::new(
                "GitHub's response did not match the read contract.",
                Some(message.clone()),
                true,
            ),
        }
    }

    fn from_github(error: &GitHubError) -> Self {
        match error {
            GitHubError::Unauthorized => Self::new(
                "GitHub rejected this credential.",
                Some(
                    "The saved token is invalid or revoked (401). Replace or revalidate it; the account was not removed."
                        .to_string(),
                ),
                false,
            ),
            GitHubError::RateLimited { retry } => Self::new(
                "GitHub rate limited this request.",
                Some(format!(
                    "The {:?} limit is exhausted{}. This is not an empty result.",
                    retry.kind,
                    retry
                        .seconds
                        .map(|seconds| format!("; retry after about {seconds}s"))
                        .unwrap_or_default()
                )),
                true,
            ),
            GitHubError::SsoRequired { authorization_url } => Self::new(
                "Organization SSO authorization is required.",
                Some(format!(
                    "Authorize this token for the organization on GitHub{}.",
                    authorization_url
                        .as_deref()
                        .map(|url| format!(": {url}"))
                        .unwrap_or_default()
                )),
                true,
            ),
            GitHubError::Forbidden => Self::new(
                "GitHub denied access to this resource.",
                Some(
                    "The token scope or an organization policy denied the read (403); no empty result is inferred."
                        .to_string(),
                ),
                false,
            ),
            // A 404 is per-resource and ambiguous by contract: absent, or not
            // visible to this token. It never asserts which.
            GitHubError::NotFound => Self::new(
                "This resource was not found.",
                Some(
                    "GitHub answered 404: the file or repository does not exist, or this account cannot access it."
                        .to_string(),
                ),
                false,
            ),
            GitHubError::UnexpectedStatus { status } => Self::new(
                "GitHub returned an unexpected status.",
                Some(format!("HTTP {status} is not a successful read.")),
                true,
            ),
            GitHubError::Network { .. } => Self::new(
                "The network request failed.",
                Some("Check connectivity and retry; no result was established.".to_string()),
                true,
            ),
            GitHubError::Timeout { seconds } => Self::new(
                "The request timed out.",
                Some(format!("GitHub did not answer within {seconds}s.")),
                true,
            ),
            GitHubError::Cancelled => Self::new(
                "The request was cancelled.",
                Some("Nothing from the cancelled request was applied.".to_string()),
                true,
            ),
            GitHubError::Parse { message } => Self::new(
                "GitHub's response could not be parsed.",
                Some(message.clone()),
                true,
            ),
            GitHubError::ResponseTooLarge { limit } => Self::new(
                "The response is too large to read.",
                Some(format!("The body exceeded {limit} bytes.")),
                false,
            ),
            GitHubError::StaleContext => Self::new(
                "The account or authorization changed.",
                Some(
                    "This read started under an earlier authorization, so its result was discarded. Activate the page again to read under the current account."
                        .to_string(),
                ),
                true,
            ),
            GitHubError::CredentialStore { message } => Self::new(
                "The platform credential is unavailable.",
                Some(format!(
                    "{message}. Revalidate or replace the PAT; this is not an empty result."
                )),
                false,
            ),
        }
    }

    /// A host-initiated pause: the page stopped work without an error.
    pub fn deactivated() -> Self {
        Self::new(
            "Loading stopped.",
            Some(
                "This page was deactivated and its requests were cancelled. Activate it again to resume."
                    .to_string(),
            ),
            true,
        )
    }
}

impl From<RepositoryError> for PageError {
    fn from(error: RepositoryError) -> Self {
        Self::from_repository(&error)
    }
}

/// The result of matching the saved/observed branch against the branch list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchChoice {
    Selected(DynamicRef),
    /// No branch can be chosen. `Empty` is only claimed for a complete, empty
    /// branch list; otherwise the page says the branch set is unconfirmed.
    None {
        empty: bool,
    },
}

/// Prefers the saved branch, then the repository's default branch, then the
/// first observed branch.
///
/// A default branch is resolved by name even when a partial branch list does not
/// contain it: the list feeds the picker, not the revision request.
pub fn branch_choice(
    branches: &BranchesOutcome,
    default_branch: Option<&DynamicRef>,
    intent: Option<&str>,
) -> BranchChoice {
    if let Some(intent) = intent {
        if let Some(branch) = branches
            .branches
            .iter()
            .find(|branch| branch.name.as_str() == intent)
            .or_else(|| {
                branches
                    .branches
                    .iter()
                    .find(|branch| branch.name.as_str().eq_ignore_ascii_case(intent))
            })
        {
            return BranchChoice::Selected(branch.name.clone());
        }
        if let Ok(saved) = DynamicRef::new(intent) {
            // The saved name is still a valid ref even if this (possibly
            // partial) page did not list it.
            return BranchChoice::Selected(saved);
        }
    }
    // A complete, empty branch list is positive evidence that no branch exists;
    // an observed default-branch name in that state is contradictory and must not
    // be used to invent a commit.
    if branches.branches.is_empty() && branches.completeness == Completeness::Complete {
        return BranchChoice::None { empty: true };
    }
    if let Some(default) = default_branch {
        return BranchChoice::Selected(default.clone());
    }
    match branches.branches.first() {
        Some(branch) => BranchChoice::Selected(branch.name.clone()),
        None => BranchChoice::None { empty: false },
    }
}

/// A one-line, honest account of an incomplete listing.
pub fn incompleteness_note(completeness: &Completeness) -> Option<String> {
    let Completeness::Partial { reasons } = completeness else {
        return None;
    };
    Some(
        reasons
            .iter()
            .map(|reason| match reason {
                IncompleteReason::Failed(error) => match error {
                    RepositoryError::GitHub(GitHubError::RateLimited { .. }) => {
                        "a later page was rate limited".to_string()
                    }
                    RepositoryError::GitHub(GitHubError::Network { .. }) => {
                        "a later page failed on the network".to_string()
                    }
                    _ => "a later page failed".to_string(),
                },
                IncompleteReason::PageLimit => "the page limit was reached".to_string(),
                IncompleteReason::EntryLimit => "the entry limit was reached".to_string(),
                IncompleteReason::ResponseLimit => "a page exceeded the size limit".to_string(),
                IncompleteReason::UntrustedPagination => {
                    "pagination left the trusted endpoint".to_string()
                }
                IncompleteReason::InvalidPagination => "pagination was malformed".to_string(),
                IncompleteReason::PaginationCycle => "pagination repeated".to_string(),
                IncompleteReason::SsoPartial { organizations } => {
                    format!(
                        "SSO authorization is required for {}",
                        organizations.join(", ")
                    )
                }
                IncompleteReason::ServerTruncated => "the server truncated the listing".to_string(),
                IncompleteReason::UnknownEntryType => {
                    "an entry had an unknown Git type".to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Whether a directory listing can be called complete.
pub fn tree_incomplete(tree: &DirectoryTree) -> bool {
    tree.truncated != Some(false) || tree.completeness != Completeness::Complete
}

/// What the reader should show when there is no document to render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadmeState {
    /// Discovery has not finished yet.
    Unknown,
    Found(ReadmeDescriptor),
    /// A complete, readable empty root tree: there is no README.
    Absent,
    /// A 404 that does not distinguish absence from lost visibility.
    Uncertain,
    Failed(PageError),
}

impl ReadmeState {
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    pub fn descriptor(&self) -> Option<&ReadmeDescriptor> {
        match self {
            Self::Found(descriptor) => Some(descriptor),
            _ => None,
        }
    }

    /// The repository-relative path of the README, when it was found.
    ///
    /// A README may live at the root, under `docs/` or under `.github/`; this is
    /// always the descriptor's own path, never a guess.
    pub fn path(&self) -> Option<&str> {
        self.descriptor().map(|descriptor| descriptor.path.as_str())
    }

    /// Applies one discovery result.
    pub fn from_discovery(result: Result<ReadmeDiscovery, RepositoryError>) -> Self {
        match result {
            Ok(ReadmeDiscovery::Found(descriptor)) => Self::Found(descriptor),
            Ok(ReadmeDiscovery::AbsentInEmptyTree { .. }) => Self::Absent,
            Ok(ReadmeDiscovery::NotFoundOrInaccessible { .. }) => Self::Uncertain,
            Err(error) => Self::Failed(PageError::from_repository(&error)),
        }
    }
}

/// One visible tree row, resolved against the loaded directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Repository-relative path of the entry.
    pub path: String,
    /// Last path segment, what the row prints.
    pub name: String,
    pub depth: usize,
    pub kind: TreeRowKind,
    /// Directory holding this entry (`""` for the root's own entries).
    pub parent: String,
    /// Index of the entry inside its parent's `DirectoryTree`.
    pub index: usize,
    /// How this row is doing: normal, loading, failed, or a proven empty folder.
    pub state: RowState,
}

/// Whether a row can be acted on, and how it is going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    Normal,
    /// An expanded directory whose listing is being read.
    Loading,
    /// An expanded directory whose listing failed; carries the reason.
    Failed(PageError),
    /// An expanded directory that a complete listing proved to be empty.
    Empty,
    /// Files may be omitted; even zero returned entries do not prove emptiness.
    Incomplete,
}

impl RowState {
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeRowKind {
    Directory { expanded: bool },
    File,
    Symlink,
    Submodule,
    Unknown,
}

impl TreeRowKind {
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory { .. })
    }

    /// Whether pressing the row opens something (a directory or a file).
    pub fn is_openable(self) -> bool {
        matches!(
            self,
            Self::Directory { .. } | Self::File | Self::Symlink | Self::Submodule
        )
    }
}

/// Lazily loaded directory trees for one fixed commit.
///
/// Only directories the user opened are here; nothing recursively downloads a
/// repository and no filesystem checkout happens.
#[derive(Clone, Debug, Default)]
pub struct TreeModel {
    root: Option<DirectoryTree>,
    children: BTreeMap<String, DirectoryTree>,
    loading: BTreeSet<String>,
    failed: BTreeMap<String, PageError>,
    /// Set when the flattened row cap cut the list.
    capped: bool,
}

impl TreeModel {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn set_root(&mut self, tree: DirectoryTree) {
        self.loading.remove("");
        self.failed.remove("");
        self.root = Some(tree);
    }

    /// Records a loaded child directory, keyed by its fixed-commit path.
    pub fn set_children(&mut self, tree: DirectoryTree) {
        let key = directory_key(&tree.directory);
        self.loading.remove(&key);
        self.failed.remove(&key);
        self.children.insert(key, tree);
    }

    pub fn begin_load(&mut self, path: &str) {
        self.loading.insert(path.to_string());
        self.failed.remove(path);
    }

    pub fn fail_load(&mut self, path: &str, error: PageError) {
        self.loading.remove(path);
        self.failed.insert(path.to_string(), error);
    }

    pub fn clear_failure(&mut self, path: &str) {
        self.failed.remove(path);
    }

    pub fn root(&self) -> Option<&DirectoryTree> {
        self.root.as_ref()
    }

    pub fn tree(&self, path: &str) -> Option<&DirectoryTree> {
        match path {
            "" => self.root.as_ref(),
            path => self.children.get(path),
        }
    }

    pub fn is_loading(&self, path: &str) -> bool {
        self.loading.contains(path)
    }

    pub fn failure(&self, path: &str) -> Option<&PageError> {
        self.failed.get(path)
    }

    pub fn capped(&self) -> bool {
        self.capped
    }

    /// The handle needed to fetch `path`'s children, from the parent's tree.
    pub fn child_directory(&self, path: &str) -> Result<SnapshotDirectory, RepositoryError> {
        let parent = RepositoryNavigation::parent(path);
        let name = RepositoryNavigation::name(path);
        let tree = self.tree(parent).ok_or(RepositoryError::InvalidInput {
            field: "loaded parent directory",
        })?;
        let index = tree
            .entries
            .iter()
            .position(|entry| entry.path.as_str() == name)
            .ok_or(RepositoryError::InvalidInput {
                field: "tree entry",
            })?;
        tree.child_directory(index)
    }

    /// The directory handle for the root commit, used to (re)load the root.
    pub fn root_of(&self) -> Option<&DirectoryTree> {
        self.root.as_ref()
    }

    /// The tree entry and index for a regular file at `path`, so a
    /// `BlobIdentity` can be derived from the exact fixed-commit entry.
    pub fn locate_file(&self, path: &str) -> Option<(&DirectoryTree, usize)> {
        let parent = RepositoryNavigation::parent(path);
        let name = RepositoryNavigation::name(path);
        let tree = self.tree(parent)?;
        let index = tree
            .entries
            .iter()
            .position(|entry| entry.path.as_str() == name)?;
        matches!(tree.entries[index].kind, TreeEntryKind::File { .. }).then_some((tree, index))
    }

    /// The kind of the entry at `path`, whether it is a file, a directory, or a
    /// link/submodule the reader will not open as text.
    pub fn entry_kind(&self, path: &str) -> Option<TreeRowKind> {
        let parent = RepositoryNavigation::parent(path);
        let name = RepositoryNavigation::name(path);
        let tree = self.tree(parent)?;
        let entry = tree
            .entries
            .iter()
            .find(|entry| entry.path.as_str() == name)?;
        Some(match &entry.kind {
            TreeEntryKind::Directory => TreeRowKind::Directory { expanded: false },
            TreeEntryKind::File { .. } => TreeRowKind::File,
            TreeEntryKind::Symlink => TreeRowKind::Symlink,
            TreeEntryKind::Submodule => TreeRowKind::Submodule,
            TreeEntryKind::Unknown { .. } => TreeRowKind::Unknown,
        })
    }

    /// Flattens the visible rows: root entries, then each expanded and loaded
    /// directory in place.
    pub fn rows(&mut self, expanded: &BTreeSet<String>) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.capped = false;
        if self.root.is_some() {
            self.push_rows("", 0, expanded, &mut rows);
        }
        rows
    }

    fn push_rows(
        &mut self,
        dir: &str,
        depth: usize,
        expanded: &BTreeSet<String>,
        out: &mut Vec<TreeRow>,
    ) {
        if depth > MAX_TREE_DEPTH || out.len() >= MAX_TREE_ROWS {
            self.capped = true;
            return;
        }
        let Some(tree) = self.tree(dir).cloned() else {
            return;
        };
        // Sorted so the list is deterministic regardless of API order: folders
        // first, then a case-insensitive name, then the raw name, then API order.
        let mut order: Vec<usize> = (0..tree.entries.len()).collect();
        order.sort_by(|a, b| {
            let left = &tree.entries[*a];
            let right = &tree.entries[*b];
            let left_dir = left.kind == TreeEntryKind::Directory;
            let right_dir = right.kind == TreeEntryKind::Directory;
            right_dir
                .cmp(&left_dir)
                .then_with(|| {
                    left.path
                        .as_str()
                        .to_ascii_lowercase()
                        .cmp(&right.path.as_str().to_ascii_lowercase())
                })
                .then_with(|| left.path.as_str().cmp(right.path.as_str()))
                .then_with(|| a.cmp(b))
        });
        for index in order {
            if out.len() >= MAX_TREE_ROWS {
                self.capped = true;
                return;
            }
            let entry = &tree.entries[index];
            let path = match dir {
                "" => entry.path.as_str().to_string(),
                dir => format!("{dir}/{}", entry.path.as_str()),
            };
            let kind = match &entry.kind {
                TreeEntryKind::Directory => {
                    let open = expanded.contains(&path);
                    TreeRowKind::Directory { expanded: open }
                }
                TreeEntryKind::File { .. } => TreeRowKind::File,
                TreeEntryKind::Symlink => TreeRowKind::Symlink,
                TreeEntryKind::Submodule => TreeRowKind::Submodule,
                TreeEntryKind::Unknown { .. } => TreeRowKind::Unknown,
            };
            let state = if !kind.is_directory() || !expanded.contains(&path) {
                RowState::Normal
            } else if self.loading.contains(&path) {
                RowState::Loading
            } else if let Some(error) = self.failed.get(&path) {
                RowState::Failed(error.clone())
            } else if self.children.get(&path).is_some_and(tree_incomplete) {
                RowState::Incomplete
            } else if self
                .children
                .get(&path)
                .is_some_and(|tree| tree.entries.is_empty())
            {
                RowState::Empty
            } else {
                RowState::Normal
            };
            out.push(TreeRow {
                name: entry.path.as_str().to_string(),
                path: path.clone(),
                depth,
                kind,
                parent: dir.to_string(),
                index,
                state,
            });
            if kind.is_directory() && expanded.contains(&path) && out.len() < MAX_TREE_ROWS {
                // An expanded directory with a proven-empty or failed listing
                // contributes no child rows; the parent row already says why.
                self.push_rows(&path, depth + 1, expanded, out);
            }
        }
    }
}

fn directory_key(directory: &SnapshotDirectory) -> String {
    directory
        .path()
        .map(|path| path.as_str().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::repository::{Branch, ObjectSha};

    fn sha(value: &str) -> ObjectSha {
        ObjectSha::new(value.repeat(40)).unwrap()
    }

    fn branches(names: &[&str], completeness: Completeness) -> BranchesOutcome {
        BranchesOutcome {
            repository: crate::domain::repository::RepositoryIdentity {
                id: 1,
                locator: crate::domain::repository::RepositoryLocator::new("octo", "repo").unwrap(),
            },
            branches: names
                .iter()
                .map(|name| Branch {
                    name: DynamicRef::new(*name).unwrap(),
                    observed_commit: sha("a"),
                    protected: false,
                })
                .collect(),
            completeness,
        }
    }

    #[test]
    fn branch_choice_prefers_intent_then_default_then_first() {
        let list = branches(&["main", "feature"], Completeness::Complete);
        let main = DynamicRef::new("main").unwrap();
        assert_eq!(
            branch_choice(&list, Some(&main), Some("feature")),
            BranchChoice::Selected(DynamicRef::new("feature").unwrap())
        );
        assert_eq!(
            branch_choice(&list, Some(&main), None),
            BranchChoice::Selected(main.clone())
        );
        assert_eq!(
            branch_choice(&list, None, None),
            BranchChoice::Selected(DynamicRef::new("main").unwrap())
        );
        // A saved branch that this (partial) page did not list is still a ref.
        assert_eq!(
            branch_choice(&list, None, Some("release")),
            BranchChoice::Selected(DynamicRef::new("release").unwrap())
        );
        assert_eq!(
            branch_choice(&list, None, Some("FEATURE")),
            BranchChoice::Selected(DynamicRef::new("feature").unwrap())
        );
    }

    #[test]
    fn an_empty_branch_list_is_only_empty_when_complete() {
        let complete = branches(&[], Completeness::Complete);
        assert_eq!(
            branch_choice(&complete, None, None),
            BranchChoice::None { empty: true }
        );
        let partial = branches(
            &[],
            Completeness::Partial {
                reasons: vec![IncompleteReason::PageLimit],
            },
        );
        assert_eq!(
            branch_choice(&partial, None, None),
            BranchChoice::None { empty: false }
        );
        assert!(incompleteness_note(&partial.completeness).is_some());
        assert!(incompleteness_note(&Completeness::Complete).is_none());
    }

    #[test]
    fn ancestors_and_parents_describe_the_same_path() {
        assert_eq!(
            RepositoryNavigation::ancestors("docs/guide/intro.md"),
            vec!["docs".to_string(), "docs/guide".to_string()]
        );
        assert!(RepositoryNavigation::ancestors("README.md").is_empty());
        assert_eq!(
            RepositoryNavigation::parent("docs/guide/intro.md"),
            "docs/guide"
        );
        assert_eq!(RepositoryNavigation::parent("README.md"), "");
        assert_eq!(RepositoryNavigation::name("docs/a.md"), "a.md");
        assert_eq!(RepositoryNavigation::name("a.md"), "a.md");
    }

    #[test]
    fn a_missing_resource_never_claims_a_permission_problem() {
        let error = PageError::from_repository(&RepositoryError::GitHub(GitHubError::NotFound));
        assert!(error.title.contains("not found"));
        assert!(
            !error.title.contains("not visible"),
            "a 404 must not assert the account lost visibility: {error:?}"
        );
        let detail = error.detail.unwrap_or_default();
        assert!(detail.contains("does not exist"));
        assert!(detail.contains("cannot access"));
    }
}
