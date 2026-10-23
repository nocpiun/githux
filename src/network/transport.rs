//! HTTP transport abstraction with a reqwest implementation and a mock for tests.

use crate::network::cancel::CancellationToken;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// HTTP method. Only reads (`GET`) are exercised by the first-release GitHub
/// layer; future write actions remain a boundary, not an implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
}

/// A prepared outbound request. Headers are owned strings; the GitHub layer is
/// responsible for injecting Authorization only for trusted hosts.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// Rate-limit snapshot parsed from response headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimitHeaders {
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    pub reset: Option<u64>,
    pub retry_after: Option<u32>,
    pub resource: Option<String>,
}

/// A received HTTP response. Headers are lower-cased for case-insensitive lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn rate_limit(&self) -> RateLimitHeaders {
        RateLimitHeaders {
            limit: self
                .header("x-ratelimit-limit")
                .and_then(|v| v.parse().ok()),
            remaining: self
                .header("x-ratelimit-remaining")
                .and_then(|v| v.parse().ok()),
            reset: self
                .header("x-ratelimit-reset")
                .and_then(|v| v.parse().ok()),
            retry_after: self.header("retry-after").and_then(|v| v.parse().ok()),
            resource: self.header("x-ratelimit-resource").map(|s| s.to_string()),
        }
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum TransportError {
    #[error("network error: {message}")]
    Network { message: String },
    #[error("request timed out")]
    Timeout,
    #[error("request cancelled")]
    Cancelled,
}

/// Trait-object-friendly transport. The returned future is `Send` so it can be
/// driven by the background executor.
pub trait Transport: Send + Sync {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>;
}

/// reqwest-backed production transport.
///
/// Redirects are **not** followed by reqwest (`Policy::none`). We follow them
/// manually and only when the target keeps the same scheme and host as the
/// original request, so an `Authorization` header can never be forwarded to a
/// different origin or downgraded from HTTPS to HTTP.
pub struct ReqwestTransport {
    client: reqwest::Client,
    timeout: Duration,
    max_redirects: usize,
}

impl ReqwestTransport {
    pub fn new(timeout: Duration) -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| TransportError::Network {
                message: e.to_string(),
            })?;
        Ok(Self {
            client,
            timeout,
            max_redirects: 5,
        })
    }

    pub fn with_client(client: reqwest::Client, timeout: Duration) -> Self {
        Self {
            client,
            timeout,
            max_redirects: 5,
        }
    }
}

impl Transport for ReqwestTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            // Deterministic early exit: an already-cancelled request must not
            // open a connection at all.
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            let origin = Origin::parse(&request.url);
            let mut url = request.url.clone();
            let mut redirects = 0usize;

            loop {
                let mut builder = self
                    .client
                    .request(reqwest::Method::GET, &url)
                    .timeout(self.timeout);
                for (name, value) in &request.headers {
                    builder = builder.header(name.as_str(), value.as_str());
                }

                let send = builder.send();
                let response = tokio::select! {
                    r = send => r.map_err(|e| {
                        // Cancellation is reported as `Cancelled` even if tearing
                        // the request down also surfaces a connect/IO error. The
                        // caller asked to stop, so that is the meaningful result.
                        if cancel.is_cancelled() {
                            TransportError::Cancelled
                        } else if e.is_timeout() {
                            TransportError::Timeout
                        } else {
                            TransportError::Network { message: e.to_string() }
                        }
                    })?,
                    _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled),
                };

                let status = response.status().as_u16();
                let mut headers = Vec::new();
                for (name, value) in response.headers() {
                    if let Ok(v) = value.to_str() {
                        headers.push((name.as_str().to_lowercase(), v.to_string()));
                    }
                }

                if (300..400).contains(&status) {
                    let location = headers
                        .iter()
                        .find(|(k, _)| k == "location")
                        .map(|(_, v)| v.clone());
                    if let Some(location) = location {
                        let next = match Origin::resolve(&url, &location) {
                            Some(next) => next,
                            None => {
                                return Ok(HttpResponse {
                                    status,
                                    headers,
                                    body: String::new(),
                                });
                            }
                        };
                        if redirects >= self.max_redirects || Origin::parse(&next) != origin {
                            // Do not follow cross-origin / downgraded / looping
                            // redirects: return the 3xx so the caller rejects it
                            // rather than leaking the credential.
                            return Ok(HttpResponse {
                                status,
                                headers,
                                body: String::new(),
                            });
                        }
                        redirects += 1;
                        url = next;
                        continue;
                    }
                }

                // Cancellation must cover the body read too, not just the
                // header phase: a large or stalled body would otherwise keep
                // running until the request timeout even after the caller
                // cancelled.
                let body = tokio::select! {
                    body = response.text() => body.map_err(|e| {
                        if cancel.is_cancelled() {
                            TransportError::Cancelled
                        } else {
                            TransportError::Network { message: e.to_string() }
                        }
                    })?,
                    _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled),
                };
                return Ok(HttpResponse {
                    status,
                    headers,
                    body,
                });
            }
        })
    }
}

