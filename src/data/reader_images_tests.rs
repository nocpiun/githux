//! Fixed-commit repository picture reads and credential-free external reads.
//!
//! Every boundary is injected: a scripted transport answers the repository
//! reads, an in-memory byte source answers external addresses, and the account
//! service uses a temporary SQLite directory with a synthetic PAT. No real
//! credential or network is touched.

use std::collections::{HashMap, VecDeque};
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
use crate::network::reader_images::ExternalImageMetadata;
use crate::network::transport::{
    HttpMethod, HttpRequest, HttpResponse, Transport, TransportError, json_response,
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

struct RequestGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

struct Route {
    response: HttpResponse,
    gates: VecDeque<RequestGate>,
}

#[derive(Default)]
struct Controlled {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<HttpRequest>>,
    // Opt in only for unordered reads; existing scripted gate/cancel tests
    // retain their FIFO contract. Responses repeat; only gates are consumed.
    routes: Mutex<Option<HashMap<String, Route>>>,
}

impl Controlled {
    fn serve(&self, path: &str, body: serde_json::Value) {
        let url = reqwest::Url::parse(&format!("https://api.github.com{path}"))
            .expect("fixture URL")
            .to_string();
        self.routes.lock().unwrap().get_or_insert_default().insert(
            url,
            Route {
                response: json_response(200, &body.to_string()),
                gates: VecDeque::new(),
            },
        );
    }

    fn gate(
        &self,
        path: &str,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        self.routes
            .lock()
            .unwrap()
            .as_mut()
            .expect("URL fixture enabled")
            .get_mut(&format!("https://api.github.com{path}"))
            .expect("model the route before gating it")
            .gates
            .push_back(RequestGate {
                entered,
                release: wait,
            });
        (ready, release)
    }

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
            self.requests.lock().unwrap().push(request.clone());
            let routed = {
                let mut routes = self.routes.lock().unwrap();
                routes.as_mut().map(|routes| {
                    assert_eq!(request.method, HttpMethod::Get, "fixture only models GET");
                    let url = reqwest::Url::parse(&request.url).expect("request URL");
                    let route = routes.get_mut(url.as_str()).unwrap_or_else(|| {
                        panic!("unmodelled reader image fixture request: {}", request.url)
                    });
                    let (entered, gate) = match route.gates.pop_front() {
                        Some(gate) => (Some(gate.entered), Some(gate.release)),
                        None => (None, None),
                    };
                    Step {
                        response: route.response.clone(),
                        entered,
                        gate,
                    }
                })
            };
            let step = routed.unwrap_or_else(|| {
                self.steps
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("scripted response")
            });
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
    fn load_with_metadata<'a>(
        &'a self,
        url: &'a str,
        _limits: &'a ExternalImageLimits,
        validation: ValidationHeaders<'a>,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalImageResponse, ExternalImageError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.reads.lock().unwrap().push(url.to_string());
            let bytes = self
                .bodies
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or(ExternalImageError::Status(404))?;

            let etag = format!("\"etag-{}\"", bytes.len());
            if validation.if_none_match == Some(etag.as_str()) {
                return Ok(ExternalImageResponse::NotModified {
                    metadata: ExternalImageMetadata {
                        etag: Some(etag),
                        last_modified: None,
                        cache_control: Some("public, max-age=60".into()),
                        age_secs: None,
                    },
                });
            }

            Ok(ExternalImageResponse::Fetched {
                bytes,
                metadata: ExternalImageMetadata {
                    etag: Some(etag),
                    last_modified: None,
                    cache_control: Some("public, max-age=60".into()),
                    age_secs: None,
                },
            })
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

    /// Reopens a cache store after the previous instance was dropped. A
    /// background cache writer may still hold a `CacheStore` clone for a few
    /// milliseconds after the service is dropped, and the exclusive lock file
    /// then reports `InUse`. Under a loaded full test suite that window is not
    /// reliably 50 ms, so wait for the writer to finish instead of flaking.
    fn reopen_cache_store(cache_cfg: crate::storage::cache::CacheConfig) -> CacheStore {
        for _ in 0..200 {
            match CacheStore::open(cache_cfg.clone()) {
                Ok(store) => return store,
                Err(crate::storage::cache::CacheError::InUse) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("reopen cache store: {error:?}"),
            }
        }
        panic!("cache store stayed InUse after 5s of background writers");
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
        self.service_with_cache(session, external, limits, None)
    }

    fn service_with_cache(
        &self,
        session: RepositorySession,
        external: Arc<dyn ExternalImageBytes>,
        limits: ImageLimits,
        cache: Option<(CacheStore, CacheScope, bool)>,
    ) -> RepositoryImageService {
        RepositoryImageService::with_cache(
            session,
            identity(),
            self.executor.clone(),
            external,
            limits,
            cache,
        )
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

/// Finding: the repository path performs several requests, so a per-request
/// transport timeout alone can exceed the advertised complete-load budget many
/// times over. `ImageLimits::total_timeout` now bounds the whole load.
#[tokio::test]
async fn the_total_timeout_bounds_the_whole_repository_load() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    // The commit read itself never completes in time.
    let _release = fixture.transport.block(200, commit_json());

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let limits = ImageLimits {
        total_timeout: Duration::from_millis(150),
        ..ImageLimits::default()
    };
    let service = fixture.service(session, source, limits);
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("slow"),
            target: repository_request(COMMIT, "a.png"),
        },
        sink.clone(),
    );
    let completion = next_completion(&sink).await;
    assert_eq!(
        completion.result,
        Err(ImageFailure::Timeout),
        "the whole load is bounded by one deadline, not per request"
    );
}

