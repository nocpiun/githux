//! Read-only GitHub REST API adapters for a single Issue and its paginated plain comments.
//!
//! Endpoints:
//! - `GET /repos/{owner}/{repo}/issues/{issue_number}`
//! - `GET /repos/{owner}/{repo}/issues/{issue_number}/comments?per_page={n}&page={p}`
//!
//! Every request is validated through `reqwest::Url` and [`GitHubClient::url_is_trusted`] (L007).
//! Pull-request payloads returned by GitHub's issues endpoint (`pull_request: Some(_)`) are
//! rejected as [`IssueDetailError::NotAnIssue`].

use std::collections::HashSet;

use reqwest::Url;
use serde::Deserialize;

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::issue_detail::{
    IssueAuthor, IssueComment, IssueCommentsCursor, IssueCommentsPage, IssueDetail,
    IssueDetailError, IssueLabel, IssueLocator, IssueState, IssueStateReason,
};
use crate::github::{
    client::GitHubClient,
    error::{classify_status, classify_transport},
    headers::parse_link_next,
};
use crate::network::cancel::CancellationToken;
use crate::network::transport::HttpResponse;

/// Bounded per-request limits for reading an issue and one page of comments.
#[derive(Debug, Clone)]
pub struct IssueDetailLimits {
    pub comments_per_page: u32,
    pub response_bytes: usize,
}

