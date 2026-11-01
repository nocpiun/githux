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
}
impl Scene {
    pub const ALL: [Self; 8] = [
        Self::Mixed,
        Self::Empty,
        Self::AppendFailure,
        Self::FirstFailure,
        Self::SsoPartial,
        Self::Unknown,
        Self::Delayed,
        Self::Long,
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
        }
    }
}
struct State {
    scene: Scene,
    failed: bool,
    requests: Vec<HttpRequest>,
}
pub struct Fixture {
    state: Mutex<State>,
}
impl Fixture {
    pub fn new(scene: Scene) -> Self {
        Self {
            state: Mutex::new(State {
                scene,
                failed: false,
                requests: vec![],
            }),
        }
    }
    pub fn set_scene(&self, scene: Scene) {
        let mut state = self.state.lock().unwrap();
        state.scene = scene;
        state.failed = false;
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
            ((page - 1) * 50..page * 50).map(thread).collect()
        } else if page == 1 {
            (0..8).map(thread).collect()
        } else {
            (7..14).map(thread).collect()
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
        let (response, delayed) = self.response(request);
        Box::pin(async move {
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
pub fn thread(index: usize) -> Value {
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
    json!({"id":format!("{}", 9000 + index), "unread":true, "reason":reason,
        "updated_at":"2026-11-01T12:00:00Z", "last_read_at":null,
        "subject":{"title":title, "type":subject_type,
            "url":"https://evil.example/opaque-subject", "latest_comment_url":null},
        "repository":{"id":100 + index % 3, "name":repository,
            "owner":{"login":"synthetic-owner"}, "url":"https://evil.example/opaque-repository"}})
}
