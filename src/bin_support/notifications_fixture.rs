//! Reusable offline transport shared by preview and acceptance tests. No queue
//! exhaustion, private reads, platform credentials or real GitHub requests.
// Preview and acceptance tests consume different instrumentation methods. The
// same real fixture is deliberately included by the adapter and controller test
// trees as well as the standalone preview/test assembly; each target needs its
// own module instance and the bodies are not compiled twice inside one target.
#![allow(dead_code, clippy::duplicate_mod)]
use crate::network::cancel::CancellationToken;
use crate::network::transport::{
    HttpMethod, HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    Mixed,
    Empty,
    AppendFailure,
    FirstFailure,
    SsoPartial,
    Unknown,
    Delayed,
    Long,
    DoneFailure,
    UnsubscribeStepOneFailure,
    UnsubscribeStepTwoFailure,
    UnsubscribeDoneRateLimited,
    DoneAlwaysFails,
}
impl Scene {
    pub const ALL: [Self; 13] = [
        Self::Mixed,
        Self::Empty,
        Self::AppendFailure,
        Self::FirstFailure,
        Self::SsoPartial,
        Self::Unknown,
        Self::Delayed,
        Self::Long,
        Self::DoneFailure,
        Self::UnsubscribeStepOneFailure,
        Self::UnsubscribeStepTwoFailure,
        Self::DoneAlwaysFails,
        Self::UnsubscribeDoneRateLimited,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Mixed => "Mixed",
            Self::Empty => "Empty",
            Self::AppendFailure => "Page error",
            Self::FirstFailure => "Load error",
            Self::SsoPartial => "SSO partial",
            Self::Unknown => "Unknown",
            Self::Delayed => "Delayed",
            Self::Long => "Long list",
            Self::DoneFailure => "Done failure",
            Self::UnsubscribeStepOneFailure => "Unsub step 1 fail",
            Self::UnsubscribeStepTwoFailure => "Unsub step 2 fail",
            Self::DoneAlwaysFails => "Done always fails",
            Self::UnsubscribeDoneRateLimited => "Unsub Done 429",
        }
    }
}
pub const PREVIEW_NOW: u64 = 1_793_534_400;

struct State {
    scene: Scene,
    now: u64,
    failed: bool,
    requests: Vec<HttpRequest>,
    /// When set, the subscription PUT parks until the returned receiver fires.
    gate_subscription: Option<tokio::sync::oneshot::Receiver<()>>,
}
pub struct Fixture {
    state: Mutex<State>,
}
impl Fixture {
    pub fn new(scene: Scene) -> Self {
        Self::with_clock(scene, PREVIEW_NOW)
    }
    pub fn with_clock(scene: Scene, now: u64) -> Self {
        Self {
            state: Mutex::new(State {
                scene,
                now,
                failed: false,
                requests: vec![],
                gate_subscription: None,
            }),
        }
    }
    pub fn set_scene(&self, scene: Scene) {
        let mut state = self.state.lock().unwrap();
        state.scene = scene;
        state.failed = false;
    }
    pub fn set_clock(&self, now: u64) {
        self.state.lock().unwrap().now = now;
    }

