//! Fixed-commit repository picture reads and credential-free external reads.
//!
//! Every boundary is injected: a scripted transport answers the repository
//! reads, an in-memory byte source answers external addresses, and the account
//! service uses a temporary SQLite directory with a synthetic PAT. No real
//! credential or network is touched.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::auth::token::Pat;
use crate::content::images::{ImageKey, ImageScope};
use crate::data::AccountService;
use crate::data::account_tests::config;
use crate::domain::repository::RepositoryLocator;
use crate::github::client::GitHubClient;
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use serde_json::json;

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ROOT: &str = "1111111111111111111111111111111111111111";
const DOCS_TREE: &str = "2222222222222222222222222222222222222222";
const IMAGES_TREE: &str = "3333333333333333333333333333333333333333";
const IMAGE_BLOB: &str = "4444444444444444444444444444444444444444";

fn identity() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 42,
        locator: RepositoryLocator::new("octo", "repo").expect("locator"),
    }
}

fn png_bytes() -> Vec<u8> {
    let image = image::RgbaImage::from_fn(6, 3, |x, y| {
        image::Rgba([(x * 20) as u8, (y * 40) as u8, 90, 255])
    });
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encode");
    cursor.into_inner()
}

fn base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn blob_json(sha: &str, bytes: &[u8]) -> serde_json::Value {
    json!({"sha": sha, "encoding": "base64", "size": bytes.len(), "content": base64(bytes)})
}

fn commit_json() -> serde_json::Value {
    json!({"sha": COMMIT, "commit": {"message": "fixture", "tree": {"sha": ROOT}, "author": null, "committer": null}})
}

fn tree(sha: &str, entries: serde_json::Value) -> serde_json::Value {
    json!({"sha": sha, "truncated": false, "tree": entries})
}

fn image_chain(bytes: &[u8]) -> Vec<serde_json::Value> {
    vec![
        commit_json(),
        tree(
            ROOT,
            json!([{"path": "docs", "type": "tree", "mode": "040000", "sha": DOCS_TREE}]),
        ),
        tree(
            DOCS_TREE,
            json!([{"path": "images", "type": "tree", "mode": "040000", "sha": IMAGES_TREE}]),
        ),
        tree(
            IMAGES_TREE,
            json!([{"path": "diagram.png", "type": "blob", "mode": "100644", "sha": IMAGE_BLOB, "size": bytes.len()}]),
        ),
        blob_json(IMAGE_BLOB, bytes),
    ]
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

    fn block(&self, status: u16, body: serde_json::Value) -> tokio::sync::oneshot::Sender<()> {
        let (entered, _ready) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        self.steps.lock().unwrap().push_back(Step {
            response: json_response(status, &body.to_string()),
            gate: Some(gate),
            entered: Some(entered),
        });
        release
    }

    /// The recorded repository read URLs. The synthetic sign-in (`GET /user`)
    /// happens once in the fixture and is not part of what a picture read does.
    fn urls(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.url.clone())
            .filter(|url| !url.ends_with("/user"))
            .collect()
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

/// An in-memory external byte source. It never touches a network and records
/// the addresses it was asked for.
#[derive(Default)]
struct MemoryExternal {
    bodies: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    reads: Mutex<Vec<String>>,
}

impl MemoryExternal {
    fn serve(&self, url: &str, bytes: Vec<u8>) {
        self.bodies.lock().unwrap().insert(url.to_string(), bytes);
    }
}

impl ExternalImageBytes for MemoryExternal {
    fn load<'a>(
        &'a self,
        url: &'a str,
        _limits: &'a ExternalImageLimits,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ExternalImageError>> + Send + 'a>> {
        Box::pin(async move {
            self.reads.lock().unwrap().push(url.to_string());
            self.bodies
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or(ExternalImageError::Status(404))
        })
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    accounts: Arc<AccountService>,
    transport: Arc<Controlled>,
    executor: Arc<BackgroundExecutor>,
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(Controlled::default());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let accounts = Arc::new(
        AccountService::open(
            config(&temp),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(Default::default(), transport.clone())),
            executor.clone(),
        )
        .await
        .expect("account service"),
    );
    Fixture {
        _temp: temp,
        accounts,
        transport,
        executor,
    }
}

impl Fixture {
    fn session(&self) -> RepositorySession {
        self.accounts.repo_service().session().expect("session")
    }

    async fn login(&self, id: u64) {
        self.transport
            .push(200, json!({"id": id, "login": format!("user{id}")}));
        self.accounts
            .login(Pat::from_str("synthetic-secret"))
            .await
            .expect("login");
    }

    fn service(
        &self,
        session: RepositorySession,
        external: Arc<dyn ExternalImageBytes>,
        limits: ImageLimits,
    ) -> RepositoryImageService {
        RepositoryImageService::new(session, identity(), self.executor.clone(), external, limits)
    }
}

fn external(url: &str, bytes: Vec<u8>) -> (Arc<MemoryExternal>, ImageTarget) {
    let source = Arc::new(MemoryExternal::default());
    source.serve(url, bytes);
    (
        source,
        ImageTarget::External {
            url: url.to_string(),
        },
    )
}

/// Waits for exactly one completion. The sink is polled, never awaited: the
/// reader drains it on its own beat, so a provider test polls the same way.
async fn next_completion(sink: &ImageSink) -> ImageCompletion {
    for _ in 0..500 {
        let mut completions = sink.drain();
        if !completions.is_empty() {
            assert_eq!(completions.len(), 1, "exactly one completion");
            return completions.remove(0);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no completion within the deadline");
}

fn test_scope(generation: u64) -> ImageScope {
    ImageScope {
        reader: 1,
        generation,
    }
}

fn repository_request(commit: &str, path: &str) -> ImageTarget {
    ImageTarget::Repository {
        owner: "octo".into(),
        name: "repo".into(),
        commit: commit.into(),
        path: path.into(),
    }
}

#[tokio::test]
async fn a_relative_picture_reads_the_fixed_commit_tree_and_blob() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let bytes = png_bytes();
    for step in image_chain(&bytes) {
        fixture.transport.push(200, step);
    }

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, ImageLimits::default());
    assert!(service.verify());

    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(7),
            key: ImageKey::new("repo-image"),
            target: repository_request(COMMIT, "docs/images/diagram.png"),
        },
        sink.clone(),
    );

    let completion = next_completion(&sink).await;
    assert_eq!(completion.scope.generation, 7);
    assert_eq!(completion.scope.reader, test_scope(0).reader);
    let decoded = completion.result.expect("picture");
    assert_eq!((decoded.width, decoded.height), (6, 3));

    let urls = fixture.transport.urls();
    assert_eq!(urls.len(), 5, "{urls:?}");
    assert!(urls[0].contains(&format!("commits/{COMMIT}")));
    assert!(urls[1].contains(&format!("git/trees/{ROOT}")));
    assert!(urls[2].contains(&format!("git/trees/{DOCS_TREE}")));
    assert!(urls[3].contains(&format!("git/trees/{IMAGES_TREE}")));
    assert!(urls[4].contains(&format!("git/blobs/{IMAGE_BLOB}")));
    assert!(
        !urls.iter().any(|url| url.contains("download_url")),
        "no contents/download_url path may be used"
    );
}

