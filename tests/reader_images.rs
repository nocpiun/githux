//! Reader picture pipeline: the address boundary, the provider protocol,
//! capability retirement, budgets, scope isolation, and the real
//! `RepositoryImageService` over the deterministic fixture transport.
//!
//! The reader-side tests use the exact `ImageBridge` the NSH view drives; the
//! service test uses the real account/repository/blob/image path, with only the
//! transport and external-picture boundaries replaced in memory.

#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;

#[path = "../src/bin_support/reader_images_modules.rs"]
mod reader_images_modules;
#[allow(unused_imports)]
use reader_images_modules::{fixture, reader};

use crate::content::images::{
    DecodedImage, ImageCompletion, ImageFailure, ImageKey, ImageLimits, ImageProvider,
    ImageRequest, ImageScope, ImageSink, ImageTarget, decode_image,
};
use crate::reader::images::{ImageBridge, Picture, resolve};
use crate::reader::{ResourceContext, fixtures};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

fn context() -> ResourceContext {
    ResourceContext::new(
        fixtures::OWNER,
        fixtures::NAME,
        fixtures::COMMIT,
        fixtures::PATH,
    )
}

fn limits(max_ready_images: usize, max_in_flight: usize) -> ImageLimits {
    ImageLimits {
        max_ready_images,
        max_in_flight,
        ..ImageLimits::default()
    }
}

/// A provider that records what the bridge handed it and can be completed by
/// the test instead of by a network.
#[derive(Default)]
struct Provider {
    requests: Mutex<Vec<(ImageRequest, ImageSink)>>,
    cancelled: Mutex<Vec<ImageScope>>,
}

impl ImageProvider for Provider {
    fn start(&self, request: ImageRequest, sink: ImageSink) {
        self.requests.lock().unwrap().push((request, sink));
    }

    fn cancel(&self, scope: ImageScope) {
        self.cancelled.lock().unwrap().push(scope);
    }
}

impl Provider {
    fn take(&self) -> (ImageRequest, ImageSink) {
        self.requests.lock().unwrap().remove(0)
    }