    /// The next subscription PUT parks until the returned receiver is signalled.
    pub fn gate_next_subscription(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.state.lock().unwrap().gate_subscription = Some(rx);
        tx
    }
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.state.lock().unwrap().requests.clone()
    }
    fn response(&self, request: HttpRequest) -> (HttpResponse, bool) {
        let mut state = self.state.lock().unwrap();
        state.requests.push(request.clone());
        let invalid = || {
            json_response(
                599,
                r#"{"message":"unmodelled notifications fixture request"}"#,
            )
        };
        let Ok(url) = reqwest::Url::parse(&request.url) else {
            return (invalid(), false);
        };
        let segment = url.path().trim_start_matches('/').trim_end_matches('/');
        if request.method == HttpMethod::Delete && segment.starts_with("notifications/threads/") {
            let thread = segment
                .trim_start_matches("notifications/threads/")
                .to_string();
            if thread.is_empty() {
                return (invalid(), false);
            }
            let scene = state.scene;
            let should_fail = match scene {
                Scene::DoneFailure => !state.failed,
                Scene::UnsubscribeStepTwoFailure => !state.failed,
                Scene::DoneAlwaysFails => true,
                _ => false,
            };
            if should_fail {
                state.failed = true;
                return (
                    HttpResponse {
                        status: 503,
                        headers: vec![("x-poll-interval".into(), "1".into())],
                        body: "{}".into(),
                    },
                    false,
                );
            }
            if scene == Scene::UnsubscribeDoneRateLimited {
                return (
                    HttpResponse {
                        status: 429,
                        headers: vec![("x-poll-interval".into(), "1".into())],
                        body: r#"{"message":"secondary rate limit"}"#.into(),
                    },
                    false,
                );
            }
            // 204 No Content is the only confirmation of Done.
            return (
                HttpResponse {
                    status: 204,
                    headers: vec![("x-poll-interval".into(), "1".into())],
                    body: String::new(),
                },
                false,
            );
        }
        if request.method == HttpMethod::Put && segment.ends_with("/subscription") {
            let scene = state.scene;
            let should_fail = match scene {
                Scene::UnsubscribeStepOneFailure => !state.failed,
                _ => false,
            };
            if should_fail {
                state.failed = true;
                return (
                    HttpResponse {
                        status: 503,
                        headers: vec![("x-poll-interval".into(), "1".into())],
                        body: "{}".into(),
                    },
                    false,
                );
            }
            let body = request.body.clone().unwrap_or_default();
            let ignored = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.get("ignored").and_then(Value::as_bool))
                .unwrap_or(false);
            if !ignored {
                return (
                    json_response(422, r#"{"message":"ignored must be true"}"#),
                    false,
                );
            }
            let mut response = json_response(200, r#"{"ignored":true}"#);
            if scene == Scene::UnsubscribeDoneRateLimited {
                response
                    .headers
                    .push(("x-ratelimit-remaining".into(), "0".into()));
                response.headers.push(("retry-after".into(), "90".into()));
                response
                    .headers
                    .push(("x-poll-interval".into(), "120".into()));
            }
            return (response, false);
        }
        if request.method != HttpMethod::Get {
            return (invalid(), false);
        }
        if url.path() == "/user" {
            let second = request
                .headers
                .iter()
                .any(|(key, value)| key == "Authorization" && value.ends_with("synthetic-second"));
            return (json_response(200, &json!({"id":if second { 43 } else { 42 },"login":if second { "second-user" } else { "preview-user" },"name":"Preview","avatar_url":"","html_url":"https://github.com/preview-user"}).to_string()), false);
        }
        if url.path() != "/notifications" || url.query_pairs().count() != 4 {
            return (invalid(), false);
        }
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        if query.get("all").map(|v| v.as_ref()) != Some("false")
            || query.get("participating").map(|v| v.as_ref()) != Some("false")
            || query.get("per_page").map(|v| v.as_ref()) != Some("50")
        {
            return (invalid(), false);
        }
        let page = query
            .get("page")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if page == 0 || page > 20 {
            return (invalid(), false);
        }
        let scene = state.scene;
        let fail = (scene == Scene::FirstFailure && page == 1)
            || (scene == Scene::AppendFailure && page == 2);
        if fail && !state.failed {
            state.failed = true;
            return (
                HttpResponse {
                    status: 503,
                    headers: vec![("x-poll-interval".into(), "1".into())],
                    body: "{}".into(),
                },
                false,
            );
        }
        let long = scene == Scene::Long;
        let pages = if long { 20 } else { 2 };
        let rows: Vec<_> = if scene == Scene::Empty {
            vec![]
        } else if long {
            ((page - 1) * 50..page * 50)
                .map(|index| timed_thread(index, state.now))
                .collect()
        } else if page == 1 {
            (0..8).map(|index| timed_thread(index, state.now)).collect()
        } else {
            (7..14)
                .map(|index| timed_thread(index, state.now))
                .collect()
        };
        let mut rows = rows;
        if scene == Scene::Unknown
            && let Some(row) = rows.first_mut()
        {
            row["subject"]["type"] = json!("FutureSubject");
            row["reason"] = json!("future_reason");
            row["subject"]["url"] = Value::Null;
        }
        let mut response = json_response(200, &serde_json::to_string(&rows).unwrap());
        response
            .headers
            .push(("x-poll-interval".into(), "1".into()));
        response.headers.push((
            "last-modified".into(),
            "Thu, 01 Nov 2026 12:00:00 GMT".into(),
        ));
        if page < pages && scene != Scene::Empty {
            response.headers.push(("link".into(), format!("<https://api.github.com/notifications?all=false&participating=false&per_page=50&page={}>; rel=\"next\"", page + 1)));
        }
        if scene == Scene::SsoPartial {
            response.headers.push((
                "x-github-sso".into(),
                "partial-results; organizations=123".into(),
            ));
        }
        (response, scene == Scene::Delayed)
    }
}
impl Transport for Fixture {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        self.execute_bounded(request, cancel, usize::MAX)
    }
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        let gate = {
            let mut state = self.state.lock().unwrap();
            let is_subscription =
                request.method == HttpMethod::Put && request.url.ends_with("/subscription");
            if is_subscription {
                state.gate_subscription.take()
            } else {
                None
            }
        };
        let (response, delayed) = self.response(request);
        Box::pin(async move {
            if let Some(gate) = gate {
                tokio::select! {
                    _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled),
                    _ = gate => {}
                }
            }
            if delayed {
                tokio::select! { _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled), _ = tokio::time::sleep(Duration::from_millis(900)) => {} }
            }
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            if response.body.len() > max {
                return Err(TransportError::ResponseTooLarge { limit: max });
            }
            Ok(response)
        })
    }
}
pub fn thread(index: usize, updated_at: &str, unread: bool) -> Value {
    let reason = [
        "subscribed",
        "mention",
        "review_requested",
        "assign",
        "comment",
        "ci_activity",
    ][index % 6];
    let subject_type = [
        "Issue",
        "PullRequest",
        "Release",
        "Commit",
        "Discussion",
        "FutureType",
    ][index % 6];
    let repository = [
        "desktop-client",
        "a-deliberately-long-repository-name",
        "core",
    ][index % 3];
    let title = if index == 0 {
        "A long notification title · 中英文 mixed text · preserve the readable title in a narrow window without widening the list or inventing a detail destination".to_string()
    } else {
        format!(
            "Notification {} — {}",
            index + 1,
            [
                "Improve keyboard focus",
                "Release v2.0",
                "Build completed",
                "Discussion update"
            ][index % 4]
        )
    };
    json!({"id":format!("{}", 9000 + index), "unread":unread, "reason":reason,
        "updated_at":updated_at, "last_read_at":null,
        "subject":{"title":title, "type":subject_type,
            "url":"https://evil.example/opaque-subject", "latest_comment_url":null},
        "repository":{"id":100 + index % 3, "name":repository,
            "owner":{"login":"synthetic-owner"}, "url":"https://evil.example/opaque-repository"}})
}

/// Clock-relative fixtures share exactly the preview's captured clock.
fn timed_thread(index: usize, now: u64) -> Value {
    let age = [300, 7200, 259200, 30, 90, 3600, 0, 60][index % 8];
    let timestamp = if index % 8 == 3 { now + age } else { now - age };
    let date = if index % 8 == 4 {
        "invalid timestamp".into()
    } else {
        utc(timestamp)
    };
    thread(index, &date, index % 3 != 1)
}

// Inverse Gregorian civil-date calculation, fixture only; the product parser
// remains the shared Home implementation. No wall clock or timezone dependency.
fn utc(seconds: u64) -> String {
    let days = (seconds / 86400) as i64 + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let clock = seconds % 86400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        clock / 3600,
        clock / 60 % 60,
        clock % 60
    )
}
