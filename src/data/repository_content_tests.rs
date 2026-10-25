//! Fixed-snapshot blob/content wiring: account gates, background preparation,
//! per-call cancellation. Synthetic PAT, temporary SQLite, controlled transport.
use super::account_tests::{config, key};
use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::content::{PreparedBody, UnsupportedContent};
use crate::domain::blob::{BlobBody, BlobIdentity, BlobLimits};
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
const BLOB: &str = "3333333333333333333333333333333333333333";
const SECOND_BLOB: &str = "4444444444444444444444444444444444444444";

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
fn tree_with_file(path: &str, sha: &str, size: u64) -> serde_json::Value {
    json!({"sha":ROOT,"truncated":false,"tree":[{"path":path,"type":"blob","mode":"100644","sha":sha,"size":size}]})
}
fn blob_json(sha: &str, encoded: &str, size: usize) -> serde_json::Value {
    json!({"sha":sha,"encoding":"base64","size":size,"content":encoded})
}
fn text_blob_json(sha: &str, text: &str) -> serde_json::Value {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    blob_json(sha, &encoded, text.len())
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

    /// Fixture bodies are already bounded; the real receive-time bound is
    /// covered by the PR21 localhost socket tests.
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

/// Full chain: repository -> revision -> directory -> BlobIdentity -> content.
/// README descriptor path is exercised separately below.
#[tokio::test]
async fn snapshot_chain_reads_fixed_blob_and_prepares_with_the_same_context() {
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
    let publication = session.context().clone();
    let limits = RepositoryLimits::default();
    transport.push(200, details());
    let repo = session
        .repository(locator(), &limits, Default::default())
        .await
        .unwrap();
    transport.push(
        200,
        json!({"sha":COMMIT,"commit":{"message":"A","tree":{"sha":ROOT},"author":null,"committer":null}}),
    );
    let snap = session
        .revision(
            &repo.value.identity,
            RevisionSpec::Commit(ObjectSha::new(COMMIT).unwrap()),
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    transport.push(200, tree_with_file("note.txt", BLOB, 6));
    let root = session
        .directory(
            &SnapshotDirectory::root(&snap.value),
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    let identity = BlobIdentity::from_file(&root.value, 0).unwrap();
    assert_eq!(identity.blob().as_str(), BLOB);
    assert_eq!(identity.commit().as_str(), COMMIT);
    transport.push(200, text_blob_json(BLOB, "héllo"));
    let prepared = session
        .content(&identity, &BlobLimits::default(), Default::default())
        .await
        .unwrap();
    assert_eq!(prepared.context, publication);
    assert!(svc.repo_service().is_current(&prepared.context));
    assert_eq!(prepared.value.identity, identity);
    assert_eq!(
        prepared.value.body,
        PreparedBody::Utf8 {
            text: "héllo".into(),
            has_bom: false
        }
    );
    // Raw read keeps the identity too, and never asks for a branch/download URL.
    transport.push(200, text_blob_json(BLOB, "héllo"));
    let raw = session
        .blob(&identity, &BlobLimits::default(), Default::default())
        .await
        .unwrap();
    assert_eq!(raw.context, publication);
    assert_eq!(raw.value.identity(), &identity);
    assert_eq!(raw.value.body(), &BlobBody::Bytes("héllo".into()));
    let requests = transport.requests.lock().unwrap();
    let blobs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.contains("/git/blobs/"))
        .collect();
    assert_eq!(blobs.len(), 2);
    for request in blobs {
        assert!(request.url.ends_with(&format!("git/blobs/{BLOB}")));
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("accept")
                    && value == "application/vnd.github+json")
        );
    }
    assert!(!format!("{session:?}{prepared:?}{raw:?}").contains("synthetic-secret"));
}

