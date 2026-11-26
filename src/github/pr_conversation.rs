//! A single GitHub PR query. The live head ref and historical head name have different lifetimes.

use crate::auth::token::Pat;
use crate::domain::{
    error::{GitHubError, RateLimitKind, RetryAfter},
    pr_conversation::{Conversation, SourceBranch, State, Target, access_denied},
    profile::{GraphqlErrorKind, ProfileError},
};
use crate::github::{
    GitHubClient,
    profile::graphql::{ProfileLimits, run_query},
};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;

const QUERY: &str = "query PRConversation($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    nameWithOwner
    pullRequest(number: $number) {
      number title state isDraft author { login } body
      baseRefName headRefName headRef { name } headRepository { nameWithOwner }
    }
  }
}";

#[derive(Deserialize)]
struct Response {
    repository: Option<Repository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    name_with_owner: String,
    pull_request: Option<PullRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequest {
    number: i32,
    title: String,
    state: State,
    is_draft: bool,
    author: Option<Author>,
    body: String,
    base_ref_name: String,
    head_ref_name: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    head_ref: Option<HeadRef>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    head_repository: Option<HeadRepository>,
}

#[derive(Deserialize)]
struct Author {
    login: String,
}

#[derive(Deserialize)]
struct HeadRef {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadRepository {
    name_with_owner: String,
}

pub(crate) async fn fetch(
    client: &GitHubClient,
    pat: &Pat,
    target: Target,
    cancel: CancellationToken,
) -> Result<Conversation, GitHubError> {
    let variables = serde_json::json!({
        "owner": target.repository.owner(),
        "name": target.repository.name(),
        "number": target.number(),
    });
    let limits = ProfileLimits {
        response_bytes: 1024 * 1024,
        ..ProfileLimits::default()
    };
    let outcome = run_query(client, pat, QUERY, &variables, &limits, cancel)
        .await
        .map_err(query_error)?;
    // Reject partial data and give permission failures precedence over other field errors.
    let mut errors = outcome
        .errors
        .into_iter()
        .map(|error| graphql_error(error.kind));
    if let Some(first) = errors.next() {
        return Err(if access_denied(&first) {
            first
        } else {
            errors.find(access_denied).unwrap_or(first)
        });
    }
    decode(target, outcome.data.ok_or_else(invalid_response)?)
}

fn decode(target: Target, data: serde_json::Value) -> Result<Conversation, GitHubError> {
    let response: Response = serde_json::from_value(data).map_err(|_| invalid_response())?;
    let repository = response.repository.ok_or(GitHubError::NotFound)?;
    let expected = format!("{}/{}", target.repository.owner(), target.repository.name());
    if !repository.name_with_owner.eq_ignore_ascii_case(&expected) {
        return Err(invalid_response());
    }
    let pr = repository.pull_request.ok_or(GitHubError::NotFound)?;
    if pr.number != target.number()
        || pr.title.trim().is_empty()
        || pr.base_ref_name.is_empty()
        || pr.head_ref_name.is_empty()
    {
        return Err(invalid_response());
    }
    let source_branch = match (pr.head_repository, pr.head_ref) {
        (Some(repo), Some(head)) if head.name == pr.head_ref_name => SourceBranch::Present {
            repository: repo.name_with_owner,
        },
        (Some(repo), None) => SourceBranch::Deleted {
            repository: repo.name_with_owner,
        },
        (None, _) => SourceBranch::Unavailable,
        _ => return Err(invalid_response()),
    };
    Ok(Conversation {
        target,
        title: pr.title,
        state: pr.state,
        draft: pr.is_draft,
        author: pr
            .author
            .map(|author| author.login)
            .filter(|login| !login.is_empty()),
        body: (!pr.body.trim().is_empty()).then_some(pr.body),
        base_branch: pr.base_ref_name,
        head_branch: pr.head_ref_name,
        source_branch,
    })
}

fn invalid_response() -> GitHubError {
    GitHubError::Parse {
        message: "invalid pull request response".into(),
    }
}

fn query_error(error: ProfileError) -> GitHubError {
    match error {
        ProfileError::GitHub(error) => error,
        ProfileError::LimitExceeded { limit, .. } => GitHubError::ResponseTooLarge { limit },
        ProfileError::RedirectNotFollowed { status } => GitHubError::UnexpectedStatus { status },
        _ => invalid_response(),
    }
}

fn graphql_error(kind: GraphqlErrorKind) -> GitHubError {
    let rate_limit = |kind| GitHubError::RateLimited {
        retry: RetryAfter {
            kind,
            seconds: None,
        },
    };
    match kind {
        GraphqlErrorKind::NotFound => GitHubError::NotFound,
        GraphqlErrorKind::Unauthorized => GitHubError::Unauthorized,
        GraphqlErrorKind::Forbidden => GitHubError::Forbidden,
        GraphqlErrorKind::SsoRequired { authorization_url } => {
            GitHubError::SsoRequired { authorization_url }
        }
        GraphqlErrorKind::RateLimitPrimary => rate_limit(RateLimitKind::Primary),
        GraphqlErrorKind::RateLimitSecondary => rate_limit(RateLimitKind::Secondary),
        GraphqlErrorKind::Unknown => invalid_response(),
    }
}
