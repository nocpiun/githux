//! Real RepositorySession + CacheStore composition; synthetic PAT/HTTP only.
use super::account_tests::{config, key};
use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::content::PreparedBody;
use crate::data::repository_cache::RepositoryCacheError;
use crate::domain::blob::{BlobBody, BlobIdentity, BlobLimits};
use crate::domain::cache::{CacheCompleteness, CacheMetadata, CacheMiss};
use crate::domain::repository::*;
use crate::github::RepositoryLimits;
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use crate::storage::cache::{AccessEvent, CacheConfig, CacheKey, CacheStore};
use serde_json::json;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TREE_A: &str = "1111111111111111111111111111111111111111";
const TREE_B: &str = "2222222222222222222222222222222222222222";
const TREE_CHILD: &str = "3333333333333333333333333333333333333333";
const BLOB_A: &str = "4444444444444444444444444444444444444444";
const BLOB_B: &str = "5555555555555555555555555555555555555555";

fn locator() -> RepositoryLocator {
    RepositoryLocator::new("octo", "repo").unwrap()
}
fn repository() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 42,
        locator: locator(),
    }
}
fn details() -> serde_json::Value {
    json!({"id":42,"name":"repo","owner":{"id":7,"login":"octo"},"private":true,"visibility":"private","default_branch":"main","fork":false,"archived":false,"stargazers_count":5,"forks_count":2,"open_issues_count":3,"size":12,"html_url":"https://github.com/octo/repo"})
}
fn commit(sha: &str, tree: &str, message: &str) -> serde_json::Value {
    json!({"sha":sha,"commit":{"message":message,"tree":{"sha":tree},"author":null,"committer":null}})
}
fn tree(sha: &str) -> serde_json::Value {
    json!({"sha":sha,"truncated":false,"tree":[
        {"path":"README.md","type":"blob","mode":"100644","sha":if sha == TREE_A { BLOB_A } else { BLOB_B },"size":5},
        {"path":"src","type":"tree","mode":"040000","sha":TREE_CHILD}
    ]})
}
fn readme(sha: &str) -> serde_json::Value {
    json!({"type":"file","path":"README.md","sha":sha,"size":5})
}
fn blob(sha: &str, text: &str) -> serde_json::Value {
    use base64::Engine;
    json!({"sha":sha,"encoding":"base64","size":text.len(),"content":base64::engine::general_purpose::STANDARD.encode(text)})
}

#[derive(Default)]
struct Fixture {
    responses: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    requests: Mutex<Vec<HttpRequest>>,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
impl Fixture {
    fn push(&self, status: u16, body: serde_json::Value) {
        self.responses
            .lock()
            .unwrap()
            .push_back(Ok(json_response(status, &body.to_string())));
    }
    fn headers(&self, status: u16, body: serde_json::Value, headers: &[(&str, &str)]) {
        let mut response = json_response(status, &body.to_string());
        response.headers.extend(
            headers
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
        );
        self.responses.lock().unwrap().push_back(Ok(response));
    }
    fn fail(&self, error: TransportError) {
        self.responses.lock().unwrap().push_back(Err(error));
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn pause_next(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release) = tokio::sync::oneshot::channel();
        *self.entered.lock().unwrap() = Some(entered);
        *self.release.lock().unwrap() = Some(release);
        (entered_rx, release_tx)
    }
}
impl Transport for Fixture {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            if let Some(entered) = self.entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            let release = self.release.lock().unwrap().take();
            if let Some(release) = release {
                release.await.expect("release response");
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted response")
        })
    }
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
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
}

async fn service(temp: &tempfile::TempDir, fixture: Arc<Fixture>) -> Arc<AccountService> {
    Arc::new(
        AccountService::open(
            config(temp),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(Default::default(), fixture)),
            Arc::new(BackgroundExecutor::new(4)),
        )
        .await
        .unwrap(),
    )
}
async fn login(service: &AccountService, fixture: &Fixture, id: u64) {
    fixture.push(200, json!({"id":id,"login":format!("user{id}")}));
    service
        .login(Pat::from_str("synthetic-secret"))
        .await
        .unwrap();
}
fn cache(temp: &tempfile::TempDir) -> CacheStore {
    CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap()
}
fn cached(service: &AccountService, store: CacheStore, id: u64) -> CachedRepositorySession {
    let session = service.repo_service().session().unwrap();
    let scope = store.saved_account(key(id)).unwrap();
    CachedRepositorySession::new(session, store, scope).unwrap()
}