/// Finding: an account change while the blocking decode runs must not publish
/// the decoded picture. `RepositorySession::content` re-checks after its own
/// preparation step; this path must do the same after decoding. The probe holds
/// the load exactly between decode and publish, so the boundary is real.
#[tokio::test]
async fn an_account_change_during_decode_publishes_nothing() {
    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let bytes = png_bytes();
    for step in image_chain(&bytes) {
        fixture.transport.push(200, step);
    }

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service(session, source, ImageLimits::default());
    let (entered, release) = service.pause_before_publish();

    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("k"),
            target: repository_request(COMMIT, "docs/images/diagram.png"),
        },
        sink.clone(),
    );

    // The picture is decoded and waiting to be published.
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the decode boundary is reached")
        .expect("probe signal");
    assert!(sink.drain().is_empty(), "nothing was published yet");

    // A different account replaces the one the session was captured under.
    fixture.login(2).await;
    let _ = release.send(());

    let completion = next_completion(&sink).await;
    assert_eq!(
        completion.result,
        Err(ImageFailure::StaleContext),
        "a picture decoded under a retired authorization is never published"
    );
}

#[tokio::test]
async fn fixed_revision_images_persist_across_reopened_store_and_service() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::{CacheConfig, CacheStore};

    let cache_dir = tempfile::tempdir().expect("cache temp dir");
    let cache_cfg = CacheConfig::at(cache_dir.path().join("cache"));
    let store = CacheStore::open(cache_cfg.clone()).expect("open cache store");
    let account_key = AccountKey::new(1, HostId::github());
    let scope = store
        .saved_account(account_key.clone())
        .expect("saved scope");

    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let bytes = png_bytes();
    for step in image_chain(&bytes) {
        fixture.transport.push(200, step);
    }

    let (source, _) = external("https://example.com/never.png", Vec::new());
    let service = fixture.service_with_cache(
        session.clone(),
        source.clone(),
        ImageLimits::default(),
        Some((store.clone(), scope.clone(), true)),
    );

    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("diagram-img"),
            target: repository_request(COMMIT, "docs/images/diagram.png"),
        },
        sink.clone(),
    );

    let completion = next_completion(&sink).await;
    assert!(completion.result.is_ok());

    // Give asynchronous put a moment to complete
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Drop first store and service, reopen store and rebuild service with same scope
    drop(service);
    drop(store);

    let store2 = Fixture::reopen_cache_store(cache_cfg);
    let scope2 = store2.saved_account(account_key).expect("saved scope");

    // Second service is completely offline: ZERO responses are pushed to transport!
    // Both commit/path resolution and blob bytes are read entirely from persistent cache.

    let service2 = fixture.service_with_cache(
        session,
        source,
        ImageLimits::default(),
        Some((store2, scope2, true)),
    );

    let sink2 = ImageSink::new();
    service2.start(
        ImageRequest {
            scope: test_scope(2),
            key: ImageKey::new("diagram-img-offline"),
            target: repository_request(COMMIT, "docs/images/diagram.png"),
        },
        sink2.clone(),
    );

    let completion2 = next_completion(&sink2).await;
    let decoded = completion2.result.expect("cached picture offline");
    assert_eq!((decoded.width, decoded.height), (6, 3));
}