/// Scheme + host + port tuple used to decide whether a redirect stays on the
/// same origin.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl Origin {
    fn parse(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        let authority = rest.split(['/', '?', '#']).next()?;
        let authority = authority.rsplit('@').next()?;
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => {
                (h.to_string(), p.parse().ok())
            }
            _ => (authority.to_string(), None),
        };
        Some(Self {
            scheme: scheme.to_lowercase(),
            host: host.to_lowercase(),
            port,
        })
    }

    /// Resolve a `Location` header against the current URL. Supports absolute
    /// URLs and absolute paths; rejects other relative forms. Returns `None`
    /// when the location cannot be safely resolved.
    fn resolve(current: &str, location: &str) -> Option<String> {
        if location.starts_with("https://") || location.starts_with("http://") {
            return Some(location.to_string());
        }
        if let Some(path) = location.strip_prefix('/') {
            let (scheme, rest) = current.split_once("://")?;
            let authority = rest.split(['/', '?', '#']).next()?;
            return Some(format!("{scheme}://{authority}/{path}"));
        }
        None
    }
}

/// Type alias for shared transports.
pub type SharedTransport = Arc<dyn Transport>;

/// Scripted mock transport for tests. Each call consumes the next programmed
/// response. It records requests so tests can assert Authorization injection and
/// redirect handling without any real network.
pub struct MockTransport {
    responses: tokio::sync::Mutex<Vec<MockResponse>>,
    recorded: tokio::sync::Mutex<Vec<HttpRequest>>,
}

pub type MockResponse = Result<HttpResponse, MockFailure>;

#[derive(Debug, Clone)]
pub enum MockFailure {
    Timeout,
    Network(String),
}

impl MockTransport {
    pub fn new() -> Self {
        Self {
            responses: tokio::sync::Mutex::new(Vec::new()),
            recorded: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    pub async fn enqueue(&self, response: MockResponse) {
        self.responses.lock().await.push(response);
    }

    pub async fn enqueue_many(&self, responses: Vec<MockResponse>) {
        self.responses.lock().await.extend(responses);
    }

    pub async fn recorded(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.recorded
            .lock()
            .await
            .iter()
            .map(|r| (r.url.clone(), r.headers.clone()))
            .collect()
    }

    async fn next_response(&self) -> Option<MockResponse> {
        self.responses.lock().await.pop()
    }

    /// Responses are consumed FIFO from the front (index 0).
    async fn next_response_fifo(&self) -> Option<MockResponse> {
        let mut guard = self.responses.lock().await;
        if guard.is_empty() {
            None
        } else {
            Some(guard.remove(0))
        }
    }
}

impl Transport for MockTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            self.recorded.lock().await.push(request.clone());
            let programmed = self.next_response_fifo().await;
            tokio::select! {
                _ = cancel.clone().cancelled() => Err(TransportError::Cancelled),
                _ = tokio::time::sleep(Duration::from_millis(0)) => {
                    match programmed {
                        None => Err(TransportError::Network { message: "no mock response queued".to_string() }),
                        Some(Ok(resp)) => Ok(resp),
                        Some(Err(MockFailure::Timeout)) => Err(TransportError::Timeout),
                        Some(Err(MockFailure::Network(m))) => Err(TransportError::Network { message: m }),
                    }
                }
            }
        })
    }
}

/// Helper used by tests that need a simple JSON body.
pub fn json_response(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("x-ratelimit-limit".to_string(), "5000".to_string()),
            ("x-ratelimit-remaining".to_string(), "4999".to_string()),
        ],
        body: body.to_string(),
    }
}

