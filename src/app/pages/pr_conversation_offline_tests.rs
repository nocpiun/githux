use super::*;
use crate::app::accounts::runtime::open_accounts_with_cache;
use crate::auth::token::Pat;
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{CacheConfig, CacheStore};
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::Semaphore;

const ONLINE: u8 = 0;
const OFFLINE: u8 = 1;
const UPDATED: u8 = 2;
const DENIED: u8 = 3;
const NO_STORE_FAILURE: u8 = 4;

fn no_store_failure() -> HttpResponse {
    let mut response = json(503, serde_json::json!({}));
    response
        .headers
        .push(("cache-control".into(), "private, no-store".into()));
    response
}

struct ConversationTransport {
    base: PageTransport,
    mode: AtomicU8,
    release_body: Semaphore,
}

impl ConversationTransport {
    fn new() -> Self {
        Self {
            base: PageTransport::default(),
            mode: AtomicU8::new(ONLINE),
            release_body: Semaphore::new(0),
        }
    }
}

impl Transport for ConversationTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
    {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max_body_bytes {
                return Err(TransportError::ResponseTooLarge {
                    limit: max_body_bytes,
                });
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
            let mode = self.mode.load(Ordering::Acquire);
            let offline = || TransportError::Network {
                message: "synthetic offline".into(),
            };
            if request
                .body
                .as_ref()
                .is_some_and(|body| body.contains("query PRConversation"))
            {
                if matches!(mode, OFFLINE | UPDATED | DENIED) {
                    tokio::select! {
                        permit = self.release_body.acquire() => permit.unwrap().forget(),
                        _ = cancel.cancelled() => return Err(TransportError::Cancelled),
                    }
                }
                return match mode {
                    OFFLINE => Err(offline()),
                    DENIED => Ok(json(403, serde_json::json!({"message": "Forbidden"}))),
                    NO_STORE_FAILURE => Ok(no_store_failure()),
                    _ => {
                        let body = if mode == UPDATED {
                            "Fresh PR body"
                        } else {
                            "Original PR body"
                        };
                        Ok(graphql_ok(serde_json::json!({"repository": {
                            "nameWithOwner": "alice/demo", "databaseId": 10,
                            "pullRequest": {"number": 5, "title": "Conversation fixture",
                                "state": "OPEN", "isDraft": false, "author": null, "body": body,
                                "baseRefName": "main", "headRefName": "change", "headRef": null,
                                "headRepository": null}
                        }})))
                    }
                };
            }
            if request.url.contains("/repos/alice/demo/issues/5/comments") {
                if mode == OFFLINE {
                    return Err(offline());
                }
                if mode == NO_STORE_FAILURE {
                    return Ok(no_store_failure());
                }
                let second = request.url.ends_with("&page=2");
                let id = if mode == UPDATED {
                    601
                } else if second {
                    502
                } else {
                    501
                };
                let mut response = json(
                    200,
                    serde_json::json!([{
                        "id": id, "user": {"login": "bob"}, "body": format!("Comment {id}"),
                        "created_at": "2026-10-28T11:00:00Z", "updated_at": "2026-10-28T11:00:00Z"
                    }]),
                );
                if mode == ONLINE {
                    let next = if second { 3 } else { 2 };
                    response.headers.push(("link".into(), format!("<https://api.github.com/repos/alice/demo/issues/5/comments?per_page=30&page={next}>; rel=\"next\"")));
                }
                return Ok(response);
            }
            if mode == OFFLINE {
                return Err(offline());
            }
            if request.url.ends_with("/user")
                && request.headers.iter().any(|(key, value)| {
                    key.eq_ignore_ascii_case("Authorization")
                        && value.ends_with("synthetic-bob-conversation")
                })
            {
                return Ok(json(200, serde_json::json!({"id": 2, "login": "bob"})));
            }
            self.base.execute(request, cancel).await
        })
    }
}

struct Harness {
    temp: tempfile::TempDir,
    clock: tokio::runtime::Runtime,
    credentials: Arc<InMemoryCredentialStore>,
    store: CacheStore,
    transport: Arc<ConversationTransport>,
}

impl Harness {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        Self {
            store: CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap(),
            temp,
            clock: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            credentials: Arc::new(InMemoryCredentialStore::new()),
            transport: Arc::new(ConversationTransport::new()),
        }
    }

    fn service(&self) -> Arc<AccountRuntime> {
        let executor = Arc::new(BackgroundExecutor::new(4));
        let service = self
            .clock
            .block_on(open_accounts_with_cache(
                AccountDatabaseConfig::at(self.temp.path().join("accounts.sqlite3")),
                self.credentials.clone(),
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    self.transport.clone(),
                )),
                executor.clone(),
                Ok(self.store.clone()),
            ))
            .unwrap();
        Arc::new(AccountRuntime::new(service, executor))
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
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(
            Route::PullRequest {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 5,
            },
            window,
            cx,
        );
    });
    (shell, cx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    wait_for_bounds(cx, selector);
    let bounds = cx.debug_bounds(selector).unwrap();
    cx.simulate_click(
        gpui::point(bounds.left() + gpui::px(10.), bounds.center().y),
        gpui::Modifiers::none(),
    );
}

