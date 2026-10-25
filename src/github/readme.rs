//! Preferred README metadata at an immutable commit. No blob requests, body
//! decoding, rendering, or download_url retention. GitHub's JSON README response
//! may contain inline content; serde ignores it rather than exposing it.

use crate::auth::token::Pat;
use crate::domain::repository::{
    Completeness, ObjectSha, ReadmeDescriptor, ReadmeDiscovery, RepoSnapshot, RepositoryError,
    RepositoryPath, SnapshotDirectory,
};
use crate::github::client::GitHubClient;
use crate::github::repository::{
    RepositoryLimits, check_single, decode, endpoint, get, invalid_response,
};
use crate::github::tree::fetch_directory;
use crate::network::cancel::CancellationToken;
use serde::Deserialize;

#[derive(Deserialize)]
struct ReadmeResponse {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    sha: String,
    size: u64,
}

pub async fn discover_readme(
    client: &GitHubClient,
    token: &Pat,
    snapshot: &RepoSnapshot,
    limits: &RepositoryLimits,
    cancel: CancellationToken,
) -> Result<ReadmeDiscovery, RepositoryError> {
    let mut url = endpoint(client, snapshot.repository.locator(), &["readme"])?;
    url.query_pairs_mut()
        .append_pair("ref", snapshot.commit.as_str());
    let response = get(client, token, &url, cancel.clone()).await?;
    if response.status == 404 {
        // Access may have changed since resolution. A complete, readable empty
        // root is positive absence evidence; a bare 404 never is. This bounded
        // extra request also propagates cancellation/authorization/409 errors.
        let root = fetch_directory(
            client,
            token,
            &SnapshotDirectory::root(snapshot),
            limits,
            cancel,
        )
        .await?;
        if root.completeness == Completeness::Complete && root.entries.is_empty() {
            return Ok(ReadmeDiscovery::AbsentInEmptyTree {
                repository: snapshot.repository.clone(),
                commit: snapshot.commit.clone(),
            });
        }
        return Ok(ReadmeDiscovery::NotFoundOrInaccessible {
            repository: snapshot.repository.clone(),
            commit: snapshot.commit.clone(),
        });
    }
    check_single(&response)?;
    let raw: ReadmeResponse = decode(&response, limits)?;
    if raw.kind != "file" {
        return Err(invalid_response("README is not a file descriptor"));
    }
    Ok(ReadmeDiscovery::Found(ReadmeDescriptor {
        repository: snapshot.repository.clone(),
        commit: snapshot.commit.clone(),
        path: RepositoryPath::new(raw.path)?,
        blob: ObjectSha::new(raw.sha)?,
        size: raw.size,
    }))
}
