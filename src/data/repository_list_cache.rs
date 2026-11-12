//! Account-bound `/user/repos` snapshot adapter.
//!
//! `RepoService::list_repos_cached` composes the existing authenticated
//! `fetch_user_repos` pagination adapter with the one application `CacheStore`
//! already bound through `RepoService::cache(&context)`. Online reads record the
//! exact response exchange (URL, status, safe headers and cleaned JSON body) and
//! publish a versioned snapshot; offline reads replay that exchange through the
//! same pagination adapter, so `Complete`/`Partial`/SSO partial-results and
//! later-page failures keep their original meaning instead of being recomputed
//! by a second policy stack. No credential, cache partition or permission path
//! is duplicated here.

use super::page_cache::{PageCacheError, PageOrigin, PageReadMode, PageValue};
use super::{RepoService, RepositoryReadContext, Scoped};
use crate::domain::cache::{CacheCompleteness, CacheMetadata, CacheMiss};
use crate::domain::error::GitHubError;
use crate::domain::repo::{ReposOutcome, ReposReadiness};
use crate::github::client::GitHubClient;
use crate::github::home::cache::{Exchange, PageTransport};
use crate::github::repos::fetch_user_repos;
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{CacheError, CacheKey, CacheLease, CacheRead, CacheScope, CacheStore};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const VERSION: u32 = 1;
const RESOURCE: &str = "repository-list-v1";
const REPRESENTATION: &str = "githux-repository-list-v1";
const QUERY: &str = "affiliation=owner,collaborator,organization_member&per_page=100&sort=pushed";

