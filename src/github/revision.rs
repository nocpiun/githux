//! Resolve a mutable ref once into immutable commit and root-tree identities.

use crate::auth::token::Pat;
use crate::domain::repository::{
    GitSignature, ObjectSha, RepoSnapshot, RepositoryError, RepositoryIdentity,
    RepositoryResponseMetadata, RevisionSpec,
};
use crate::github::client::GitHubClient;
use crate::github::repository::{
    RepositoryLimits, check_single, decode, endpoint, get_with_metadata, invalid_response,
};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;

#[derive(Deserialize)]
struct CommitResponse {
    sha: String,
    commit: GitCommit,
}
#[derive(Deserialize)]
struct GitCommit {
    message: String,
    tree: TreePointer,
    author: Option<SignatureResponse>,
    committer: Option<SignatureResponse>,
}
#[derive(Deserialize)]
struct TreePointer {
    sha: String,
}
#[derive(Deserialize)]
struct SignatureResponse {
    name: Option<String>,
    email: Option<String>,
    date: Option<String>,
}
impl From<SignatureResponse> for GitSignature {
    fn from(raw: SignatureResponse) -> Self {
        Self {
            name: raw.name,
            email: raw.email,
            date: raw.date,
        }
    }
}

pub async fn resolve_revision(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    revision: RevisionSpec,
    limits: &RepositoryLimits,
    cancel: CancellationToken,
) -> Result<RepoSnapshot, RepositoryError> {
    resolve_revision_with_metadata(client, token, repository, revision, limits, None, cancel)
        .await
        .map(|(snapshot, _)| snapshot.expect("unconditional read cannot be 304"))
}

pub async fn resolve_revision_with_metadata(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    revision: RevisionSpec,
    limits: &RepositoryLimits,
    conditional: Option<&RepositoryResponseMetadata>,
    cancel: CancellationToken,
) -> Result<(Option<RepoSnapshot>, RepositoryResponseMetadata), RepositoryError> {
    let reference = match &revision {
        RevisionSpec::Ref(reference) => reference.as_str(),
        RevisionSpec::Commit(sha) => sha.as_str(),
    };
    let url = endpoint(client, repository.locator(), &["commits", reference])?;
    let (response, metadata) = get_with_metadata(client, token, &url, conditional, cancel).await?;
    if response.status == 304 {
        return Ok((None, metadata));
    }
    check_single(&response)?;
    let raw: CommitResponse = decode(&response, limits)?;
    let commit = ObjectSha::new(raw.sha)?;
    if let RevisionSpec::Commit(expected) = &revision
        && expected != &commit
    {
        return Err(invalid_response(
            "resolved commit differs from requested SHA",
        ));
    }
    Ok((
        Some(RepoSnapshot {
            repository: repository.clone(),
            requested: revision,
            commit,
            root_tree: ObjectSha::new(raw.commit.tree.sha)?,
            message: raw.commit.message,
            author: raw.commit.author.map(Into::into),
            committer: raw.commit.committer.map(Into::into),
        }),
        metadata,
    ))
}