#[tokio::test]
async fn external_badges_honor_freshness_revalidation_and_no_store() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::{CacheConfig, CacheStore};

    let cache_dir = tempfile::tempdir().expect("cache temp dir");
    let cache_cfg = CacheConfig::at(cache_dir.path().join("cache"));
    let store = CacheStore::open(cache_cfg).expect("open cache store");
    let account_key = AccountKey::new(1, HostId::github());
    let scope = store.saved_account(account_key).expect("saved scope");

    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let external_source = Arc::new(MemoryExternal::default());
    let bytes = png_bytes();
    external_source.serve("https://badge.invalid/build.png", bytes.clone());

    let service = fixture.service_with_cache(
        session.clone(),
        external_source.clone(),
        ImageLimits::default(),
        Some((store.clone(), scope.clone(), false)),
    );

    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("badge"),
            target: ImageTarget::External {
                url: "https://badge.invalid/build.png".into(),
            },
        },
        sink.clone(),
    );

    let completion = next_completion(&sink).await;
    assert!(completion.result.is_ok());
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Second read should be served directly or via 304 Not Modified
    let sink2 = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(2),
            key: ImageKey::new("badge-cached"),
            target: ImageTarget::External {
                url: "https://badge.invalid/build.png".into(),
            },
        },
        sink2.clone(),
    );

    let completion2 = next_completion(&sink2).await;
    assert!(completion2.result.is_ok());

    // 3. Test no-store response: no-store response clears existing cache entry
    let nostore_url = "https://badge.invalid/nostore.png";
    external_source
        .bodies
        .lock()
        .unwrap()
        .insert(nostore_url.to_string(), bytes.clone());

    // In a custom implementation of load_with_metadata for a specific url:
    // When served with no-store cache-control, cache.read_external_image should return None or be invalidated
    let cache_helper = ReaderImageCache::new(store.clone(), scope.clone(), None, false);
    let meta_nostore = ExternalImageMetadata {
        etag: Some("\"etag-1\"".into()),
        last_modified: None,
        cache_control: Some("no-store".into()),
        age_secs: None,
    };
    cache_helper
        .put_external_image(nostore_url, &bytes, &meta_nostore)
        .expect("put outcome");
    let after_nostore = cache_helper.read_external_image(nostore_url).expect("read");
    assert!(after_nostore.is_none(), "no-store must not be stored");
}

#[tokio::test]
async fn branch_switch_or_other_repo_does_not_mix_cached_images() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::{CacheConfig, CacheStore};

    let cache_dir = tempfile::tempdir().expect("cache temp dir");
    let cache_cfg = CacheConfig::at(cache_dir.path().join("cache"));
    let store = CacheStore::open(cache_cfg).expect("open cache store");
    let account_key = AccountKey::new(1, HostId::github());
    let scope = store.saved_account(account_key).expect("saved scope");

    let cache_repo1 = ReaderImageCache::new(store.clone(), scope.clone(), Some(101), true);
    let cache_repo2 = ReaderImageCache::new(store.clone(), scope.clone(), Some(102), true);

    let sha = ObjectSha::new(IMAGE_BLOB).unwrap();
    let bytes1 = b"repo1 image bytes".to_vec();
    let bytes2 = b"repo2 image bytes".to_vec();

    cache_repo1.put_repository_blob(&sha, &bytes1).unwrap();
    cache_repo2.put_repository_blob(&sha, &bytes2).unwrap();

    let read1 = cache_repo1.read_repository_blob(&sha).unwrap();
    let read2 = cache_repo2.read_repository_blob(&sha).unwrap();

    assert_eq!(read1, Some(bytes1));
    assert_eq!(read2, Some(bytes2));
    assert_ne!(
        read1, read2,
        "Different repositories must have distinct namespaces for blobs"
    );
}

