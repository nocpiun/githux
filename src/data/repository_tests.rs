//! Synthetic credentials, temporary SQLite, controlled async/blocking gates.
use super::account_tests::{config, key};
use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::domain::repository::*;
use crate::github::RepositoryLimits;
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use serde_json::json;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ROOT: &str = "1111111111111111111111111111111111111111";
const CHILD: &str = "2222222222222222222222222222222222222222";
const BLOB: &str = "3333333333333333333333333333333333333333";
fn locator() -> RepositoryLocator {
    RepositoryLocator::new("octo", "repo").unwrap()
}
fn details() -> serde_json::Value {
    json!({"id":42,"name":"repo","owner":{"id":7,"login":"octo"},"private":false,"default_branch":"main","fork":false,"archived":false,"stargazers_count":5,"forks_count":2,"open_issues_count":3,"size":12,"html_url":"https://github.com/octo/repo"})
}
fn snapshot() -> RepoSnapshot {
    RepoSnapshot {
        repository: RepositoryIdentity {
            id: 42,
            locator: locator(),
        },
        requested: RevisionSpec::Commit(ObjectSha::new(COMMIT).unwrap()),
        commit: ObjectSha::new(COMMIT).unwrap(),
        root_tree: ObjectSha::new(ROOT).unwrap(),
        message: "fixture".into(),
        author: None,
        committer: None,
    }
}
fn stale() -> RepositoryError {
    GitHubError::StaleContext.into()
}
fn cancelled() -> RepositoryError {
    GitHubError::Cancelled.into()
}

struct Step {
    response: HttpResponse,
    gate: Option<tokio::sync::oneshot::Receiver<()>>,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
}
#[derive(Default)]
struct Controlled {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<HttpRequest>>,
}
impl Controlled {
    fn push(&self, status: u16, body: serde_json::Value) {
        self.steps.lock().unwrap().push_back(Step {
            response: json_response(status, &body.to_string()),
            gate: None,
            entered: None,
        });
    }
    fn block(
        &self,
        status: u16,
        body: serde_json::Value,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        self.steps.lock().unwrap().push_back(Step {
            response: json_response(status, &body.to_string()),
            gate: Some(gate),
            entered: Some(entered),
        });
        (ready, release)
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Transport for Controlled {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted response");
            if let Some(entered) = step.entered {
                let _ = entered.send(());
            }
            if let Some(gate) = step.gate {
                gate.await.expect("release request");
            }
            Ok(step.response)
        })
    }
}
async fn ready(signal: tokio::sync::oneshot::Receiver<()>) {
    tokio::time::timeout(Duration::from_secs(5), signal)
        .await
        .expect("boundary reached")
        .unwrap();
}
async fn service(
    temp: &tempfile::TempDir,
    transport: Arc<Controlled>,
    store: Arc<dyn CredentialStore>,
    concurrency: usize,
) -> Arc<AccountService> {
    Arc::new(
        AccountService::open(
            config(temp),
            store,
            Arc::new(GitHubClient::new(Default::default(), transport)),
            Arc::new(BackgroundExecutor::new(concurrency)),
        )
        .await
        .unwrap(),
    )
}
async fn login(svc: &AccountService, transport: &Controlled, id: u64) {
    transport.push(200, json!({"id":id,"login":format!("user{id}")}));
    svc.login(Pat::from_str("synthetic-secret")).await.unwrap();
}

