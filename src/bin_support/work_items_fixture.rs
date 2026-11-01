//! Repeatable request-keyed fixture shared by preview and its tests (L010).
use crate::{
    auth::{InMemoryCredentialStore, token::Pat},
    data::AccountService,
    github::GitHubClient,
    network::{
        cancel::CancellationToken,
        executor::BackgroundExecutor,
        transport::{HttpRequest, HttpResponse, Transport, TransportError, json_response},
    },
    storage::accounts::AccountDatabaseConfig,
};
use serde_json::json;
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    Mixed,
    InitialFailure,
    AppendFailure,
    Empty,
    Partial,
    Long,
    Delayed,
}
pub const SCENES: [(Scene, &str); 7] = [
    (Scene::Mixed, "Mixed"),
    (Scene::InitialFailure, "Initial failure / retry"),
    (Scene::AppendFailure, "Append failure / retry"),
    (Scene::Empty, "Empty"),
    (Scene::Partial, "Partial"),
    (Scene::Long, "Long / 1,000 limit"),
    (Scene::Delayed, "Delayed / switch filters"),
];
struct FixtureState {
    scene: Scene,
    armed: bool,
    requests: Vec<String>,
}
/// Deterministic RFC3339 timestamps for the preview and tests: minutes, an
/// hour, a day, several days, plus one offset spelling and invalid/empty cases.
pub fn updated_at(index: u32) -> &'static str {
    // Preview clock is 2026-11-02T12:00:00Z (see examples/global_work_items.rs).
    // Keep a deterministic spread: just now, minutes, hours, yesterday, days.
    const STAMPS: [&str; 8] = [
        "2026-11-02T11:59:40Z",
        "2026-11-02T11:57:00Z",
        "2026-11-02T11:50:00Z",
        "2026-11-02T09:00:00Z",
        "2026-11-01T10:00:00Z",
        "2026-10-29T12:00:00Z",
        "2026-11-02T20:50:00+09:00",
        "2026-11-02T11:30:00Z",
    ];
    STAMPS[(index as usize) % STAMPS.len()]
}

pub use updated_at as timestamp_for_index;

pub struct FixtureTransport {
    state: Mutex<FixtureState>,
}
impl std::fmt::Debug for FixtureTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FixtureTransport")
    }
}
impl FixtureTransport {
    pub fn new(scene: Scene) -> Self {
        Self {
            state: Mutex::new(FixtureState {
                scene,
                armed: true,
                requests: Vec::new(),
            }),
        }
    }
    pub fn scene(&self) -> Scene {
        self.state.lock().unwrap().scene
    }
    pub fn set_scene(&self, scene: Scene) {
        let mut state = self.state.lock().unwrap();
        state.scene = scene;
        state.armed = true;
    }
    pub fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }
    fn answer(&self, request: &HttpRequest) -> (HttpResponse, bool) {
        let mut state = self.state.lock().unwrap();
        state.requests.push(request.url.clone());
        let Ok(url) = reqwest::Url::parse(&request.url) else {
            return (
                json_response(599, "{\"message\":\"unmodelled fixture URL\"}"),
                false,
            );
        };
        if url.path() == "/user" {
            return (
                json_response(200, "{\"id\":7,\"login\":\"test-user\"}"),
                false,
            );
        }
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let q = pairs.get("q").map(String::as_str).unwrap_or("");
        let page: u32 = pairs
            .get("page")
            .and_then(|page| page.parse().ok())
            .unwrap_or(0);
        let per_page: u32 = pairs
            .get("per_page")
            .and_then(|page| page.parse().ok())
            .unwrap_or(0);
        if url.path() != "/search/issues"
            || page == 0
            || per_page == 0
            || !(q.starts_with("is:issue ") || q.starts_with("is:pr "))
            || !(q.contains(" assignee:test-user") || q.contains(" author:test-user"))
        {
            return (
                json_response(599, "{\"message\":\"unmodelled fixture request\"}"),
                false,
            );
        }
        let scene = state.scene;
        if state.armed
            && ((scene == Scene::InitialFailure && page == 1)
                || (scene == Scene::AppendFailure && page == 2))
        {
            state.armed = false;
            return (json_response(503, "{}"), false);
        }
        let total: u32 = match scene {
            Scene::Empty => 0,
            Scene::Long => 1200,
            _ => 123,
        };
        let pr = q.starts_with("is:pr");
        let created = q.contains("author:");
        let start = (page - 1) * per_page;
        let count = per_page.min(total.saturating_sub(start));
        let items: Vec<_> = (start..start + count).map(|index| {
            // Repeat the previous page's last id to exercise real dedup.
            let identity = if index > 0 && index.is_multiple_of(per_page) { index - 1 } else { index };
            let closed = q.contains("is:closed") || (!q.contains("is:open") && index % 3 != 0);
            let mut item = json!({"id":(if pr { 20000 } else { 10000 }) + (if created { 5000 } else { 0 }) + identity,
                "number":index + 1,"title": if index % 5 == 0 { format!("Improve narrow-window keyboard navigation and preserve selection while loading additional results — synthetic item {index}") } else { format!("{} {} work item {index}", if created { "Created" } else { "Assigned" }, if pr { "PR" } else { "Issue" }) },
                "state":if closed { "closed" } else { "open" },"updated_at":updated_at(index), "repository_url":format!("https://api.github.com/repos/acme/{}", if index % 4 == 0 { "a-very-long-repository-name-for-narrow-layouts" } else { "desktop" })});
            if pr { item["pull_request"] = json!({"merged_at":if closed && index % 2 == 0 { Some("2026-11-01T04:00:00Z") } else { None }}); }
            item
        }).collect();
        let mut response = json_response(200, &json!({"total_count":total,"incomplete_results":scene == Scene::Partial,"items":items}).to_string());
        if start + count < total.min(1000) {
            response.headers.push((
                "link".into(),
                format!(
                    "<https://api.github.com/search/issues?page={}>; rel=\"next\"",
                    page + 1
                ),
            ));
        }
        if scene == Scene::Partial {
            response.headers.push((
                "x-github-sso".into(),
                "partial-results; organizations=123".into(),
            ));
        }
        (response, scene == Scene::Delayed && !created)
    }
}
impl Transport for FixtureTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let (response, delayed) = self.answer(&request);
            // Deliberately returns the old response even after cancellation,
            // exercising the data/publication barriers as well as transport.
            if delayed {
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
            Ok(response)
        })
    }
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max {
                Err(TransportError::ResponseTooLarge { limit: max })
            } else {
                Ok(response)
            }
        })
    }
}
pub struct FixtureRuntime {
    pub accounts: Arc<AccountService>,
    pub executor: Arc<BackgroundExecutor>,
    pub transport: Arc<FixtureTransport>,
    _temp: tempfile::TempDir,
}
impl FixtureRuntime {
    pub fn new(scene: Scene) -> Self {
        let temp = tempfile::tempdir().expect("temporary preview directory");
        let transport = Arc::new(FixtureTransport::new(scene));
        let executor = Arc::new(BackgroundExecutor::new(4));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let accounts = runtime.block_on(async {
            let accounts = AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(Default::default(), transport.clone())),
                executor.clone(),
            )
            .await
            .unwrap();
            accounts
                .login(Pat::from_str("synthetic-global-list-preview"))
                .await
                .unwrap();
            Arc::new(accounts)
        });
        Self {
            accounts,
            executor,
            transport,
            _temp: temp,
        }
    }
}
