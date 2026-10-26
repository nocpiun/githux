//! Read-only GraphQL POST plumbing shared by the profile reads.
//!
//! Boundaries:
//! - Only queries are sent. There is no mutation entry point, and the transport
//!   never auto-replays a request.
//! - The response body is bounded at receive time by the existing transport
//!   limit; cancellation and timeouts reuse the existing machinery.
//! - A `200` is never treated as full success: `data` and `errors` may coexist,
//!   and `errors` are reduced to a safe category plus the query path. The raw
//!   server message is deliberately discarded (it may echo caller input).
//! - A POST redirect is surfaced as [`ProfileError::RedirectNotFollowed`] and
//!   never followed, so the query body cannot be re-sent to another target.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::profile::{GraphqlErrorKind, ProfileError, ProfileGraphqlError};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::github::headers::parse_sso_authorization_url;
use crate::network::cancel::CancellationToken;
use crate::network::transport::{HttpResponse, RateLimitHeaders, TransportError};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Budgets for profile reads. All are engineering limits, not completeness
/// guarantees.
#[derive(Debug, Clone)]
pub struct ProfileLimits {
    /// Receive-time bound for a GraphQL JSON response body.
    pub response_bytes: usize,
    /// `first:` budget for `pinnedItems`.
    pub pinned_items: u32,
    /// `first:` budget for `itemShowcase.items`.
    pub showcase_items: u32,
    /// Defensive cap on the number of returned contribution weeks.
    pub max_weeks: usize,
}

impl Default for ProfileLimits {
    fn default() -> Self {
        Self {
            response_bytes: 8 * 1024 * 1024,
            pinned_items: 6,
            showcase_items: 6,
            max_weeks: 60,
        }
    }
}

#[derive(Deserialize)]
struct RawGraphQlBody {
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    errors: Option<Vec<RawGraphQlError>>,
}

#[derive(Deserialize)]
struct RawGraphQlError {
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    path: Option<Vec<Value>>,
}

/// Result of one GraphQL query. `data` may be present even when `errors` is
/// non-empty (a partial result).
pub(crate) struct GraphQlOutcome {
    pub data: Option<Value>,
    pub errors: Vec<ProfileGraphqlError>,
}

/// Execute a fixed GraphQL query with variables. The body is built by the
/// caller from a fixed query string plus a `serde_json` variables object, so
/// variable values are JSON-escaped rather than interpolated into the query.
pub(crate) async fn run_query(
    client: &GitHubClient,
    token: &Pat,
    query: &str,
    variables: &Value,
    limits: &ProfileLimits,
    cancel: CancellationToken,
) -> Result<GraphQlOutcome, ProfileError> {
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    let body = serde_json::json!({
        "query": query,
        "variables": variables,
    })
    .to_string();
    let request = client.build_post("graphql", token, body);
    let response = client
        .transport()
        .execute_bounded(request, cancel.clone(), limits.response_bytes)
        .await
        .map_err(|error| match error {
            TransportError::ResponseTooLarge { limit } => ProfileError::LimitExceeded {
                resource: "GraphQL JSON bytes",
                limit,
            },
            error => classify_transport(error).into(),
        })?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    // Defense for custom transports as well as the real receive-time bound.
    if response.body.len() > limits.response_bytes {
        return Err(ProfileError::LimitExceeded {
            resource: "GraphQL JSON bytes",
            limit: limits.response_bytes,
        });
    }
    parse_response(&response)
}

fn parse_response(response: &HttpResponse) -> Result<GraphQlOutcome, ProfileError> {
    if (300..400).contains(&response.status) {
        return Err(ProfileError::RedirectNotFollowed {
            status: response.status,
        });
    }
    if let Some(error) = classify_status(response) {
        return Err(error.into());
    }
    let parsed: RawGraphQlBody =
        serde_json::from_str(&response.body).map_err(|_| ProfileError::InvalidResponse {
            message: "invalid GraphQL JSON".into(),
        })?;
    let errors = parsed
        .errors
        .as_deref()
        .map(|errors| classify_errors(response, errors))
        .unwrap_or_default();
    Ok(GraphQlOutcome {
        data: parsed.data,
        errors,
    })
}