#[tokio::test]
async fn a_second_picture_in_the_same_directory_reuses_the_trees() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let bytes = png_bytes();
    for step in image_chain(&bytes) {
        fixture.transport.push(200, step);
    }
    fixture.transport.push(200, blob_json(IMAGE_BLOB, &bytes));

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, ImageLimits::default());

    for generation in 1..=2u64 {
        let sink = ImageSink::new();
        service.start(
            ImageRequest {
                scope: test_scope(generation),
                key: ImageKey::new(format!("k{generation}")),
                target: repository_request(COMMIT, "docs/images/diagram.png"),
            },
            sink.clone(),
        );
        next_completion(&sink).await.result.expect("picture");
    }
    assert_eq!(
        fixture.transport.urls().len(),
        6,
        "{:?}",
        fixture.transport.urls()
    );
}

#[tokio::test]
async fn a_symlinked_picture_is_not_followed() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    fixture.transport.push(200, commit_json());
    fixture.transport.push(
        200,
        tree(
            ROOT,
            json!([{"path": "diagram.png", "type": "blob", "mode": "120000", "sha": IMAGE_BLOB}]),
        ),
    );

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, ImageLimits::default());
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("k"),
            target: repository_request(COMMIT, "diagram.png"),
        },
        sink.clone(),
    );
    assert_eq!(
        next_completion(&sink).await.result,
        Err(ImageFailure::NotFound)
    );
}

#[tokio::test]
async fn a_picture_for_another_repository_is_rejected() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, ImageLimits::default());
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("k"),
            target: ImageTarget::Repository {
                owner: "someone-else".into(),
                name: "repo".into(),
                commit: COMMIT.into(),
                path: "a.png".into(),
            },
        },
        sink.clone(),
    );
    assert_eq!(
        next_completion(&sink).await.result,
        Err(ImageFailure::StaleContext)
    );
    assert!(fixture.transport.urls().is_empty());
}

#[tokio::test]
async fn an_external_picture_never_touches_the_repository_session() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();
    let before = fixture.transport.urls().len();

    let bytes = png_bytes();
    let (source, target) = external("https://cdn.example.com/a.png", bytes);
    let service = fixture.service(session, source.clone(), ImageLimits::default());
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("k"),
            target,
        },
        sink.clone(),
    );
    let decoded = next_completion(&sink).await.result.expect("picture");
    assert_eq!((decoded.width, decoded.height), (6, 3));
    assert_eq!(source.reads.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.transport.urls().len(),
        before,
        "no GitHub request for a CDN image"
    );
}