#[tokio::test]
async fn private_cache_isolation_and_quarantine_rejection() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::{AccessEvent, CacheConfig, CacheStore};

    let cache_dir = tempfile::tempdir().expect("cache temp dir");
    let cache_cfg = CacheConfig::at(cache_dir.path().join("cache"));
    let store = CacheStore::open(cache_cfg).expect("open cache store");
    let account_key = AccountKey::new(1, HostId::github());
    let scope = store.saved_account(account_key).expect("saved scope");

    let cache_repo = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), true);
    let sha = ObjectSha::new(IMAGE_BLOB).unwrap();
    let bytes = png_bytes();

    cache_repo.put_repository_blob(&sha, &bytes).unwrap();
    assert!(cache_repo.read_repository_blob(&sha).unwrap().is_some());

    // When credential is rejected, private cached content is quarantined / denied
    store
        .access_event(&scope, AccessEvent::CredentialRejected)
        .unwrap();

    let read_result = cache_repo.read_repository_blob(&sha);
    assert!(
        read_result.is_err(),
        "AccessDenied expected after CredentialRejected"
    );
}

#[tokio::test]
#[should_panic(expected = "unmodelled reader image fixture request")]
async fn unordered_image_fixture_rejects_unmodelled_requests() {
    let transport = Controlled::default();
    transport.serve("/known", json!({}));
    transport
        .execute(
            HttpRequest::get("https://api.github.com/known?unmodelled=1"),
            CancellationToken::default(),
        )
        .await
        .expect("fixture request");
}

async fn missing_image_fixture() -> (Fixture, RepositoryImageService) {
    let fixture = fixture().await;
    fixture.login(1).await;
    let bytes = png_bytes();
    fixture
        .transport
        .serve(&format!("/repos/octo/repo/commits/{COMMIT}"), commit_json());
    fixture.transport.serve(
        &format!("/repos/octo/repo/git/trees/{ROOT}"),
        tree(ROOT, json!([
            {"path": "good.png", "type": "blob", "mode": "100644", "sha": IMAGE_BLOB, "size": bytes.len()},
        ])),
    );
    fixture.transport.serve(
        &format!("/repos/octo/repo/git/blobs/{IMAGE_BLOB}"),
        blob_json(IMAGE_BLOB, &bytes),
    );
    let service = fixture.service(
        fixture.session(),
        Arc::new(MemoryExternal::default()),
        ImageLimits::default(),
    );
    (fixture, service)
}

async fn request_entered(ready: tokio::sync::oneshot::Receiver<()>) {
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .expect("request reached its gate")
        .expect("request entry signal");
}

fn start_picture(service: &RepositoryImageService, name: &str) -> ImageSink {
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new(name),
            target: repository_request(COMMIT, &format!("{name}.png")),
        },
        sink.clone(),
    );
    sink
}

