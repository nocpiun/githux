//! `GET /user` — stable account identity verification.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::{AccountIdentity, AccountKey, HostId};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;

/// Verified account identity returned by `GET /user`.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub identity: AccountIdentity,
}

#[derive(Deserialize)]
struct UserResponse {
    id: u64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
    html_url: Option<String>,
}

/// Fetch and verify the authenticated user identity. This is the authoritative
/// identity check before credentials are persisted.
pub async fn fetch_authenticated_user(
    client: &GitHubClient,
    token: &Pat,
    cancel: CancellationToken,
) -> Result<AuthenticatedUser, GitHubError> {
    let request = client.build_get("user", token);
    let response = client
        .transport()
        .execute(request, cancel)
        .await
        .map_err(classify_transport)?;

    if let Some(err) = classify_status(&response) {
        return Err(err);
    }

    let parsed: UserResponse =
        serde_json::from_str(&response.body).map_err(|e| GitHubError::Parse {
            message: e.to_string(),
        })?;

    let identity = AccountIdentity {
        key: AccountKey::new(parsed.id, HostId::github()),
        login: parsed.login,
        display_name: parsed.name,
        avatar_url: parsed.avatar_url,
        html_url: parsed.html_url,
    };

    Ok(AuthenticatedUser { identity })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::transport::{HttpResponse, MockTransport};
    use std::sync::Arc;

    fn user_body(id: u64, login: &str) -> String {
        serde_json::json!({
            "id": id,
            "login": login,
            "name": null,
            "avatar_url": format!("https://github.com/identicons/{login}.png"),
            "html_url": format!("https://github.com/{login}")
        })
        .to_string()
    }

    #[tokio::test]
    async fn parses_user_identity() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: user_body(12345, "octocat"),
        }))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let result = fetch_authenticated_user(&client, &token, CancellationToken::default()).await;
        let user = result.expect("should parse");
        assert_eq!(user.identity.key.id, 12345);
        assert_eq!(user.identity.login, "octocat");
    }

    #[tokio::test]
    async fn classifies_401_as_unauthorized() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 401,
            headers: vec![],
            body: r#"{"message":"Bad credentials"}"#.to_string(),
        }))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_bad");
        let result = fetch_authenticated_user(&client, &token, CancellationToken::default()).await;
        assert!(matches!(result, Err(GitHubError::Unauthorized)));
    }
}