#[tokio::test]
async fn pipeline_keeps_ownership_fixed_shas_partial_results_and_readme_uncertainty() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(
        &temp,
        transport.clone(),
        Arc::new(InMemoryCredentialStore::new()),
        2,
    )
    .await;
    login(&svc, &transport, 1).await;
    let session = svc.repo_service().session().unwrap();
    let limits = RepositoryLimits::default();
    transport.push(200, details());
    let repo = session
        .repository(locator(), &limits, Default::default())
        .await
        .unwrap();
    transport.push(
        200,
        json!([{"name":"main","commit":{"sha":COMMIT},"protected":false}]),
    );
    let branches = session
        .branches(&repo.value.identity, &limits, Default::default())
        .await
        .unwrap();
    assert_eq!(branches.value.completeness, Completeness::Complete);
    transport.push(200,json!({"sha":COMMIT,"commit":{"message":"A","tree":{"sha":ROOT},"author":null,"committer":null}}));
    let snap = session
        .revision(
            &repo.value.identity,
            RevisionSpec::Commit(branches.value.branches[0].observed_commit.clone()),
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    // A later observation says main moved, but all subsequent content stays A.
    transport.push(200,json!([{"name":"main","commit":{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"protected":false}]));
    session
        .branches(&repo.value.identity, &limits, Default::default())
        .await
        .unwrap();
    transport.push(200,json!({"sha":ROOT,"truncated":false,"tree":[{"path":"src","type":"tree","mode":"040000","sha":CHILD}]}));
    let root = session
        .directory(
            &SnapshotDirectory::root(&snap.value),
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    transport.push(200, json!({"sha":CHILD,"truncated":false,"tree":[]}));
    let child = session
        .directory(
            &root.value.child_directory(0).unwrap(),
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(child.value.directory.commit().as_str(), COMMIT);
    transport.push(
        200,
        json!({"type":"file","path":"README.md","sha":BLOB,"size":12,"content":"ignored"}),
    );
    let readme = session
        .readme(&snap.value, &limits, Default::default())
        .await
        .unwrap();
    assert!(matches!(readme.value, ReadmeDiscovery::Found(_)));
    for context in [
        &branches.context,
        &snap.context,
        &root.context,
        &child.context,
        &readme.context,
    ] {
        assert_eq!(context, &repo.context);
        assert!(svc.repo_service().is_current(context));
    }
    transport.push(404, json!({}));
    transport.push(200,json!({"sha":ROOT,"truncated":false,"tree":[{"path":"file","type":"blob","mode":"100644","sha":BLOB}]}));
    assert!(matches!(
        session
            .readme(&snap.value, &limits, Default::default())
            .await
            .unwrap()
            .value,
        ReadmeDiscovery::NotFoundOrInaccessible { .. }
    ));
    // Do not erase a page/entry limit to make a complete empty collection.
    let bounded = RepositoryLimits {
        branch_pages: 0,
        ..limits.clone()
    };
    assert!(matches!(
        session
            .branches(&repo.value.identity, &bounded, Default::default())
            .await
            .unwrap()
            .value
            .completeness,
        Completeness::Partial { .. }
    ));
    transport.push(409, json!({}));
    assert_eq!(
        session
            .revision(
                &repo.value.identity,
                RevisionSpec::Ref(DynamicRef::new("main").unwrap()),
                &limits,
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryError::EmptyOrUnavailable
    );
    let requests = transport.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|r| r.url.ends_with(&format!("readme?ref={COMMIT}")))
    );
    assert!(
        requests
            .iter()
            .any(|r| r.url.ends_with(&format!("git/trees/{CHILD}")))
    );
    assert!(!format!("{session:?}{repo:?}{snap:?}{readme:?}").contains("synthetic-secret"));
}

#[tokio::test]
async fn every_step_rejects_a_context_lost_before_the_call_without_io() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(
        &temp,
        transport.clone(),
        Arc::new(InMemoryCredentialStore::new()),
        2,
    )
    .await;
    login(&svc, &transport, 1).await;
    let session = svc.repo_service().session().unwrap();
    // Model metadata -> event loop -> next step after switching to B.
    transport.push(200, details());
    session
        .repository(locator(), &Default::default(), Default::default())
        .await
        .unwrap();
    login(&svc, &transport, 2).await;
    let count = transport.count();
    let snap = snapshot();
    let limits = RepositoryLimits::default();
    assert_eq!(
        session
            .repository(locator(), &limits, Default::default())
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(
        session
            .branches(snap.repository(), &limits, Default::default())
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(
        session
            .revision(
                snap.repository(),
                RevisionSpec::Commit(snap.commit().clone()),
                &limits,
                Default::default()
            )
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(
        session
            .directory(&SnapshotDirectory::root(&snap), &limits, Default::default())
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(
        session
            .readme(&snap, &limits, Default::default())
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(transport.count(), count);
}

#[tokio::test]
async fn in_flight_success_and_errors_are_stale_for_all_account_mutations() {
    for change in [
        "switch",
        "aba",
        "replace",
        "remove-readd",
        "revalidate",
        "close",
    ] {
        for status in [200, 401] {
            let temp = tempfile::tempdir().unwrap();
            let transport = Arc::new(Controlled::default());
            let svc = service(
                &temp,
                transport.clone(),
                Arc::new(InMemoryCredentialStore::new()),
                3,
            )
            .await;
            login(&svc, &transport, 2).await;
            login(&svc, &transport, 1).await;
            let session = svc.repo_service().session().unwrap();
            let context = session.context().clone();
            let (entered, release) = transport.block(
                status,
                if status == 200 {
                    details()
                } else {
                    json!({"message":"Bad credentials"})
                },
            );
            let task = tokio::spawn(async move {
                session
                    .repository(locator(), &Default::default(), Default::default())
                    .await
            });
            ready(entered).await;
            match change {
                "switch" => svc.set_active(key(2)).await.unwrap(),
                "aba" => {
                    svc.set_active(key(2)).await.unwrap();
                    svc.set_active(key(1)).await.unwrap();
                }
                "replace" => {
                    transport.push(200, json!({"id":1,"login":"user1"}));
                    svc.replace(key(1), Pat::from_str("replacement-synthetic"))
                        .await
                        .unwrap();
                }
                "remove-readd" => {
                    svc.remove_saved(key(1)).await.unwrap();
                    login(&svc, &transport, 1).await;
                }
                "revalidate" => {
                    transport.push(200, json!({"id":1,"login":"user1"}));
                    svc.revalidate(key(1)).await.unwrap();
                }
                "close" => svc.close().await.unwrap(),
                _ => unreachable!(),
            }
            release.send(()).unwrap();
            assert_eq!(
                task.await.unwrap().unwrap_err(),
                stale(),
                "{change}, {status}"
            );
            assert!(!svc.repo_service().is_current(&context));
        }
    }
}

struct PausedLoad {
    inner: InMemoryCredentialStore,
    gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    outcome: u8,
}
impl CredentialStore for PausedLoad {
    fn save(&self, r: &CredentialRef, p: &Pat) -> Result<(), CredentialError> {
        self.inner.save(r, p)
    }
    fn delete(&self, r: &CredentialRef) -> Result<(), CredentialError> {
        self.inner.delete(r)
    }
    fn describe(&self) -> &'static str {
        "controlled load"
    }
    fn load(&self, r: &CredentialRef) -> Result<Option<Pat>, CredentialError> {
        let gate = self.gate.lock().unwrap().take();
        if let Some((entered, resume)) = gate {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
            match self.outcome {
                0 => {} // Read after cleanup: old slot is now missing.
                1 => return Err(CredentialError::Corrupted),
                2 => panic!("synthetic credential worker failure"),
                _ => unreachable!(),
            }
        }
        self.inner.load(r)
    }
}
#[tokio::test]
async fn retired_slot_missing_load_error_and_worker_failure_are_discarded() {
    for outcome in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let store = Arc::new(PausedLoad {
            inner: Default::default(),
            gate: Mutex::new(None),
            outcome,
        });
        let svc = service(&temp, transport.clone(), store.clone(), 2).await;
        login(&svc, &transport, 1).await;
        let old = svc
            .registry_handle()
            .lock()
            .unwrap()
            .credential_ref(key(1))
            .unwrap();
        let session = svc.repo_service().session().unwrap();
        let (entered, ready_signal) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        *store.gate.lock().unwrap() = Some((entered, resume));
        let read = tokio::spawn(async move {
            session
                .repository(locator(), &Default::default(), Default::default())
                .await
        });
        ready(ready_signal).await;
        transport.push(200, json!({"id":1,"login":"user1"}));
        svc.replace(key(1), Pat::from_str("synthetic-new"))
            .await
            .unwrap();
        assert!(
            store.inner.load(&old).unwrap().is_none(),
            "retired slot was cleaned"
        );
        release.send(()).unwrap();
        assert_eq!(read.await.unwrap().unwrap_err(), stale());
        assert_eq!(
            transport.count(),
            2,
            "only login and replacement verify sent"
        );
        // Current slot failures remain real typed errors.
        let live = svc
            .registry_handle()
            .lock()
            .unwrap()
            .credential_ref(key(1))
            .unwrap();
        store.inner.delete(&live).unwrap();
        assert!(matches!(
            svc.repo_service()
                .session()
                .unwrap()
                .repository(locator(), &Default::default(), Default::default())
                .await,
            Err(RepositoryError::GitHub(GitHubError::CredentialStore { .. }))
        ));
    }
}

#[tokio::test]
async fn cancellation_interrupts_blocking_load_without_touching_other_reads() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let store = Arc::new(PausedLoad {
        inner: Default::default(),
        gate: Mutex::new(None),
        outcome: 0,
    });
    let svc = service(&temp, transport.clone(), store.clone(), 1).await;
    login(&svc, &transport, 1).await;
    let session = svc.repo_service().session().unwrap();
    let (entered, signal) = tokio::sync::oneshot::channel();
    let (release, resume) = std::sync::mpsc::channel();
    *store.gate.lock().unwrap() = Some((entered, resume));
    let (cancel, source) = CancellationToken::new();
    let task = tokio::spawn(async move {
        session
            .repository(locator(), &Default::default(), cancel)
            .await
    });
    ready(signal).await;
    source.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        cancelled()
    );
    release.send(()).unwrap(); // OS load itself is not abortable; let worker drain.
    transport.push(200, details());
    svc.repo_service()
        .session()
        .unwrap()
        .repository(locator(), &Default::default(), Default::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn queued_cancellation_and_context_change_never_send_and_in_flight_cancel_is_independent() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(
        &temp,
        transport.clone(),
        Arc::new(InMemoryCredentialStore::new()),
        1,
    )
    .await;
    login(&svc, &transport, 2).await;
    login(&svc, &transport, 1).await;
    let semaphore = svc.executor.semaphore();
    let hold = semaphore.clone().acquire_owned().await.unwrap();
    let session = svc.repo_service().session().unwrap();
    let (cancel, source) = CancellationToken::new();
    let limits = RepositoryLimits::default();
    let mut future = Box::pin(session.repository(locator(), &limits, cancel));
    // Poll through credential loading while the only request permit is held.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut future)
            .await
            .is_err()
    );
    source.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), future)
            .await
            .unwrap()
            .unwrap_err(),
        cancelled()
    );
    assert_eq!(transport.count(), 2);
    let session = svc.repo_service().session().unwrap();
    let limits = RepositoryLimits::default();
    let mut future = Box::pin(session.repository(locator(), &limits, Default::default()));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut future)
            .await
            .is_err()
    );
    svc.set_active(key(2)).await.unwrap();
    svc.set_active(key(1)).await.unwrap();
    drop(hold);
    assert_eq!(future.await.unwrap_err(), stale());
    assert_eq!(transport.count(), 2);
    // Two independent sessions share the semaphore; cancel one truly in flight.
    let first = svc.repo_service().session().unwrap();
    let second = svc.repo_service().session().unwrap();
    let (entered, _release) = transport.block(200, details());
    let (cancel, source) = CancellationToken::new();
    let task = tokio::spawn(async move {
        first
            .repository(locator(), &Default::default(), cancel)
            .await
    });
    ready(entered).await;
    source.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        cancelled()
    );
    assert_eq!(semaphore.available_permits(), 1);
    transport.push(200, details());
    second
        .repository(locator(), &limits, Default::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn context_cannot_cross_service_instances_and_closed_service_rejects_it() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let first = service(
        &a,
        transport.clone(),
        Arc::new(InMemoryCredentialStore::new()),
        1,
    )
    .await;
    let second = service(
        &b,
        transport.clone(),
        Arc::new(InMemoryCredentialStore::new()),
        1,
    )
    .await;
    login(&first, &transport, 1).await;
    login(&second, &transport, 1).await;
    let session = first.repo_service().session().unwrap();
    assert_eq!(
        session.context().auth(),
        second.repo_service().session().unwrap().context().auth()
    );
    assert!(!second.repo_service().is_current(session.context()));
    first.close().await.unwrap();
    assert!(!session.verify());
    assert_eq!(first.repo_service().session().unwrap_err(), stale());
    assert_eq!(
        session
            .repository(locator(), &Default::default(), Default::default())
            .await
            .unwrap_err(),
        stale()
    );
}
