//! Deterministic HomePage composition with the real page-cache adapter.
//! No GitHub, real credential, or native-window claim is made by these tests.

use crate::data::page_cache::metadata_feed_collection;
use std::sync::Arc;

use gpui::{Entity, TestAppContext, VisualContext, VisualTestContext};

use super::*;
use crate::app::accounts::runtime::AccountRuntime;
use crate::auth::{InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{MockFailure, MockTransport, json_response};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};
use crate::ui::home::{FeedAppendState, PanelContent, PanelState};

fn response(status: u16, body: &str) -> crate::network::transport::HttpResponse {
    json_response(status, body)
}

fn home(title: &str) -> String {
    serde_json::json!({
        "total_count": 1, "incomplete_results": false,
        "items": [{"id": 900, "number": 12, "title": title,
            "html_url": "https://github.com/octo/repo/issues/12", "state": "open",
            "updated_at": "2026-10-28T10:00:00Z",
            "repository_url": "https://api.github.com/repos/octo/repo", "pull_request": null}]
    })
    .to_string()
}

fn events(id: &str, next_page: Option<u32>) -> crate::network::transport::HttpResponse {
    let mut response = response(200, &serde_json::json!([{
        "id": id, "type": "WatchEvent",
        "actor": {"id": 7, "login": "bob", "avatar_url": null},
        "repo": {"id": 10, "name": "octo/repo", "url": "https://api.github.com/repos/octo/repo"},
        "payload": {"action": "started"}, "created_at": "2026-10-28T06:00:00Z"
    }]).to_string());
    if let Some(next) = next_page {
        response.headers.push((
            "link".into(),
            format!("<https://api.github.com/users/octo/received_events?per_page=100&page={next}>; rel=\"next\""),
        ));
    }
    response
}

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        crate::platform::fonts::register(cx).unwrap();
        crate::ui::Shell::bind_keys(cx);
    });
}

#[derive(Default)]
struct DescriptionGate {
    enabled: std::sync::atomic::AtomicBool,
    entered: std::sync::atomic::AtomicBool,
    release: tokio::sync::Notify,
    pause_search: std::sync::atomic::AtomicBool,
    search_entered: std::sync::atomic::AtomicBool,
}

struct ControlledTransport {
    mock: Arc<MockTransport>,
    gate: Arc<DescriptionGate>,
}

impl crate::network::transport::Transport for ControlledTransport {
    fn execute<'a>(
        &'a self,
        request: crate::network::transport::HttpRequest,
        cancel: CancellationToken,
    ) -> std::pin::Pin<
        Box<
            dyn Future<
                    Output = Result<
                        crate::network::transport::HttpResponse,
                        crate::network::transport::TransportError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            use std::sync::atomic::Ordering::SeqCst;
            if self.gate.pause_search.load(SeqCst) && request.url.contains("/search/issues") {
                self.gate.search_entered.store(true, SeqCst);
                tokio::select! {
                    _ = self.gate.release.notified() => {},
                    _ = cancel.clone().cancelled() => return Err(crate::network::transport::TransportError::Cancelled),
                }
            }
            if self.gate.enabled.load(SeqCst) && request.url.contains("/repos/octo/") {
                let name = request.url.rsplit('/').next().unwrap();
                if name == "repo" {
                    self.gate.entered.store(true, SeqCst);
                    tokio::select! {
                        _ = self.gate.release.notified() => {},
                        _ = cancel.clone().cancelled() => return Err(crate::network::transport::TransportError::Cancelled),
                    }
                }
                return Ok(response(
                    200,
                    &serde_json::json!({
                        "id": 10, "name": name, "owner": {"id": 1, "login": "octo"},
                        "private": false, "default_branch": "main", "fork": false,
                        "archived": false, "stargazers_count": 0, "forks_count": 0,
                        "open_issues_count": 0, "size": 1,
                        "description": format!("description for {name}"),
                        "html_url": format!("https://github.com/octo/{name}")
                    })
                    .to_string(),
                ));
            }
            self.mock.execute(request, cancel).await
        })
    }

    fn execute_bounded<'a>(
        &'a self,
        request: crate::network::transport::HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> std::pin::Pin<
        Box<
            dyn Future<
                    Output = Result<
                        crate::network::transport::HttpResponse,
                        crate::network::transport::TransportError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let value = self.execute(request, cancel).await?;
            if value.body.len() > max_body_bytes {
                return Err(
                    crate::network::transport::TransportError::ResponseTooLarge {
                        limit: max_body_bytes,
                    },
                );
            }
            Ok(value)
        })
    }
}

struct Fixture {
    service: Option<AccountService>,
    executor: Arc<BackgroundExecutor>,
    mock: Arc<MockTransport>,
    gate: Arc<DescriptionGate>,
    directory: tempfile::TempDir,
    cache_directory: tempfile::TempDir,
    store: Option<CacheStore>,
    credentials: Arc<InMemoryCredentialStore>,
    account: crate::domain::AccountKey,
}

impl Fixture {
    async fn new(with_cache: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cache_directory = tempfile::tempdir().unwrap();
        let credentials = Arc::new(InMemoryCredentialStore::new());
        let cache = if with_cache {
            Some(CacheStore::open(CacheConfig::at(cache_directory.path().to_path_buf())).unwrap())
        } else {
            None
        };
        Self::at(directory, cache_directory, cache, credentials).await
    }