impl Default for IssueDetailLimits {
    fn default() -> Self {
        Self {
            comments_per_page: 30,
            response_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Parsed response alongside HTTP cache directives needed by the data layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueResponse<T> {
    pub value: T,
    pub no_store: bool,
    pub etag: Option<String>,
}

#[derive(Deserialize)]
struct RawUser {
    id: Option<u64>,
    login: String,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct RawLabel {
    name: String,
    color: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct RawIssue {
    id: u64,
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    state_reason: Option<String>,
    #[serde(default)]
    user: Option<RawUser>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    comments: u64,
    #[serde(default)]
    labels: Vec<RawLabel>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    closed_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawComment {
    id: u64,
    #[serde(default)]
    user: Option<RawUser>,
    #[serde(default)]
    body: Option<String>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    author_association: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
}

fn build_issue_url(
    client: &GitHubClient,
    locator: &IssueLocator,
    comments: bool,
) -> Result<Url, IssueDetailError> {
    let mut url = Url::parse(&client.config().base_url)
        .map_err(|_| IssueDetailError::InvalidInput { field: "API base" })?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| IssueDetailError::InvalidInput { field: "API base" })?;
        segments.pop_if_empty();
        segments.push("repos");
        segments.push(locator.owner());
        segments.push(locator.name());
        segments.push("issues");
        segments.push(&locator.number().to_string());
        if comments {
            segments.push("comments");
        }
    }
    if !client.url_is_trusted(url.as_str()) {
        return Err(IssueDetailError::InvalidInput {
            field: "API origin",
        });
    }
    Ok(url)
}

fn response_no_store(response: &HttpResponse) -> bool {
    response.header("cache-control").is_some_and(|header| {
        header
            .split(',')
            .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"))
    })
}

fn normalize_body(raw: Option<String>) -> Option<String> {
    raw.filter(|text| !text.trim().is_empty())
}

fn map_author(raw: Option<RawUser>) -> Option<IssueAuthor> {
    raw.and_then(|user| {
        let login = user.login.trim().to_string();
        if login.is_empty() {
            return None;
        }
        Some(IssueAuthor {
            id: user.id,
            login,
            avatar_url: user.avatar_url.filter(|url| !url.trim().is_empty()),
        })
    })
}

fn check_http_status(response: &HttpResponse) -> Result<(), IssueDetailError> {
    if response.status == 410 {
        return Err(IssueDetailError::Gone);
    }
    if let Some(error) = classify_status(response) {
        return Err(error.into());
    }
    if response.status != 200 {
        return Err(GitHubError::UnexpectedStatus {
            status: response.status,
        }
        .into());
    }
    Ok(())
}

/// Fetch one GitHub Issue by `(owner, repo, number)`.
pub async fn fetch_issue(
    client: &GitHubClient,
    pat: &Pat,
    locator: &IssueLocator,
    limits: &IssueDetailLimits,
    cancel: CancellationToken,
) -> Result<IssueResponse<IssueDetail>, IssueDetailError> {
    if limits.response_bytes == 0 {
        return Err(IssueDetailError::InvalidInput {
            field: "response limit",
        });
    }
    let url = build_issue_url(client, locator, false)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    let response = client
        .transport()
        .execute_bounded(
            client.build_get(url.as_str(), pat),
            cancel.clone(),
            limits.response_bytes,
        )
        .await
        .map_err(classify_transport)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if response.body.len() > limits.response_bytes {
        return Err(GitHubError::ResponseTooLarge {
            limit: limits.response_bytes,
        }
        .into());
    }
    check_http_status(&response)?;

    let raw: RawIssue =
        serde_json::from_str(&response.body).map_err(|error| GitHubError::Parse {
            message: error.to_string(),
        })?;
    if raw.pull_request.is_some() {
        return Err(IssueDetailError::NotAnIssue);
    }
    if raw.number == 0 {
        return Err(IssueDetailError::InvalidResponse {
            message: "issue number must be positive".into(),
        });
    }

    let detail = IssueDetail {
        id: raw.id,
        number: raw.number,
        repository: locator.repository().clone(),
        title: raw.title,
        state: IssueState::parse(&raw.state),
        state_reason: raw
            .state_reason
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(IssueStateReason::parse),
        author: map_author(raw.user),
        body: normalize_body(raw.body),
        comments_count: raw.comments,
        labels: raw
            .labels
            .into_iter()
            .filter(|label| !label.name.trim().is_empty())
            .map(|label| IssueLabel {
                name: label.name,
                color: label.color.filter(|c| !c.trim().is_empty()),
                description: label.description.filter(|d| !d.trim().is_empty()),
            })
            .collect(),
        created_at: raw.created_at,
        updated_at: raw.updated_at,
        closed_at: raw.closed_at.filter(|s| !s.trim().is_empty()),
        html_url: raw.html_url.filter(|s| !s.trim().is_empty()),
    };

    Ok(IssueResponse {
        value: detail,
        no_store: response_no_store(&response),
        etag: response.header("etag").map(str::to_string),
    })
}

/// Fetch one bounded page of plain comments for a single Issue.
pub async fn fetch_comments_page(
    client: &GitHubClient,
    pat: &Pat,
    locator: &IssueLocator,
    cursor: Option<&IssueCommentsCursor>,
    limits: &IssueDetailLimits,
    cancel: CancellationToken,
) -> Result<IssueResponse<IssueCommentsPage>, IssueDetailError> {
    if !(1..=100).contains(&limits.comments_per_page) || limits.response_bytes == 0 {
        return Err(IssueDetailError::InvalidInput {
            field: "comments limits",
        });
    }
    let endpoint_url = build_issue_url(client, locator, true)?;
    let origin = endpoint_url.as_str().to_string();
    let page = if let Some(cursor) = cursor {
        if !cursor.locator.same_resource(locator)
            || cursor.per_page != limits.comments_per_page
            || cursor.origin != origin
        {
            return Err(IssueDetailError::InvalidInput {
                field: "comments cursor identity",
            });
        }
        cursor.page
    } else {
        1
    };
    if page == 0 {
        return Err(IssueDetailError::InvalidInput {
            field: "comments page",
        });
    }
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }

    let mut url = endpoint_url.clone();
    url.query_pairs_mut()
        .append_pair("per_page", &limits.comments_per_page.to_string())
        .append_pair("page", &page.to_string());

    let response = client
        .transport()
        .execute_bounded(
            client.build_get(url.as_str(), pat),
            cancel.clone(),
            limits.response_bytes,
        )
        .await
        .map_err(classify_transport)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if response.body.len() > limits.response_bytes {
        return Err(GitHubError::ResponseTooLarge {
            limit: limits.response_bytes,
        }
        .into());
    }
    check_http_status(&response)?;

    let raw_comments: Vec<RawComment> =
        serde_json::from_str(&response.body).map_err(|error| GitHubError::Parse {
            message: error.to_string(),
        })?;
    if raw_comments.len() > limits.comments_per_page as usize {
        return Err(IssueDetailError::InvalidResponse {
            message: "oversized issue comments page".into(),
        });
    }

    let mut seen = HashSet::new();
    let mut comments = Vec::with_capacity(raw_comments.len());
    for raw in raw_comments {
        if !seen.insert(raw.id) {
            continue;
        }
        comments.push(IssueComment {
            id: raw.id,
            author: map_author(raw.user),
            body: normalize_body(raw.body),
            created_at: raw.created_at,
            updated_at: raw.updated_at,
            author_association: raw
                .author_association
                .filter(|assoc| !assoc.trim().is_empty() && !assoc.eq_ignore_ascii_case("NONE")),
            html_url: raw.html_url.filter(|s| !s.trim().is_empty()),
        });
    }

    let mut next = None;
    if let Some(target) = parse_link_next(response.header("link"))
        && client.url_is_trusted(&target)
        && let Ok(parsed) = Url::parse(&target)
        && parsed.path() == endpoint_url.path()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
    {
        let pages: Vec<_> = parsed
            .query_pairs()
            .filter(|(key, _)| key == "page")
            .collect();
        if pages.len() == 1
            && let Ok(next_page) = pages[0].1.parse::<u32>()
            && next_page == page.saturating_add(1)
        {
            next = Some(IssueCommentsCursor {
                locator: locator.clone(),
                page: next_page,
                per_page: limits.comments_per_page,
                origin,
            });
        }
    }

    Ok(IssueResponse {
        value: IssueCommentsPage {
            comments,
            next,
            page,
        },
        no_store: response_no_store(&response),
        etag: response.header("etag").map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::client::GitHubConfig;
    use crate::network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    struct MockTransport {
        responses: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl MockTransport {
        fn new(responses: Vec<Result<HttpResponse, TransportError>>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
            })
        }

        fn recorded(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Transport for MockTransport {
        fn execute(
            &self,
            request: HttpRequest,
            _cancel: CancellationToken,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + '_>,
        > {
            self.requests.lock().unwrap().push(request);
            let next = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("mock response");
            Box::pin(async move { next })
        }

        fn execute_bounded(
            &self,
            request: HttpRequest,
            cancel: CancellationToken,
            _max_bytes: usize,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + '_>,
        > {
            self.execute(request, cancel)
        }
    }

    fn client(transport: Arc<MockTransport>) -> GitHubClient {
        GitHubClient::new(GitHubConfig::default(), transport)
    }

    fn pat() -> Pat {
        Pat::new("ghp_test_token_1234567890".to_string())
    }

    fn json_response(status: u16, body: &str, headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_string(),
        }
    }

    #[tokio::test]
    async fn fetches_issue_with_body_author_labels_and_cache_headers() {
        let transport = MockTransport::new(vec![Ok(json_response(
            200,
            r#"{
                "id": 9001,
                "number": 42,
                "title": "Render issue detail",
                "state": "open",
                "state_reason": null,
                "user": { "id": 7, "login": "octocat", "avatar_url": "https://avatars.githubusercontent.com/u/7" },
                "body": "Hello **markdown**",
                "comments": 2,
                "labels": [{ "name": "bug", "color": "d73a4a", "description": "Something is broken" }],
                "created_at": "2026-11-01T10:00:00Z",
                "updated_at": "2026-11-02T12:00:00Z",
                "closed_at": null,
                "html_url": "https://github.com/acme/widgets/issues/42"
            }"#,
            &[("etag", "\"abc\""), ("cache-control", "private, no-store")],
        ))]);
        let locator = IssueLocator::new("acme", "widgets", 42).unwrap();
        let response = fetch_issue(
            &client(transport.clone()),
            &pat(),
            &locator,
            &IssueDetailLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap();

        assert!(response.no_store);
        assert_eq!(response.etag.as_deref(), Some("\"abc\""));
        assert_eq!(response.value.number, 42);
        assert_eq!(response.value.title, "Render issue detail");
        assert_eq!(response.value.state, IssueState::Open);
        assert_eq!(
            response.value.author.as_ref().map(|a| a.login.as_str()),
            Some("octocat")
        );
        assert_eq!(response.value.body.as_deref(), Some("Hello **markdown**"));
        assert_eq!(response.value.labels.len(), 1);
        assert_eq!(
            transport.recorded()[0].url,
            "https://api.github.com/repos/acme/widgets/issues/42"
        );
    }

    #[tokio::test]
    async fn normalizes_empty_body_and_handles_deleted_author() {
        let transport = MockTransport::new(vec![Ok(json_response(
            200,
            r#"{
                "id": 9002,
                "number": 7,
                "title": "No body issue",
                "state": "closed",
                "state_reason": "not_planned",
                "user": null,
                "body": "   \n  ",
                "comments": 0,
                "labels": [],
                "created_at": "2026-11-01T10:00:00Z",
                "updated_at": "2026-11-01T11:00:00Z",
                "closed_at": "2026-11-01T11:00:00Z"
            }"#,
            &[],
        ))]);
        let locator = IssueLocator::new("acme", "widgets", 7).unwrap();
        let response = fetch_issue(
            &client(transport),
            &pat(),
            &locator,
            &IssueDetailLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap();

        assert_eq!(response.value.body, None);
        assert!(!response.value.has_body());
        assert_eq!(response.value.author, None);
        assert_eq!(response.value.state, IssueState::Closed);
        assert_eq!(
            response.value.state_reason,
            Some(IssueStateReason::NotPlanned)
        );
    }

    #[tokio::test]
    async fn rejects_pull_request_payload_as_not_an_issue() {
        let transport = MockTransport::new(vec![Ok(json_response(
            200,
            r#"{
                "id": 9003,
                "number": 42,
                "title": "Actually a PR",
                "state": "open",
                "user": { "id": 7, "login": "octocat" },
                "body": "PR body",
                "comments": 0,
                "created_at": "2026-11-01T10:00:00Z",
                "updated_at": "2026-11-01T10:00:00Z",
                "pull_request": { "url": "https://api.github.com/repos/acme/widgets/pulls/42" }
            }"#,
            &[],
        ))]);
        let locator = IssueLocator::new("acme", "widgets", 42).unwrap();
        let err = fetch_issue(
            &client(transport),
            &pat(),
            &locator,
            &IssueDetailLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, IssueDetailError::NotAnIssue);
    }

    #[tokio::test]
    async fn classifies_missing_gone_and_permission_statuses() {
        for (status, expected) in [
            (404, IssueDetailError::GitHub(GitHubError::NotFound)),
            (410, IssueDetailError::Gone),
            (401, IssueDetailError::GitHub(GitHubError::Unauthorized)),
            (403, IssueDetailError::GitHub(GitHubError::Forbidden)),
        ] {
            let transport = MockTransport::new(vec![Ok(json_response(status, "{}", &[]))]);
            let locator = IssueLocator::new("acme", "widgets", 5).unwrap();
            let err = fetch_issue(
                &client(transport),
                &pat(),
                &locator,
                &IssueDetailLimits::default(),
                CancellationToken::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(err, expected, "status {status}");
        }
    }

    #[tokio::test]
    async fn paginates_comments_and_rejects_mismatched_cursor_or_untrusted_link() {
        let transport = MockTransport::new(vec![
            Ok(json_response(
                200,
                r#"[{
                    "id": 101,
                    "user": { "id": 8, "login": "alice" },
                    "body": "First comment",
                    "created_at": "2026-11-01T12:00:00Z",
                    "updated_at": "2026-11-01T12:00:00Z",
                    "author_association": "CONTRIBUTOR"
                }]"#,
                &[(
                    "link",
                    r#"<https://api.github.com/repos/acme/widgets/issues/42/comments?per_page=1&page=2>; rel="next""#,
                )],
            )),
            Ok(json_response(
                200,
                r#"[{
                    "id": 102,
                    "user": { "id": 9, "login": "bob" },
                    "body": "Second comment",
                    "created_at": "2026-11-01T13:00:00Z",
                    "updated_at": "2026-11-01T13:00:00Z",
                    "author_association": "NONE"
                }]"#,
                &[(
                    "link",
                    r#"<https://evil.example/repos/acme/widgets/issues/42/comments?per_page=1&page=3>; rel="next""#,
                )],
            )),
        ]);
        let locator = IssueLocator::new("acme", "widgets", 42).unwrap();
        let other = IssueLocator::new("acme", "other", 42).unwrap();
        let limits = IssueDetailLimits {
            comments_per_page: 1,
            response_bytes: 1024 * 1024,
        };
        let gh = client(transport);

        let page1 = fetch_comments_page(
            &gh,
            &pat(),
            &locator,
            None,
            &limits,
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(page1.value.comments.len(), 1);
        assert_eq!(
            page1.value.comments[0].author_association.as_deref(),
            Some("CONTRIBUTOR")
        );
        let cursor = page1.value.next.clone().expect("page 2 cursor");
        assert_eq!(cursor.page(), 2);

        // Cursor from acme/widgets #42 cannot be used on acme/other #42.
        let mismatch = fetch_comments_page(
            &gh,
            &pat(),
            &other,
            Some(&cursor),
            &limits,
            CancellationToken::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            mismatch,
            IssueDetailError::InvalidInput {
                field: "comments cursor identity"
            }
        );

        let page2 = fetch_comments_page(
            &gh,
            &pat(),
            &locator,
            Some(&cursor),
            &limits,
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(page2.value.comments.len(), 1);
        assert_eq!(page2.value.comments[0].author_association, None);
        assert!(
            page2.value.next.is_none(),
            "untrusted host next link must be ignored"
        );
    }
}
