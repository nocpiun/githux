//! One directory at a time by tree SHA. Never resolves a branch or recurses.

use crate::auth::token::Pat;
use crate::domain::repository::{
    Completeness, DirectoryTree, IncompleteReason, ObjectSha, RepositoryError, RepositoryPath,
    SnapshotDirectory, TreeEntry, TreeEntryKind,
};
use crate::github::client::GitHubClient;
use crate::github::headers::parse_sso_header;
use crate::github::repository::{
    RepositoryLimits, check_status, decode, endpoint, get, invalid_response,
};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;

#[derive(Deserialize)]
struct TreeResponse {
    sha: String,
    truncated: Option<bool>,
    tree: Vec<EntryResponse>,
}
#[derive(Deserialize)]
struct EntryResponse {
    path: String,
    sha: String,
    mode: String,
    #[serde(rename = "type")]
    git_type: String,
    size: Option<u64>,
}

pub async fn fetch_directory(
    client: &GitHubClient,
    token: &Pat,
    directory: &SnapshotDirectory,
    limits: &RepositoryLimits,
    cancel: CancellationToken,
) -> Result<DirectoryTree, RepositoryError> {
    let url = endpoint(
        client,
        directory.repository.locator(),
        &["git", "trees", directory.tree.as_str()],
    )?;
    let response = get(client, token, &url, cancel).await?;
    check_status(&response)?;
    let mut reasons = Vec::new();
    if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
        reasons.push(IncompleteReason::SsoPartial { organizations });
    }
    if response.body.len() > limits.response_bytes {
        reasons.push(IncompleteReason::ResponseLimit);
        return Ok(DirectoryTree {
            directory: directory.clone(),
            entries: Vec::new(),
            truncated: None,
            completeness: Completeness::from_reasons(reasons),
        });
    }
    let raw: TreeResponse = decode(&response, limits)?;
    if ObjectSha::new(raw.sha)? != directory.tree {
        return Err(invalid_response("tree SHA differs from requested object"));
    }
    if raw.truncated != Some(false) {
        reasons.push(IncompleteReason::ServerTruncated);
    }
    if raw.tree.len() > limits.tree_entries {
        reasons.push(IncompleteReason::EntryLimit);
    }
    let mut entries = Vec::new();
    let mut paths = std::collections::HashSet::new();
    for entry in raw.tree.into_iter().take(limits.tree_entries) {
        let path = RepositoryPath::new(entry.path)?;
        if path.as_str().contains('/') || !paths.insert(path.as_str().to_owned()) {
            return Err(invalid_response(
                "non-recursive tree has nested or duplicate paths",
            ));
        }
        let kind = match (entry.git_type.as_str(), entry.mode.as_str()) {
            ("blob", "100644") => TreeEntryKind::File { executable: false },
            ("blob", "100755") => TreeEntryKind::File { executable: true },
            ("tree", "040000") => TreeEntryKind::Directory,
            ("blob", "120000") => TreeEntryKind::Symlink,
            ("commit", "160000") => TreeEntryKind::Submodule,
            _ => {
                if !reasons.contains(&IncompleteReason::UnknownEntryType) {
                    reasons.push(IncompleteReason::UnknownEntryType);
                }
                TreeEntryKind::Unknown {
                    git_type: entry.git_type,
                    mode: entry.mode,
                }
            }
        };
        entries.push(TreeEntry {
            path,
            sha: ObjectSha::new(entry.sha)?,
            size: entry.size,
            kind,
        });
    }
    Ok(DirectoryTree {
        directory: directory.clone(),
        entries,
        truncated: raw.truncated,
        completeness: Completeness::from_reasons(reasons),
    })
}
