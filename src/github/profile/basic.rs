//! `GET /users/{login}` — public profile summary.
//!
//! Public information only. Private fields that require `read:user` are out of
//! scope for this adapter; the login is encoded through the same URL parser the
//! client uses for trust decisions, never concatenated raw into a path.

use crate::auth::token::Pat;
use crate::domain::profile::{ProfileBasic, ProfileError};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;

#[derive(Deserialize)]
struct ProfileResponse {
    id: u64,
    login: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    bio: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    company: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    blog: Option<String>,
    #[serde(default)]
    followers: Option<u64>,
    #[serde(default)]
    following: Option<u64>,
    #[serde(default)]
    public_repos: Option<u64>,
    #[serde(default)]
    public_gists: Option<u64>,
    #[serde(default)]
    created_at: Option<String>,
}

/// Fetch a user's public profile. A 404 is reported as
/// [`crate::domain::error::GitHubError::NotFound`] (invisible or non-existent),
/// never as an empty profile.
pub async fn fetch_profile(
    client: &GitHubClient,
    token: &Pat,
    login: &str,
    cancel: CancellationToken,
) -> Result<ProfileBasic, ProfileError> {
    crate::github::profile::validate_login(login)?;
    let url = users_endpoint(client, login)?;
    let request = client.build_get(url.as_str(), token);
    let response = client
        .transport()
        .execute(request, cancel)
        .await
        .map_err(classify_transport)?;
    if let Some(error) = classify_status(&response) {
        return Err(error.into());
    }
    let raw: ProfileResponse =
        serde_json::from_str(&response.body).map_err(|_| ProfileError::InvalidResponse {
            message: "invalid user profile JSON".into(),
        })?;
    Ok(ProfileBasic {
        id: raw.id,
        login: raw.login,
        name: raw.name,
        bio: raw.bio,
        avatar_url: raw.avatar_url,
        profile_url: raw.html_url,
        company: raw.company,
        location: raw.location,
        website_url: raw.blog,
        followers: raw.followers,
        following: raw.following,
        public_repos: raw.public_repos,
        public_gists: raw.public_gists,
        created_at: raw.created_at,
    })
}

fn users_endpoint(client: &GitHubClient, login: &str) -> Result<reqwest::Url, ProfileError> {
    let mut url =
        reqwest::Url::parse(&client.config().base_url).map_err(|_| ProfileError::InvalidInput {
            field: "API base URL",
        })?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(ProfileError::InvalidInput {
            field: "API base URL",
        });
    }
    url.path_segments_mut()
        .map_err(|_| ProfileError::InvalidInput {
            field: "API base URL",
        })?
        .pop_if_empty()
        .extend(["users", login]);
    if !client.url_is_trusted(url.as_str()) {
        return Err(ProfileError::InvalidInput {
            field: "API origin",
        });
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::GitHubError;
    use crate::network::test_server::{LocalHttpServer, ScriptedResponse};
    use crate::network::transport::{MockTransport, ReqwestTransport, json_response};
    use std::sync::Arc;
    use std::time::Duration;

    fn profile_json(login: &str) -> String {
        serde_json::json!({
            "id": 583231,
            "login": login,
            "name": "The Octocat",
            "bio": "There once was...",
            "avatar_url": "https://avatars.example.invalid/u/583231",
            "html_url": format!("https://github.com/{login}"),
            "company": "@github",
            "location": "San Francisco",
            "blog": "https://example.invalid",
            "followers": 100,
            "following": 9,
            "public_repos": 8,
            "public_gists": 8,
            "created_at": "2011-01-25T18:44:36Z"
        })
        .to_string()
    }

    #[tokio::test]
    async fn parses_public_profile_fields() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(json_response(200, &profile_json("octocat"))))
            .await;
        let client = GitHubClient::new(Default::default(), mock);
        let profile = fetch_profile(
            &client,
            &Pat::from_str("ghp_test"),
            "octocat",
            CancellationToken::default(),
        )
        .await
        .expect("profile");
        assert_eq!(profile.id, 583231);
        assert_eq!(profile.login, "octocat");
        assert_eq!(profile.name.as_deref(), Some("The Octocat"));
        assert_eq!(profile.bio.as_deref(), Some("There once was..."));
        assert_eq!(
            profile.profile_url.as_deref(),
            Some("https://github.com/octocat")
        );
        assert_eq!(
            profile.website_url.as_deref(),
            Some("https://example.invalid")
        );
        assert_eq!(profile.public_repos, Some(8));
        assert_eq!(profile.followers, Some(100));
    }

    #[tokio::test]
    async fn classifies_404_as_not_found() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(json_response(404, r#"{"message":"Not Found"}"#)))
            .await;
        let client = GitHubClient::new(Default::default(), mock);
        let result = fetch_profile(
            &client,
            &Pat::from_str("ghp_test"),
            "ghost",
            CancellationToken::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ProfileError::GitHub(GitHubError::NotFound))
        ));
    }

    #[test]
    fn rejects_logins_that_could_change_the_request_target() {
        for bad in ["", "a/b", "a\\b", "a?b", "a#b", "a b", "a\nb"] {
            assert!(
                crate::github::profile::validate_login(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert!(crate::github::profile::validate_login("octocat").is_ok());
    }

    /// Real socket round-trip: the login is percent-encoded through the URL
    /// parser and the request carries the fixed headers and Authorization.
    #[tokio::test]
    async fn localhost_request_targets_the_users_endpoint() {
        let server =
            LocalHttpServer::start(vec![ScriptedResponse::json(200, &profile_json("octo%cat"))]);
        let transport = Arc::new(ReqwestTransport::new(Duration::from_secs(5)).expect("client"));
        let client = GitHubClient::new(
            crate::github::GitHubConfig {
                base_url: server.base_url().to_string(),
                ..Default::default()
            },
            transport,
        );
        let profile = fetch_profile(
            &client,
            &Pat::from_str("ghp_local_mock"),
            "octo%cat",
            CancellationToken::default(),
        )
        .await
        .expect("profile");
        assert_eq!(profile.login, "octo%cat");
        let recorded = server.recorded();
        assert_eq!(recorded.len(), 1);
        assert!(
            recorded[0]
                .request_line
                .starts_with("GET /users/octo%25cat")
        );
        assert_eq!(
            recorded[0].header("authorization"),
            Some("Bearer ghp_local_mock")
        );
    }
}
