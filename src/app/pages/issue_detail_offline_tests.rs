use super::*;
use crate::app::accounts::runtime::open_accounts_with_cache;
use crate::auth::token::Pat;
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{CacheConfig, CacheStore};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Semaphore;

struct IssueTransport {
    base: PageTransport,
    offline: AtomicBool,
    refreshed: AtomicBool,
    hold_body: AtomicBool,
    release: Semaphore,
}

impl IssueTransport {
    fn new() -> Self {
        Self {
            base: PageTransport::default(),
            offline: AtomicBool::new(false),
            refreshed: AtomicBool::new(false),
            hold_body: AtomicBool::new(false),
            release: Semaphore::new(0),
        }
    }
}

impl Transport for IssueTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max: usize,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
    {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max {
                return Err(TransportError::ResponseTooLarge { limit: max });
            }
            Ok(response)
        })
    }

    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
    {
        Box::pin(async move {
            let url = request.url.clone();
            assert!(!url.contains("/demo/issues/21/comments?per_page=30&page=3"));
            if self.hold_body.load(Ordering::Acquire) && url.ends_with("/demo/issues/21") {
                tokio::select! {
                    permit = self.release.acquire() => permit.unwrap().forget(),
                    _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled),
                }
            }
            if self.offline.load(Ordering::Acquire) {
                return Err(TransportError::Network {
                    message: "synthetic offline".into(),
                });
            }
            if self.refreshed.load(Ordering::Acquire) && url.contains("/demo/issues/21/comments") {
                assert!(url.ends_with("&page=1"));
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 401, "body": "Replacement comment", "created_at": "2026-11-20T00:00:00Z",
                        "updated_at": "2026-11-20T00:00:00Z"
                    }]),
                ));
            }
            let mut response = self.base.execute(request, cancel).await?;
            if url.contains("/demo/issues/21/comments") && url.ends_with("&page=2") {
                response.headers.push(("link".into(), "<https://api.github.com/repos/alice/demo/issues/21/comments?per_page=30&page=3>; rel=\"next\"".into()));
            }
            Ok(response)
        })
    }
}

fn mount(
    cx: &mut TestAppContext,
    runtime: Arc<AccountRuntime>,
) -> (Entity<Shell>, &mut VisualTestContext) {
    let (shell, cx) = cx.add_window_view(|window, cx| {
        let controller = cx.new(|cx| Accounts::new(Some(runtime), window, cx));
        cx.set_global(AccountsHandle(controller));
        Shell::new(window, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(1100.)));
    let controller = cx.read(AccountsHandle::get).unwrap();
    wait_until(&controller, cx, |accounts| {
        accounts.snapshot().active.is_some()
    });
    navigate(&shell, cx, "demo");
    (shell, cx)
}

fn navigate(shell: &Entity<Shell>, cx: &mut VisualTestContext, repo: &str) {
    cx.update_window_entity(shell, |shell, window, cx| {
        shell.navigate(
            Route::Issue {
                namespace: "alice".into(),
                name: repo.into(),
                number: 21,
            },
            window,
            cx,
        );
    });
}

#[gpui::test]
fn issue_detail_reopens_two_read_pages_and_retries_from_page_one(cx: &mut TestAppContext) {
    setup(cx);
    let temp = tempfile::tempdir().unwrap();
    let clock = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let transport = Arc::new(IssueTransport::new());
    let store = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
    let open = || {
        let executor = Arc::new(BackgroundExecutor::new(4));
        let accounts = clock
            .block_on(open_accounts_with_cache(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                credentials.clone(),
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    transport.clone(),
                )),
                executor.clone(),
                Ok(store.clone()),
            ))
            .unwrap();
        Arc::new(AccountRuntime::new(accounts, executor))
    };
    let online = open();
    clock
        .block_on(
            online
                .accounts
                .login(Pat::from_str("synthetic-issue-reader")),
        )
        .unwrap();
    let (shell, view) = mount(cx, online.clone());
    wait_for_bounds(view, "issue-detail-body");
    wait_for_bounds(view, "issue-comment-301");
    wait_for_bounds(view, "issue-detail-load-more");
    let more = view.debug_bounds("issue-detail-load-more").unwrap();
    view.simulate_click(more.center(), gpui::Modifiers::none());
    wait_for_bounds(view, "issue-comment-302");
    assert!(view.debug_bounds("issue-detail-load-more").is_some());
    view.update_window_entity(&shell, |_, window, _| window.remove_window());
    clock.block_on(online.accounts.close()).unwrap();
    drop(online);

    transport.offline.store(true, Ordering::Release);
    transport.hold_body.store(true, Ordering::Release);
    let reopened = open();
    let (shell, view) = mount(cx, reopened.clone());
    wait_for_bounds(view, "issue-detail-body");
    wait_for_bounds(view, "issue-comment-302");
    assert!(view.debug_bounds("issue-comment-301").is_some());
    assert!(view.debug_bounds("issue-detail-load-more").is_none());
    assert!(view.debug_bounds("issue-detail-comments-partial").is_some());
    assert!(view.debug_bounds("issue-detail-comments-cached").is_none());
    assert!(view.debug_bounds("issue-detail-cached-badge").is_none());
    transport.release.add_permits(1);
    wait_for_bounds(view, "issue-detail-cached-badge");
    wait_for_bounds(view, "issue-detail-comments-cached");
    assert!(view.debug_bounds("issue-comment-302").is_some());

    transport.refreshed.store(true, Ordering::Release);
    transport.offline.store(false, Ordering::Release);
    assert!(view.update_window_entity(&shell, |shell, window, cx| {
        shell.refresh_active_page(window, cx)
    }));
    wait_for_bounds(view, "issue-detail-refreshing");
    assert!(view.debug_bounds("issue-detail-body").is_some());
    assert!(view.debug_bounds("issue-comment-302").is_some());
    assert!(view.debug_bounds("issue-detail-comments-cached").is_none());
    assert!(view.debug_bounds("issue-detail-cached-badge").is_none());
    transport.release.add_permits(1);
    wait_for_bounds(view, "issue-comment-401");
    assert!(view.debug_bounds("issue-comment-301").is_none());
    assert!(view.debug_bounds("issue-comment-302").is_none());
    assert!(view.debug_bounds("issue-detail-comments-partial").is_none());

    transport.offline.store(true, Ordering::Release);
    navigate(&shell, view, "other");
    wait_for_bounds(view, "issue-detail-error-failed");
    assert!(view.debug_bounds("issue-detail-body").is_none());
    assert!(view.debug_bounds("issue-comment-401").is_none());

    navigate(&shell, view, "demo");
    wait_for_bounds(view, "issue-comment-401");
    let repos = reopened.accounts.repo_service();
    let cache = repos.cache(repos.session().unwrap().context()).unwrap();
    cache
        .store()
        .revoke_repository(cache.scope(), Some(10))
        .unwrap();
    wait_for_bounds(view, "issue-detail-error-permission");
    assert!(view.debug_bounds("issue-detail-body").is_none());
    assert!(view.debug_bounds("issue-comment-401").is_none());
    clock.block_on(reopened.accounts.close()).unwrap();
}
