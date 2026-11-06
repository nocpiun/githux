use super::*;
use crate::data::AccountService;
use crate::network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use std::{pin::Pin, sync::Mutex, time::Duration};
use tokio::sync::oneshot;

type Pause = (oneshot::Sender<()>, oneshot::Receiver<()>);

/// Pause one response after the mock has consumed its scripted exchange. Other
/// requests (including PAT replacement's /user) can finish while it is held.
struct HeldTransport {
    mock: Arc<MockTransport>,
    pause: Mutex<Option<Pause>>,
}
impl HeldTransport {
    fn hold(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, ready) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        *self.pause.lock().unwrap() = Some((entered, resume));
        (ready, release)
    }
}
impl Transport for HeldTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let pause = self.pause.lock().unwrap().take();
            let response = self.mock.execute(request, cancel).await;
            if let Some((entered, resume)) = pause {
                entered.send(()).unwrap();
                tokio::time::timeout(Duration::from_secs(10), resume)
                    .await
                    .expect("release held response")
                    .unwrap();
            }
            response
        })
    }
}

struct Harness {
    temp: tempfile::TempDir,
    credentials: Arc<InMemoryCredentialStore>,
    mock: Arc<MockTransport>,
    executor: Arc<BackgroundExecutor>,
    transport: Arc<HeldTransport>,
}
impl Harness {
    fn new() -> Self {
        let mock = Arc::new(MockTransport::new());
        Self {
            temp: tempfile::tempdir().unwrap(),
            credentials: Arc::new(InMemoryCredentialStore::new()),
            executor: Arc::new(BackgroundExecutor::new(4)),
            transport: Arc::new(HeldTransport {
                mock: mock.clone(),
                pause: Mutex::new(None),
            }),
            mock,
        }
    }
    async fn open(&self) -> Arc<AccountService> {
        let cache = CacheStore::open(CacheConfig::at(self.temp.path().join("cache"))).unwrap();
        Arc::new(
            AccountService::open_with_cache(
                config(&self.temp),
                self.credentials.clone(),
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    self.transport.clone(),
                )),
                self.executor.clone(),
                cache,
            )
            .await
            .unwrap(),
        )
    }
    async fn login(&self, service: &AccountService, id: u64) {
        user(&self.mock, id).await;
        service
            .login(Pat::from_str("synthetic-held-response"))
            .await
            .unwrap();
    }
    async fn read(&self, repos: &RepoService, id: u64) {
        self.mock
            .enqueue(Ok(json_response(200, &page(&[(id, "cached")]))))
            .await;
        repos
            .list_repos_cached(PageReadMode::Online, Default::default())
            .await
            .unwrap();
    }
}
fn binding(repos: &RepoService) -> crate::data::AccountCache {
    let session = repos.session().unwrap();
    repos.cache(session.context()).unwrap()
}
fn complete_key(repos: &RepoService) -> CacheKey {
    let context = repos.capture_read_context().unwrap();
    CacheKey::mutable(
        None,
        true,
        RESOURCE,
        &request_identity(context.active_login(), repos.client.config(), false),
        REPRESENTATION,
        VERSION,
    )
    .unwrap()
}
async fn entered(signal: oneshot::Receiver<()>) {
    tokio::time::timeout(Duration::from_secs(5), signal)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn no_store_401_deletes_old_snapshots_before_quarantining() {
    let h = Harness::new();
    let service = h.open().await;
    h.login(&service, 1).await;
    let repos = service.repo_service();
    h.read(&repos, 1).await;
    let mut rejected = json_response(401, "{}");
    rejected
        .headers
        .push(("cache-control".into(), "no-store".into()));
    h.mock.enqueue(Ok(rejected)).await;
    assert!(matches!(
        repos
            .list_repos_cached(PageReadMode::Online, Default::default())
            .await,
        Err(PageCacheError::Context(GitHubError::Unauthorized))
    ));
    user(&h.mock, 1).await;
    service
        .replace(key(1), Pat::from_str("synthetic-next"))
        .await
        .unwrap();
    // Explicit proof only to inspect retained bytes after quarantine, not an
    // action performed by this adapter or by a single-repository proof.
    let context = repos.capture_read_context().unwrap();
    let proof = repos.begin_revalidation(&context, None).unwrap();
    repos.permission_confirmed(&context, proof).await.unwrap();
    assert!(matches!(
        repos
            .list_repos_cached(PageReadMode::Offline, Default::default())
            .await
            .unwrap()
            .value,
        PageValue::Miss(CacheMiss::NotCached)
    ));
    service.close().await.unwrap();
}

#[tokio::test]
async fn failed_401_gate_update_suspends_old_lease_and_journals_across_restart() {
    let h = Harness::new();
    let service = h.open().await;
    h.login(&service, 1).await;
    let repos = service.repo_service();
    h.read(&repos, 1).await;
    let binding = binding(&repos);
    let lease = match binding
        .store()
        .read(binding.scope(), &complete_key(&repos))
        .unwrap()
    {
        CacheRead::Hit(lease) => lease,
        CacheRead::Miss(_) => panic!("recorded snapshot"),
    };
    let db = rusqlite::Connection::open(h.temp.path().join("cache/cache.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_reject BEFORE UPDATE ON scopes BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    h.mock.enqueue(Ok(json_response(401, "{}"))).await;
    assert!(matches!(
        repos
            .list_repos_cached(PageReadMode::Online, Default::default())
            .await,
        Err(PageCacheError::Context(GitHubError::CredentialStore { .. }))
    ));
    db.execute_batch("DROP TRIGGER fail_reject").unwrap();
    drop(db);
    assert!(matches!(lease.snapshot(), Err(CacheError::AccessDenied)));
    assert!(
        repos
            .cache(&repos.capture_read_context().unwrap())
            .is_none()
    );
    service.close().await.unwrap();
    drop(lease);
    drop(binding);
    drop(repos);
    drop(service);
    // A genuinely new CacheStore has no process-local suspension map. The
    // account rejection journal must still deny this persisted private value.
    let service = h.open().await;
    assert!(matches!(
        service
            .repo_service()
            .list_repos_cached(PageReadMode::Offline, Default::default())
            .await,
        Err(PageCacheError::Cache(CacheError::AccessDenied))
    ));
    service.close().await.unwrap();
}

#[tokio::test]
async fn actual_in_flight_reads_observe_switch_replacement_removal_and_revocation() {
    for action in ["switch", "replace", "remove", "revoke"] {
        let h = Harness::new();
        let service = h.open().await;
        h.login(&service, 2).await;
        h.login(&service, 1).await;
        let repos = service.repo_service();
        h.read(&repos, 10).await;
        h.mock
            .enqueue(Ok(json_response(200, &page(&[(99, "late")]))))
            .await;
        let (ready, release) = h.transport.hold();
        let reading = {
            let repos = repos.clone();
            tokio::spawn(async move {
                repos
                    .list_repos_cached(PageReadMode::Online, Default::default())
                    .await
            })
        };
        entered(ready).await;
        match action {
            "switch" => service.set_active(key(2)).await.unwrap(),
            "replace" => {
                user(&h.mock, 1).await;
                service
                    .replace(key(1), Pat::from_str("synthetic-new"))
                    .await
                    .unwrap();
            }
            "remove" => service.remove_saved(key(1)).await.unwrap(),
            "revoke" => {
                let b = binding(&repos);
                b.store().revoke_repository(b.scope(), Some(10)).unwrap();
            }
            _ => unreachable!(),
        }
        release.send(()).unwrap();
        let result = reading.await.unwrap();
        if action == "revoke" {
            assert!(
                matches!(result, Err(PageCacheError::Cache(CacheError::Stale))),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(
                    result,
                    Err(PageCacheError::Context(GitHubError::StaleContext))
                ),
                "{action}: {result:?}"
            );
        }
        if action == "switch" {
            service.set_active(key(1)).await.unwrap();
            let (outcome, _, _) = value(
                repos
                    .list_repos_cached(PageReadMode::Offline, Default::default())
                    .await
                    .unwrap(),
            );
            assert_eq!(outcome.repositories[0].id, 10);
        }
        if action == "remove" {
            h.login(&service, 1).await;
        }
        if matches!(action, "remove" | "revoke") {
            assert!(matches!(
                repos
                    .list_repos_cached(PageReadMode::Offline, Default::default())
                    .await
                    .unwrap()
                    .value,
                PageValue::Miss(_)
            ));
        }
        if action == "replace" {
            let context = repos.capture_read_context().unwrap();
            let proof = repos.begin_revalidation(&context, Some(10)).unwrap();
            repos.permission_confirmed(&context, proof).await.unwrap();
            // A repository proof cannot resurrect discarded old aggregate data.
            assert!(matches!(
                repos
                    .list_repos_cached(PageReadMode::Offline, Default::default())
                    .await
                    .unwrap()
                    .value,
                PageValue::Miss(_)
            ));
        }
        service.close().await.unwrap();
    }
}

#[tokio::test]
async fn old_401_does_not_reject_replacement_and_old_refresh_cannot_overwrite_new() {
    let h = Harness::new();
    let service = h.open().await;
    h.login(&service, 1).await;
    let repos = service.repo_service();
    h.mock.enqueue(Ok(json_response(401, "{}"))).await;
    let (ready, release) = h.transport.hold();
    let reading = {
        let repos = repos.clone();
        tokio::spawn(async move {
            repos
                .list_repos_cached(PageReadMode::Online, Default::default())
                .await
        })
    };
    entered(ready).await;
    user(&h.mock, 1).await;
    service
        .replace(key(1), Pat::from_str("synthetic-current"))
        .await
        .unwrap();
    let context = repos.capture_read_context().unwrap();
    // Explicit aggregate proof solely to inspect whether the late old 401
    // poisons the new epoch; this adapter never issues such proof itself.
    let proof = repos.begin_revalidation(&context, None).unwrap();
    repos.permission_confirmed(&context, proof).await.unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        reading.await.unwrap(),
        Err(PageCacheError::Context(GitHubError::StaleContext))
    ));
    h.read(&repos, 20).await;
    h.mock
        .enqueue(Ok(json_response(200, &page(&[(21, "old")]))))
        .await;
    let (ready, release) = h.transport.hold();
    let reading = {
        let repos = repos.clone();
        tokio::spawn(async move {
            repos
                .list_repos_cached(PageReadMode::Online, Default::default())
                .await
        })
    };
    entered(ready).await;
    h.read(&repos, 22).await;
    release.send(()).unwrap();
    assert!(matches!(
        reading.await.unwrap(),
        Err(PageCacheError::Cache(CacheError::Stale))
    ));
    let (outcome, _, _) = value(
        repos
            .list_repos_cached(PageReadMode::Offline, Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(outcome.repositories[0].id, 22);
    service.close().await.unwrap();
}

#[tokio::test]
async fn malformed_later_page_and_sensitive_headers_are_not_persisted_verbatim() {
    for malformed in [
        "proxy error: synthetic-error-marker",
        r#"{"message":"synthetic-error-marker"}"#,
        r#"["synthetic-error-marker"]"#,
    ] {
        let h = Harness::new();
        let service = h.open().await;
        h.login(&service, 1).await;
        let repos = service.repo_service();
        let mut first = json_response(200, &page(&[(1, "first")]));
        first.headers.push((
            "link".into(),
            r#"<https://api.github.com/user/repos?page=2>; rel="next""#.into(),
        ));
        first
            .headers
            .push(("set-cookie".into(), "synthetic-cookie-marker".into()));
        h.mock.enqueue(Ok(first)).await;
        h.mock.enqueue(Ok(json_response(200, malformed))).await;
        let (online, _, _) = value(
            repos
                .list_repos_cached(PageReadMode::Online, Default::default())
                .await
                .unwrap(),
        );
        assert!(matches!(online.readiness, ReposReadiness::Partial { .. }));
        let b = binding(&repos);
        let context = repos.capture_read_context().unwrap();
        let partial = CacheKey::mutable(
            None,
            true,
            RESOURCE,
            &request_identity(context.active_login(), repos.client.config(), true),
            REPRESENTATION,
            VERSION,
        )
        .unwrap();
        let snapshot = match b.store().read(b.scope(), &partial).unwrap() {
            CacheRead::Hit(lease) => lease.snapshot().unwrap().unwrap(),
            CacheRead::Miss(_) => panic!("partial persisted"),
        };
        let bytes = String::from_utf8(snapshot.bytes).unwrap();
        for marker in [
            "synthetic-error-marker",
            "synthetic-cookie-marker",
            "synthetic-held-response",
            "Authorization",
        ] {
            assert!(!bytes.contains(marker), "persisted {marker}");
        }
        let (offline, _, _) = value(
            repos
                .list_repos_cached(PageReadMode::Offline, Default::default())
                .await
                .unwrap(),
        );
        assert!(matches!(offline.readiness, ReposReadiness::Partial { .. }));
        assert_eq!(offline.repositories, online.repositories);
        service.close().await.unwrap();
    }
}

/// Production composition with a real catalog/store restart: replacement admits only new aggregate
/// results, never old sibling Home/Profile/Feed bytes, leases, writers, proofs or queued replies.
#[tokio::test]
async fn replacement_records_new_list_and_reopens_offline_without_resurrecting_old_aggregates() {
    let h = Harness::new();
    let service = h.open().await;
    h.login(&service, 1).await;
    let repos = service.repo_service();
    h.read(&repos, 10).await;
    let b = binding(&repos);
    let list_key = complete_key(&repos);
    let home_key = CacheKey::mutable(None, true, "home-old", "page=1", "test-json", 1).unwrap();
    b.store()
        .put(
            b.store().begin_write(b.scope(), &home_key).unwrap(),
            b"old home",
            metadata(1, CacheCompleteness::Complete, false),
        )
        .unwrap();
    let old_lease = match b.store().read(b.scope(), &list_key).unwrap() {
        CacheRead::Hit(lease) => lease,
        _ => panic!("list hit"),
    };
    let old_writer = b.store().begin_write(b.scope(), &list_key).unwrap();
    let old_proof = repos
        .begin_revalidation(&repos.capture_read_context().unwrap(), None)
        .unwrap();
    let old_session = repos.session().unwrap();
    let old_authorization = b.store().authorization(b.scope(), None).unwrap();
    user(&h.mock, 1).await;
    service
        .replace(key(1), Pat::from_str("valid-new-credential"))
        .await
        .unwrap();
    assert!(!old_session.verify());
    assert!(!old_authorization.is_current());
    assert!(old_lease.snapshot().is_err());
    assert!(matches!(
        b.store().put(
            old_writer,
            b"late",
            metadata(1, CacheCompleteness::Complete, false)
        ),
        Err(CacheError::Stale)
    ));
    assert!(matches!(
        b.store().permission_confirmed(old_proof),
        Err(CacheError::Stale)
    ));
    assert!(matches!(
        b.store().read(b.scope(), &home_key),
        Ok(CacheRead::Miss(_))
    ));
    h.read(&repos, 20).await;
    let (fresh, _, _) = value(
        repos
            .list_repos_cached(PageReadMode::Offline, Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(fresh.repositories[0].id, 20);
    assert!(
        !old_authorization.is_current(),
        "new data cannot revive queued old replies"
    );
    service.close().await.unwrap();
    drop(old_lease);
    drop(b);
    drop(old_session);
    drop(repos);
    drop(service);
    let service = h.open().await;
    assert!(service.summaries().iter().all(|s| matches!(
        s.authorization,
        crate::domain::account::AuthorizationState::PendingVerification
    )));
    let repos = service.repo_service();
    let (fresh, origin, _) = value(
        repos
            .list_repos_cached(PageReadMode::Offline, Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(fresh.repositories[0].id, 20);
    assert_eq!(origin, PageOrigin::HistoricalSnapshot);
    assert!(matches!(
        binding(&repos)
            .store()
            .read(binding(&repos).scope(), &home_key),
        Ok(CacheRead::Miss(_))
    ));
    service.close().await.unwrap();
}