#[tokio::test]
async fn readme_descriptor_chain_stays_on_the_original_commit_after_a_branch_moves() {
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
        json!({"sha":COMMIT,"commit":{"message":"A","tree":{"sha":ROOT},"author":null,"committer":null}}),
    );
    let snap = session
        .revision(
            &repo.value.identity,
            RevisionSpec::Commit(ObjectSha::new(COMMIT).unwrap()),
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    transport.push(
        200,
        json!({"type":"file","path":"README.md","sha":BLOB,"size":5,"content":"ignored"}),
    );
    let readme = session
        .readme(&snap.value, &limits, Default::default())
        .await
        .unwrap();
    let ReadmeDiscovery::Found(descriptor) = readme.value else {
        panic!("expected a README descriptor");
    };
    let identity = BlobIdentity::from_readme(&descriptor);
    // main moved afterwards: the derived handle still names the old blob SHA.
    transport.push(
        200,
        json!([{"name":"main","commit":{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"protected":false}]),
    );
    session
        .branches(&repo.value.identity, &limits, Default::default())
        .await
        .unwrap();
    transport.push(200, text_blob_json(BLOB, "hello"));
    let prepared = session
        .content(&identity, &BlobLimits::default(), Default::default())
        .await
        .unwrap();
    assert_eq!(prepared.value.identity.commit().as_str(), COMMIT);
    assert_eq!(prepared.value.identity.path().as_str(), "README.md");
    assert_eq!(
        prepared.value.body,
        PreparedBody::Utf8 {
            text: "hello".into(),
            has_bom: false
        }
    );
    assert!(
        transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.url.ends_with(&format!("git/blobs/{BLOB}")))
    );
}