async fn read_online_chain(
    session: &CachedRepositorySession,
) -> (RepoSnapshot, DirectoryTree, BlobIdentity) {
    let limits = RepositoryLimits::default();
    let repo = session
        .repository(
            &repository(),
            &limits,
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    let snap = session
        .revision(
            &repo.identity,
            RevisionSpec::Ref(DynamicRef::new("main").unwrap()),
            &limits,
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    let root = session
        .directory(
            &SnapshotDirectory::root(&snap),
            &limits,
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    let discovery = session
        .readme(
            &snap,
            &limits,
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    let ReadmeDiscovery::Found(descriptor) = discovery else {
        panic!("README")
    };
    let identity = BlobIdentity::from_readme(&descriptor);
    session
        .content(
            &identity,
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    (snap, root, identity)
}

#[tokio::test]
async fn online_then_reopened_store_reads_the_complete_offline_chain_and_misses_unread_directory() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    fixture.push(200, details());
    fixture.push(200, commit(A, TREE_A, "A"));
    fixture.push(200, tree(TREE_A));
    fixture.push(200, readme(BLOB_A));
    fixture.push(200, blob(BLOB_A, "hello"));
    let store = cache(&storage);
    let session = cached(&svc, store.clone(), 1);
    let (snap, root, identity) = read_online_chain(&session).await;
    drop(session);
    drop(store);
    svc.close().await.unwrap();
    drop(svc);

    // Rebuild both the account service and persistent cache. OfflineOnly has
    // no responses queued: any network attempt fails the test.
    let svc = service(&accounts, fixture.clone()).await;
    let store = cache(&storage);
    let session = cached(&svc, store.clone(), 1);
    let details = session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        details.value.provenance.origin,
        RepositoryCacheOrigin::Snapshot
    );
    assert!(details.value.provenance.fetched_at_ms > 0);
    assert!(details.value.provenance.verified_at_ms >= details.value.provenance.fetched_at_ms);
    let snap_offline = session
        .revision(
            &repository(),
            RevisionSpec::Ref(DynamicRef::new("main").unwrap()),
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    assert_eq!(snap_offline, snap);
    assert!(matches!(snap_offline.requested(), RevisionSpec::Ref(_)));
    let root_offline = session
        .directory(
            &SnapshotDirectory::root(&snap_offline),
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    assert_eq!(root_offline, root);
    let discovery = session
        .readme(
            &snap_offline,
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    let ReadmeDiscovery::Found(descriptor) = discovery else {
        panic!("offline README")
    };
    assert_eq!(descriptor.blob, ObjectSha::new(BLOB_A).unwrap());
    let content = session
        .content(
            &identity,
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        content.value.value.body,
        PreparedBody::Utf8 {
            text: "hello".into(),
            has_bom: false
        }
    );
    assert_eq!(
        content.value.provenance.origin,
        RepositoryCacheOrigin::Snapshot
    );
    assert_eq!(
        session
            .directory(
                &root_offline.child_directory(1).unwrap(),
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Miss(CacheMiss::NotCached)
    );
    assert_eq!(
        fixture.count(),
        6,
        "login plus five online reads; no offline requests"
    );
}

#[tokio::test]
async fn moving_main_switches_the_whole_snapshot_and_accounts_do_not_share_fixed_shas() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    fixture.push(200, details());
    fixture.push(200, commit(A, TREE_A, "A"));
    fixture.push(200, tree(TREE_A));
    fixture.push(200, readme(BLOB_A));
    fixture.push(200, blob(BLOB_A, "hello"));
    let store = cache(&storage);
    let session = cached(&svc, store.clone(), 1);
    let (snap_a, _, blob_a) = read_online_chain(&session).await;
    fixture.push(200, commit(B, TREE_B, "B"));
    fixture.push(200, tree(TREE_B));
    fixture.push(200, readme(BLOB_B));
    fixture.push(200, blob(BLOB_B, "other"));
    let snap_b = session
        .revision(
            &repository(),
            RevisionSpec::Ref(DynamicRef::new("main").unwrap()),
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    assert_eq!(snap_b.commit().as_str(), B);
    let root_b = session
        .directory(
            &SnapshotDirectory::root(&snap_b),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    assert_eq!(root_b.entries[0].sha.as_str(), BLOB_B);
    let ReadmeDiscovery::Found(readme_b) = session
        .readme(
            &snap_b,
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value
    else {
        panic!()
    };
    let content_b = session
        .content(
            &BlobIdentity::from_readme(&readme_b),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        content_b.value.value.body,
        PreparedBody::Utf8 {
            text: "other".into(),
            has_bom: false
        }
    );
    assert_eq!(snap_a.commit().as_str(), A);
    assert_eq!(blob_a.blob().as_str(), BLOB_A);

    // The second account sees the same SHA, but no cached bytes are inherited.
    login(&svc, &fixture, 2).await;
    fixture.push(200, blob(BLOB_A, "hello"));
    let other = cached(&svc, store, 2);
    assert_eq!(
        other
            .blob(
                &blob_a,
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Miss(CacheMiss::NotCached)
    );
    other
        .blob(
            &blob_a,
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    svc.set_active(key(1)).await.unwrap();
    assert!(!session.verify());
    assert_eq!(
        session
            .blob(
                &blob_a,
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryError::GitHub(GitHubError::StaleContext).into()
    );
}

#[tokio::test]
async fn matching_304_revalidates_and_missing_or_wrong_validator_retries_unconditionally() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    let store = cache(&storage);
    let session = cached(&svc, store, 1);
    fixture.headers(200, details(), &[("etag", "\"repo-v1\"")]);
    let first = session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    fixture.headers(304, json!({}), &[("etag", "\"repo-v1\"")]);
    let second = session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(second.value.value, first.value.value);
    assert_eq!(
        second.value.provenance.origin,
        RepositoryCacheOrigin::Network
    );
    assert_eq!(
        second.value.provenance.fetched_at_ms,
        first.value.provenance.fetched_at_ms
    );
    assert!(second.value.provenance.verified_at_ms >= first.value.provenance.verified_at_ms);
    // A response without the matching ETag cannot turn cached bytes into a 304
    // success: ForceRefresh performs an unconditional follow-up request.
    fixture.headers(304, json!({}), &[("etag", "\"different\"")]);
    fixture.push(200, details());
    session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    let requests = fixture.requests.lock().unwrap();
    assert!(
        !requests[1]
            .headers
            .iter()
            .any(|(k, _)| k == "If-None-Match")
    );
    assert!(
        requests[2]
            .headers
            .iter()
            .any(|(k, v)| k == "If-None-Match" && v == "\"repo-v1\"")
    );
    assert!(
        requests[3]
            .headers
            .iter()
            .any(|(k, _)| k == "If-None-Match")
    );
    assert!(
        !requests[4]
            .headers
            .iter()
            .any(|(k, _)| k == "If-None-Match")
    );
}

#[tokio::test]
async fn no_store_invalidates_old_value_and_transient_failure_returns_explicit_snapshot() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    let store = cache(&storage);
    let session = cached(&svc, store, 1);
    fixture.push(200, details());
    session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    fixture.fail(TransportError::Network {
        message: "offline".into(),
    });
    let fallback = session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        fallback.value.provenance.origin,
        RepositoryCacheOrigin::Snapshot
    );
    fixture.headers(200, details(), &[("cache-control", "private, no-store")]);
    session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .repository(
                &repository(),
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Miss(CacheMiss::NotCached)
    );
    fixture.push(401, json!({"message":"bad"}));
    assert!(matches!(
        session
            .repository(
                &repository(),
                &Default::default(),
                RepositoryCacheMode::OnlinePreferred,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Online(RepositoryError::GitHub(GitHubError::Unauthorized))
    ));
    fixture.push(404, json!({}));
    assert!(matches!(
        session
            .repository(
                &repository(),
                &Default::default(),
                RepositoryCacheMode::OnlinePreferred,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Online(RepositoryError::GitHub(GitHubError::NotFound))
    ));
}

#[tokio::test]
async fn partial_empty_and_complete_empty_are_distinct_and_binary_semantics_survive_restart() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    let store = cache(&storage);
    let session = cached(&svc, store.clone(), 1);
    fixture.push(200, commit(A, TREE_A, "A"));
    let snap = session
        .revision(
            &repository(),
            RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    fixture.push(200, json!({"sha":TREE_A,"truncated":false,"tree":[]}));
    let root = session
        .directory(
            &SnapshotDirectory::root(&snap),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(root.value.value.completeness, Completeness::Complete);
    assert_eq!(
        root.value.provenance.completeness,
        CacheCompleteness::Complete
    );
    fixture.push(200, json!({"sha":TREE_A,"truncated":true,"tree":[]}));
    let partial = session
        .directory(
            &SnapshotDirectory::root(&snap),
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        partial.value.provenance.completeness,
        CacheCompleteness::Partial { .. }
    ));
    let offline = session
        .directory(
            &SnapshotDirectory::root(&snap),
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        offline.value.provenance.completeness,
        CacheCompleteness::Partial { .. }
    ));
    assert!(matches!(
        offline.value.value.completeness,
        Completeness::Partial { .. }
    ));

    let file_tree = DirectoryTree {
        directory: SnapshotDirectory::root(&snap),
        entries: vec![TreeEntry {
            path: RepositoryPath::new("bin.dat").unwrap(),
            kind: TreeEntryKind::File { executable: false },
            sha: ObjectSha::new(BLOB_A).unwrap(),
            size: Some(3),
        }],
        truncated: Some(false),
        completeness: Completeness::Complete,
    };
    let identity = BlobIdentity::from_file(&file_tree, 0).unwrap();
    fixture.push(
        200,
        json!({"sha":BLOB_A,"encoding":"base64","size":3,"content":"QQBC"}),
    );
    session
        .blob(
            &identity,
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    drop(session);
    drop(store);
    let store = cache(&storage);
    let session = cached(&svc, store, 1);
    let binary = session
        .content(
            &identity,
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        binary.value.value.body,
        PreparedBody::Unsupported(_)
    ));
}

#[tokio::test]
async fn cancellation_and_stale_write_ticket_do_not_replace_the_last_snapshot() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    let store = cache(&storage);
    let session = cached(&svc, store.clone(), 1);
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    assert!(matches!(
        session
            .repository(
                &repository(),
                &Default::default(),
                RepositoryCacheMode::OnlinePreferred,
                cancel
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Online(RepositoryError::GitHub(GitHubError::Cancelled))
    ));
    fixture.push(200, details());
    session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    // Exercise the core stale-writer path through the same key representation:
    // a newer refresh makes the earlier ticket unable to overwrite it.
    let scope = store.saved_account(key(1)).unwrap();
    let key = CacheKey::mutable(
        Some(42),
        true,
        "repository-details",
        "canonical-repository",
        "githux-repository-v1",
        1,
    )
    .unwrap();
    let old = store.begin_write(&scope, &key).unwrap();
    fixture.push(200, details());
    session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        store.put(
            old,
            b"stale",
            CacheMetadata {
                representation_version: 1,
                fetched_at_ms: 1,
                verified_at_ms: 1,
                completeness: CacheCompleteness::Complete,
                etag: None,
                last_modified: None,
                cache_control: None
            }
        ),
        Err(crate::storage::cache::CacheError::Stale)
    ));
    let offline = session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(offline.value.value.identity, repository());
}

#[tokio::test]
async fn authorization_gate_requires_real_repository_success_not_fixed_sha_or_local_rejection() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    let store = cache(&storage);
    let session = cached(&svc, store.clone(), 1);
    fixture.push(200, commit(A, TREE_A, "A"));
    let snap = session
        .revision(
            &repository(),
            RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    store
        .access_event(
            &store.saved_account(key(1)).unwrap(),
            AccessEvent::PatReplaced,
        )
        .unwrap();
    assert_eq!(
        session
            .revision(
                &repository(),
                RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::AccessDenied
    );
    // A known oversized blob is rejected locally and cannot confirm permission.
    let tree = DirectoryTree {
        directory: SnapshotDirectory::root(&snap),
        entries: vec![TreeEntry {
            path: RepositoryPath::new("big.bin").unwrap(),
            kind: TreeEntryKind::File { executable: false },
            sha: ObjectSha::new(BLOB_A).unwrap(),
            size: Some(99),
        }],
        truncated: Some(false),
        completeness: Completeness::Complete,
    };
    let identity = BlobIdentity::from_file(&tree, 0).unwrap();
    let large = session
        .blob(
            &identity,
            &BlobLimits {
                decoded_bytes: 1,
                response_bytes: 1024,
            },
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        large.value.value.body(),
        BlobBody::TooLarge { .. }
    ));
    assert_eq!(
        session
            .revision(
                &repository(),
                RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::AccessDenied
    );
    // The fixed commit's successful repository request is the proof that
    // unlocks the same repository after PAT replacement.
    fixture.push(200, commit(A, TREE_A, "A"));
    session
        .revision(
            &repository(),
            RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    let offline = session
        .revision(
            &repository(),
            RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(offline.value.value, snap);
}

#[tokio::test]
async fn damaged_dto_is_a_corrupt_miss_not_an_empty_success() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    let store = cache(&storage);
    let session = cached(&svc, store.clone(), 1);
    fixture.push(200, details());
    session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    // Corrupt the cached DTO while leaving its storage checksum valid. The
    // adapter must reject its identity/version rather than publish empty data.
    let scope = store.saved_account(key(1)).unwrap();
    let key = CacheKey::mutable(
        Some(42),
        true,
        "repository-details",
        "canonical-repository",
        "githux-repository-v1",
        1,
    )
    .unwrap();
    let ticket = store.begin_write(&scope, &key).unwrap();
    store
        .put(
            ticket,
            br#"{"version":1,"repository_id":7,"value":{}}"#,
            CacheMetadata {
                representation_version: 1,
                fetched_at_ms: 1,
                verified_at_ms: 1,
                completeness: CacheCompleteness::Complete,
                etag: None,
                last_modified: None,
                cache_control: None,
            },
        )
        .unwrap();
    assert_eq!(
        session
            .repository(
                &repository(),
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Miss(CacheMiss::Corrupt)
    );
}

#[tokio::test]
async fn review_boundaries_keep_identity_gate_storage_directives_and_tree_context() {
    let accounts = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let svc = service(&accounts, fixture.clone()).await;
    login(&svc, &fixture, 1).await;
    let store = cache(&storage);
    let session = cached(&svc, store.clone(), 1);
    let scope = store.saved_account(key(1)).unwrap();

    // A reused locator resolving to another numeric repository is not proof
    // for the requested repository and cannot overwrite its cache.
    fixture.push(200, commit(A, TREE_A, "A"));
    let snap = session
        .revision(
            &repository(),
            RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    store
        .access_event(&scope, AccessEvent::PatReplaced)
        .unwrap();
    fixture.push(200, json!({"id":99,"name":"repo","owner":{"id":7,"login":"octo"},"private":true,"default_branch":"main","fork":false,"archived":false,"stargazers_count":0,"forks_count":0,"open_issues_count":0,"size":0,"html_url":"https://github.com/octo/repo"}));
    assert!(matches!(
        session
            .repository(
                &repository(),
                &Default::default(),
                RepositoryCacheMode::ForceRefresh,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Online(RepositoryError::InvalidResponse { .. })
    ));
    assert_eq!(
        session
            .revision(
                &repository(),
                RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::AccessDenied
    );

    // OnlinePreferred does not let quarantine prevent fresh proof; success
    // reopens only this repository and persists the validated observation.
    fixture.push(200, details());
    session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .revision(
                &repository(),
                RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap()
            .value
            .value,
        snap
    );

    // Revocation after the initial cache read, while the network request is
    // actually in flight, must beat the transient fallback.
    fixture.push(200, details());
    session
        .repository(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    let (entered, release) = fixture.pause_next();
    fixture.fail(TransportError::Network {
        message: "offline".into(),
    });
    let pending = tokio::spawn({
        let session = session.clone();
        async move {
            session
                .repository(
                    &repository(),
                    &Default::default(),
                    RepositoryCacheMode::OnlinePreferred,
                    Default::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    store.revoke_repository(&scope, Some(42)).unwrap();
    release.send(()).unwrap();
    assert_eq!(
        pending.await.unwrap().unwrap_err(),
        RepositoryCacheError::AccessDenied
    );

    // Restore proof for storage-directive checks, through the aggregate
    // branches adapter itself rather than a different resource endpoint.
    fixture.push(
        200,
        json!([{"name":"main","commit":{"sha":A},"protected":false}]),
    );
    session
        .branches(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    let offline_branches = session
        .branches(
            &repository(),
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(offline_branches.value.value.branches.len(), 1);
    fixture.push(200, tree(TREE_A));
    session
        .directory(
            &SnapshotDirectory::root(&snap),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    fixture.headers(200, tree(TREE_A), &[("cache-control", "no-store")]);
    session
        .directory(
            &SnapshotDirectory::root(&snap),
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .directory(
                &SnapshotDirectory::root(&snap),
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Miss(CacheMiss::NotCached)
    );

    fixture.push(200, readme(BLOB_A));
    session
        .readme(
            &snap,
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    fixture.headers(
        200,
        readme(BLOB_A),
        &[("cache-control", "private, no-store")],
    );
    session
        .readme(
            &snap,
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .readme(
                &snap,
                &Default::default(),
                RepositoryCacheMode::OfflineOnly,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryCacheError::Miss(CacheMiss::NotCached)
    );

    // One immutable tree payload is valid at another commit and path. Only
    // the second context is fetched; both contexts can then read offline.
    let other_commit = RepoSnapshot {
        repository: repository(),
        requested: RevisionSpec::Commit(ObjectSha::new(B).unwrap()),
        commit: ObjectSha::new(B).unwrap(),
        root_tree: ObjectSha::new(TREE_A).unwrap(),
        message: "B".into(),
        author: None,
        committer: None,
    };
    fixture.push(200, tree(TREE_A));
    let other = session
        .directory(
            &SnapshotDirectory::root(&other_commit),
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    assert_eq!(other.directory.commit().as_str(), B);
    let original = session
        .directory(
            &SnapshotDirectory::root(&snap),
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    assert_eq!(original.directory.commit().as_str(), A);
    let path = SnapshotDirectory {
        repository: repository(),
        commit: ObjectSha::new(B).unwrap(),
        tree: ObjectSha::new(TREE_A).unwrap(),
        path: Some(RepositoryPath::new("alias").unwrap()),
    };
    let aliased = session
        .directory(
            &path,
            &Default::default(),
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value;
    assert_eq!(aliased.directory.path().unwrap().as_str(), "alias");

    // Response budgets are representation identity: a tiny budget caches a
    // ResponseLimit partial, while a larger budget gets its own online read.
    let tight = RepositoryLimits {
        response_bytes: 1,
        ..Default::default()
    };
    fixture.push(200, tree(TREE_A));
    let limited = session
        .directory(
            &SnapshotDirectory::root(&snap),
            &tight,
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        limited.value.value.completeness,
        Completeness::Partial { .. }
    ));
    fixture.push(200, tree(TREE_A));
    let larger = session
        .directory(
            &SnapshotDirectory::root(&snap),
            &Default::default(),
            RepositoryCacheMode::ForceRefresh,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(larger.value.value.completeness, Completeness::Complete);

    // Cached raw bytes still obey the caller's current decoded budget.
    fixture.push(200, readme(BLOB_A));
    let ReadmeDiscovery::Found(descriptor) = session
        .readme(
            &snap,
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap()
        .value
        .value
    else {
        panic!()
    };
    let identity = BlobIdentity::from_readme(&descriptor);
    fixture.push(200, blob(BLOB_A, "hello"));
    session
        .blob(
            &identity,
            &Default::default(),
            RepositoryCacheMode::OnlinePreferred,
            Default::default(),
        )
        .await
        .unwrap();
    let small = session
        .blob(
            &identity,
            &BlobLimits {
                decoded_bytes: 1,
                response_bytes: 1024,
            },
            RepositoryCacheMode::OfflineOnly,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        small.value.value.body(),
        &BlobBody::TooLarge { size: 5, limit: 1 }
    );
}

#[test]
fn scope_must_match_the_session_account() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::default());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let svc = service(&temp, fixture.clone()).await;
        login(&svc, &fixture, 1).await;
        let store = CacheStore::open(CacheConfig::at(temp.path().join("other-cache"))).unwrap();
        let session = svc.repo_service().session().unwrap();
        let wrong = store.saved_account(key(2)).unwrap();
        assert!(CachedRepositorySession::new(session, store, wrong).is_err());
        tokio::time::timeout(Duration::from_secs(1), async {})
            .await
            .unwrap();
    });
}
