//! Deterministic stage probes; no live network or wall-clock pass thresholds.
use super::*;
#[path = "reader_image_concurrency_tests.rs"]
mod concurrency_tests;
#[path = "reader_image_memory_probe.rs"]
mod memory_probe;
#[path = "reader_image_policy_tests.rs"]
mod policy_tests;

#[tokio::test]
async fn performance_cached_picture_with_download_slots_occupied() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::CacheConfig;
    let fixture = fixture().await;
    fixture.login(1).await;
    let dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(dir.path().join("cache"))).unwrap();
    let scope = store
        .saved_account(AccountKey::new(1, HostId::github()))
        .unwrap();
    let cache = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), false);
    let url = "https://fixture.invalid/cached.png";
    cache
        .put_external_image(
            url,
            &png_bytes(),
            &ExternalImageMetadata {
                cache_control: Some("max-age=600".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let service = fixture.service_with_cache(
        fixture.session(),
        Arc::new(MemoryExternal::default()),
        ImageLimits::default(),
        Some((store, scope, false)),
    );
    let occupied = service
        .inner
        .permits
        .clone()
        .acquire_many_owned(3)
        .await
        .unwrap();
    let sink = ImageSink::new();
    let target = ImageTarget::External { url: url.into() };
    let started = std::time::Instant::now();
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: target.key(),
            target,
        },
        sink.clone(),
    );
    let completion = next_completion(&sink).await;
    eprintln!(
        "cached_probe: ready_while_network_gated=true elapsed_us={}",
        started.elapsed().as_micros()
    );
    assert!(completion.result.is_ok());
    drop(occupied);
}

#[test]
#[ignore = "explicit same-profile before/after stage measurement"]
fn performance_original_raster_and_svg_stage_measurement() {
    use crate::content::images::{RenderUpdate, decode_for_renderer};
    let rgba = image::RgbaImage::from_fn(2559, 1520, |x, y| {
        image::Rgba([
            ((x / 13 + y / 11) % 256) as u8,
            ((x / 7) % 256) as u8,
            ((y / 5) % 256) as u8,
            255,
        ])
    });
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    let started = std::time::Instant::now();
    let decoded = decode_for_renderer(
        bytes.clone(),
        &ImageLimits::default(),
        ImageRenderOptions::default(),
        &|| Ok(()),
        &mut |_| panic!("static raster"),
    )
    .unwrap()
    .unwrap();
    assert!(decoded.bytes.is_empty());
    let pixels = decoded
        .pixels
        .as_ref()
        .expect("one validated raster decode");
    assert_eq!((pixels.width, pixels.height), (2559, 1520));
    let provider_us = started.elapsed().as_micros();
    eprintln!(
        "raster_probe: encoded={} retained={} provider_us={} second_decode_us=0",
        bytes.len(),
        decoded.decoded_bytes(),
        provider_us
    );
    drop(decoded);
    let started = std::time::Instant::now();
    let last_check = std::cell::Cell::new(started);
    let check_spans = std::cell::RefCell::new(Vec::new());
    let resized = decode_for_renderer(
        bytes.clone(),
        &ImageLimits::default(),
        ImageRenderOptions::for_device_scale(2.0).with_display_width(600.0),
        &|| {
            let now = std::time::Instant::now();
            check_spans
                .borrow_mut()
                .push(now.duration_since(last_check.replace(now)).as_micros());
            Ok(())
        },
        &mut |_| panic!("static display raster"),
    )
    .unwrap()
    .unwrap();
    let pixels = resized.pixels.as_ref().unwrap();
    assert_eq!((pixels.width, pixels.height), (1200, 713));
    eprintln!(
        "raster_display_probe: encoded={} grid={}x{} retained={} provider_us={} second_decode_us=0",
        bytes.len(),
        pixels.width,
        pixels.height,
        resized.decoded_bytes(),
        started.elapsed().as_micros()
    );
    let mut largest_spans = check_spans.into_inner();
    largest_spans.sort_unstable_by(|a, b| b.cmp(a));
    largest_spans.truncate(5);
    eprintln!("raster_display_largest_cancel_check_intervals_us={largest_spans:?}");
    drop(resized);
    let started = std::time::Instant::now();
    let mut first_us = 0;
    let mut frames = 0;
    let mut retained = 0;
    let svg = decode_for_renderer(
        include_bytes!("../content/images/fixtures/entry-stats.svg").to_vec(),
        &ImageLimits::default(),
        ImageRenderOptions::default(),
        &|| Ok(()),
        &mut |update| {
            match update {
                RenderUpdate::First(decoded) => {
                    first_us = started.elapsed().as_micros();
                    retained = decoded.decoded_bytes();
                    frames += decoded.animation.unwrap().frames.len();
                }
                RenderUpdate::Frames(batch) => {
                    frames += batch.len();
                    retained += batch.iter().map(|f| f.pixels.bgra.len()).sum::<usize>();
                }
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(svg.is_none());
    eprintln!(
        "svg_probe: first_us={} complete_us={} frames={} retained={}",
        first_us,
        started.elapsed().as_micros(),
        frames,
        retained
    );
}