    async fn at(
        directory: tempfile::TempDir,
        cache_directory: tempfile::TempDir,
        cache: Option<CacheStore>,
        credentials: Arc<InMemoryCredentialStore>,
    ) -> Self {
        let mock = Arc::new(MockTransport::new());
        let executor = Arc::new(BackgroundExecutor::new(4));
        let gate = Arc::new(DescriptionGate::default());
        let client = Arc::new(GitHubClient::new(
            GitHubConfig::default(),
            Arc::new(ControlledTransport {
                mock: mock.clone(),
                gate: gate.clone(),
            }),
        ));
        let store = cache.clone();
        let service = if let Some(cache) = cache {
            AccountService::open_with_cache(
                AccountDatabaseConfig::at(directory.path().join("accounts.sqlite3")),
                credentials.clone(),
                client,
                executor.clone(),
                cache,
            )
            .await
        } else {
            AccountService::open(
                AccountDatabaseConfig::at(directory.path().join("accounts.sqlite3")),
                credentials.clone(),
                client,
                executor.clone(),
            )
            .await
        }
        .unwrap();
        if service.active_summary().is_none() {
            mock.enqueue(Ok(response(200, r#"{"id":1,"login":"octo"}"#)))
                .await;
            service
                .login(Pat::from_str("synthetic-home-cache"))
                .await
                .unwrap();
        } else {
            let active = service.registry_handle().lock().unwrap().summaries()[0]
                .identity
                .key
                .clone();
            mock.enqueue(Ok(response(200, r#"{"id":1,"login":"octo"}"#)))
                .await;
            let _ = service.revalidate(active).await;
        }
        let account = service.active_summary().unwrap().identity.key;
        Self {
            service: Some(service),
            executor,
            mock,
            gate,
            directory,
            cache_directory,
            store,
            credentials,
            account,
        }
    }

    fn store(&self) -> CacheStore {
        self.store
            .as_ref()
            .expect("fixture opened without cache")
            .clone()
    }

    async fn warm(&self) {
        self.mock
            .enqueue(Ok(response(200, &home("cached home"))))
            .await;
        let store = self.store();
        let scope = store.saved_account(self.account.clone()).unwrap();
        let session = CachedPageReadSession::new(
            self.service.as_ref().unwrap().page_session().unwrap(),
            store,
            scope,
        )
        .unwrap();
        session
            .home(
                PageReadMode::Online,
                None,
                &HomeSearchLimits::default(),
                Default::default(),
            )
            .await
            .unwrap();
        self.mock.enqueue(Ok(events("first", Some(2)))).await;
        self.mock
            .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
            .await;
        let first = session
            .feed(
                PageReadMode::Online,
                &FeedLimits::default(),
                &FollowingLimits::default(),
                &PullRequestDetailLimits::default(),
                30,
                Default::default(),
            )
            .await
            .unwrap();
        let collection = match &first.value {
            PageValue::Value { metadata, .. } => metadata_feed_collection(metadata),
            _ => None,
        };
        self.mock.enqueue(Ok(events("second", Some(3)))).await;
        self.mock
            .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
            .await;
        session
            .feed_page(
                PageReadMode::Online,
                crate::domain::home::FeedSourceCursor { page: 2 },
                collection,
                &FeedLimits::default(),
                &FollowingLimits::default(),
                &PullRequestDetailLimits::default(),
                30,
                &mut Default::default(),
                Default::default(),
            )
            .await
            .unwrap();
        self.mock.enqueue(Ok(response(200, r#"{"id":10,"name":"repo","owner":{"id":1,"login":"octo"},"visibility":"public","private":false,"default_branch":"main","fork":false,"archived":false,"stargazers_count":0,"forks_count":0,"open_issues_count":0,"size":1,"description":"saved description","html_url":"https://github.com/octo/repo"}"#))).await;
        session
            .repository_descriptions(
                PageReadMode::Online,
                vec![("octo".into(), "repo".into())],
                20,
                Default::default(),
            )
            .await
            .unwrap();
    }

    async fn network_error(&self) {
        self.mock
            .enqueue(Err(MockFailure::Network("offline fixture".into())))
            .await;
    }
}

fn pump(controller: &Entity<crate::app::accounts::Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.drain_for_test(cx));
    cx.run_until_parked();
}

struct AccountsHolder {
    accounts: Entity<crate::app::accounts::Accounts>,
    page: Option<Entity<HomePage>>,
}

impl gpui::Render for AccountsHolder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        match &self.page {
            Some(page) => page.clone().into_any_element(),
            None => gpui::div().into_any_element(),
        }
    }
}

fn open_page<'a>(
    fixture: &mut Fixture,
    cx: &'a mut TestAppContext,
) -> (Entity<HomePage>, &'a mut VisualTestContext) {
    open_page_with_navigation(fixture, std::rc::Rc::new(|_, _, _| {}), cx)
}

fn open_page_with_navigation<'a>(
    fixture: &mut Fixture,
    navigation: NavigationSink,
    cx: &'a mut TestAppContext,
) -> (Entity<HomePage>, &'a mut VisualTestContext) {
    setup(cx);
    let runtime = AccountRuntime::new(
        fixture
            .service
            .take()
            .expect("mount consumes the fixture service"),
        fixture.executor.clone(),
    );
    let (holder, cx) = cx.add_window_view(|window, cx| {
        let accounts =
            cx.new(|cx| crate::app::accounts::Accounts::new(Some(Arc::new(runtime)), window, cx));
        cx.set_global(crate::app::AccountsHandle(accounts.clone()));
        AccountsHolder {
            accounts,
            page: None,
        }
    });
    let controller = holder.read_with(cx, |h, _| h.accounts.clone());
    for _ in 0..400 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(2));
        if cx.update(|_, cx| crate::app::AccountsHandle::services(cx).is_some()) {
            break;
        }
    }
    let page = cx.update_window_entity(&holder, |holder, window, cx| {
        let services = crate::app::AccountsHandle::services(cx);
        let workspace = crate::workspace::Workspace::new(cx);
        let page = cx.new(|cx| {
            HomePage::new(
                workspace.active_id(),
                services,
                1,
                navigation,
                PageAvatars::new(None),
                window,
                cx,
            )
        });
        holder.page = Some(page.clone());
        cx.notify();
        page
    });
    (page, cx)
}

fn settle(page: &Entity<HomePage>, cx: &mut VisualTestContext) {
    for _ in 0..400 {
        cx.run_until_parked();
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.update(|window, _| window.refresh());
        std::thread::sleep(Duration::from_millis(2));
        if page.read_with(cx, |page, cx| page.is_ready_for_test(cx)) {
            return;
        }
    }
    let (home, feed) = page.read_with(cx, |page, cx| {
        (
            page.view.read(cx).home_state().clone(),
            page.view.read(cx).feed_state().clone(),
        )
    });
    panic!("HomePage did not settle: home={home:?}, feed={feed:?}");
}

fn home_title(page: &Entity<HomePage>, cx: &VisualTestContext) -> Option<String> {
    page.read_with(cx, |page, cx| match page.view.read(cx).home_state() {
        PanelState::Ready {
            content: PanelContent::Ready(outcome),
            ..
        } => outcome.items.first().map(|item| item.title.clone()),
        _ => None,
    })
}

fn feed_ids(page: &Entity<HomePage>, cx: &VisualTestContext) -> Vec<String> {
    page.read_with(cx, |page, cx| match page.view.read(cx).feed_state() {
        PanelState::Ready {
            content: PanelContent::Ready(outcome),
            ..
        } => outcome
            .activities
            .iter()
            .map(|item| item.event.id.clone())
            .collect(),
        _ => vec![],
    })
}

#[gpui::test]
fn online_cache_reopens_and_offline_reads_the_same_home_feed_and_older_page(
    cx: &mut TestAppContext,
) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(true));
    runtime.block_on(fixture.warm());
    let credentials = fixture.credentials.clone();
    let service = fixture.service.take().unwrap();
    let executor = fixture.executor.clone();
    runtime.block_on(async move {
        let _ = service.close().await;
        drop(executor);
    });
    let accounts_directory =
        std::mem::replace(&mut fixture.directory, tempfile::tempdir().unwrap());
    let cache_directory =
        std::mem::replace(&mut fixture.cache_directory, tempfile::tempdir().unwrap());
    drop(fixture);
    // A genuinely reopened account service/cache store replays the persisted pages.
    let store = CacheStore::open(CacheConfig::at(cache_directory.path().to_path_buf())).unwrap();
    let mut fixture = runtime.block_on(Fixture::at(
        accounts_directory,
        cache_directory,
        Some(store),
        credentials,
    ));
    runtime.block_on(async {
        fixture.network_error().await;
        fixture.network_error().await;
    });
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    assert_eq!(
        home_title(&page, cx).as_deref(),
        Some("cached home"),
        "home={:?}",
        page.read_with(cx, |page, cx| page.view.read(cx).home_state().clone())
    );
    assert_eq!(feed_ids(&page, cx), ["first"]);
    assert!(page.read_with(cx, |page, cx| page.cached_enabled_for_test()
        && page.view.read(cx).provenance(Column::Home).is_some()));
    cx.update_window_entity(&page, |page, window, cx| {
        page.load_more_for_test(window, cx)
    });
    for _ in 0..100 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(feed_ids(&page, cx), ["first", "second"]);
    assert!(page.read_with(cx, |page, cx| {
        page.view.read(cx).provenance(Column::Feed).is_some()
    }));
    assert!(page.read_with(cx, |page, _cx| {
        page.description_for_test("octo/repo").as_deref() == Some("saved description")
    }));
}