/// One durable account-aggregate snapshot. It holds the raw response exchange
/// set for the `/user/repos` read, never decoded repositories: replay still runs
/// the real pagination/classification adapter.
#[derive(Serialize, Deserialize)]
struct ReposSnapshotV1 {
    version: u32,
    exchanges: Vec<Exchange>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn metadata(stamp: i64, completeness: CacheCompleteness, no_store: bool) -> CacheMetadata {
    CacheMetadata {
        representation_version: VERSION,
        fetched_at_ms: stamp,
        verified_at_ms: stamp,
        completeness,
        etag: None,
        last_modified: None,
        cache_control: no_store.then(|| "no-store".into()),
    }
}

fn completeness(outcome: &ReposOutcome) -> CacheCompleteness {
    match outcome.readiness {
        ReposReadiness::Complete => CacheCompleteness::Complete,
        ReposReadiness::Partial { .. } => CacheCompleteness::Partial {
            reason: "Repository list incomplete".into(),
        },
    }
}

fn request_identity(
    login: &str,
    config: &crate::github::client::GitHubConfig,
    partial: bool,
) -> String {
    serde_json::json!({
        "login": login,
        "endpoint": "user/repos",
        "query": QUERY,
        "base": config.base_url,
        "accept": config.accept,
        "api": config.api_version,
        "partial": partial,
    })
    .to_string()
}

/// Keep only the representation consumed by the repository adapter. The shared
/// recorder already removes failed HTTP bodies and transport error text; a 2xx
/// malformed body must also not persist a proxy/error page verbatim. `null`
/// preserves its parse-failure semantics when replayed through fetch_user_repos.
fn encode_snapshot(exchanges: Vec<Exchange>) -> Result<Vec<u8>, CacheError> {
    let mut dto = serde_json::to_value(ReposSnapshotV1 {
        version: VERSION,
        exchanges,
    })
    .map_err(|_| CacheError::Corrupt)?;
    if let Some(exchanges) = dto["exchanges"].as_array_mut() {
        for exchange in exchanges {
            let Some(body) = exchange["response"]["body"].as_str() else {
                continue;
            };
            let body = crate::github::repos::snapshot_body(body);
            exchange["response"]["body"] = serde_json::Value::String(body);
        }
    }
    serde_json::to_vec(&dto).map_err(|_| CacheError::Corrupt)
}

fn load_snapshot(
    store: &CacheStore,
    scope: &CacheScope,
    complete: &CacheKey,
    partial: &CacheKey,
) -> Result<Result<(ReposSnapshotV1, CacheMetadata, CacheLease), CacheMiss>, CacheError> {
    let mut missing = CacheMiss::NotCached;
    // Complete wins over Partial; a newer Partial never replaces older Complete.
    for key in [complete, partial] {
        match store.read(scope, key)? {
            CacheRead::Miss(reason) => {
                if reason != CacheMiss::NotCached {
                    missing = reason;
                }
            }
            CacheRead::Hit(lease) => {
                if let Some(snapshot) = lease.snapshot()? {
                    match serde_json::from_slice::<ReposSnapshotV1>(&snapshot.bytes) {
                        Ok(dto) if dto.version == VERSION => {
                            return Ok(Ok((dto, snapshot.metadata, lease)));
                        }
                        Ok(_) => missing = CacheMiss::RepresentationVersion,
                        Err(_) => missing = CacheMiss::Corrupt,
                    }
                }
            }
        }
    }
    Ok(Err(missing))
}

impl RepoService {
    /// Repository list with the account-bound snapshot semantics of
    /// [`crate::data::page_cache`].
    ///
    /// `Online` performs the full `/user/repos` read and records a versioned
    /// snapshot; `Offline` replays the last Complete (falling back to Partial)
    /// snapshot without a network request. The account binding comes from
    /// [`RepoService::cache`]; when no cache binding exists the returned error is
    /// `PageCacheError::Cache(CacheError::AccessDenied)` so callers can fall back
    /// to the plain `list_repos_cancellable` online path.
    pub async fn list_repos_cached(
        &self,
        mode: PageReadMode,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<ReposOutcome>>, PageCacheError<GitHubError>> {
        let context = self
            .capture_read_context()
            .map_err(PageCacheError::Context)?;
        let (store, scope) = match self.cache(&context) {
            Some(binding) => binding.into_parts(),
            None => return Err(PageCacheError::Cache(CacheError::AccessDenied)),
        };
        self.guard(&context, &cancel)?;
        let authorization = store
            .authorization(&scope, None)
            .ok_or(PageCacheError::Cache(CacheError::Stale))?;

        let config = self.client.config();
        let complete_key = CacheKey::mutable(
            None,
            true,
            RESOURCE,
            &request_identity(context.active_login(), config, false),
            REPRESENTATION,
            VERSION,
        )
        .map_err(PageCacheError::Cache)?;
        let partial_key = CacheKey::mutable(
            None,
            true,
            RESOURCE,
            &request_identity(context.active_login(), config, true),
            REPRESENTATION,
            VERSION,
        )
        .map_err(PageCacheError::Cache)?;

        let (transport, cached_metadata, tickets, lease) = match mode {
            PageReadMode::Offline => {
                let complete = complete_key.clone();
                let partial = partial_key.clone();
                let loaded = self
                    .cache_blocking(
                        &context,
                        store.clone(),
                        scope.clone(),
                        move |store, scope| load_snapshot(&store, &scope, &complete, &partial),
                    )
                    .await?;
                self.guard(&context, &cancel)?;
                let (dto, metadata, lease) = match loaded {
                    Ok(value) => value,
                    Err(reason) => {
                        return Ok(Scoped {
                            context: context.clone(),
                            value: PageValue::Miss(reason),
                        });
                    }
                };
                (
                    Arc::new(PageTransport::replay(dto.exchanges)),
                    Some(metadata),
                    None,
                    Some(lease),
                )
            }
            PageReadMode::Online => {
                let complete = complete_key.clone();
                let partial = partial_key.clone();
                let tickets = self
                    .cache_blocking(
                        &context,
                        store.clone(),
                        scope.clone(),
                        move |store, scope| {
                            Ok((
                                store.begin_write(&scope, &complete)?,
                                store.begin_write(&scope, &partial)?,
                            ))
                        },
                    )
                    .await?;
                (
                    Arc::new(PageTransport::recording(self.client.transport().clone())),
                    None,
                    Some(tickets),
                    None,
                )
            }
        };

        // The original authorization path still loads the credential, checks the
        // account epoch and acquires the concurrency permit; only the transport
        // is swapped for recording/replay. Offline replay therefore observes the
        // exact same staleness/cancellation gates as an online read.
        let read_client = Arc::new(GitHubClient::new(config.clone(), transport.clone()));
        // An offline replay performs no HTTP request, so it must not hold one of the bounded
        // network permits that the concurrent revalidation needs.
        let local = matches!(mode, PageReadMode::Offline);
        let read = {
            let read_client = read_client.clone();
            async move |token, cancel| fetch_user_repos(&read_client, &token, cancel).await
        };
        let outcome = if local {
            self.read_authenticated_local(&context, cancel.clone(), read)
                .await
        } else {
            self.read_authenticated(&context, cancel.clone(), read)
                .await
        };

        if !self.is_current(&context) {
            return Err(PageCacheError::Context(GitHubError::StaleContext));
        }
        if !authorization.is_current() {
            return Err(PageCacheError::Cache(CacheError::Stale));
        }
        if transport.rejected() {
            // Honor no-store even on a 401. Attempt invalidation while these
            // pre-request tickets still belong to the active cache epoch, then
            // record rejection even if invalidation failed. Rejection itself
            // advances the epoch and would make the tickets stale.
            let invalidation = if transport.no_store()
                && let Some((complete, partial)) = tickets
            {
                let invalidate = metadata(now_ms(), CacheCompleteness::Complete, true);
                self.cache_blocking(&context, store.clone(), scope.clone(), move |store, _| {
                    store.put(complete, &[], invalidate.clone())?;
                    store.put(partial, &[], invalidate)?;
                    Ok(())
                })
                .await
            } else {
                Ok(())
            };
            // A current-session 401 is an explicit credential rejection. Route it
            // through the account lifecycle so the durable account journal and
            // local scope suspension keep a later writable cache from serving the
            // old private snapshot, including after restart.
            self.credential_rejected(&context)
                .await
                .map_err(PageCacheError::Context)?;
            invalidation?;
            return Err(PageCacheError::Context(GitHubError::Unauthorized));
        }
        if let Some(lease) = lease {
            // Revocation/PAT replacement during replay invalidates the lease; the
            // already-copied outcome must not be handed out as a live snapshot.
            let valid = self
                .cache_blocking(
                    &context,
                    store.clone(),
                    scope.clone(),
                    move |_store, _scope| Ok(lease.snapshot()?.is_some()),
                )
                .await?;
            if !valid {
                return Err(PageCacheError::Cache(CacheError::AccessDenied));
            }
        }

        let stamp = now_ms();
        if transport.no_store()
            && let Some((complete, partial)) = tickets
        {
            // no-store invalidates both slots even when this first page later
            // fails classification/parsing; only afterwards is the read error
            // surfaced, matching the page-cache adapter.
            let invalidate = metadata(stamp, CacheCompleteness::Complete, true);
            self.cache_blocking(
                &context,
                store.clone(),
                scope.clone(),
                move |store, _scope| {
                    store.put(complete, &[], invalidate.clone())?;
                    store.put(partial, &[], invalidate)?;
                    Ok(())
                },
            )
            .await?;
            let outcome = outcome.map_err(PageCacheError::Read)?;
            let observed = completeness(&outcome);
            self.guard(&context, &cancel)?;
            return Ok(Scoped {
                context: context.clone(),
                value: PageValue::Value {
                    value: outcome,
                    origin: PageOrigin::Network,
                    metadata: metadata(stamp, observed, true),
                },
            });
        }

        let outcome = outcome.map_err(PageCacheError::Read)?;
        let observed = completeness(&outcome);
        let mut value_metadata =
            cached_metadata.unwrap_or_else(|| metadata(stamp, observed.clone(), false));
        if observed != CacheCompleteness::Complete {
            value_metadata.completeness = observed.clone();
        }
        let origin = if tickets.is_some() {
            PageOrigin::Network
        } else {
            PageOrigin::HistoricalSnapshot
        };
        if let Some((complete, partial)) = tickets {
            let ticket = if observed == CacheCompleteness::Complete {
                complete
            } else {
                partial
            };
            let exchanges = transport.exchanges();
            let stored_metadata = value_metadata.clone();
            self.cache_blocking(
                &context,
                store.clone(),
                scope.clone(),
                move |store, _scope| {
                    let bytes = encode_snapshot(exchanges)?;
                    match store.put(ticket, &bytes, stored_metadata) {
                        Ok(_) | Err(CacheError::TooLarge | CacheError::Capacity) => Ok(()),
                        Err(error) => Err(error),
                    }
                },
            )
            .await?;
        }
        self.guard(&context, &cancel)?;
        if !authorization.is_current() {
            return Err(PageCacheError::Cache(CacheError::Stale));
        }
        Ok(Scoped {
            context: context.clone(),
            value: PageValue::Value {
                value: outcome,
                origin,
                metadata: value_metadata,
            },
        })
    }

    fn guard(
        &self,
        context: &RepositoryReadContext,
        cancel: &CancellationToken,
    ) -> Result<(), PageCacheError<GitHubError>> {
        if !self.is_current(context) {
            return Err(PageCacheError::Context(GitHubError::StaleContext));
        }
        if cancel.is_cancelled() {
            return Err(PageCacheError::Context(GitHubError::Cancelled));
        }
        Ok(())
    }

    /// Run a cache side effect on the blocking pool while holding the shared
    /// account commit lock, so the final context check and the cache effect are
    /// serialized with PAT/selection commits. Network reads never hold this lock.
    async fn cache_blocking<T: Send + 'static>(
        &self,
        context: &RepositoryReadContext,
        store: CacheStore,
        scope: CacheScope,
        work: impl FnOnce(CacheStore, CacheScope) -> Result<T, CacheError> + Send + 'static,
    ) -> Result<T, PageCacheError<GitHubError>> {
        let gate = self.credential_commit.clone();
        let registry = self.registry.clone();
        let closed = self.closed.clone();
        let expected = context.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _guard = gate.lock().map_err(|_| CacheError::Stale)?;
            if closed.load(Ordering::Acquire)
                || registry
                    .lock()
                    .expect("registry lock")
                    .active_context()
                    .as_ref()
                    != Some(expected.auth())
            {
                return Err(CacheError::Stale);
            }
            work(store, scope)
        })
        .await;
        if !self.is_current(context) {
            return Err(PageCacheError::Context(GitHubError::StaleContext));
        }
        result
            .map_err(|_| PageCacheError::Cache(CacheError::Io))?
            .map_err(PageCacheError::Cache)
    }
}

