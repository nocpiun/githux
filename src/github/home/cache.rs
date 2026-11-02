//! Private page-adapter response recording/replay. Never wraps the shared client
//! or changes the application's transport; only CachedPageReadSession uses it.
use crate::network::{
    cancel::CancellationToken,
    transport::{HttpRequest, HttpResponse, Transport, TransportError},
};
use serde::{Deserialize, Serialize};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Exchange {
    request: String,
    response: Option<Response>,
    failure: Option<Failure>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}
#[derive(Clone, Serialize, Deserialize)]
enum Failure {
    Network,
    Timeout(u32),
    TooLarge(usize),
    Unsupported,
    Cancelled,
}
impl Failure {
    fn from_error(error: &TransportError) -> Self {
        match error {
            TransportError::Network { .. } => Self::Network,
            TransportError::Timeout { seconds } => Self::Timeout(*seconds),
            TransportError::ResponseTooLarge { limit } => Self::TooLarge(*limit),
            TransportError::BoundedReadUnsupported => Self::Unsupported,
            TransportError::Cancelled => Self::Cancelled,
        }
    }
    fn error(&self) -> TransportError {
        match self {
            Self::Network => TransportError::Network {
                message: "recorded page read failed".into(),
            },
            Self::Timeout(seconds) => TransportError::Timeout { seconds: *seconds },
            Self::TooLarge(limit) => TransportError::ResponseTooLarge { limit: *limit },
            Self::Unsupported => TransportError::BoundedReadUnsupported,
            Self::Cancelled => TransportError::Cancelled,
        }
    }
}
fn identity(request: &HttpRequest) -> String {
    // Deliberately excludes Authorization, cookies and every other header except
    // the two representation headers. GraphQL contains fixed queries/variables.
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    };
    serde_json::json!([
        format!("{:?}", request.method),
        request.url,
        request.body,
        header("accept"),
        header("x-github-api-version")
    ])
    .to_string()
}
fn safe_header(name: &str) -> bool {
    [
        "etag",
        "last-modified",
        "cache-control",
        "link",
        "x-poll-interval",
        "x-github-sso",
        "x-ratelimit-limit",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
        "retry-after",
        "x-ratelimit-resource",
    ]
    .iter()
    .any(|allowed| name.eq_ignore_ascii_case(allowed))
}
// Download locators aren't used by any of these adapters. Do not persist them.
fn strip_download_urls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("download_url");
            for child in map.values_mut() {
                strip_download_urls(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                strip_download_urls(child);
            }
        }
        _ => {}
    }
}
pub(crate) struct PageTransport {
    upstream: Option<Arc<dyn Transport>>,
    exchanges: Mutex<Vec<Exchange>>,
    no_store: std::sync::atomic::AtomicBool,
    rejected: std::sync::atomic::AtomicBool,
}
impl PageTransport {
    pub fn recording(upstream: Arc<dyn Transport>) -> Self {
        Self {
            upstream: Some(upstream),
            exchanges: Mutex::new(Vec::new()),
            no_store: false.into(),
            rejected: false.into(),
        }
    }
    pub fn replay(exchanges: Vec<Exchange>) -> Self {
        Self {
            upstream: None,
            exchanges: Mutex::new(exchanges),
            no_store: false.into(),
            rejected: false.into(),
        }
    }
    pub fn rejected(&self) -> bool {
        self.rejected.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn exchanges(&self) -> Vec<Exchange> {
        self.exchanges.lock().unwrap().clone()
    }
    pub fn no_store(&self) -> bool {
        self.no_store.load(std::sync::atomic::Ordering::Acquire)
    }
    async fn run(
        &self,
        request: HttpRequest,
        cancel: CancellationToken,
        bound: Option<usize>,
    ) -> Result<HttpResponse, TransportError> {
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        let key = identity(&request);
        let Some(upstream) = &self.upstream else {
            let mut exchanges = self.exchanges.lock().unwrap();
            let Some(index) = exchanges.iter().position(|entry| entry.request == key) else {
                return Err(TransportError::Network {
                    message: "page representation was not cached".into(),
                });
            };
            let exchange = exchanges.remove(index);
            return match exchange.response {
                Some(value) if bound.is_some_and(|limit| value.body.len() > limit) => {
                    Err(TransportError::ResponseTooLarge {
                        limit: bound.unwrap(),
                    })
                }
                Some(value) => Ok(HttpResponse {
                    status: value.status,
                    headers: value.headers,
                    body: value.body,
                }),
                None => Err(exchange.failure.unwrap_or(Failure::Network).error()),
            };
        };
        // Page aggregates deliberately refresh unconditionally: one 304 cannot
        // validate sibling pages or Feed's following/author dependencies.
        let response = match bound {
            Some(limit) => {
                upstream
                    .execute_bounded(request.clone(), cancel.clone(), limit)
                    .await
            }
            None => upstream.execute(request.clone(), cancel.clone()).await,
        };
        if response.as_ref().is_ok_and(|value| {
            value.header("cache-control").is_some_and(|v| {
                v.split(',').any(|d| {
                    d.split('=')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("no-store")
                })
            })
        }) {
            self.no_store
                .store(true, std::sync::atomic::Ordering::Release);
        }
        // An unsolicited 304 has no matching conditional representation. Retry
        // once without validators; never turn it into a fabricated empty body.
        let response = if response
            .as_ref()
            .is_ok_and(|response| response.status == 304)
        {
            let mut unconditional = request;
            unconditional.headers.retain(|(name, _)| {
                !name.eq_ignore_ascii_case("if-none-match")
                    && !name.eq_ignore_ascii_case("if-modified-since")
            });
            match bound {
                Some(limit) => upstream.execute_bounded(unconditional, cancel, limit).await,
                None => upstream.execute(unconditional, cancel).await,
            }
        } else {
            response
        };
        let entry = match &response {
            Ok(value) => {
                if value.status == 401 {
                    self.rejected
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                if value.header("cache-control").is_some_and(|v| {
                    v.split(',').any(|d| {
                        d.split('=')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .eq_ignore_ascii_case("no-store")
                    })
                }) {
                    self.no_store
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                let mut body = if (200..300).contains(&value.status) {
                    value.body.clone()
                } else {
                    "{}".into()
                };
                let original_length = body.len();
                if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&body) {
                    // GraphQL error messages can echo input; adapters only need
                    // the safe type/path fields to reproduce Partial semantics.
                    if let Some(errors) = json
                        .get_mut("errors")
                        .and_then(serde_json::Value::as_array_mut)
                    {
                        for error in errors {
                            if let Some(map) = error.as_object_mut() {
                                map.retain(|key, _| key == "type" || key == "path");
                            }
                        }
                    }
                    strip_download_urls(&mut json);
                    body = json.to_string();
                    // Keep receive/parse budget behavior identical on replay.
                    // Removed sensitive fields become harmless JSON whitespace.
                    if body.len() < original_length {
                        body.extend(std::iter::repeat_n(' ', original_length - body.len()));
                    }
                }
                Exchange {
                    request: key,
                    response: Some(Response {
                        status: value.status,
                        headers: value
                            .headers
                            .iter()
                            .filter(|(k, _)| safe_header(k))
                            .cloned()
                            .collect(),
                        body,
                    }),
                    failure: None,
                }
            }
            Err(error) => Exchange {
                request: key,
                response: None,
                failure: Some(Failure::from_error(error)),
            },
        };
        self.exchanges.lock().unwrap().push(entry);
        response
    }
}
impl Transport for PageTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(self.run(request, cancel, None))
    }
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(self.run(request, cancel, Some(limit)))
    }
}