fn body(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    shell.read_with(cx, |shell, cx| {
        shell
            .pull_request_summary_for_test(cx)
            .and_then(|pr| pr.body.clone())
    })
}

#[gpui::test]
fn pr_conversation_reopens_offline_retries_and_clears_on_denial(cx: &mut TestAppContext) {
    setup(cx);
    let harness = Harness::new();
    let original = harness.service();
    harness
        .clock
        .block_on(
            original
                .accounts
                .login(Pat::from_str("synthetic-alice-conversation")),
        )
        .unwrap();
    harness
        .clock
        .block_on(
            original
                .accounts
                .login(Pat::from_str("synthetic-bob-conversation")),
        )
        .unwrap();
    let alice = AccountKey::new(1, HostId::github());
    let bob = AccountKey::new(2, HostId::github());
    harness
        .clock
        .block_on(original.accounts.set_active(alice.clone()))
        .unwrap();
    {
        let (shell, cx) = mount(cx, original.clone());
        wait_for_bounds(cx, "pr-comment-501");
        assert_eq!(body(&shell, cx).as_deref(), Some("Original PR body"));
        wait_for_bounds(cx, "pr-comments-load-more");
        let more = cx.debug_bounds("pr-comments-load-more").unwrap();
        cx.simulate_click(
            gpui::point(more.left() + gpui::px(35.), more.center().y),
            gpui::Modifiers::none(),
        );
        wait_for_bounds(cx, "pr-comment-502");
    }
    harness.clock.block_on(original.accounts.close()).unwrap();
    harness.transport.mode.store(OFFLINE, Ordering::Release);
    let other_account = harness.service();
    harness
        .clock
        .block_on(other_account.accounts.set_active(bob))
        .unwrap();
    {
        let (shell, cx) = mount(cx, other_account.clone());
        wait_for_bounds(cx, "compact-loading-Loading pull request");
        assert!(body(&shell, cx).is_none());
        assert!(cx.debug_bounds("pr-comment-502").is_none());
        harness.transport.release_body.add_permits(1);
        wait_for_bounds(cx, "compact-failed-Could not read this pull request");
    }
    harness
        .clock
        .block_on(other_account.accounts.close())
        .unwrap();
    let reopened = harness.service();
    harness
        .clock
        .block_on(reopened.accounts.set_active(alice))
        .unwrap();
    {
        let (shell, cx) = mount(cx, reopened.clone());
        wait_for_bounds(cx, "pr-comment-502");
        assert_eq!(body(&shell, cx).as_deref(), Some("Original PR body"));
        assert!(cx.debug_bounds("pr-comment-501").is_some());
        assert!(cx.debug_bounds("pr-comments-partial").is_some());
        assert!(cx.debug_bounds("pr-comments-empty").is_none());
        assert!(cx.debug_bounds("pr-comments-load-more").is_none());
        assert!(
            cx.debug_bounds("compact-failed-Cached · Pull request refresh failed")
                .is_none()
        );
        assert!(cx.debug_bounds("pr-comments-retry").is_none());

        harness.transport.release_body.add_permits(1);
        wait_for_bounds(cx, "compact-failed-Cached · Pull request refresh failed");
        wait_for_bounds(cx, "pr-comments-retry");
        assert!(cx.debug_bounds("pr-comment-502").is_some());
        harness.transport.mode.store(UPDATED, Ordering::Release);
        click(cx, "compact-failed-Cached · Pull request refresh failed");
        cx.run_until_parked();
        assert_eq!(body(&shell, cx).as_deref(), Some("Original PR body"));
        assert!(cx.debug_bounds("pr-comment-502").is_some());
        assert!(
            cx.debug_bounds("compact-failed-Cached · Pull request refresh failed")
                .is_none()
        );
        assert!(cx.debug_bounds("pr-comments-retry").is_none());

        harness.transport.release_body.add_permits(1);
        wait_for_bounds(cx, "pr-comment-601");
        assert_eq!(body(&shell, cx).as_deref(), Some("Fresh PR body"));
        assert!(cx.debug_bounds("pr-comment-501").is_none());
        assert!(cx.debug_bounds("pr-comment-502").is_none());
        assert!(cx.debug_bounds("pr-comments-partial").is_none());

        harness.transport.mode.store(DENIED, Ordering::Release);
        assert!(cx.update_window_entity(&shell, |shell, window, cx| {
            shell.refresh_active_page(window, cx)
        }));
        harness.transport.release_body.add_permits(1);
        wait_for_bounds(cx, "compact-failed-Could not read this pull request");
        assert!(body(&shell, cx).is_none());
        assert!(cx.debug_bounds("pr-comment-601").is_none());
    }
    harness.clock.block_on(reopened.accounts.close()).unwrap();
    harness.transport.mode.store(OFFLINE, Ordering::Release);
    let denied = harness.service();
    {
        let (shell, cx) = mount(cx, denied.clone());
        harness.transport.release_body.add_permits(1);
        wait_for_bounds(cx, "compact-failed-Could not read this pull request");
        assert!(body(&shell, cx).is_none());
    }
    harness.clock.block_on(denied.accounts.close()).unwrap();
}