/// Header-map helper for constructing responses in tests.
pub fn headers(map: HashMap<&str, &str>) -> Vec<(String, String)> {
    map.into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::test_server::{LocalHttpServer, ScriptedResponse};

    fn json_user() -> String {
        serde_json::json!({
            "id": 4242,
            "login": "localmock",
            "avatar_url": "https://example.invalid/a.png",
            "html_url": "https://example.invalid/localmock"
        })
        .to_string()
    }

    /// Real socket round-trip against a localhost mock. Confirms the production
    /// reqwest transport sends the fixed headers, the Authorization header and
    /// reads a JSON body without touching GitHub.
    #[tokio::test]
    async fn real_localhost_request_carries_authorization() {
        let server = LocalHttpServer::start(vec![ScriptedResponse::json(200, &json_user())]);
        let transport = ReqwestTransport::new(Duration::from_secs(5)).expect("client");

        let base = server.base_url().to_string();
        let client = crate::github::client::GitHubClient::new(
            crate::github::client::GitHubConfig {
                base_url: base.clone(),
                ..Default::default()
            },
            Arc::new(crate::network::transport::MockTransport::new()),
        );
        let pat = crate::auth::token::Pat::from_str("ghp_local_mock_token");
        let request = client.build_get("user", &pat);

        let response = transport
            .execute(request, CancellationToken::default())
            .await
            .expect("request should succeed");
        assert_eq!(response.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&response.body).expect("json");
        assert_eq!(parsed["id"], 4242);

        let recorded = server.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].header("authorization"),
            Some("Bearer ghp_local_mock_token")
        );
        assert_eq!(
            recorded[0].header("user-agent"),
            Some("githux/0.1 (+https://github.com/nocpiun/githux)")
        );
        assert_eq!(
            recorded[0].header("x-github-api-version"),
            Some("2022-11-28")
        );
        assert_eq!(
            recorded[0].header("accept"),
            Some("application/vnd.github+json")
        );
    }

    /// Real socket round-trip: a same-origin redirect is followed and the
    /// credential stays on the trusted origin.
    #[tokio::test]
    async fn same_origin_redirect_is_followed() {
        let server = LocalHttpServer::start(vec![
            ScriptedResponse::json(301, "{}").with_header("Location", "/user/repos"),
            ScriptedResponse::json(200, "[]"),
        ]);
        let transport = ReqwestTransport::new(Duration::from_secs(5)).expect("client");
        let request = HttpRequest::get(format!("{}user", server.base_url()))
            .with_header("Authorization", "Bearer ghp_local_mock_token");

        let response = transport
            .execute(request, CancellationToken::default())
            .await
            .expect("request should succeed");
        assert_eq!(response.status, 200);
        let recorded = server.recorded();
        assert_eq!(recorded.len(), 2);
        // Credential is still injected on the second same-origin hop.
        assert_eq!(
            recorded[1].header("authorization"),
            Some("Bearer ghp_local_mock_token")
        );
    }

    /// Real socket round-trip: a cross-origin redirect is NOT followed. The 3xx
    /// is surfaced so the GitHub layer rejects it instead of leaking the token.
    #[tokio::test]
    async fn cross_origin_redirect_is_not_followed() {
        let server = LocalHttpServer::start(vec![
            ScriptedResponse::json(302, "{}")
                .with_header("Location", "http://127.0.0.1:1/other-host"),
        ]);
        let transport = ReqwestTransport::new(Duration::from_secs(5)).expect("client");
        let request = HttpRequest::get(format!("{}user", server.base_url()))
            .with_header("Authorization", "Bearer ghp_local_mock_token");

        let response = transport
            .execute(request, CancellationToken::default())
            .await
            .expect("transport returns the 3xx");
        assert_eq!(response.status, 302);
        // Only the original request was made; we never hit the redirect target.
        assert_eq!(server.recorded().len(), 1);
    }

    /// Real socket round-trip: server-side failure is classified from headers.
    #[tokio::test]
    async fn rate_limited_response_is_classified() {
        let server = LocalHttpServer::start(vec![
            ScriptedResponse::json(403, r#"{"message":"API rate limit exceeded"}"#)
                .with_header("X-RateLimit-Remaining", "0")
                .with_header("X-RateLimit-Reset", "1700000000")
                .with_header("Retry-After", "120"),
        ]);
        let transport = ReqwestTransport::new(Duration::from_secs(5)).expect("client");
        let request = HttpRequest::get(format!("{}user", server.base_url()));
        let response = transport
            .execute(request, CancellationToken::default())
            .await
            .expect("transport ok");

        let err = crate::github::error::classify_status(&response).expect("must classify");
        match err {
            crate::domain::GitHubError::RateLimited { retry } => {
                assert_eq!(retry.seconds, Some(120));
                assert_eq!(retry.kind, crate::domain::error::RateLimitKind::Primary);
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
        let snapshot = response.rate_limit();
        assert_eq!(snapshot.remaining, Some(0));
        assert_eq!(snapshot.reset, Some(1_700_000_000));
    }

    /// Real socket round-trip: SSO partial-results header is detected on a 200.
    #[tokio::test]
    async fn sso_header_parsed_from_real_response() {
        let server = LocalHttpServer::start(vec![
            ScriptedResponse::json(200, "[]")
                .with_header("X-GitHub-SSO", "partial-results; organizations=acme"),
        ]);
        let transport = ReqwestTransport::new(Duration::from_secs(5)).expect("client");
        let request = HttpRequest::get(format!("{}user/repos", server.base_url()));
        let response = transport
            .execute(request, CancellationToken::default())
            .await
            .expect("transport ok");
        assert_eq!(
            crate::github::headers::parse_sso_header(response.header("x-github-sso")),
            Some(vec!["acme".to_string()])
        );
    }

    /// Real socket round-trip: cancellation stops the request without waiting
    /// for the server, and an independent request still completes.
    #[tokio::test]
    async fn cancelled_request_does_not_affect_independent_request() {
        let server = LocalHttpServer::start(vec![ScriptedResponse::json(200, "[]")]);
        let transport = Arc::new(ReqwestTransport::new(Duration::from_secs(5)).expect("client"));
        let url = format!("{}user", server.base_url());

        let (cancel, source) = CancellationToken::new();
        source.cancel();
        let cancelled = transport
            .clone()
            .execute(HttpRequest::get(&url), cancel)
            .await;
        assert_eq!(cancelled, Err(TransportError::Cancelled));

        let ok = transport
            .execute(HttpRequest::get(&url), CancellationToken::default())
            .await
            .expect("independent request succeeds");
        assert_eq!(ok.status, 200);
    }

    /// Real socket round-trip: timeout classification.
    #[tokio::test]
    async fn timeout_is_classified() {
        // A server that accepts but never answers drives the client timeout.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            let _keep = listener.accept();
            std::thread::sleep(Duration::from_millis(500));
        });

        let transport = ReqwestTransport::new(Duration::from_millis(60)).expect("client");
        let response = transport
            .execute(
                HttpRequest::get(format!("http://{addr}/user")),
                CancellationToken::default(),
            )
            .await;
        assert_eq!(response, Err(TransportError::Timeout));
        let _ = handle.join();
    }

    /// Regression: cancelling after the headers arrive but *while the body is
    /// still streaming* must stop the read. A server that sends headers with a
    /// large `Content-Length` and then stalls drives this path.
    ///
    /// Deterministic: the server signals over a channel once the headers are on
    /// the wire, and only then does the test cancel, so the cancel lands in the
    /// body-read phase rather than racing the connect.
    #[tokio::test]
    async fn cancellation_stops_a_stalled_body_read() {
        use std::io::Write;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (headers_sent_tx, headers_sent_rx) = std::sync::mpsc::channel::<()>();

        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Read the request head first, like a real HTTP/1.1 server, so
                // the client is not racing a response that arrives before it has
                // finished writing the request.
                let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone stream"));
                let mut request_head = String::new();
                loop {
                    let mut line = String::new();
                    if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let done = line == "\r\n" || line == "\n";
                    request_head.push_str(&line);
                    if done {
                        break;
                    }
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n[",
                );
                let _ = stream.flush();
                // Tell the test the headers are on the wire and the body is
                // stalled, then hold the connection open.
                let _ = headers_sent_tx.send(());
                std::thread::sleep(Duration::from_secs(10));
            }
        });

        let transport = ReqwestTransport::new(Duration::from_secs(30)).expect("client");
        let (cancel, source) = CancellationToken::new();

        let request = tokio::spawn(async move {
            transport
                .execute(HttpRequest::get(format!("http://{addr}/user")), cancel)
                .await
        });

        // Wait until the server has actually written the headers, so the client
        // is blocked on the body, then cancel.
        tokio::task::spawn_blocking(move || headers_sent_rx.recv())
            .await
            .expect("join")
            .expect("headers sent signal");
        source.cancel();

        let outcome = tokio::time::timeout(Duration::from_secs(5), request)
            .await
            .expect("cancellation must resolve well before the request timeout")
            .expect("join request task");
        assert_eq!(outcome, Err(TransportError::Cancelled));

        // The server thread sleeps for 10s; detach it (it holds no shared state).
        drop(handle);
    }
}
