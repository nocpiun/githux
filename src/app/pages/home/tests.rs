//! Deterministic HomePage composition with the real page-cache adapter.
//! No GitHub, real credential, or native-window claim is made by these tests.

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

struct Fixture {
    service: Option<AccountService>,
    executor: Arc<BackgroundExecutor>,
    mock: Arc<MockTransport>,
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
        let client = Arc::new(GitHubClient::new(GitHubConfig::default(), mock.clone()));
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
        self.mock.enqueue(Ok(events("second", Some(3)))).await;
        self.mock
            .enqueue(Ok(response(200, r#"[{"id":7,"login":"bob"}]"#)))
            .await;
        session
            .feed_page(
                PageReadMode::Online,
                crate::domain::home::FeedSourceCursor { page: 2 },
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

struct AccountsHolder(Entity<crate::app::accounts::Accounts>);

impl gpui::Render for AccountsHolder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

fn open_page<'a>(
    fixture: &mut Fixture,
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
        let controller =
            cx.new(|cx| crate::app::accounts::Accounts::new(Some(Arc::new(runtime)), window, cx));
        cx.set_global(crate::app::AccountsHandle(controller.clone()));
        AccountsHolder(controller)
    });
    let controller = holder.read_with(cx, |h, _| h.0.clone());
    // Let the background account catalog restore settle before constructing the HomePage.
    for _ in 0..400 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(2));
        if cx.update(|_, cx| crate::app::AccountsHandle::services(cx).is_some()) {
            break;
        }
    }
    let page = cx.update(|window, cx| cx.new(|cx| HomePage::new_for_test(window, cx)));
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
    for _ in 0..200 {
        page.update(cx, |page, cx| page.drain_for_test(cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(page.read_with(cx, |page, cx| matches!(
        page.view.read(cx).home_state(),
        PanelState::Ready {
            content: PanelContent::Error(_),
            ..
        }
    )));
    page.update(cx, |page, _| {
        if let Some(work) = page.work[0].take() {
            work.source.cancel();
        }
    });
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