#[gpui::test]
fn offline_miss_partial_and_refresh_replacement_are_distinct(cx: &mut TestAppContext) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(true));
    runtime.block_on(fixture.warm());
    runtime.block_on(async {
        fixture.network_error().await;
        fixture.network_error().await;
    });
    let mock = fixture.mock.clone();
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    // Cached first page remains historical; an uncached page never looks like a normal end.
    // The second page is cached and loads; the third page is missing.
    cx.update_window_entity(&page, |page, window, cx| {
        page.load_more_for_test(window, cx)
    });
    for _ in 0..100 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(feed_ids(&page, cx), ["first", "second"]);
    cx.update_window_entity(&page, |page, window, cx| {
        page.load_more_for_test(window, cx)
    });
    for _ in 0..100 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        page.read_with(cx, |page, cx| matches!(
            page.view.read(cx).feed_append_state(),
            FeedAppendState::NotCached { page: 3 }
        )),
        "actual_state={:?}",
        page.read_with(cx, |page, cx| page
            .view
            .read(cx)
            .feed_append_state()
            .clone())
    );
    runtime.block_on(async {
        mock.enqueue(Ok(response(200, &home("fresh home")))).await;
        mock.enqueue(Ok(events("fresh", None))).await;
        mock.enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
            .await;
    });
    cx.update_window_entity(&page, |page, window, cx| {
        page.refresh_for_test(Column::Home, window, cx);
        page.expire_feed_cooldown_for_test(cx);
        page.refresh_for_test(Column::Feed, window, cx);
    });
    cx.run_until_parked();
    settle(&page, cx);
    assert_eq!(home_title(&page, cx).as_deref(), Some("fresh home"));
    assert_eq!(feed_ids(&page, cx), ["fresh"]);
    assert!(page.read_with(cx, |page, cx| {
        page.view.read(cx).provenance(Column::Feed).is_none()
    }));
}

