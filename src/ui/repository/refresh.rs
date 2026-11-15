//! Prepare one fixed-commit replacement off the UI thread. Nothing here mutates the displayed
//! page: a failed read leaves its authorized historical snapshot available.

use super::*;

#[derive(Clone)]
pub(super) struct Target {
    pub snapshot: RepoSnapshot,
    pub provenance: Option<RepositoryCacheProvenance>,
}

pub(super) struct Replacement {
    pub tree: TreeModel,
    pub readme: ReadmeState,
    pub content: Option<PreparedContent>,
    pub status: ReaderStatus,
}

struct Reads<'a> {
    cached: Option<&'a CachedRepositorySession>,
    plain: &'a RepositorySession,
    limits: &'a PageLimits,
    cancel: CancellationToken,
}

impl Reads<'_> {
    async fn directory(&self, handle: &SnapshotDirectory) -> Result<DirectoryTree, PageError> {
        match self.cached {
            Some(cached) => cached
                .directory(
                    handle,
                    &self.limits.repository,
                    RepositoryCacheMode::OnlinePreferred,
                    self.cancel.clone(),
                )
                .await
                .map(|read| read.value.value)
                .map_err(PageError::from),
            None => self
                .plain
                .directory(handle, &self.limits.repository, self.cancel.clone())
                .await
                .map(|read| read.value)
                .map_err(|error| PageError::from_repository(&error)),
        }
    }

    async fn readme(&self, snapshot: &RepoSnapshot) -> Result<ReadmeState, PageError> {
        match self.cached {
            Some(cached) => cached
                .readme(
                    snapshot,
                    &self.limits.repository,
                    RepositoryCacheMode::OnlinePreferred,
                    self.cancel.clone(),
                )
                .await
                .map(|read| ReadmeState::from_discovery(Ok(read.value.value)))
                .map_err(PageError::from),
            None => self
                .plain
                .readme(snapshot, &self.limits.repository, self.cancel.clone())
                .await
                .map(|read| ReadmeState::from_discovery(Ok(read.value)))
                .map_err(|error| PageError::from_repository(&error)),
        }
    }

    async fn content(&self, identity: &BlobIdentity) -> Result<PreparedContent, PageError> {
        match self.cached {
            Some(cached) => cached
                .content(
                    identity,
                    &self.limits.blob,
                    RepositoryCacheMode::OnlinePreferred,
                    self.cancel.clone(),
                )
                .await
                .map(|read| read.value.value)
                .map_err(PageError::from),
            None => self
                .plain
                .content(identity, &self.limits.blob, self.cancel.clone())
                .await
                .map(|read| read.value)
                .map_err(|error| PageError::from_repository(&error)),
        }
    }
}

pub(super) async fn prepare(
    cached: Option<&CachedRepositorySession>,
    plain: &RepositorySession,
    snapshot: &RepoSnapshot,
    intent: RepositoryNavigation,
    selection: Selection,
    limits: &PageLimits,
    cancel: CancellationToken,
) -> Result<Replacement, PageError> {
    let reads = Reads {
        cached,
        plain,
        limits,
        cancel,
    };
    let root = reads.directory(&SnapshotDirectory::root(snapshot)).await?;
    let readme = reads.readme(snapshot).await?;
    let mut tree = TreeModel::default();
    tree.set_root(root);
    // Restore only the directories the user had open or needs to resolve the selected file. This
    // remains a bounded lazy browse, never a recursive repository download.
    let mut paths = intent.expanded;
    if let Selection::File(path) = &selection {
        paths.extend(RepositoryNavigation::ancestors(path));
    }
    if let ReadmeState::Found(descriptor) = &readme {
        paths.extend(RepositoryNavigation::ancestors(descriptor.path.as_str()));
    }
    for path in paths {
        let Ok(handle) = tree.child_directory(&path) else {
            continue;
        };
        match reads.directory(&handle).await {
            Ok(child) => tree.set_children(child),
            Err(error) => {
                // A selected file needs its ancestors; unrelated directory failures are local.
                if selection.path().is_some_and(|selected| {
                    RepositoryNavigation::ancestors(selected).contains(&path)
                }) {
                    return Err(error);
                }
                tree.fail_load(&path, error);
            }
        }
    }
    let (identity, status) = match &selection {
        Selection::Readme => match &readme {
            ReadmeState::Found(descriptor) => (
                Some(BlobIdentity::from_readme(descriptor)),
                ReaderStatus::Showing,
            ),
            ReadmeState::Absent => (None, ReaderStatus::NoReadme),
            ReadmeState::Uncertain => (None, ReaderStatus::ReadmeUncertain),
            ReadmeState::Failed(error) => return Err(error.clone()),
            ReadmeState::Unknown => {
                return Err(PageError::new("README could not be confirmed.", None, true));
            }
        },
        Selection::File(path) => {
            let identity = tree
                .locate_file(path)
                .and_then(|(directory, index)| BlobIdentity::from_file(directory, index).ok());
            let status = if identity.is_some() {
                ReaderStatus::Showing
            } else {
                ReaderStatus::SelectionUnavailable {
                    reason: format!(
                        "`{path}` is not available as an ordinary file in the refreshed listing. Incomplete listings do not prove that a path was deleted."
                    ),
                }
            };
            (identity, status)
        }
    };
    let content = match identity {
        Some(identity) => Some(reads.content(&identity).await?),
        None => None,
    };
    Ok(Replacement {
        tree,
        readme,
        content,
        status,
    })
}