#[tokio::test]
async fn cancelling_a_generation_publishes_nothing() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let bytes = png_bytes();
    fixture.transport.push(200, commit_json());
    let release = fixture.transport.block(
        200,
        tree(
            ROOT,
            json!([{"path": "a.png", "type": "blob", "mode": "100644", "sha": IMAGE_BLOB, "size": bytes.len()}]),
        ),
    );

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, ImageLimits::default());
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(9),
            key: ImageKey::new("k"),
            target: repository_request(COMMIT, "a.png"),
        },
        sink.clone(),
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    service.cancel(test_scope(9));
    let _ = release.send(());
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        sink.drain().is_empty(),
        "a cancelled generation must publish nothing"
    );
}

#[tokio::test]
async fn an_account_change_discards_an_in_flight_picture() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let bytes = png_bytes();
    fixture.transport.push(200, commit_json());
    fixture.transport.push(
        200,
        tree(
            ROOT,
            json!([{"path": "a.png", "type": "blob", "mode": "100644", "sha": IMAGE_BLOB, "size": bytes.len()}]),
        ),
    );
    let release = fixture.transport.block(200, blob_json(IMAGE_BLOB, &bytes));

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, ImageLimits::default());
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("k"),
            target: repository_request(COMMIT, "a.png"),
        },
        sink.clone(),
    );

    tokio::time::sleep(Duration::from_millis(30)).await;
    fixture.login(2).await;
    let _ = release.send(());
    assert_eq!(
        next_completion(&sink).await.result,
        Err(ImageFailure::StaleContext)
    );
}

#[tokio::test]
async fn an_oversized_repository_picture_is_reported_as_too_large() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let limits = ImageLimits {
        max_response_bytes: 32,
        ..ImageLimits::default()
    };
    let bytes = png_bytes();
    assert!(bytes.len() > 32);
    fixture.transport.push(200, commit_json());
    fixture.transport.push(
        200,
        tree(
            ROOT,
            json!([{"path": "a.png", "type": "blob", "mode": "100644", "sha": IMAGE_BLOB, "size": bytes.len()}]),
        ),
    );

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, limits);
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("k"),
            target: repository_request(COMMIT, "a.png"),
        },
        sink.clone(),
    );
    assert!(matches!(
        next_completion(&sink).await.result,
        Err(ImageFailure::TooLarge { .. })
    ));
}

#[tokio::test]
async fn a_malformed_picture_is_rejected_by_the_decode_budget() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let (source, target) = external(
        "https://cdn.example.com/bad.png",
        b"\x89PNG\r\n\x1a\nnot-really-a-png".to_vec(),
    );
    let service = fixture.service(session, source, ImageLimits::default());
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("k"),
            target,
        },
        sink.clone(),
    );
    assert_eq!(
        next_completion(&sink).await.result,
        Err(ImageFailure::Malformed)
    );
}

#[tokio::test]
async fn a_cancelled_session_reports_stale_context() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();
    let (source, target) = external("https://cdn.example.com/a.png", png_bytes());
    let service = fixture.service(session, source, ImageLimits::default());
    fixture.login(2).await;
    assert!(!service.verify());
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("k"),
            target,
        },
        sink.clone(),
    );
    assert_eq!(
        next_completion(&sink).await.result,
        Err(ImageFailure::StaleContext)
    );
}

/// Two readers can share one provider. Cancelling one document's scope must
/// leave the other reader's in-flight picture alone, even at the same
/// generation number.
#[tokio::test]
async fn cancelling_one_scope_does_not_cancel_another_readers_work() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let bytes = png_bytes();
    // Reader 1's picture: the slow blob. Reader 2's picture: a fast one.
    fixture.transport.push(200, commit_json());
    fixture.transport.push(
        200,
        tree(
            ROOT,
            json!([
                {"path": "a.png", "type": "blob", "mode": "100644", "sha": IMAGE_BLOB, "size": bytes.len()},
                {"path": "b.png", "type": "blob", "mode": "100644", "sha": IMAGE_BLOB, "size": bytes.len()},
            ]),
        ),
    );
    let release = fixture.transport.block(200, blob_json(IMAGE_BLOB, &bytes));

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, ImageLimits::default());

    let first = ImageScope {
        reader: 1,
        generation: 1,
    };
    let second = ImageScope {
        reader: 2,
        generation: 1,
    };
    for (scope, path) in [(first, "a.png"), (second, "b.png")] {
        service.start(
            ImageRequest {
                scope,
                key: ImageKey::new(format!("{}:{path}", scope.reader)),
                target: repository_request(COMMIT, path),
            },
            ImageSink::new(),
        );
    }
    // Only reader 1's document is replaced.
    tokio::time::sleep(Duration::from_millis(30)).await;
    service.cancel(first);
    let _ = release.send(());
    tokio::time::sleep(Duration::from_millis(60)).await;
    // The shared service is still usable for the other reader, and its cache is
    // not keyed by a credential or a generation.
    assert!(service.verify());
    assert_eq!(
        service
            .inner
            .work
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![second],
        "only the replaced reader's scope was cancelled"
    );
}