#[gpui::test]
fn authentication_failure_cancellation_and_uncached_pages_are_not_historical_success(
    cx: &mut TestAppContext,
) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(true));
    runtime.block_on(fixture.warm());
    runtime.block_on(async {
        fixture.mock.enqueue(Ok(response(401, "{}"))).await;
        fixture.mock.enqueue(Ok(response(401, "{}"))).await;
    });
    let (page, cx) = open_page(&mut fixture, cx);
    for _ in 0..300 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(home_title(&page, cx), None);
    assert!(
        page.read_with(cx, |page, cx| matches!(
            page.view.read(cx).home_state(),
            PanelState::Ready {
                content: PanelContent::Unavailable { .. },
                ..
            }
        )),
        "state={:?}",
        page.read_with(cx, |page, cx| page.view.read(cx).home_state().clone())
    );

    let mut fixture = runtime.block_on(Fixture::new(true));
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    assert!(page.read_with(cx, |page, cx| matches!(
        page.view.read(cx).home_state(),
        PanelState::Ready {
            content: PanelContent::Unavailable { message },
            ..
        } if message.contains("not available offline")
    )));

    let mut fixture = runtime.block_on(Fixture::new(true));
    runtime.block_on(fixture.warm());
    let gate = fixture.gate.clone();
    gate.pause_search
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (page, cx) = open_page(&mut fixture, cx);
    for _ in 0..300 {
        if gate
            .search_entered
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    page.update(cx, |page, _| {
        if let Some(work) = page.work[0].as_ref() {
            work.source.cancel();
        }
    });
    for _ in 0..200 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        home_title(&page, cx),
        None,
        "cancellation must not fall back to saved Home"
    );
    assert!(page.read_with(cx, |page, cx| matches!(
        page.view.read(cx).home_state(),
        PanelState::Ready {
            content: PanelContent::Error(HomeError::GitHub(GitHubError::Cancelled)),
            ..
        }
    )));

    let mut fixture = runtime.block_on(Fixture::new(true));
    runtime.block_on(fixture.warm());
    let account = fixture.account.clone();
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    assert_eq!(home_title(&page, cx).as_deref(), Some("cached home"));
    let services = cx
        .update(|_, cx| crate::app::AccountsHandle::services(cx))
        .unwrap();
    runtime
        .block_on(services.accounts().remove_saved(account))
        .unwrap();
    page.update(cx, |page, cx| page.drain_for_test(cx));
    assert_eq!(
        home_title(&page, cx),
        None,
        "account removal clears the displayed copy"
    );
    assert!(page.read_with(cx, |page, cx| matches!(
        page.view.read(cx).home_state(),
        PanelState::Ready {
            content: PanelContent::Unavailable { .. },
            ..
        }
    )));
}