fn classify_errors(
    response: &HttpResponse,
    errors: &[RawGraphQlError],
) -> Vec<ProfileGraphqlError> {
    let rate_limit = response.rate_limit();
    let sso_url = response
        .header("x-github-sso")
        .and_then(parse_sso_authorization_url);
    errors
        .iter()
        .map(|error| ProfileGraphqlError {
            kind: classify_error(error, &rate_limit, sso_url.as_deref()),
            path: error
                .path
                .as_ref()
                .map(|path| {
                    path.iter()
                        .map(|segment| match segment {
                            Value::String(value) => value.clone(),
                            other => other.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

fn classify_error(
    error: &RawGraphQlError,
    rate_limit: &RateLimitHeaders,
    sso_url: Option<&str>,
) -> GraphqlErrorKind {
    // A primary GraphQL rate limit is signalled by the header, not the type.
    if rate_limit.remaining == Some(0) {
        return GraphqlErrorKind::RateLimitPrimary;
    }
    if let Some(url) = sso_url {
        return GraphqlErrorKind::SsoRequired {
            authorization_url: Some(url.to_string()),
        };
    }
    match error.error_type.as_deref() {
        Some("NOT_FOUND") => GraphqlErrorKind::NotFound,
        Some("UNAUTHORIZED") => GraphqlErrorKind::Unauthorized,
        Some("FORBIDDEN") => GraphqlErrorKind::Forbidden,
        Some("RATE_LIMITED") => GraphqlErrorKind::RateLimitSecondary,
        _ => GraphqlErrorKind::Unknown,
    }
}

/// Extract the typed `user` payload, turning a missing/null user into the first
/// classified GraphQL error (or an invalid-response error when there is none).
pub(crate) fn require_user<T: DeserializeOwned>(
    outcome: &GraphQlOutcome,
) -> Result<(T, Vec<ProfileGraphqlError>), ProfileError> {
    let data = outcome
        .data
        .as_ref()
        .ok_or_else(|| no_data_error(outcome))?;
    let user = data.get("user").ok_or_else(|| no_data_error(outcome))?;
    if user.is_null() {
        return Err(no_data_error(outcome));
    }
    let user: T =
        serde_json::from_value(user.clone()).map_err(|_| ProfileError::InvalidResponse {
            message: "invalid GraphQL user payload".into(),
        })?;
    Ok((user, outcome.errors.clone()))
}

fn no_data_error(outcome: &GraphQlOutcome) -> ProfileError {
    match outcome.errors.first() {
        Some(error) => ProfileError::Graphql {
            kind: error.kind.clone(),
            path: error.path.clone(),
        },
        None => ProfileError::InvalidResponse {
            message: "GraphQL response had no user data".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::transport::{MockTransport, json_response};
    use std::sync::Arc;

    fn client(mock: Arc<MockTransport>) -> GitHubClient {
        GitHubClient::new(Default::default(), mock)
    }

    async fn enqueue_body(mock: &MockTransport, status: u16, body: &str) {
        mock.enqueue(Ok(json_response(status, body))).await;
    }

    /// Fixed query + variables must be encoded as a JSON object in a POST body,
    /// with the Authorization header on the trusted origin and the JSON content
    /// type.
    #[tokio::test]
    async fn fixed_query_and_variables_are_encoded_as_json() {
        let mock = Arc::new(MockTransport::new());
        enqueue_body(&mock, 200, r#"{"data":{"ok":true}}"#).await;
        let client = client(mock.clone());
        let token = Pat::from_str("ghp_graphql_secret");
        let query = "query Q($login: String!) { user(login: $login) { id } }";
        let variables = serde_json::json!({ "login": "octocat" });

        let outcome = run_query(
            &client,
            &token,
            query,
            &variables,
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("query");

        assert_eq!(outcome.data.unwrap()["ok"], serde_json::json!(true));

        let recorded = mock.recorded_requests().await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].method,
            crate::network::transport::HttpMethod::Post
        );
        assert!(recorded[0].url.ends_with("/graphql"));
        let sent: Value =
            serde_json::from_str(recorded[0].body.as_deref().expect("body")).expect("json");
        assert_eq!(sent["query"], serde_json::json!(query));
        assert_eq!(sent["variables"]["login"], serde_json::json!("octocat"));
        assert!(
            recorded[0]
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer ghp_graphql_secret")
        );
        assert!(
            recorded[0]
                .headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/json")
        );
    }

    /// `Debug` must not print the query body (variables may carry data that
    /// should not reach logs).
    #[test]
    fn request_debug_redacts_the_body() {
        let request = crate::network::transport::HttpRequest::post(
            "https://api.github.com/graphql",
            r#"{"variables":{"login":"private-login-value"}}"#,
        );
        let rendered = format!("{request:?}");
        assert!(
            !rendered.contains("private-login-value"),
            "Debug must not print the GraphQL body: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
    }

    #[tokio::test]
    async fn http_200_with_only_errors_has_no_data() {
        let mock = Arc::new(MockTransport::new());
        enqueue_body(
            &mock,
            200,
            r#"{"data":null,"errors":[{"type":"NOT_FOUND","path":["user"],"message":"Could not resolve to a User"}]}"#,
        )
        .await;
        let client = client(mock);
        let outcome = run_query(
            &client,
            &Pat::from_str("ghp_test"),
            "query { viewer { login } }",
            &serde_json::json!({}),
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("query");
        assert!(outcome.data.is_none());
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].kind, GraphqlErrorKind::NotFound);
        assert_eq!(outcome.errors[0].path, vec!["user".to_string()]);
    }

    /// A 200 with both `data` and `errors` is a partial result: the data is
    /// retained and the errors are classified with their path.
    #[tokio::test]
    async fn http_200_with_data_and_errors_is_partial() {
        let mock = Arc::new(MockTransport::new());
        enqueue_body(
            &mock,
            200,
            r#"{"data":{"user":{"login":"octocat"}},"errors":[{"type":"FORBIDDEN","path":["user","contributionsCollection"],"message":"Resource not accessible"}]}"#,
        )
        .await;
        let client = client(mock);
        let outcome = run_query(
            &client,
            &Pat::from_str("ghp_test"),
            "query { user(login: \"octocat\") { login } }",
            &serde_json::json!({}),
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("query");
        assert_eq!(outcome.data.unwrap()["user"]["login"], "octocat");
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].kind, GraphqlErrorKind::Forbidden);
        assert_eq!(
            outcome.errors[0].path,
            vec!["user".to_string(), "contributionsCollection".to_string()]
        );
    }

    /// Primary GraphQL rate limiting is a 200 with `x-ratelimit-remaining: 0`,
    /// not a 4xx status. It must classify as a rate limit, not as success.
    #[tokio::test]
    async fn graphql_primary_rate_limit_is_classified_from_the_header() {
        let mock = Arc::new(MockTransport::new());
        let mut response = json_response(
            200,
            r#"{"data":null,"errors":[{"type":"RATE_LIMITED","path":["user"],"message":"API rate limit exceeded"}]}"#,
        );
        // `json_response` seeds a non-zero remaining header; overwrite it so the
        // primary-limit signal is unambiguous.
        for (name, value) in response.headers.iter_mut() {
            if name == "x-ratelimit-remaining" {
                *value = "0".to_string();
            }
        }
        mock.enqueue(Ok(response)).await;
        let client = client(mock);
        let outcome = run_query(
            &client,
            &Pat::from_str("ghp_test"),
            "query { viewer { login } }",
            &serde_json::json!({}),
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("query");
        assert_eq!(outcome.errors[0].kind, GraphqlErrorKind::RateLimitPrimary);
    }

    /// A POST redirect is surfaced, never followed, and the body is not
    /// re-sent.
    #[tokio::test]
    async fn post_redirect_is_not_followed() {
        use crate::network::test_server::{LocalHttpServer, ScriptedResponse};
        let server = LocalHttpServer::start(vec![
            ScriptedResponse::json(302, "{}").with_header("Location", "/graphql-elsewhere"),
        ]);
        let transport = Arc::new(
            crate::network::transport::ReqwestTransport::new(std::time::Duration::from_secs(5))
                .expect("client"),
        );
        let client = GitHubClient::new(
            crate::github::GitHubConfig {
                base_url: server.base_url().to_string(),
                ..Default::default()
            },
            transport,
        );
        let result = run_query(
            &client,
            &Pat::from_str("ghp_local_mock"),
            "query { viewer { login } }",
            &serde_json::json!({}),
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ProfileError::RedirectNotFollowed { status: 302 })
        ));
        // Only the original POST was made.
        assert_eq!(server.recorded().len(), 1);
    }

    #[tokio::test]
    async fn non_2xx_status_reuses_the_typed_github_error() {
        let mock = Arc::new(MockTransport::new());
        enqueue_body(&mock, 401, r#"{"message":"Bad credentials"}"#).await;
        let client = client(mock);
        let result = run_query(
            &client,
            &Pat::from_str("ghp_test"),
            "query { viewer { login } }",
            &serde_json::json!({}),
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ProfileError::GitHub(GitHubError::Unauthorized))
        ));
    }

    #[tokio::test]
    async fn oversized_graphql_body_is_rejected() {
        let mock = Arc::new(MockTransport::new());
        let big = format!(r#"{{"data":{{"x":"{}"}}}}"#, "a".repeat(64));
        enqueue_body(&mock, 200, &big).await;
        let client = client(mock);
        let limits = ProfileLimits {
            response_bytes: 16,
            ..ProfileLimits::default()
        };
        let result = run_query(
            &client,
            &Pat::from_str("ghp_test"),
            "query { viewer { login } }",
            &serde_json::json!({}),
            &limits,
            CancellationToken::default(),
        )
        .await;
        assert!(matches!(result, Err(ProfileError::LimitExceeded { .. })));
    }

    #[tokio::test]
    async fn cancelled_graphql_query_returns_cancelled() {
        let mock = Arc::new(MockTransport::new());
        let client = client(mock);
        let (cancel, source) = CancellationToken::new();
        source.cancel();
        let result = run_query(
            &client,
            &Pat::from_str("ghp_test"),
            "query { viewer { login } }",
            &serde_json::json!({}),
            &ProfileLimits::default(),
            cancel,
        )
        .await;
        assert!(matches!(
            result,
            Err(ProfileError::GitHub(GitHubError::Cancelled))
        ));
    }
}
