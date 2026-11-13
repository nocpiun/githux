//! Expired provisional pixels must recover through real validation/cache/decode.
use super::*;
use crate::{
    data::{reader_image_cache::ReaderImageCache, reader_images::RepositoryImageService},
    domain::repository::{RepositoryIdentity, RepositoryLocator},
    network::{
        cancel::CancellationToken,
        reader_images::{
            ExternalImageBytes, ExternalImageError, ExternalImageLimits, ExternalImageMetadata,
            ExternalImageResponse, ValidationHeaders,
        },
    },
};
use std::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};
struct DelayedValidation {
    bytes: Arc<Vec<u8>>,
    same_200: bool,
    reads: AtomicUsize,
    gate: tokio::sync::Semaphore,
}
impl ExternalImageBytes for DelayedValidation {
    fn load_with_metadata<'a>(
        &'a self,
        _: &'a str,
        _: &'a ExternalImageLimits,
        _: ValidationHeaders<'a>,
        _: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalImageResponse, ExternalImageError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.gate.acquire().await.unwrap().forget();
            let metadata = ExternalImageMetadata {
                etag: Some("\"original\"".into()),
                cache_control: Some("max-age=600".into()),
                age_secs: Some(0),
                ..Default::default()
            };
            Ok(if self.same_200 {
                ExternalImageResponse::Fetched {
                    bytes: (*self.bytes).clone(),
                    metadata,
                }
            } else {
                ExternalImageResponse::NotModified { metadata }
            })
        })
    }
}
fn expired_validation(cx: &mut TestAppContext, same_200: bool) {
    let limits = ImageLimits::default();
    let mut harness = PerformanceFixture::new(false, limits.clone());
    let source = Arc::new(DelayedValidation {
        bytes: harness.source.bytes.clone(),
        same_200,
        reads: AtomicUsize::new(0),
        gate: tokio::sync::Semaphore::new(0),
    });
    let session = harness
        ._pipeline
        .accounts()
        .repo_service()
        .session()
        .unwrap();
    harness.service = Arc::new(RepositoryImageService::with_cache(
        session,
        RepositoryIdentity {
            id: 42,
            locator: RepositoryLocator::new(crate::fixture::OWNER, crate::fixture::NAME).unwrap(),
        },
        harness._pipeline.executor().clone(),
        source.clone(),
        limits.clone(),
        Some((harness.store.clone(), harness.scope.clone(), false)),
    ));
    let url = "https://originals.invalid/0.png";
    let cache = ReaderImageCache::new(
        harness.store.clone(),
        harness.scope.clone(),
        Some(42),
        false,
    );
    cache
        .put_external_image(
            url,
            &source.bytes,
            &ExternalImageMetadata {
                etag: Some("\"original\"".into()),
                cache_control: Some("max-age=0, stale-while-revalidate=3".into()),
                age_secs: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    let (harness, host, cx) = open_prepared(
        cx,
        harness,
        format!("![original]({url})"),
        false,
        false,
        limits,
    );
    let reader = host.read_with(cx, |host, _| host.reader.clone());
    let key = target(0).key();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while (reader
        .read_with(cx, |reader, _| reader.image_state(&key))
        .as_deref()
        != Some("ready")
        || source.reads.load(Ordering::SeqCst) == 0)
        && std::time::Instant::now() < deadline
    {
        frames(cx);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        reader
            .read_with(cx, |reader, _| reader.image_state(&key))
            .as_deref(),
        Some("ready"),
        "explicit SWR paints before validation"
    );
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while reader
        .read_with(cx, |reader, _| reader.image_state(&key))
        .as_deref()
        == Some("ready")
        && std::time::Instant::now() < deadline
    {
        frames(cx);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        reader
            .read_with(cx, |reader, _| reader.image_state(&key))
            .as_deref(),
        Some("authorization changed"),
        "permitted stale window really expires before response"
    );
    assert!(
        harness.service.verify(),
        "expiry does not invalidate the active session"
    );
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.image_memory_debug().0),
        0
    );
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    source.gate.add_permits(1);
    settle_current(&reader, cx);
    assert_eq!(
        reader
            .read_with(cx, |reader, _| reader.image_state(&key))
            .as_deref(),
        Some("ready"),
        "successful same-byte validation restores expired pixels"
    );
    assert!(cx.debug_bounds("markdown-picture-frame-0").is_some());
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "recovery decodes the renewed cache without another fetch"
    );
    assert_eq!(
        harness.service.metrics().stage(ImageStage::Decode).count,
        2,
        "one provisional decode plus one recovery after its pixels were retired"
    );
    assert_eq!(
        cache.read_external_image(url).unwrap().unwrap().bytes,
        *source.bytes
    );
    assert!(
        !reader.read_with(cx, |reader, _| reader.has_pending_images()),
        "late terminal event cannot leak an in-flight slot"
    );
    cx.update(|window, _| window.remove_window());
    drop(reader);
    drop(host);
    cx.run_until_parked();
    drop(harness);
}
#[gpui::test]
fn performance_expired_swr_304_restores_pixels_from_the_validated_cache(cx: &mut TestAppContext) {
    expired_validation(cx, false);
}
#[gpui::test]
fn performance_expired_swr_same_200_restores_pixels_from_the_validated_cache(
    cx: &mut TestAppContext,
) {
    expired_validation(cx, true);
}