#[cfg(test)]
mod tests {
    mod lifecycle_tests;

    use super::*;
    use crate::auth::memory::InMemoryCredentialStore;
    use crate::auth::token::Pat;
    use crate::data::account_tests::{config, key, user};
    use crate::github::client::GitHubConfig;
    use crate::network::executor::BackgroundExecutor;
    use crate::network::transport::{MockTransport, json_response};
    use crate::storage::cache::CacheConfig;

    struct Fixture {
        service: crate::data::AccountService,
        mock: Arc<MockTransport>,
        _executor: Arc<BackgroundExecutor>,
        _temp: tempfile::TempDir,
    }

    impl Fixture {
        async fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let mock = Arc::new(MockTransport::new());
            let executor = Arc::new(BackgroundExecutor::new(4));
            let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
            let service = crate::data::AccountService::open_with_cache(
                config(&temp),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(GitHubConfig::default(), mock.clone())),
                executor.clone(),
                cache,
            )
            .await
            .unwrap();
            user(&mock, 1).await;
            service
                .login(Pat::from_str("synthetic-repo-list"))
                .await
                .unwrap();
            Self {
                service,
                mock,
                _executor: executor,
                _temp: temp,
            }
        }
    }

    fn page(repos: &[(u64, &str)]) -> String {
        let arr: Vec<_> = repos
            .iter()
            .map(|(id, name)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "full_name": format!("user1/{name}"),
                    "owner": {"id": 1, "login": "user1"},
                    "visibility": "public",
                    "fork": false,
                    "archived": false
                })
            })
            .collect();
        serde_json::json!(arr).to_string()
    }

    fn value(read: Scoped<PageValue<ReposOutcome>>) -> (ReposOutcome, PageOrigin, CacheMetadata) {
        match read.value {
            PageValue::Value {
                value,
                origin,
                metadata,
            } => (value, origin, metadata),
            PageValue::Miss(reason) => panic!("unexpected miss: {reason:?}"),
        }
    }

    #[tokio::test]
    async fn online_then_offline_restart_replays_without_network() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(1, "alpha"), (2, "beta")]))))
            .await;
        let (outcome, origin, metadata) = value(
            repos
                .list_repos_cached(PageReadMode::Online, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::Network);
        assert_eq!(metadata.completeness, CacheCompleteness::Complete);
        assert_eq!(outcome.repositories.len(), 2);
        assert_eq!(outcome.readiness, ReposReadiness::Complete);

        let online_requests = fixture.mock.recorded_requests().await.len();
        // Login verified /user once; the online cached read fetched one page.
        assert_eq!(online_requests, 2);

        // Offline replay must not touch the (now empty) mock transport.
        let (offline, origin, historical) = value(
            repos
                .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::HistoricalSnapshot);
        assert_eq!(historical, metadata);
        assert_eq!(offline.repositories.len(), 2);
        assert_eq!(
            fixture.mock.recorded_requests().await.len(),
            online_requests
        );
    }

    #[tokio::test]
    async fn missing_snapshot_is_a_miss_not_an_error() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        let read = repos
            .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
            .await
            .unwrap();
        assert!(matches!(read.value, PageValue::Miss(CacheMiss::NotCached)));
    }

    #[tokio::test]
    async fn multi_page_complete_and_later_page_failure_preserve_partial() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        let mut first = json_response(200, &page(&[(1, "alpha")]));
        first.headers.push((
            "link".into(),
            r#"<https://api.github.com/user/repos?page=2>; rel="next""#.into(),
        ));
        fixture.mock.enqueue(Ok(first)).await;
        fixture.mock.enqueue(Ok(json_response(503, "{}"))).await;
        let (outcome, _, metadata) = value(
            repos
                .list_repos_cached(PageReadMode::Online, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert!(matches!(outcome.readiness, ReposReadiness::Partial { .. }));
        assert!(matches!(
            metadata.completeness,
            CacheCompleteness::Partial { .. }
        ));
        assert_eq!(outcome.repositories.len(), 1);

        // Offline replay reproduces the same partial result and page failure.
        let (offline, origin, historical) = value(
            repos
                .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::HistoricalSnapshot);
        assert_eq!(historical, metadata);
        assert!(matches!(offline.readiness, ReposReadiness::Partial { .. }));
        assert_eq!(offline.repositories.len(), 1);
    }

    #[tokio::test]
    async fn sso_partial_results_are_preserved() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        let mut response = json_response(200, &page(&[(7, "solo")]));
        response.headers.push((
            "x-github-sso".into(),
            "partial-results; organizations=acme".into(),
        ));
        fixture.mock.enqueue(Ok(response)).await;
        let (outcome, _, metadata) = value(
            repos
                .list_repos_cached(PageReadMode::Online, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert!(matches!(
            outcome.readiness,
            ReposReadiness::Partial {
                reason: crate::domain::repo::PartialReason::SsoPartial { .. }
            }
        ));
        assert!(matches!(
            metadata.completeness,
            CacheCompleteness::Partial { .. }
        ));

        let (offline, _, _) = value(
            repos
                .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert!(matches!(
            offline.readiness,
            ReposReadiness::Partial {
                reason: crate::domain::repo::PartialReason::SsoPartial { .. }
            }
        ));
    }

    #[tokio::test]
    async fn no_binding_is_access_denied_so_consumer_can_fall_back() {
        // AccountService::new keeps caching disabled, so RepoService::cache
        // returns None and list_repos_cached must fail with a distinguishable
        // Cache(AccessDenied) error instead of pretending to be online.
        let mock = Arc::new(MockTransport::new());
        let executor = Arc::new(BackgroundExecutor::new(2));
        let service = crate::data::AccountService::new(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(GitHubConfig::default(), mock.clone())),
            executor,
        );
        user(&mock, 1).await;
        service
            .login(Pat::from_str("synthetic-unbound"))
            .await
            .unwrap();
        let result = service
            .repo_service()
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await;
        assert!(matches!(
            result,
            Err(PageCacheError::Cache(CacheError::AccessDenied))
        ));
    }

    #[tokio::test]
    async fn credential_rejected_401_is_not_persisted() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(1, "alpha")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await
            .unwrap();
        // A fresh online read now receives an explicit 401; the aggregate must
        // be quarantined and the prior snapshot not returned as current.
        fixture.mock.enqueue(Ok(json_response(401, "{}"))).await;
        let result = repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await;
        assert!(matches!(
            result,
            Err(PageCacheError::Context(GitHubError::Unauthorized))
        ));
        let offline = repos
            .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
            .await;
        assert!(matches!(
            offline,
            Err(PageCacheError::Cache(CacheError::AccessDenied))
        ));
    }

    #[tokio::test]
    async fn pat_replacement_quarantines_old_snapshot() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(5, "old")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await
            .unwrap();
        // Replacement discards old aggregate representations; only new-credential results may fill it.
        user(&fixture.mock, 1).await;
        fixture
            .service
            .replace(key(1), Pat::from_str("synthetic-replacement"))
            .await
            .unwrap();
        let offline = repos
            .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
            .await
            .unwrap();
        assert!(matches!(offline.value, PageValue::Miss(_)));
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(6, "new")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, Default::default())
            .await
            .unwrap();
        let (fresh, origin, _) = value(
            repos
                .list_repos_cached(PageReadMode::Offline, Default::default())
                .await
                .unwrap(),
        );
        assert_eq!(fresh.repositories[0].id, 6);
        assert_eq!(origin, PageOrigin::HistoricalSnapshot);
    }

    #[tokio::test]
    async fn multi_page_complete_replays_offline() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        let mut first = json_response(200, &page(&[(1, "a"), (2, "b")]));
        first.headers.push((
            "link".into(),
            r#"<https://api.github.com/user/repos?page=2>; rel="next""#.into(),
        ));
        fixture.mock.enqueue(Ok(first)).await;
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(3, "c")]))))
            .await;
        let (outcome, origin, metadata) = value(
            repos
                .list_repos_cached(PageReadMode::Online, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::Network);
        assert_eq!(metadata.completeness, CacheCompleteness::Complete);
        assert_eq!(outcome.repositories.len(), 3);
        assert_eq!(outcome.readiness, ReposReadiness::Complete);

        let (offline, _, _) = value(
            repos
                .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert_eq!(offline.repositories.len(), 3);
        assert_eq!(offline.readiness, ReposReadiness::Complete);
    }

    #[tokio::test]
    async fn no_store_invalidates_both_snapshot_slots() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(1, "a")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await
            .unwrap();
        let mut response = json_response(200, &page(&[(2, "b")]));
        response
            .headers
            .push(("Cache-Control".into(), "private, NO-STORE".into()));
        fixture.mock.enqueue(Ok(response)).await;
        let (_, origin, metadata) = value(
            repos
                .list_repos_cached(PageReadMode::Online, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::Network);
        assert!(!metadata.permits_storage());
        let offline = repos
            .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
            .await
            .unwrap();
        assert!(matches!(offline.value, PageValue::Miss(_)));
    }

    #[tokio::test]
    async fn no_store_on_failed_first_page_still_invalidates_snapshots() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(1, "a")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await
            .unwrap();
        let mut response = json_response(503, "{}");
        response
            .headers
            .push(("Cache-Control".into(), "no-store".into()));
        fixture.mock.enqueue(Ok(response)).await;
        let result = repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await;
        assert!(result.is_err());
        let offline = repos
            .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
            .await
            .unwrap();
        assert!(matches!(offline.value, PageValue::Miss(_)));
    }

    #[tokio::test]
    async fn account_switch_isolates_snapshots() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(21, "one")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await
            .unwrap();
        user(&fixture.mock, 2).await;
        fixture
            .service
            .login(Pat::from_str("synthetic-two"))
            .await
            .unwrap();
        let read = fixture
            .service
            .repo_service()
            .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
            .await
            .unwrap();
        assert!(matches!(read.value, PageValue::Miss(CacheMiss::NotCached)));
        fixture.service.set_active(key(1)).await.unwrap();
        let (outcome, origin, _) = value(
            fixture
                .service
                .repo_service()
                .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::HistoricalSnapshot);
        assert_eq!(outcome.repositories.len(), 1);
        assert_eq!(outcome.repositories[0].id, 21);
    }

    #[tokio::test]
    async fn removal_blocks_the_retired_snapshot() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(31, "gone")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await
            .unwrap();
        fixture.service.remove_saved(key(1)).await.unwrap();
        let result = repos
            .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
            .await;
        assert!(matches!(
            result,
            Err(PageCacheError::Context(GitHubError::StaleContext))
        ));
    }

    #[tokio::test]
    async fn repository_revocation_invalidates_aggregate_and_in_flight_ticket() {
        let fixture = Fixture::new().await;
        let repos = fixture.service.repo_service();
        fixture
            .mock
            .enqueue(Ok(json_response(200, &page(&[(11, "kept")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await
            .unwrap();
        let session = fixture.service.repo_service().session().unwrap();
        let context = session.context().clone();
        let binding = fixture.service.repo_service().cache(&context).unwrap();
        let store = binding.store().clone();
        let scope = binding.scope().clone();
        let config = repos.client.config().clone();
        let key = CacheKey::mutable(
            None,
            true,
            super::RESOURCE,
            &super::request_identity(context.active_login(), &config, false),
            super::REPRESENTATION,
            super::VERSION,
        )
        .unwrap();
        let ticket = store.begin_write(&scope, &key).unwrap();
        store.revoke_repository(&scope, Some(123)).unwrap();
        assert!(matches!(
            store.put(
                ticket,
                b"late",
                super::metadata(1, CacheCompleteness::Complete, false),
            ),
            Err(CacheError::Stale)
        ));
        let offline = repos
            .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
            .await
            .unwrap();
        assert!(matches!(
            offline.value,
            PageValue::Miss(CacheMiss::NotCached)
        ));
    }

    #[tokio::test]
    async fn reopening_the_cache_store_restores_the_snapshot_offline() {
        let temp = tempfile::tempdir().unwrap();
        let credentials = Arc::new(InMemoryCredentialStore::new());
        let mock = Arc::new(MockTransport::new());
        let executor = Arc::new(BackgroundExecutor::new(4));
        let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        let service = crate::data::AccountService::open_with_cache(
            config(&temp),
            credentials.clone(),
            Arc::new(GitHubClient::new(GitHubConfig::default(), mock.clone())),
            executor.clone(),
            cache,
        )
        .await
        .unwrap();
        user(&mock, 1).await;
        service
            .login(Pat::from_str("synthetic-restart"))
            .await
            .unwrap();
        let repos = service.repo_service();
        mock.enqueue(Ok(json_response(200, &page(&[(9, "kept")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, CancellationToken::default())
            .await
            .unwrap();
        let recorded = mock.recorded_requests().await.len();
        service.close().await.unwrap();
        drop(repos);
        drop(service);

        let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        let service = crate::data::AccountService::open_with_cache(
            config(&temp),
            credentials.clone(),
            Arc::new(GitHubClient::new(GitHubConfig::default(), mock.clone())),
            executor.clone(),
            cache,
        )
        .await
        .unwrap();
        let repos = service.repo_service();
        let (outcome, origin, _) = value(
            repos
                .list_repos_cached(PageReadMode::Offline, CancellationToken::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::HistoricalSnapshot);
        assert_eq!(outcome.repositories.len(), 1);
        assert_eq!(outcome.repositories[0].name, "kept");
        assert_eq!(mock.recorded_requests().await.len(), recorded);
    }
}