/// A local size rejection is not an authorization success: no network, but all
/// gates still apply, including a switch that happens after the call starts.
#[tokio::test]
async fn known_too_large_skips_network_but_never_skips_the_context_gates() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(
        &temp,
        transport.clone(),
        Arc::new(InMemoryCredentialStore::new()),
        2,
    )
    .await;
    login(&svc, &transport, 2).await;
    login(&svc, &transport, 1).await;
    let session = svc.repo_service().session().unwrap();
    transport.push(200, tree_with_file("big.bin", BLOB, 4096));
    let limits = BlobLimits {
        decoded_bytes: 1024,
        ..Default::default()
    };
    let tree = DirectoryTree {
        directory: SnapshotDirectory::root(&snapshot()),
        entries: vec![TreeEntry {
            path: RepositoryPath::new("big.bin").unwrap(),
            kind: TreeEntryKind::File { executable: false },
            sha: ObjectSha::new(BLOB).unwrap(),
            size: Some(4096),
        }],
        completeness: Completeness::Complete,
        truncated: Some(false),
    };
    let identity = BlobIdentity::from_file(&tree, 0).unwrap();
    let count = transport.count();
    let prepared = session
        .content(&identity, &limits, Default::default())
        .await
        .unwrap();
    assert!(svc.repo_service().is_current(&prepared.context));
    assert_eq!(
        prepared.value.body,
        PreparedBody::TooLarge {
            size: 4096,
            limit: 1024
        }
    );
    assert_eq!(transport.count(), count);
    // Already stale before the call: no local result escapes either.
    svc.set_active(key(2)).await.unwrap();
    assert_eq!(
        session
            .blob(&identity, &limits, Default::default())
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(
        session
            .content(&identity, &limits, Default::default())
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(transport.count(), count);
    // Cancellation while waiting for credentials still wins over the local path.
    let session = svc.repo_service().session().unwrap();
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    assert_eq!(
        session
            .content(&identity, &limits, cancel)
            .await
            .unwrap_err(),
        cancelled()
    );
}

#[tokio::test]
async fn blob_and_content_reject_a_context_lost_before_the_call_without_io() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(
        &temp,
        transport.clone(),
        Arc::new(InMemoryCredentialStore::new()),
        2,
    )
    .await;
    login(&svc, &transport, 2).await;
    login(&svc, &transport, 1).await;
    let session = svc.repo_service().session().unwrap();
    let tree = DirectoryTree {
        directory: SnapshotDirectory::root(&snapshot()),
        entries: vec![TreeEntry {
            path: RepositoryPath::new("a.txt").unwrap(),
            kind: TreeEntryKind::File { executable: false },
            sha: ObjectSha::new(BLOB).unwrap(),
            size: Some(1),
        }],
        completeness: Completeness::Complete,
        truncated: Some(false),
    };
    let identity = BlobIdentity::from_file(&tree, 0).unwrap();
    svc.set_active(key(2)).await.unwrap();
    let count = transport.count();
    assert_eq!(
        session
            .blob(&identity, &Default::default(), Default::default())
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(
        session
            .content(&identity, &Default::default(), Default::default())
            .await
            .unwrap_err(),
        stale()
    );
    assert_eq!(transport.count(), count);
}

/// In-flight success/Unauthorized for every account mutation. The separate
/// preparation-gate test below covers the prepare-to-publication boundary.
#[tokio::test]
async fn in_flight_blob_results_are_stale_for_all_account_mutations() {
    for change in ["switch", "aba", "replace", "remove-readd", "close"] {
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
            let tree = DirectoryTree {
                directory: SnapshotDirectory::root(&snapshot()),
                entries: vec![TreeEntry {
                    path: RepositoryPath::new("a.txt").unwrap(),
                    kind: TreeEntryKind::File { executable: false },
                    sha: ObjectSha::new(BLOB).unwrap(),
                    size: Some(1),
                }],
                completeness: Completeness::Complete,
                truncated: Some(false),
            };
            let identity = BlobIdentity::from_file(&tree, 0).unwrap();
            let body = if status == 200 {
                text_blob_json(BLOB, "a")
            } else {
                json!({"message":"Bad credentials"})
            };
            let (entered, release) = transport.block(status, body);
            let task = tokio::spawn(async move {
                session
                    .content(&identity, &Default::default(), Default::default())
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

/// Preparation must not run on the caller's thread, and its result is still
/// gated. A large UTF-8 body makes the blocking work observable by thread id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preparation_runs_off_the_calling_thread() {
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
    let tree = DirectoryTree {
        directory: SnapshotDirectory::root(&snapshot()),
        entries: vec![TreeEntry {
            path: RepositoryPath::new("big.txt").unwrap(),
            kind: TreeEntryKind::File { executable: false },
            sha: ObjectSha::new(BLOB).unwrap(),
            size: None,
        }],
        completeness: Completeness::Complete,
        truncated: Some(false),
    };
    let identity = BlobIdentity::from_file(&tree, 0).unwrap();
    let text = "x".repeat(512 * 1024);
    transport.push(200, text_blob_json(BLOB, &text));
    let caller = std::thread::current().id();
    let prepared = session
        .content(&identity, &Default::default(), Default::default())
        .await
        .unwrap();
    assert_eq!(
        prepared.value.body,
        PreparedBody::Utf8 {
            text,
            has_bom: false
        }
    );
    // The calling thread only awaited; the preparation thread is observable via
    // the preparation hook installed for this test build.
    assert_ne!(
        crate::data::repository::test_hooks::last_prepare_thread(),
        Some(caller)
    );
    assert!(crate::data::repository::test_hooks::last_prepare_thread().is_some());
}

/// The account can change after the read completed and before the prepared
/// value is published. The preparation gate makes that interleaving exact.
#[tokio::test]
async fn context_changes_and_cancellation_during_preparation_discard_the_result() {
    for (change, size) in [
        ("switch", 1),
        ("aba", 1),
        ("close", 1),
        ("cancel", 1),
        // A purely local TooLarge outcome is gated at the same boundary.
        ("switch", 4096),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let svc = service(
            &temp,
            transport.clone(),
            Arc::new(InMemoryCredentialStore::new()),
            2,
        )
        .await;
        login(&svc, &transport, 2).await;
        login(&svc, &transport, 1).await;
        let session = svc.repo_service().session().unwrap();
        let tree = DirectoryTree {
            directory: SnapshotDirectory::root(&snapshot()),
            entries: vec![TreeEntry {
                path: RepositoryPath::new("a.txt").unwrap(),
                kind: TreeEntryKind::File { executable: false },
                sha: ObjectSha::new(BLOB).unwrap(),
                size: Some(size),
            }],
            completeness: Completeness::Complete,
            truncated: Some(false),
        };
        let identity = BlobIdentity::from_file(&tree, 0).unwrap();
        let limits = BlobLimits {
            decoded_bytes: 1024,
            ..Default::default()
        };
        if size == 1 {
            transport.push(200, text_blob_json(BLOB, "a"));
        }
        let count = transport.count();
        let (entered, release) = crate::data::repository::test_hooks::pause_next_prepare();
        let (cancel, source) = CancellationToken::new();
        let task = tokio::spawn(async move { session.content(&identity, &limits, cancel).await });
        // The blocking prepare thread signals over std mpsc; keep this async
        // test thread yielding so the spawned read can make progress.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while entered.try_recv().is_err() {
            assert!(std::time::Instant::now() < deadline, "preparation started");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        match change {
            "switch" => svc.set_active(key(2)).await.unwrap(),
            "aba" => {
                svc.set_active(key(2)).await.unwrap();
                svc.set_active(key(1)).await.unwrap();
            }
            "close" => svc.close().await.unwrap(),
            "cancel" => source.cancel(),
            _ => unreachable!(),
        }
        release.send(()).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            if change == "cancel" {
                cancelled()
            } else {
                stale()
            },
            "{change}, size={size}"
        );
        // No extra request was issued to recover or revalidate the old content.
        assert_eq!(transport.count(), count + usize::from(size == 1));
    }
}

#[tokio::test]
async fn content_preserves_empty_non_utf8_binary_and_preparation_limits() {
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
    for (name, bytes, expected) in [
        ("empty.txt", Vec::new(), PreparedBody::Empty),
        (
            "latin1.txt",
            vec![0xff, 0xfe, 0x41],
            PreparedBody::Unsupported(UnsupportedContent::NonUtf8),
        ),
        (
            "binary.txt",
            vec![0x41, 0x00, 0x42],
            PreparedBody::Unsupported(UnsupportedContent::BinaryControlCharacters),
        ),
    ] {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let tree = DirectoryTree {
            directory: SnapshotDirectory::root(&snapshot()),
            entries: vec![TreeEntry {
                path: RepositoryPath::new(name).unwrap(),
                kind: TreeEntryKind::File { executable: false },
                sha: ObjectSha::new(BLOB).unwrap(),
                size: None,
            }],
            completeness: Completeness::Complete,
            truncated: Some(false),
        };
        let identity = BlobIdentity::from_file(&tree, 0).unwrap();
        transport.push(200, blob_json(BLOB, &encoded, bytes.len()));
        let prepared = session
            .content(&identity, &Default::default(), Default::default())
            .await
            .unwrap();
        assert_eq!(prepared.value.body, expected, "{name}");
        assert_eq!(prepared.value.identity.path().as_str(), name);
        // The preparation budget follows the same BlobLimits as the read.
        let tight = BlobLimits {
            decoded_bytes: 1,
            ..Default::default()
        };
        transport.push(200, blob_json(BLOB, &encoded, bytes.len()));
        let limited = session
            .content(&identity, &tight, Default::default())
            .await
            .unwrap();
        if bytes.len() > 1 {
            assert_eq!(
                limited.value.body,
                PreparedBody::TooLarge {
                    size: bytes.len() as u64,
                    limit: 1
                }
            );
        }
    }
}

#[tokio::test]
async fn queued_blob_reads_never_send_after_cancellation_or_context_loss() {
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
    let tree = DirectoryTree {
        directory: SnapshotDirectory::root(&snapshot()),
        entries: vec![TreeEntry {
            path: RepositoryPath::new("a.txt").unwrap(),
            kind: TreeEntryKind::File { executable: false },
            sha: ObjectSha::new(BLOB).unwrap(),
            size: Some(1),
        }],
        completeness: Completeness::Complete,
        truncated: Some(false),
    };
    let identity = BlobIdentity::from_file(&tree, 0).unwrap();
    let semaphore = svc.executor.semaphore();
    let hold = semaphore.clone().acquire_owned().await.unwrap();
    let session = svc.repo_service().session().unwrap();
    let (cancel, source) = CancellationToken::new();
    let limits = BlobLimits::default();
    let mut future = Box::pin(session.content(&identity, &limits, cancel));
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
    let mut future = Box::pin(session.blob(&identity, &limits, Default::default()));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut future)
            .await
            .is_err()
    );
    svc.set_active(key(2)).await.unwrap();
    drop(hold);
    assert_eq!(future.await.unwrap_err(), stale());
    assert_eq!(transport.count(), 2);
}

#[tokio::test]
async fn cancelling_one_in_flight_blob_does_not_affect_an_independent_read() {
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
    let first = svc.repo_service().session().unwrap();
    let second = svc.repo_service().session().unwrap();
    let make = |sha: &str| {
        let tree = DirectoryTree {
            directory: SnapshotDirectory::root(&snapshot()),
            entries: vec![TreeEntry {
                path: RepositoryPath::new("a.txt").unwrap(),
                kind: TreeEntryKind::File { executable: false },
                sha: ObjectSha::new(sha).unwrap(),
                size: Some(1),
            }],
            completeness: Completeness::Complete,
            truncated: Some(false),
        };
        BlobIdentity::from_file(&tree, 0).unwrap()
    };
    let (entered, _release) = transport.block(200, text_blob_json(BLOB, "a"));
    let (cancel, source) = CancellationToken::new();
    let identity = make(BLOB);
    let task =
        tokio::spawn(async move { first.blob(&identity, &Default::default(), cancel).await });
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
    transport.push(200, text_blob_json(SECOND_BLOB, "b"));
    let other = second
        .content(&make(SECOND_BLOB), &Default::default(), Default::default())
        .await
        .unwrap();
    assert_eq!(other.value.identity.blob().as_str(), SECOND_BLOB);
    assert_eq!(
        other.value.body,
        PreparedBody::Utf8 {
            text: "b".into(),
            has_bom: false
        }
    );
    assert!(svc.repo_service().is_current(&other.context));
}

struct PausedLoad {
    inner: InMemoryCredentialStore,
    gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
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
        if let Some((entered, resume)) = self.gate.lock().unwrap().take() {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        self.inner.load(r)
    }
}

#[tokio::test]
async fn credential_load_cancellation_and_late_replacement_drop_blob_results() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let store = Arc::new(PausedLoad {
        inner: Default::default(),
        gate: Mutex::new(None),
    });
    let svc = service(&temp, transport.clone(), store.clone(), 1).await;
    login(&svc, &transport, 1).await;
    let tree = DirectoryTree {
        directory: SnapshotDirectory::root(&snapshot()),
        entries: vec![TreeEntry {
            path: RepositoryPath::new("a.txt").unwrap(),
            kind: TreeEntryKind::File { executable: false },
            sha: ObjectSha::new(BLOB).unwrap(),
            size: Some(1),
        }],
        completeness: Completeness::Complete,
        truncated: Some(false),
    };
    let identity = BlobIdentity::from_file(&tree, 0).unwrap();
    // Cancel while the OS credential load is blocked.
    let session = svc.repo_service().session().unwrap();
    let (entered, signal) = tokio::sync::oneshot::channel();
    let (release, resume) = std::sync::mpsc::channel();
    *store.gate.lock().unwrap() = Some((entered, resume));
    let (cancel, source) = CancellationToken::new();
    let second_identity = identity.clone();
    let task = tokio::spawn(async move {
        session
            .content(&second_identity, &Default::default(), cancel)
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
    release.send(()).unwrap();
    // Replace while loading: the old session cannot publish even a local error.
    let session = svc.repo_service().session().unwrap();
    let (entered, signal) = tokio::sync::oneshot::channel();
    let (release, resume) = std::sync::mpsc::channel();
    *store.gate.lock().unwrap() = Some((entered, resume));
    let last_identity = identity.clone();
    let task = tokio::spawn(async move {
        session
            .blob(&last_identity, &Default::default(), Default::default())
            .await
    });
    ready(signal).await;
    transport.push(200, json!({"id":1,"login":"user1"}));
    svc.replace(key(1), Pat::from_str("synthetic-new"))
        .await
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(task.await.unwrap().unwrap_err(), stale());
    // Current account: missing credentials remain a typed error, not empty text.
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
            .blob(&identity, &Default::default(), Default::default())
            .await,
        Err(RepositoryError::GitHub(GitHubError::CredentialStore { .. }))
    ));
}