#[test]
fn pr_conversation_replaces_and_invalidates_snapshots_under_the_replay_key() {
    use crate::data::pr_conversation::ConversationSession;
    use crate::domain::{
        issue_detail::IssueLocator, pr_conversation::Target, repository::RepositoryLocator,
    };
    use crate::github::issue_detail::IssueDetailLimits;

    let harness = Harness::new();
    let runtime = harness.service();
    harness.clock.block_on(async {
        runtime
            .accounts
            .login(Pat::from_str("synthetic-alice-conversation"))
            .await
            .unwrap();
        let repos = runtime.accounts.repo_service();
        let session = repos.session().unwrap();
        let cache = repos.cache(session.context()).unwrap();
        let repository = RepositoryLocator::new("alice", "demo").unwrap();
        let locator = IssueLocator::from_repository(repository.clone(), 5).unwrap();
        let target = Target::new(repository, 5).unwrap();
        let conversation = ConversationSession::new(session, target.clone(), locator.clone());
        let cached =
            || conversation.cached(IssueDetailLimits::default(), CancellationToken::default());
        let refresh = || conversation.refresh(CancellationToken::default());

        let scope = refresh().await.result.unwrap().cache_scope;
        let refresh_comments = |collection| {
            conversation.refresh_comments(
                scope.clone(),
                collection,
                None,
                IssueDetailLimits::default(),
                CancellationToken::default(),
                true,
            )
        };
        let first = refresh_comments(None).await.result.unwrap();
        assert_eq!(
            cached().await.unwrap().comments.unwrap().comments[0].id,
            501
        );

        harness.transport.mode.store(UPDATED, Ordering::Release);
        let replacement = refresh_comments(first.collection).await.result.unwrap();
        let saved = cached().await.unwrap().comments.unwrap();
        assert_eq!(
            saved
                .comments
                .iter()
                .map(|comment| comment.id)
                .collect::<Vec<_>>(),
            [601]
        );
        assert!(!saved.partial);

        harness
            .transport
            .mode
            .store(NO_STORE_FAILURE, Ordering::Release);
        let failed_comments = refresh_comments(replacement.collection).await;
        assert!(failed_comments.no_store && failed_comments.result.is_err());
        assert!(cached().await.unwrap().comments.is_none());
        let failed = refresh().await;
        assert!(failed.no_store && failed.result.is_err());
        assert!(cached().await.is_none());

        harness.transport.mode.store(ONLINE, Ordering::Release);
        assert!(refresh().await.result.is_ok());
        let copied = cached().await.unwrap();
        cache
            .store()
            .revoke_repository(cache.scope(), Some(10))
            .unwrap();
        assert!(!copied.authorization.is_current());
        assert!(cached().await.is_none());
        assert!(
            refresh().await.result.is_ok(),
            "cache denial preserves the live read path"
        );

        runtime
            .accounts
            .replace(
                AccountKey::new(1, HostId::github()),
                Pat::from_str("synthetic-replacement-conversation"),
            )
            .await
            .unwrap();
        let conversation = ConversationSession::new(repos.session().unwrap(), target, locator);
        let cancel = CancellationToken::default();
        let limits = IssueDetailLimits::default();
        assert!(
            conversation
                .cached(limits.clone(), cancel.clone())
                .await
                .is_none()
        );
        let scope = conversation
            .refresh(cancel.clone())
            .await
            .result
            .unwrap()
            .cache_scope;
        assert!(
            scope.is_some(),
            "a successful Conversation read must restore repository caching after PAT replacement"
        );
        assert!(
            conversation
                .refresh_comments(scope, None, None, limits.clone(), cancel.clone(), true)
                .await
                .result
                .is_ok()
        );
        let restored = conversation.cached(limits, cancel).await.unwrap();
        assert_eq!(restored.reading.body.as_deref(), Some("Original PR body"));
        assert_eq!(restored.comments.unwrap().comments[0].id, 501);
    });
}