fn assert_good(completion: ImageCompletion) {
    assert_eq!(completion.key.as_str(), "good");
    assert_eq!(completion.scope, test_scope(1));
    let decoded = completion.result.expect("good picture remains readable");
    assert_eq!((decoded.width, decoded.height), (6, 3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_images_fail_without_blocking_other_images_or_context() {
    // start() spawns independent tasks on the real background executor. Hold
    // both commit reads before either can fill the shared resolution cache,
    // then reverse their order at the tree boundary. Exercise both start orders.
    for first in ["missing", "good"] {
        let (fixture, service) = missing_image_fixture().await;
        let commit_path = format!("/repos/octo/repo/commits/{COMMIT}");
        let tree_path = format!("/repos/octo/repo/git/trees/{ROOT}");
        let (commit1, release_commit1) = fixture.transport.gate(&commit_path);
        let (commit2, release_commit2) = fixture.transport.gate(&commit_path);
        let (tree1, release_tree1) = fixture.transport.gate(&tree_path);
        let (tree2, release_tree2) = fixture.transport.gate(&tree_path);

        let first_sink = start_picture(&service, first);
        request_entered(commit1).await;
        let second_sink = start_picture(
            &service,
            if first == "missing" {
                "good"
            } else {
                "missing"
            },
        );
        request_entered(commit2).await;
        assert!(first_sink.drain().is_empty());
        assert!(second_sink.drain().is_empty(), "both loads are in flight");

        release_commit2.send(()).unwrap();
        request_entered(tree1).await;
        release_commit1.send(()).unwrap();
        request_entered(tree2).await;
        release_tree2.send(()).unwrap();
        release_tree1.send(()).unwrap();

        let (missing, good) = if first == "missing" {
            (&first_sink, &second_sink)
        } else {
            (&second_sink, &first_sink)
        };
        let missing = next_completion(missing).await;
        assert_eq!(missing.key.as_str(), "missing");
        assert_eq!(missing.scope, test_scope(1));
        assert_eq!(missing.result, Err(ImageFailure::NotFound));
        assert_good(next_completion(good).await);
        assert!(
            service.verify(),
            "missing image preserves the account context"
        );
        let urls = fixture.transport.urls();
        assert_eq!(urls.len(), 5, "two commits, two trees, one blob: {urls:?}");
        assert_eq!(
            urls.iter()
                .filter(|url| url.ends_with(&commit_path))
                .count(),
            2
        );
        assert_eq!(
            urls.iter().filter(|url| url.ends_with(&tree_path)).count(),
            2
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_image_reuses_resolution_while_good_image_is_still_in_flight() {
    let (fixture, service) = missing_image_fixture().await;
    let blob_path = format!("/repos/octo/repo/git/blobs/{IMAGE_BLOB}");
    let (blob, release_blob) = fixture.transport.gate(&blob_path);
    let good = start_picture(&service, "good");
    request_entered(blob).await;
    // The good load is blocked, but its commit and tree are already cached.
    // A second independent task must finish without another resolution read.
    let missing = start_picture(&service, "missing");
    let missing = next_completion(&missing).await;
    assert_eq!(missing.key.as_str(), "missing");
    assert_eq!(missing.result, Err(ImageFailure::NotFound));
    assert!(
        good.drain().is_empty(),
        "good load is still waiting on its blob"
    );
    assert_eq!(
        fixture.transport.urls().len(),
        3,
        "one commit, one tree, one blob"
    );
    release_blob.send(()).unwrap();
    assert_good(next_completion(&good).await);
    assert!(service.verify());

    // Responses remain available after the one-shot gates have been consumed.
    // Successful follow-up reads also prove that the missing image did not
    // invalidate the service's account/document context.
    let again = start_picture(&service, "good");
    assert_good(next_completion(&again).await);
    let urls = fixture.transport.urls();
    assert_eq!(urls.len(), 4, "only the repeated blob is fetched: {urls:?}");
    assert_eq!(
        urls.iter().filter(|url| url.ends_with(&blob_path)).count(),
        2
    );
}

#[tokio::test]
async fn svg_badge_stats_card_and_chart_load_through_provider_and_cache() {
    use crate::content::images::ImageFormat;
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::{CacheConfig, CacheStore};

    let cache_dir = tempfile::tempdir().expect("cache temp dir");
    let cache_cfg = CacheConfig::at(cache_dir.path().join("cache"));
    let store = CacheStore::open(cache_cfg.clone()).expect("open cache store");
    let account_key = AccountKey::new(1, HostId::github());
    let scope = store
        .saved_account(account_key.clone())
        .expect("saved scope");

    let fixture = fixture().await;
    fixture.login(1).await;
    let session = fixture.session();

    let badge_svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="88" height="20"><rect width="37" height="20" fill="#555"/><rect x="37" width="51" height="20" fill="#4b0"/><text x="19" y="14" fill="#fff" font-size="11">build</text></svg>"##.to_vec();
    let stats_svg = include_bytes!("../content/images/fixtures/entry-stats.svg").to_vec();
    let chart_svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="533"><path d="M 10 500 L 780 30" stroke="#dd4528" stroke-width="3" fill="none"/><text x="400" y="30" font-size="20">Star History</text></svg>"##.to_vec();

    fixture
        .transport
        .serve(&format!("/repos/octo/repo/commits/{COMMIT}"), commit_json());
    fixture.transport.serve(
        &format!("/repos/octo/repo/git/trees/{ROOT}"),
        tree(
            ROOT,
            json!([
                {"path": "chart.svg", "type": "blob", "mode": "100644", "sha": IMAGE_BLOB, "size": chart_svg.len()},
            ]),
        ),
    );
    fixture.transport.serve(
        &format!("/repos/octo/repo/git/blobs/{IMAGE_BLOB}"),
        blob_json(IMAGE_BLOB, &chart_svg),
    );

    let external_source = Arc::new(MemoryExternal::default());
    external_source.serve("https://badge.invalid/build.svg", badge_svg);
    external_source.serve("https://stats.invalid/card.svg", stats_svg);

    let service = fixture.service_with_cache(
        session.clone(),
        external_source.clone(),
        ImageLimits::default(),
        Some((store.clone(), scope.clone(), true)),
    );

    let badge_sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("badge-svg"),
            target: ImageTarget::External {
                url: "https://badge.invalid/build.svg".into(),
            },
        },
        badge_sink.clone(),
    );
    let badge = next_completion(&badge_sink)
        .await
        .result
        .expect("badge svg decodes");
    assert_eq!(badge.format, ImageFormat::Png);
    assert_eq!((badge.width, badge.height), (88, 20));
    assert!(image::load_from_memory(&badge.bytes).is_ok());

    let stats_sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("stats-svg"),
            target: ImageTarget::External {
                url: "https://stats.invalid/card.svg".into(),
            },
        },
        stats_sink.clone(),
    );
    let stats = next_completion(&stats_sink)
        .await
        .result
        .expect("stats svg decodes");
    assert_eq!(stats.format, ImageFormat::Png);
    assert_eq!((stats.width, stats.height), (240, 100));
    assert!(
        stats
            .animation
            .as_ref()
            .is_some_and(|a| a.duration_ms == 1350)
    );
    assert_eq!(
        image::load_from_memory(&stats.bytes)
            .unwrap()
            .to_rgba8()
            .get_pixel(12, 72)
            .0,
        [0, 255, 255, 255],
        "provider returns visible final row values"
    );

    let chart_sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: ImageKey::new("chart-svg"),
            target: repository_request(COMMIT, "chart.svg"),
        },
        chart_sink.clone(),
    );
    let chart = next_completion(&chart_sink)
        .await
        .result
        .expect("repo chart svg decodes");
    assert_eq!(chart.format, ImageFormat::Png);
    assert_eq!((chart.width, chart.height), (800, 533));

    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(service);
    drop(store);

    // Reopen cache store offline and verify cached repository SVG bytes decode
    // with zero network requests and preserve account/cache authorization.
    let store2 = Fixture::reopen_cache_store(cache_cfg);
    let scope2 = store2.saved_account(account_key).expect("saved scope");
    let external_reads = external_source.reads.lock().unwrap().len();
    external_source.bodies.lock().unwrap().clear();
    let offline_external = external_source.clone();
    let service2 = fixture.service_with_cache(
        session,
        external_source,
        ImageLimits::default(),
        Some((store2.clone(), scope2.clone(), true)),
    );
    let chart_offline_sink = ImageSink::new();
    service2.start(
        ImageRequest {
            scope: test_scope(2),
            key: ImageKey::new("chart-svg-offline"),
            target: repository_request(COMMIT, "chart.svg"),
        },
        chart_offline_sink.clone(),
    );
    let chart_offline = next_completion(&chart_offline_sink)
        .await
        .result
        .expect("cached svg offline");
    assert_eq!((chart_offline.width, chart_offline.height), (800, 533));
    let stats_offline_sink = ImageSink::new();
    service2.start(
        ImageRequest {
            scope: test_scope(2),
            key: ImageKey::new("stats-svg-offline"),
            target: ImageTarget::External {
                url: "https://stats.invalid/card.svg".into(),
            },
        },
        stats_offline_sink.clone(),
    );
    let stats_offline = next_completion(&stats_offline_sink)
        .await
        .result
        .expect("cached animation re-evaluates offline");
    assert!(stats_offline.animation.is_some());
    assert_eq!(stats_offline.bytes, stats.bytes);
    assert_eq!(
        offline_external.reads.lock().unwrap().len(),
        external_reads,
        "fresh cache avoids external network"
    );

    // Credential rejection quarantines private repository SVG cache reads too.
    store2
        .access_event(
            &scope2,
            crate::storage::cache::AccessEvent::CredentialRejected,
        )
        .unwrap();
    let denied_sink = ImageSink::new();
    service2.start(
        ImageRequest {
            scope: test_scope(3),
            key: ImageKey::new("chart-svg-denied"),
            target: repository_request(COMMIT, "chart.svg"),
        },
        denied_sink.clone(),
    );
    let denied = next_completion(&denied_sink).await;
    assert_eq!(denied.result, Err(ImageFailure::StaleContext));
}