#[gpui::test]
fn append_keeps_a_delayed_first_page_description_and_refresh_rejects_old_batches(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering::SeqCst;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(false));
    let gate = fixture.gate.clone();
    gate.enabled.store(true, SeqCst);
    runtime.block_on(async {
        fixture
            .mock
            .enqueue(Ok(response(200, r#"{"id":1,"login":"octo"}"#)))
            .await;
        fixture
            .mock
            .enqueue(Ok(response(200, &home("online home"))))
            .await;
        fixture.mock.enqueue(Ok(events("first", Some(2)))).await;
        fixture
            .mock
            .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
            .await;
    });
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    for _ in 0..300 {
        if gate.entered.load(SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        gate.entered.load(SeqCst),
        "first page description is paused in transport"
    );
    page.update(cx, |page, cx| {
        let mut activities = match page.view.read(cx).feed_state() {
            PanelState::Ready {
                content: PanelContent::Ready(feed),
                ..
            } => feed.activities.clone(),
            state => panic!("expected online feed, got {state:?}"),
        };
        activities[0].event.id = "second".into();
        activities[0].event.repository.name = "octo/later".into();
        page.apply_append(
            AppendReply {
                generation: page.generation,
                append_epoch: page.feed_append_epoch,
                selection: page.selection,
                authorization: page.cache_authorization.clone(),
                context: page.session.as_ref().unwrap().context().clone(),
                result: Ok((
                    FeedOutcome {
                        activities,
                        etag: None,
                        poll_interval_seconds: None,
                        next_page: None,
                        readiness: crate::domain::home::FeedReadiness::Complete,
                    },
                    Default::default(),
                    ReadSource::Network { collection: None },
                )),
            },
            cx,
        );
        assert_eq!(
            page.feed_enrich_work.len(),
            2,
            "both same-collection batches remain live"
        );
        assert!(
            page.feed_enrich_work
                .values()
                .all(|job| !job.work.token.is_cancelled())
        );
    });
    for _ in 0..300 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        if page.read_with(cx, |page, _| {
            page.description_for_test("octo/later").is_some()
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        page.read_with(cx, |page, _| page.description_for_test("octo/later"))
            .as_deref(),
        Some("description for later")
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.description_for_test("octo/repo")),
        None
    );
    gate.release.notify_one();
    for _ in 0..300 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        if page.read_with(cx, |page, _| {
            page.description_for_test("octo/repo").is_some()
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        page.read_with(cx, |page, _| page.description_for_test("octo/repo"))
            .as_deref(),
        Some("description for repo")
    );

    page.update(cx, |page, cx| {
        let old_epoch = page.feed_append_epoch;
        let old_request = page.feed_enrich_request;
        let context = page.session.as_ref().unwrap().context().clone();
        page.feed_ready = None;
        page.start(Column::Feed, false, cx);
        assert!(page.feed_repo_descriptions.is_empty());
        page.apply_enrichment(
            EnrichReply {
                generation: page.generation,
                append_epoch: old_epoch,
                selection: page.selection,
                authorization: page.cache_authorization.clone(),
                context,
                request: old_request,
                result: Ok(EnrichmentResult {
                    descriptions: [("octo/repo".into(), Some("stale description".into()))].into(),
                    cache_partition: None,
                }),
            },
            cx,
        );
        assert!(
            page.feed_repo_descriptions.is_empty(),
            "refresh rejects the old description epoch"
        );
    });
}

#[gpui::test]
fn offline_filtered_empty_first_page_continues_to_merged_pr_and_read_more_routes_internally(
    cx: &mut TestAppContext,
) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(true));
    let long_body = "Detailed offline summary word ".repeat(12);
    runtime.block_on(async {
        let store = fixture.store();
        let scope = store.saved_account(fixture.account.clone()).unwrap();
        let session = CachedPageReadSession::new(fixture.service.as_ref().unwrap().page_session().unwrap(), store, scope).unwrap();
        fixture.mock.enqueue(Ok(response(200, &home("partial home")))).await;
        session.home(PageReadMode::Online, None, &HomeSearchLimits::default(), Default::default()).await.unwrap();
        let mut empty = response(200, &serde_json::json!([{
            "id": "noise", "type": "PushEvent",
            "actor": {"id": 7, "login": "bob", "avatar_url": null},
            "repo": {"id": 10, "name": "octo/repo", "url": "https://api.github.com/repos/octo/repo"},
            "payload": {"ref": "refs/heads/main", "before": "a", "head": "b", "size": 1},
            "created_at": "2026-10-28T09:00:00Z"
        }]).to_string());
        empty.headers.push(("link".into(), "<https://api.github.com/users/octo/received_events?per_page=100&page=2>; rel=\"next\"".into()));
        fixture.mock.enqueue(Ok(empty)).await;
        fixture.mock.enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"},{"id":8,"login":"carol"}]"#))).await;
        let first = session.feed(PageReadMode::Online, &FeedLimits::default(), &FollowingLimits::default(), &PullRequestDetailLimits::default(), 30, Default::default()).await.unwrap();
        let collection = match &first.value {
            PageValue::Value { metadata, .. } => metadata_feed_collection(metadata),
            _ => None,
        };
        fixture.mock.enqueue(Ok(response(200, &serde_json::json!([{
            "id": "merged-pr", "type": "PullRequestEvent",
            "actor": {"id": 7, "login": "bob", "avatar_url": null},
            "repo": {"id": 10, "name": "octo/repo", "url": "https://api.github.com/repos/octo/repo"},
            "payload": {"action": "merged", "number": 44, "pull_request": {"url": "https://api.github.com/repos/octo/repo/pulls/44", "id": 500, "number": 44, "head": {"ref": "f", "sha": "1"}, "base": {"ref": "main", "sha": "2"}}},
            "created_at": "2026-10-28T08:00:00Z"
        }]).to_string()))).await;
        fixture.mock.enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"},{"id":8,"login":"carol"}]"#))).await;
        fixture.mock.enqueue(Ok(response(200, &serde_json::json!({
            "id": 500, "number": 44, "state": "closed", "merged": true,
            "title": "Offline merged pull request", "html_url": "https://github.com/octo/repo/pull/44",
            "user": {"id": 8, "login": "carol", "avatar_url": null}, "body": long_body
        }).to_string()))).await;
        session.feed_page(PageReadMode::Online, crate::domain::home::FeedSourceCursor { page: 2 }, collection, &FeedLimits::default(), &FollowingLimits::default(), &PullRequestDetailLimits::default(), 30, &mut Default::default(), Default::default()).await.unwrap();
    });
    let navigated = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_routes = navigated.clone();
    let (page, cx) = open_page_with_navigation(
        &mut fixture,
        std::rc::Rc::new(move |action, _, _| {
            if let PageNavigation::Open(route) = action {
                sink_routes.lock().unwrap().push(route);
            }
        }),
        cx,
    );
    settle(&page, cx);
    for _ in 0..200 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        page.update(cx, |page, cx| page.drain_for_test(cx));
        if feed_ids(&page, cx) == ["merged-pr"] {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        feed_ids(&page, cx),
        ["merged-pr"],
        "filtered-empty page automatically loads page 2"
    );
    assert!(page.read_with(cx, |page, cx| {
        page.view.read(cx).provenance(Column::Feed).is_some()
    }));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let read_more = cx
        .debug_bounds("feed-pr-read-more-0")
        .expect("offline merged PR Read more control");
    cx.simulate_click(read_more.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        *navigated.lock().unwrap(),
        vec![Route::PullRequest {
            namespace: "octo".into(),
            name: "repo".into(),
            number: 44
        }]
    );
}

// Coordinator acceptance: exercise current credentials with a quarantined private cache.
#[tokio::test]
async fn acceptance_valid_replacement_pat_keeps_home_online() {
    let fixture = Fixture::new(true).await;
    let service = fixture.service.as_ref().unwrap();
    fixture
        .mock
        .enqueue(Ok(response(200, r#"{"id":1,"login":"octo"}"#)))
        .await;
    service
        .replace(
            fixture.account.clone(),
            Pat::from_str("synthetic-replacement"),
        )
        .await
        .unwrap();
    let session = service.page_session().unwrap();
    assert!(session.verify());
    let store = fixture.store();
    let scope = store.saved_account(fixture.account.clone()).unwrap();
    let cached = CachedPageReadSession::new(session.clone(), store, scope).unwrap();
    fixture
        .mock
        .enqueue(Ok(response(200, &home("fresh after PAT replacement"))))
        .await;
    let result = read_home(
        session.clone(),
        Some(cached),
        "acceptance".into(),
        Default::default(),
    )
    .await;
    // A control read proves this is cache quarantine, not an expired credential/network failure.
    if result.is_err() {
        let online = session
            .home(None, &HomeSearchLimits::default(), Default::default())
            .await
            .unwrap();
        assert_eq!(online.value.items[0].title, "fresh after PAT replacement");
    }
    assert!(
        result.is_ok(),
        "current verified PAT must retain online Home access; actual={:?}",
        result.err()
    );
}

#[tokio::test]
async fn acceptance_valid_replacement_pat_keeps_feed_online() {
    let fixture = Fixture::new(true).await;
    let service = fixture.service.as_ref().unwrap();
    fixture
        .mock
        .enqueue(Ok(response(200, r#"{"id":1,"login":"octo"}"#)))
        .await;
    service
        .replace(
            fixture.account.clone(),
            Pat::from_str("synthetic-replacement"),
        )
        .await
        .unwrap();
    let session = service.page_session().unwrap();
    let store = fixture.store();
    let scope = store.saved_account(fixture.account.clone()).unwrap();
    let cached = CachedPageReadSession::new(session.clone(), store, scope).unwrap();
    fixture
        .mock
        .enqueue(Ok(events("fresh after replacement", None)))
        .await;
    fixture
        .mock
        .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
        .await;
    let result = read_feed(
        session.clone(),
        Some(cached),
        "acceptance".into(),
        Default::default(),
    )
    .await;
    if result.is_err() {
        let online = session
            .feed(
                &FeedLimits::default(),
                &FollowingLimits::default(),
                &PullRequestDetailLimits::default(),
                30,
                Default::default(),
            )
            .await
            .unwrap();
        assert_eq!(online.value.activities.len(), 1);
    }
    assert!(
        result.is_ok(),
        "current verified PAT must retain online Feed access; actual={:?}",
        result.err()
    );
}

#[tokio::test]
async fn online_feed_continuation_survives_cache_failure_after_replacement() {
    let fixture = Fixture::new(true).await;
    fixture.warm().await;
    let service = fixture.service.as_ref().unwrap();
    fixture
        .mock
        .enqueue(Ok(response(200, r#"{"id":1,"login":"octo"}"#)))
        .await;
    service
        .replace(
            fixture.account.clone(),
            Pat::from_str("synthetic-replacement"),
        )
        .await
        .unwrap();
    let session = service.page_session().unwrap();
    let store = fixture.store();
    let scope = store.saved_account(fixture.account.clone()).unwrap();
    let cached = CachedPageReadSession::new(session.clone(), store, scope).unwrap();
    fixture.mock.enqueue(Ok(events("first", Some(2)))).await;
    fixture
        .mock
        .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
        .await;
    let first = read_feed(
        session.clone(),
        Some(cached.clone()),
        "continuation".into(),
        Default::default(),
    )
    .await
    .unwrap()
    .unwrap();
    let next = first.value.next_page.unwrap();
    fixture.mock.enqueue(Ok(events("second", None))).await;
    fixture
        .mock
        .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
        .await;
    let mut seen = Default::default();
    let page = session
        .feed_page(
            next,
            &FeedLimits::default(),
            &FollowingLimits::default(),
            &PullRequestDetailLimits::default(),
            30,
            &mut seen,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(page.value.activities.len(), 1);
    assert_eq!(page.value.activities[0].event.id, "second");
}

#[gpui::test]
fn acceptance_confirmed_revocation_clears_already_rendered_home(cx: &mut TestAppContext) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(true));
    runtime.block_on(fixture.warm());
    runtime.block_on(async {
        // Exactly one failed first-page read per column; extra scripted failures
        // would otherwise be consumed by the recovery refresh below.
        fixture.network_error().await;
        fixture.network_error().await;
    });
    let store = fixture.store();
    let scope = store.saved_account(fixture.account.clone()).unwrap();
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    assert_eq!(home_title(&page, cx).as_deref(), Some("cached home"));
    assert_eq!(feed_ids(&page, cx), ["first"]);
    let context = page.read_with(cx, |page, _| {
        page.session.as_ref().unwrap().context().clone()
    });
    let (generation, selection, authorization) = page.read_with(cx, |page, _| {
        (
            page.generation,
            page.selection,
            page.cache_authorization.clone(),
        )
    });
    store.revoke_repository(&scope, Some(10)).unwrap();
    assert!(
        page.read_with(cx, |page, _| page.session.as_ref().unwrap().verify()),
        "repository revocation does not revoke the whole account"
    );
    for _ in 0..5 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.run_until_parked();
    }
    assert_eq!(
        home_title(&page, cx),
        None,
        "the existing rendered aggregate must be cleared after confirmed repository revocation"
    );
    assert!(feed_ids(&page, cx).is_empty());
    assert!(page.read_with(cx, |page, _| page.feed_repo_descriptions.is_empty()));
    assert!(page.read_with(cx, |page, _| {
        page.cache_authorization
            .as_ref()
            .is_none_or(|authorization| !authorization.is_current())
    }));
    page.update(cx, |page, cx| page.drain_for_test(cx));
    assert_eq!(home_title(&page, cx), None);
    page.update(cx, |page, cx| {
        let auth = authorization.clone();
        page.apply_append(
            AppendReply {
                generation,
                append_epoch: page.feed_append_epoch,
                selection,
                authorization: auth.clone(),
                context: context.clone(),
                result: Ok((
                    FeedOutcome {
                        activities: Vec::new(),
                        etag: None,
                        poll_interval_seconds: None,
                        next_page: None,
                        readiness: crate::domain::home::FeedReadiness::Complete,
                    },
                    Default::default(),
                    ReadSource::Historical {
                        partition: scope.partition().to_string(),
                        collection: None,
                    },
                )),
            },
            cx,
        );
        page.apply_enrichment(
            EnrichReply {
                generation,
                append_epoch: page.feed_append_epoch,
                selection,
                authorization,
                context,
                request: 1,
                result: Ok(EnrichmentResult {
                    descriptions: [("octo/repo".into(), Some("revoked description".into()))].into(),
                    cache_partition: Some(scope.partition().to_string()),
                }),
            },
            cx,
        );
    });
    assert_eq!(home_title(&page, cx), None);
    assert!(feed_ids(&page, cx).is_empty());
    assert_eq!(
        page.read_with(cx, |page, _| page.description_for_test("octo/repo")),
        None,
        "late append and enrichment must not resurrect revoked content"
    );
    runtime.block_on(async {
        fixture
            .mock
            .enqueue(Ok(response(200, &home("recovered online"))))
            .await;
        fixture.mock.enqueue(Ok(events("recovered", None))).await;
        fixture
            .mock
            .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
            .await;
    });
    cx.update_window_entity(&page, |page, window, cx| {
        page.refresh_for_test(Column::Home, window, cx);
    });
    cx.run_until_parked();
    settle(&page, cx);
    assert_eq!(
        home_title(&page, cx).as_deref(),
        Some("recovered online"),
        "cache-only invalidation must keep an online recovery path"
    );
    assert!(page.read_with(cx, |page, cx| {
        page.view.read(cx).provenance(Column::Home).is_none()
    }));
}

#[tokio::test]
async fn operational_cache_failure_degrades_to_online_without_unlocking_old_snapshots() {
    let fixture = Fixture::new(true).await;
    // Simulate the failure at the existing cache boundary, not by deleting a
    // live SQLite database (Windows correctly locks it against that deletion).
    assert!(matches!(
        cache_failure(PageCacheError::Cache(crate::storage::cache::CacheError::Io)),
        CachedReadFailure::UncachedOnline
    ));
    let store = fixture.store();
    let scope = store.saved_account(fixture.account.clone()).unwrap();
    let session = fixture.service.as_ref().unwrap().page_session().unwrap();
    let cached = CachedPageReadSession::new(session.clone(), store, scope).unwrap();
    fixture
        .mock
        .enqueue(Ok(response(200, &home("cache failed but online works"))))
        .await;
    let result = read_home(
        session,
        Some(cached),
        "operational-failure".into(),
        Default::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.unwrap().value.items[0].title,
        "cache failed but online works"
    );
}

#[tokio::test]
async fn replacement_pat_rejection_never_replays_the_quarantined_snapshot() {
    let fixture = Fixture::new(true).await;
    fixture.warm().await;
    let service = fixture.service.as_ref().unwrap();
    fixture
        .mock
        .enqueue(Ok(response(200, r#"{"id":1,"login":"octo"}"#)))
        .await;
    service
        .replace(
            fixture.account.clone(),
            Pat::from_str("synthetic-replacement"),
        )
        .await
        .unwrap();
    let session = service.page_session().unwrap();
    let store = fixture.store();
    let scope = store.saved_account(fixture.account.clone()).unwrap();
    let cached = CachedPageReadSession::new(session.clone(), store, scope).unwrap();
    fixture.mock.enqueue(Ok(response(401, "{}"))).await;
    let result = read_home(
        session,
        Some(cached),
        "rejected-replacement".into(),
        Default::default(),
    )
    .await;
    assert!(matches!(
        result,
        Err(HomeError::GitHub(GitHubError::Unauthorized))
    ));
}

#[gpui::test]
fn acceptance_cold_start_without_network_revalidation_replays_saved_home(cx: &mut TestAppContext) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(true));
    runtime.block_on(fixture.warm());
    runtime.block_on(async {
        let old = fixture.service.take().unwrap();
        old.close().await.unwrap();
        drop(old);
        let store = fixture.store.take().unwrap();
        drop(store);
        let reopened_store = CacheStore::open(CacheConfig::at(
            fixture.cache_directory.path().to_path_buf(),
        ))
        .unwrap();
        let client = Arc::new(GitHubClient::new(
            GitHubConfig::default(),
            fixture.mock.clone(),
        ));
        let reopened = AccountService::open_with_cache(
            AccountDatabaseConfig::at(fixture.directory.path().join("accounts.sqlite3")),
            fixture.credentials.clone(),
            client,
            fixture.executor.clone(),
            reopened_store.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            reopened.active_summary().unwrap().authorization,
            crate::domain::account::AuthorizationState::PendingVerification
        );
        fixture.service = Some(reopened);
        fixture.store = Some(reopened_store);
        for _ in 0..6 {
            fixture.network_error().await;
        }
    });
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    assert_eq!(home_title(&page, cx).as_deref(), Some("cached home"));
    assert_eq!(feed_ids(&page, cx), ["first"]);
    assert!(page.read_with(cx, |page, cx| {
        page.view.read(cx).provenance(Column::Home).is_some()
    }));
}

#[gpui::test]
fn acceptance_replaced_first_page_never_appends_older_generation_continuation_offline(
    cx: &mut TestAppContext,
) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(true));
    // 1. Warm two pages under generation A
    runtime.block_on(fixture.warm());
    // 2. Online: replace only page one with a fresh snapshot (starts generation B)
    runtime.block_on(async {
        let store = fixture.store();
        let scope = store.saved_account(fixture.account.clone()).unwrap();
        let session = CachedPageReadSession::new(
            fixture.service.as_ref().unwrap().page_session().unwrap(),
            store,
            scope,
        )
        .unwrap();
        fixture
            .mock
            .enqueue(Ok(events("fresh-first-b", Some(2))))
            .await;
        fixture
            .mock
            .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
            .await;
        session
            .feed(
                PageReadMode::Online,
                &FeedLimits::default(),
                &FollowingLimits::default(),
                &PullRequestDetailLimits::default(),
                30,
                Default::default(),
            )
            .await
            .unwrap();
    });
    // 3. Reopen account service + cache store offline without network
    runtime.block_on(async {
        let old = fixture.service.take().unwrap();
        old.close().await.unwrap();
        drop(old);
        let store = fixture.store.take().unwrap();
        drop(store);
        let reopened_store = CacheStore::open(CacheConfig::at(
            fixture.cache_directory.path().to_path_buf(),
        ))
        .unwrap();
        let client = Arc::new(GitHubClient::new(
            GitHubConfig::default(),
            fixture.mock.clone(),
        ));
        let reopened = AccountService::open_with_cache(
            AccountDatabaseConfig::at(fixture.directory.path().join("accounts.sqlite3")),
            fixture.credentials.clone(),
            client,
            fixture.executor.clone(),
            reopened_store.clone(),
        )
        .await
        .unwrap();
        fixture.service = Some(reopened);
        fixture.store = Some(reopened_store);
        for _ in 0..6 {
            fixture.network_error().await;
        }
    });
    // 4. Mount HomePage offline: page 1 shows fresh-first-b (generation B)
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    assert_eq!(feed_ids(&page, cx), ["fresh-first-b"]);
    // 5. Append page 2: the cached page 2 belonged to generation A, not B.
    // It must NOT append "second" (from generation A) to "fresh-first-b"!
    cx.update_window_entity(&page, |page, window, cx| {
        page.load_more_for_test(window, cx);
    });
    for _ in 0..100 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        feed_ids(&page, cx),
        ["fresh-first-b"],
        "older generation continuation must never be appended to replaced first page"
    );
    assert!(
        page.read_with(cx, |page, cx| matches!(
            page.view.read(cx).feed_append_state(),
            FeedAppendState::NotCached { page: 2 }
        )),
        "incompatible continuation must be reported as not cached, actual={:?}",
        page.read_with(cx, |page, cx| page
            .view
            .read(cx)
            .feed_append_state()
            .clone())
    );
}

#[gpui::test]
fn no_cache_binding_keeps_the_original_online_path(cx: &mut TestAppContext) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(false));
    runtime.block_on(async {
        fixture.mock.enqueue(Ok(response(200, r#"{"id":1,"login":"octo"}"#))).await;
        fixture.mock.enqueue(Ok(response(200, &home("online only")))).await;
        fixture.mock.enqueue(Ok(events("online", None))).await;
        fixture.mock.enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#))).await;
        fixture.mock.enqueue(Ok(response(200, r#"{"id":10,"name":"repo","owner":{"id":1,"login":"octo"},"visibility":"public","private":false,"default_branch":"main","fork":false,"archived":false,"stargazers_count":0,"forks_count":0,"open_issues_count":0,"size":1,"description":"online description","html_url":"https://github.com/octo/repo"}"#))).await;
    });
    let (page, cx) = open_page(&mut fixture, cx);
    settle(&page, cx);
    assert_eq!(
        home_title(&page, cx).as_deref(),
        Some("online only"),
        "state={:?}",
        page.read_with(cx, |page, cx| page.view.read(cx).home_state().clone())
    );
    assert_eq!(feed_ids(&page, cx), ["online"]);
    assert!(!page.read_with(cx, |page, _| page.cached_enabled_for_test()));
}