    fn len(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

fn png(width: u32, height: u32) -> DecodedImage {
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(width, height)
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    decode_image(output.into_inner(), &ImageLimits::default()).expect("a real decodable picture")
}

fn finish(sink: &ImageSink, scope: ImageScope, key: ImageKey) {
    sink.push(ImageCompletion {
        scope,
        key,
        result: Ok(png(2, 2)),
    });
}

fn bridge(provider: Arc<Provider>) -> ImageBridge {
    ImageBridge::new(Some(provider), Arc::new(|| true), ImageLimits::default())
}

#[test]
fn a_source_is_requested_once_and_starts_pending() {
    let provider = Arc::new(Provider::default());
    let mut bridge = bridge(provider.clone());
    assert_eq!(bridge.picture("images/a.png", &context()), Picture::Pending);
    // Rendering another frame asks for the same source again: no duplicate.
    assert_eq!(bridge.picture("images/a.png", &context()), Picture::Pending);
    // Only the host drain hands work to the provider.
    assert_eq!(provider.len(), 0);
    bridge.poll();
    assert_eq!(provider.len(), 1);
    let (request, _) = provider.take();
    assert!(matches!(
        request.target,
        ImageTarget::Repository { ref path, ref commit, .. }
            if path == "docs/images/a.png" && commit == fixtures::COMMIT
    ));
    assert_eq!(request.scope, bridge.scope());
}

#[test]
fn without_a_provider_a_source_is_unavailable_and_never_requested() {
    let mut bridge = ImageBridge::new(None, Arc::new(|| true), ImageLimits::default());
    assert_eq!(
        bridge.picture("images/a.png", &context()),
        Picture::Failed(ImageFailure::NoProvider)
    );
    assert!(!bridge.poll());
    assert!(!bridge.has_work());
}

#[test]
fn the_in_flight_budget_bounds_outstanding_requests() {
    let provider = Arc::new(Provider::default());
    let mut bridge = ImageBridge::new(Some(provider.clone()), Arc::new(|| true), limits(4, 2));
    for name in ["a", "b", "c", "d"] {
        bridge.picture(&format!("images/{name}.png"), &context());
    }
    bridge.poll();
    assert_eq!(provider.len(), 2);
    assert!(bridge.has_work());
}

#[test]
fn a_ready_picture_is_provider_validated_bytes_and_keeps_dimensions() {
    let provider = Arc::new(Provider::default());
    let mut bridge = bridge(provider.clone());
    bridge.picture("images/a.png", &context());
    bridge.poll();
    let (request, sink) = provider.take();
    sink.push(ImageCompletion {
        scope: request.scope,
        key: request.key.clone(),
        result: Ok(png(3, 5)),
    });
    assert!(bridge.poll());
    let picture = bridge.picture("images/a.png", &context());
    let Picture::Ready {
        source,
        width,
        height,
    } = picture
    else {
        panic!("expected a ready picture, got {picture:?}");
    };
    assert!(source.starts_with("data:image/png;base64,"));
    assert_eq!((width, height), (3, 5));
}

#[test]
fn an_oversized_decoded_picture_is_refused_and_reported() {
    let provider = Arc::new(Provider::default());
    let mut bridge = ImageBridge::new(
        Some(provider.clone()),
        Arc::new(|| true),
        ImageLimits {
            max_ready_bytes: 4,
            ..ImageLimits::default()
        },
    );
    bridge.picture("images/a.png", &context());
    bridge.poll();
    let (request, sink) = provider.take();
    finish(&sink, request.scope, request.key);
    bridge.poll();
    assert!(matches!(
        bridge.picture("images/a.png", &context()),
        Picture::Failed(ImageFailure::TooLarge { .. })
    ));
}

#[test]
fn a_completion_from_another_generation_is_dropped() {
    let provider = Arc::new(Provider::default());
    let mut bridge = bridge(provider.clone());
    bridge.picture("images/a.png", &context());
    bridge.poll();
    let (old_request, sink) = provider.take();
    let old_scope = bridge.scope();
    bridge.suspend();
    assert_ne!(bridge.scope(), old_scope);
    finish(&sink, old_scope, old_request.key);
    bridge.poll();
    // The hidden generation's late result cannot fill the new generation.
    assert_eq!(bridge.picture("images/a.png", &context()), Picture::Pending);
}

#[test]
fn suspending_keeps_ready_pictures_and_redemands_the_rest() {
    let provider = Arc::new(Provider::default());
    let mut bridge = bridge(provider.clone());
    bridge.picture("images/ready.png", &context());
    bridge.picture("images/pending.png", &context());
    bridge.poll();
    let mut requests = std::mem::take(&mut *provider.requests.lock().unwrap());
    let (ready_request, ready_sink) = requests.remove(0);
    finish(&ready_sink, ready_request.scope, ready_request.key);
    bridge.poll();
    assert!(matches!(
        bridge.picture("images/ready.png", &context()),
        Picture::Ready { .. }
    ));
    bridge.suspend();
    // Ready survives hiding; the unfinished source is demanded again and the
    // old scope was cancelled.
    assert!(matches!(
        bridge.picture("images/ready.png", &context()),
        Picture::Ready { .. }
    ));
    assert_eq!(
        bridge.picture("images/pending.png", &context()),
        Picture::Pending
    );
    assert!(
        provider
            .cancelled
            .lock()
            .unwrap()
            .contains(&ready_request.scope)
    );
}

#[test]
fn the_decoded_budget_releases_the_oldest_picture_without_refetching_it() {
    let provider = Arc::new(Provider::default());
    let mut bridge = ImageBridge::new(Some(provider.clone()), Arc::new(|| true), limits(1, 2));
    for name in ["first", "second"] {
        bridge.picture(&format!("images/{name}.png"), &context());
    }
    bridge.poll();
    for _ in 0..2 {
        let (request, sink) = provider.take();
        finish(&sink, request.scope, request.key);
        bridge.poll();
    }
    // One slot: the first picture was released instead of being drawn again.
    assert_eq!(
        bridge.picture("images/first.png", &context()),
        Picture::Released
    );
    assert!(matches!(
        bridge.picture("images/second.png", &context()),
        Picture::Ready { .. }
    ));
    // A frame that still draws the released block must not queue it again.
    bridge.poll();
    assert!(!bridge.has_work());
    assert_eq!(provider.len(), 0);
    // Hiding and resuming demands it again.
    bridge.suspend();
    assert_eq!(
        bridge.picture("images/first.png", &context()),
        Picture::Pending
    );
}

#[test]
fn resolving_a_picture_uses_the_link_boundary() {
    for raw in [
        "file:///secret.png",
        "data:image/png;base64,abc",
        "//host/a.png",
        "http://host/a.png",
        "\\\\server\\a.png",
        "#frag",
        "",
    ] {
        assert!(
            matches!(resolve(raw, &context()), Err(ImageFailure::Refused(_))),
            "{raw} must never become a request"
        );
    }
    assert!(matches!(
        resolve("images/a.png", &context()),
        Ok(ImageTarget::Repository { .. })
    ));
    assert!(matches!(
        resolve("https://example.com/a.png", &context()),
        Ok(ImageTarget::External { .. })
    ));
}

#[test]
fn the_same_path_at_two_commits_is_two_sources() {
    let first = resolve("images/a.png", &context()).unwrap().key();
    let other = ResourceContext::new(
        fixtures::OWNER,
        fixtures::NAME,
        "2222222222222222222222222222222222222222",
        fixtures::PATH,
    );
    let second = resolve("images/a.png", &other).unwrap().key();
    assert_ne!(first, second);
}

#[test]
fn a_capability_retirement_clears_displayed_and_in_flight_work_permanently() {
    let provider = Arc::new(Provider::default());
    let allowed = Arc::new(AtomicBool::new(true));
    let gate = allowed.clone();
    let mut bridge = ImageBridge::new(
        Some(provider.clone()),
        Arc::new(move || gate.load(Ordering::Acquire)),
        ImageLimits::default(),
    );
    bridge.picture("images/ready.png", &context());
    bridge.poll();
    let (request, sink) = provider.take();
    finish(&sink, request.scope, request.key);
    bridge.poll();
    assert!(matches!(
        bridge.picture("images/ready.png", &context()),
        Picture::Ready { .. }
    ));
    // An in-flight second picture exists when the capability is revoked.
    bridge.picture("images/late.png", &context());
    bridge.poll();
    let (late, late_sink) = provider.take();

    allowed.store(false, Ordering::Release);
    assert!(bridge.poll());
    assert!(bridge.is_retired());
    // Already displayed data is gone, not just new requests.
    assert_eq!(
        bridge.picture("images/ready.png", &context()),
        Picture::Failed(ImageFailure::StaleContext)
    );
    // A late completion after retirement cannot revive anything.
    finish(&late_sink, late.scope, late.key);
    bridge.poll();
    allowed.store(true, Ordering::Release);
    bridge.poll();
    assert_eq!(
        bridge.picture("images/ready.png", &context()),
        Picture::Failed(ImageFailure::StaleContext)
    );
    assert!(provider.cancelled.lock().unwrap().contains(&late.scope));
}

#[test]
fn two_readers_showing_the_same_picture_are_independent() {
    let provider = Arc::new(Provider::default());
    let mut first = bridge(provider.clone());
    let mut second = bridge(provider.clone());
    assert_ne!(first.scope().reader, second.scope().reader);
    first.picture("images/a.png", &context());
    second.picture("images/a.png", &context());
    first.poll();
    second.poll();
    let scope = first.scope();
    drop(first);
    // Dropping one reader cancels only its own scope.
    assert_eq!(provider.cancelled.lock().unwrap().as_slice(), &[scope]);
    assert!(matches!(
        second.picture("images/a.png", &context()),
        Picture::Pending
    ));
}

#[test]
fn the_real_repository_image_service_loads_through_the_bridge() {
    let pipeline = fixture::Pipeline::new(fixture::Scene::Healthy);
    let opened = pipeline.open(fixture::Scene::Healthy, ImageLimits::default());
    let mut bridge = ImageBridge::new(
        Some(opened.provider.clone() as Arc<dyn ImageProvider>),
        Arc::new(|| true),
        ImageLimits::default(),
    );
    // The fixture README's first relative picture, resolved against the fixed
    // commit the document itself came from.
    assert!(matches!(
        bridge.picture("images/diagram.png", &opened.context),
        Picture::Pending
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        bridge.poll();
        if matches!(
            bridge.picture("images/diagram.png", &opened.context),
            Picture::Ready { .. }
        ) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the fixture service never completed: {:?}",
            bridge.picture("images/diagram.png", &opened.context)
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let Picture::Ready {
        source,
        width,
        height,
    } = bridge.picture("images/diagram.png", &opened.context)
    else {
        unreachable!()
    };
    assert!(source.starts_with("data:image/png;base64,"));
    assert!(width > 0 && height > 0);
    assert!(
        pipeline.transport().mismatches().is_empty(),
        "{:?}",
        pipeline.transport().mismatches()
    );
    assert_eq!(opened.layout, crate::reader::ReaderLayout::Repository);
}
