use super::*;
fn assert_good(completion: ImageCompletion) {
    let image = completion.result.expect("shared image succeeds");
    assert_eq!((image.width, image.height), (6, 3));
}
async fn subscribers_ready(read: impl Fn() -> usize, expected: usize) {
    for _ in 0..500 {
        if read() == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("shared read did not acquire its independent subscribers");
}
fn sibling_routes(fixture: &Fixture, bytes: &[u8]) {
    fixture
        .transport
        .serve(&format!("/repos/octo/repo/commits/{COMMIT}"), commit_json());
    fixture.transport.serve(&format!("/repos/octo/repo/git/trees/{ROOT}"), tree(ROOT, json!([
        {"path":"a.png", "mode":"100644", "type":"blob", "sha":IMAGE_BLOB, "size":bytes.len()},
        {"path":"b.png", "mode":"100644", "type":"blob", "sha":IMAGE_BLOB, "size":bytes.len()}
    ])));
    fixture.transport.serve(
        &format!("/repos/octo/repo/git/blobs/{IMAGE_BLOB}"),
        blob_json(IMAGE_BLOB, bytes),
    );
}
fn start_alias(service: &RepositoryImageService, path: &str, generation: u64) -> ImageSink {
    let target = repository_request(COMMIT, path);
    let sink = ImageSink::new();
    service.start(
        ImageRequest {
            scope: test_scope(generation),
            key: target.key(),
            target,
        },
        sink.clone(),
    );
    sink
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn performance_siblings_share_metadata_and_blob_without_cancellation_poisoning() {
    let fixture = fixture().await;
    fixture.login(1).await;
    sibling_routes(&fixture, &png_bytes());
    let (entered, release) = fixture
        .transport
        .gate(&format!("/repos/octo/repo/commits/{COMMIT}"));
    let service = fixture.service(
        fixture.session(),
        Arc::new(MemoryExternal::default()),
        ImageLimits::default(),
    );
    let cancelled = start_alias(&service, "a.png", 1);
    request_entered(entered).await;
    let live = start_alias(&service, "b.png", 2);
    subscribers_ready(|| service.inner.snapshots.waiters(COMMIT), 2).await;
    service.cancel(test_scope(1));
    release.send(()).unwrap();
    assert_good(next_completion(&live).await);
    assert!(cancelled.drain().is_empty());
    assert_eq!(
        fixture.transport.urls().len(),
        3,
        "one commit, root and blob"
    );
    let (entered, release) = fixture
        .transport
        .gate(&format!("/repos/octo/repo/git/blobs/{IMAGE_BLOB}"));
    let a = start_alias(&service, "a.png", 3);
    request_entered(entered).await;
    let b = start_alias(&service, "b.png", 4);
    subscribers_ready(|| service.inner.blobs.waiters(IMAGE_BLOB), 2).await;
    release.send(()).unwrap();
    assert_good(next_completion(&a).await);
    assert_good(next_completion(&b).await);
    assert_eq!(
        fixture.transport.urls().len(),
        4,
        "aliases share a single immutable blob transfer"
    );
    assert_eq!(service.metrics().stage(ImageStage::Metadata).count, 2);
}
#[tokio::test]
async fn performance_failed_shared_entries_do_not_block_explicit_retry() {
    let fixture = fixture().await;
    fixture.login(1).await;
    fixture.transport.push(503, json!({"message":"temporary"}));
    let service = fixture.service(
        fixture.session(),
        Arc::new(MemoryExternal::default()),
        ImageLimits::default(),
    );
    let first = start_alias(&service, "a.png", 1);
    assert!(next_completion(&first).await.result.is_err());
    sibling_routes(&fixture, &png_bytes());
    let retry = start_alias(&service, "a.png", 2);
    assert_good(next_completion(&retry).await);
    assert_eq!(fixture.transport.urls().len(), 4);
}
