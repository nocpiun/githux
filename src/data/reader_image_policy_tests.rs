use super::*;
#[test]
fn performance_older_cache_age_metadata_is_validated_once_without_dropping_bytes() {
    use crate::domain::cache::{CacheCompleteness, CacheMetadata};
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::CacheConfig;
    let dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(dir.path().join("cache"))).unwrap();
    let scope = store
        .saved_account(AccountKey::new(1, HostId::github()))
        .unwrap();
    let cache = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), false);
    let url = "https://fixture.invalid/old-age.png";
    let key = cache.external_image_key(url).unwrap();
    let now = now_ms();
    store
        .put(
            store.begin_write(&scope, &key).unwrap(),
            &png_bytes(),
            CacheMetadata {
                representation_version: 1,
                fetched_at_ms: now,
                verified_at_ms: now,
                completeness: CacheCompleteness::Complete,
                etag: Some("\"one\"".into()),
                last_modified: None,
                cache_control: Some("max-age=600, stale-while-revalidate=60, githux-age=0".into()),
            },
        )
        .unwrap();
    let old = cache.read_external_image(url).unwrap().unwrap();
    let freshness = ExternalFreshness::from_snapshot(&old);
    assert!(!freshness.is_fresh_at(now));
    assert_eq!(freshness.stale_window_remaining_ms(now), None);
    assert_eq!(freshness.etag.as_deref(), Some("\"one\""));
    cache
        .revalidate_external_image(
            url,
            old,
            &ExternalImageMetadata {
                age_secs: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    let current = cache.read_external_image(url).unwrap().unwrap();
    assert_eq!(current.bytes, png_bytes());
    assert!(ExternalFreshness::from_snapshot(&current).is_fresh_at(now_ms()));
}

#[tokio::test]
async fn performance_304_for_a_replaced_representation_refetches_without_old_validators() {
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
    let url = "https://fixture.invalid/replaced.png";
    cache
        .put_external_image(
            url,
            &png_bytes(),
            &ExternalImageMetadata {
                etag: Some("\"one\"".into()),
                cache_control: Some("max-age=0, stale-while-revalidate=60".into()),
                age_secs: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let (source, mut entered) = gated_external(ExternalImageResponse::NotModified {
        metadata: ExternalImageMetadata::default(),
    });
    let service = fixture.service_with_cache(
        fixture.session(),
        source.clone(),
        ImageLimits::default(),
        Some((store, scope, false)),
    );
    let sink = ImageSink::new().for_request(1);
    let target = ImageTarget::External { url: url.into() };
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: target.key(),
            target,
        },
        sink.clone(),
    );
    let first = events_until(&sink, false).await;
    assert!(matches!(&first[0].result, Ok(ImageUpdate::Ready(image)) if image.width == 6));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), entered.recv())
            .await
            .unwrap()
            .unwrap()
            .1
    );
    let mut replacement = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        12,
        6,
        image::Rgba([80, 20, 150, 255]),
    ))
    .write_to(&mut replacement, image::ImageFormat::Png)
    .unwrap();
    let replacement = replacement.into_inner();
    let metadata = ExternalImageMetadata {
        etag: Some("\"two\"".into()),
        cache_control: Some("max-age=600".into()),
        ..Default::default()
    };
    cache
        .put_external_image(url, &replacement, &metadata)
        .unwrap();
    source.gate.add_permits(1);
    assert!(
        !tokio::time::timeout(Duration::from_secs(5), entered.recv())
            .await
            .unwrap()
            .unwrap()
            .1,
        "retry must not carry the old validator"
    );
    *source.response.lock().unwrap() = Ok(ExternalImageResponse::Fetched {
        bytes: replacement.clone(),
        metadata,
    });
    source.gate.add_permits(1);
    let rest = events_until(&sink, true).await;
    assert!(
        rest.iter().any(
            |event| matches!(&event.result, Ok(ImageUpdate::Ready(image)) if image.width == 12)
        )
    );
    assert_eq!(
        cache.read_external_image(url).unwrap().unwrap().bytes,
        replacement
    );
}

#[tokio::test]
async fn performance_stale_without_permission_waits_for_validation() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::CacheConfig;
    for cc in [
        "max-age=0",
        "no-cache, max-age=600, stale-while-revalidate=60",
        "max-age=0, stale-while-revalidate=60, must-revalidate",
    ] {
        let fixture = fixture().await;
        fixture.login(1).await;
        let dir = tempfile::tempdir().unwrap();
        let store = CacheStore::open(CacheConfig::at(dir.path().join("cache"))).unwrap();
        let scope = store
            .saved_account(AccountKey::new(1, HostId::github()))
            .unwrap();
        let url = "https://fixture.invalid/policy.png";
        let cache = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), false);
        cache
            .put_external_image(
                url,
                &png_bytes(),
                &ExternalImageMetadata {
                    etag: Some("\"one\"".into()),
                    cache_control: Some(cc.into()),
                    age_secs: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        let (source, mut entered) = gated_external(ExternalImageResponse::Fetched {
            bytes: png_bytes(),
            metadata: ExternalImageMetadata {
                cache_control: Some("max-age=600".into()),
                ..Default::default()
            },
        });
        let service = fixture.service_with_cache(
            fixture.session(),
            source.clone(),
            ImageLimits::default(),
            Some((store, scope, false)),
        );
        let sink = ImageSink::new().for_request(1);
        let target = ImageTarget::External { url: url.into() };
        service.start(
            ImageRequest {
                scope: test_scope(1),
                key: target.key(),
                target,
            },
            sink.clone(),
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(5), entered.recv())
                .await
                .unwrap()
                .unwrap()
                .1
        );
        assert!(
            sink.drain_events().is_empty(),
            "policy {cc} cannot paint old bytes"
        );
        source.gate.add_permits(1);
        assert!(
            events_until(&sink, true)
                .await
                .iter()
                .any(|event| matches!(event.result, Ok(ImageUpdate::Ready(_))))
        );
    }
}
#[tokio::test]
async fn performance_no_store_revalidation_error_invalidates_previously_painted_bytes() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::CacheConfig;
    let fixture = fixture().await;
    fixture.login(1).await;
    let dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(dir.path().join("cache"))).unwrap();
    let scope = store
        .saved_account(AccountKey::new(1, HostId::github()))
        .unwrap();
    let url = "https://fixture.invalid/stale-policy.png";
    let cache = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), false);
    cache
        .put_external_image(
            url,
            &png_bytes(),
            &ExternalImageMetadata {
                etag: Some("\"one\"".into()),
                cache_control: Some("max-age=0, stale-while-revalidate=60".into()),
                age_secs: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let (source, mut entered) = gated_external(ExternalImageResponse::NotModified {
        metadata: ExternalImageMetadata::default(),
    });
    *source.response.lock().unwrap() = Err(ExternalImageError::NoStore(Box::new(
        ExternalImageError::Status(503),
    )));
    let service = fixture.service_with_cache(
        fixture.session(),
        source.clone(),
        ImageLimits::default(),
        Some((store, scope, false)),
    );
    let sink = ImageSink::new().for_request(1);
    let target = ImageTarget::External { url: url.into() };
    service.start(
        ImageRequest {
            scope: test_scope(1),
            key: target.key(),
            target,
        },
        sink.clone(),
    );
    let first = events_until(&sink, false).await;
    assert!(matches!(first[0].result, Ok(ImageUpdate::Ready(_))));
    tokio::time::timeout(Duration::from_secs(5), entered.recv())
        .await
        .unwrap()
        .unwrap();
    source.gate.add_permits(1);
    let terminal = events_until(&sink, true).await;
    assert!(matches!(
        terminal.last().unwrap().result,
        Err(ImageFailure::Invalidated)
    ));
    assert!(cache.read_external_image(url).unwrap().is_none());
}

#[test]
fn performance_external_policy_requires_explicit_stale_permission() {
    let policy = |cc: &str, age| ExternalFreshness {
        etag: Some("\"one\"".into()),
        last_modified: None,
        cache_control: Some(cc.into()),
        fetched_at_ms: 0,
        verified_at_ms: 1000,
        age_secs: age,
    };
    for (cc, age, fresh, stale) in [
        ("max-age=60", 0, true, false),
        ("max-age=60", 61, false, false),
        ("max-age=60, stale-while-revalidate=30", 61, false, true),
        ("max-age=60, stale-while-revalidate=30", 90, false, false),
        (
            "max-age=60, stale-while-revalidate=30, must-revalidate",
            61,
            false,
            false,
        ),
        (
            "max-age=60, stale-while-revalidate=30, no-cache",
            1,
            false,
            false,
        ),
        (
            "max-age=60, stale-while-revalidate=30, no-store",
            1,
            false,
            false,
        ),
        (
            "max-age=\"60\", stale-while-revalidate=\"30\"",
            61,
            false,
            true,
        ),
        ("MAX-AGE=60, STALE-WHILE-REVALIDATE=30", 61, false, true),
        (
            "max-age=60, max-age=600, stale-while-revalidate=30",
            61,
            false,
            false,
        ),
        ("max-age=bad, stale-while-revalidate=30", 61, false, false),
        ("x-no-cache, max-age=60", 1, true, false),
    ] {
        let p = policy(cc, age);
        assert_eq!(p.is_fresh_at(1000), fresh, "{cc}");
        assert_eq!(p.stale_window_remaining_ms(1000).is_some(), stale, "{cc}");
    }
    let p = policy("max-age=60, stale-while-revalidate=30", 59);
    assert!(p.is_fresh_at(1999));
    assert!(!p.is_fresh_at(2000));
    assert_eq!(p.stale_window_remaining_ms(2000), Some(30_000));
    assert_eq!(p.stale_window_remaining_ms(32_000), None);
}

struct GatedExternal {
    entered: tokio::sync::mpsc::UnboundedSender<(String, bool)>,
    gate: tokio::sync::Semaphore,
    response: Mutex<Result<ExternalImageResponse, ExternalImageError>>,
}
impl ExternalImageBytes for GatedExternal {
    fn load_with_metadata<'a>(
        &'a self,
        url: &'a str,
        _limits: &'a ExternalImageLimits,
        validation: ValidationHeaders<'a>,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalImageResponse, ExternalImageError>> + Send + 'a>>
    {
        Box::pin(async move {
            let _ = self
                .entered
                .send((url.to_string(), validation.if_none_match == Some("\"one\"")));
            tokio::select! {
                _ = cancel.cancelled() => return Err(ExternalImageError::Cancelled),
                permit = self.gate.acquire() => permit.unwrap().forget(),
            }
            self.response.lock().unwrap().clone()
        })
    }
}
fn gated_external(
    response: ExternalImageResponse,
) -> (
    Arc<GatedExternal>,
    tokio::sync::mpsc::UnboundedReceiver<(String, bool)>,
) {
    let (entered, waiting) = tokio::sync::mpsc::unbounded_channel();
    (
        Arc::new(GatedExternal {
            entered,
            gate: tokio::sync::Semaphore::new(0),
            response: Mutex::new(Ok(response)),
        }),
        waiting,
    )
}
async fn events_until(sink: &ImageSink, terminal: bool) -> Vec<crate::content::images::ImageEvent> {
    let mut events = Vec::new();
    for _ in 0..500 {
        events.extend(sink.drain_events());
        if if terminal {
            events.iter().any(|e| e.terminal)
        } else {
            !events.is_empty()
        } {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("gated publication did not arrive");
}
#[tokio::test]
async fn performance_fresh_cached_pixels_pass_three_real_gated_downloads() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::CacheConfig;
    let fixture = fixture().await;
    fixture.login(1).await;
    let dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(dir.path().join("cache"))).unwrap();
    let scope = store
        .saved_account(AccountKey::new(1, HostId::github()))
        .unwrap();
    let bytes = png_bytes();
    let cache = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), false);
    let url = "https://fixture.invalid/fresh.png";
    cache
        .put_external_image(
            url,
            &bytes,
            &ExternalImageMetadata {
                cache_control: Some("max-age=600".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (source, mut entered) = gated_external(ExternalImageResponse::Fetched {
        bytes,
        metadata: ExternalImageMetadata::default(),
    });
    let service = fixture.service_with_cache(
        fixture.session(),
        source.clone(),
        ImageLimits::default(),
        Some((store, scope, false)),
    );
    for i in 0..3 {
        let target = ImageTarget::External {
            url: format!("https://fixture.invalid/slow-{i}"),
        };
        service.start(
            ImageRequest {
                scope: test_scope(i + 1),
                key: target.key(),
                target,
            },
            ImageSink::new(),
        );
    }
    for _ in 0..3 {
        tokio::time::timeout(Duration::from_secs(5), entered.recv())
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(service.inner.permits.available_permits(), 0);
    let sink = ImageSink::new().for_request(10);
    let target = ImageTarget::External { url: url.into() };
    service.start(
        ImageRequest {
            scope: test_scope(10),
            key: target.key(),
            target,
        },
        sink.clone(),
    );
    let events = events_until(&sink, true).await;
    assert!(
        matches!(&events[0].result, Ok(ImageUpdate::Ready(decoded)) if decoded.pixels.is_some())
    );
    assert_eq!(
        service.inner.permits.available_permits(),
        0,
        "downloads are still gated"
    );
    for i in 1..=3 {
        service.cancel(test_scope(i));
    }
}
#[tokio::test]
async fn performance_swr_bad_cached_content_revalidates_to_a_valid_original() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::CacheConfig;
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(1, 1))
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    let good = output.into_inner();
    for case in 0..4 {
        let fixture = fixture().await;
        fixture.login(1).await;
        let dir = tempfile::tempdir().unwrap();
        let store = CacheStore::open(CacheConfig::at(dir.path().join("cache"))).unwrap();
        let scope = store
            .saved_account(AccountKey::new(1, HostId::github()))
            .unwrap();
        let cache = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), false);
        let url = "https://fixture.invalid/repaired.png";
        let mut limits = ImageLimits::default();
        let (bad, expected) = match case {
            0 => (
                b"unsupported old response".to_vec(),
                ImageFailure::Unsupported,
            ),
            1 => (b"\x89PNG\r\n\x1a\n".to_vec(), ImageFailure::Malformed),
            2 => {
                limits.max_response_bytes = good.len();
                (png_bytes(), ImageFailure::TooLarge { limit: good.len() })
            }
            _ => {
                limits.max_pixels = 1;
                (
                    png_bytes(),
                    ImageFailure::TooManyPixels {
                        width: 6,
                        height: 3,
                        limit: 1,
                    },
                )
            }
        };
        assert_eq!(
            crate::content::images::decode_image(bad.clone(), &limits).unwrap_err(),
            expected
        );
        cache
            .put_external_image(
                url,
                &bad,
                &ExternalImageMetadata {
                    etag: Some("\"one\"".into()),
                    cache_control: Some("max-age=0, stale-while-revalidate=60".into()),
                    age_secs: Some(0),
                    ..Default::default()
                },
            )
            .unwrap();
        let (source, mut network_entered) = gated_external(ExternalImageResponse::Fetched {
            bytes: good.clone(),
            metadata: ExternalImageMetadata {
                cache_control: Some("max-age=600".into()),
                age_secs: Some(0),
                ..Default::default()
            },
        });
        let service = fixture.service_with_cache(
            fixture.session(),
            source.clone(),
            limits,
            Some((store, scope, false)),
        );
        let sink = ImageSink::new().for_request(71 + case);
        let target = ImageTarget::External { url: url.into() };
        service.start(
            ImageRequest {
                scope: test_scope(71 + case),
                key: target.key(),
                target,
            },
            sink.clone(),
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(5), network_entered.recv())
                .await
                .unwrap()
                .unwrap()
                .1
        );
        source.gate.add_permits(1);
        let events = events_until(&sink, true).await;
        assert!(events.iter().any(|event| matches!(&event.result, Ok(ImageUpdate::Ready(decoded)) if decoded.width == 1 && decoded.height == 1)));
        assert!(events.iter().all(|event| event.result.is_ok()));
        assert_eq!(
            service
                .metrics()
                .stage(crate::content::images::ImageStage::Decode)
                .count,
            2
        );
        assert_eq!(cache.read_external_image(url).unwrap().unwrap().bytes, good);
    }
}

#[tokio::test]
async fn performance_swr_expiry_while_queued_skips_only_optional_pixels_and_revalidates() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::CacheConfig;
    for (case, same_200, revoke) in [(51, false, false), (52, true, false), (53, false, true)] {
        let fixture = fixture().await;
        fixture.login(1).await;
        let dir = tempfile::tempdir().unwrap();
        let store = CacheStore::open(CacheConfig::at(dir.path().join("cache"))).unwrap();
        let scope = store
            .saved_account(AccountKey::new(1, HostId::github()))
            .unwrap();
        let cache = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), false);
        let url = "https://fixture.invalid/queued-stale.png";
        cache
            .put_external_image(
                url,
                &png_bytes(),
                &ExternalImageMetadata {
                    etag: Some("\"one\"".into()),
                    cache_control: Some("max-age=0, stale-while-revalidate=3".into()),
                    age_secs: Some(0),
                    ..Default::default()
                },
            )
            .unwrap();
        let metadata = ExternalImageMetadata {
            cache_control: Some("max-age=600".into()),
            age_secs: Some(0),
            ..Default::default()
        };
        let response = if same_200 {
            ExternalImageResponse::Fetched {
                bytes: png_bytes(),
                metadata,
            }
        } else {
            ExternalImageResponse::NotModified { metadata }
        };
        let (source, mut network_entered) = gated_external(response);
        let service = fixture.service_with_cache(
            fixture.session(),
            source.clone(),
            ImageLimits::default(),
            Some((store, scope, false)),
        );
        let held = service
            .inner
            .decode_permits
            .clone()
            .acquire_many_owned(2)
            .await
            .unwrap();
        let (queued, at_queue) = tokio::sync::oneshot::channel();
        *service.inner.decode_queue_probe.lock().unwrap() = Some(queued);
        let sink = ImageSink::new().for_request(case);
        let target = ImageTarget::External { url: url.into() };
        service.start(
            ImageRequest {
                scope: test_scope(case),
                key: target.key(),
                target,
            },
            sink.clone(),
        );
        tokio::time::timeout(Duration::from_secs(5), at_queue)
            .await
            .unwrap()
            .unwrap();
        assert!(sink.drain_events().is_empty());
        if revoke {
            fixture.login(2).await;
        } else {
            tokio::time::sleep(Duration::from_millis(3100)).await;
        }
        drop(held);
        if revoke {
            let events = events_until(&sink, true).await;
            assert!(
                events
                    .iter()
                    .any(|event| matches!(&event.result, Err(ImageFailure::StaleContext)))
            );
            assert!(
                network_entered.try_recv().is_err(),
                "retired authorization still stops before revalidation"
            );
            continue;
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(5), network_entered.recv())
                .await
                .unwrap()
                .unwrap()
                .1,
            "expired provisional decode must still reach conditional revalidation"
        );
        source.gate.add_permits(1);
        let events = events_until(&sink, true).await;
        assert!(events.iter().any(
            |event| matches!(&event.result, Ok(ImageUpdate::Ready(decoded)) if decoded.width == 6)
        ));
        assert!(events.iter().all(|event| event.result.is_ok()));
        assert_eq!(
            service
                .metrics()
                .stage(crate::content::images::ImageStage::Decode)
                .count,
            2,
            "expired queued attempt then drawable validated bytes"
        );
        assert_eq!(
            cache.read_external_image(url).unwrap().unwrap().bytes,
            png_bytes()
        );
        assert!(service.verify());
    }
}

#[tokio::test]
async fn performance_permitted_swr_paints_before_304_and_does_not_decode_twice() {
    use crate::domain::{AccountKey, HostId};
    use crate::storage::cache::CacheConfig;
    let fixture = fixture().await;
    fixture.login(1).await;
    let dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(dir.path().join("cache"))).unwrap();
    let scope = store
        .saved_account(AccountKey::new(1, HostId::github()))
        .unwrap();
    let url = "https://fixture.invalid/stale.png";
    let cache = ReaderImageCache::new(store.clone(), scope.clone(), Some(42), false);
    cache
        .put_external_image(
            url,
            &png_bytes(),
            &ExternalImageMetadata {
                etag: Some("\"one\"".into()),
                cache_control: Some("max-age=0, stale-while-revalidate=60".into()),
                age_secs: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let (source, mut entered) = gated_external(ExternalImageResponse::NotModified {
        metadata: ExternalImageMetadata {
            cache_control: Some("max-age=600".into()),
            ..Default::default()
        },
    });
    let service = fixture.service_with_cache(
        fixture.session(),
        source.clone(),
        ImageLimits::default(),
        Some((store, scope, false)),
    );
    let sink = ImageSink::new().for_request(20);
    let target = ImageTarget::External { url: url.into() };
    service.start(
        ImageRequest {
            scope: test_scope(20),
            key: target.key(),
            target,
        },
        sink.clone(),
    );
    let first = events_until(&sink, false).await;
    assert_eq!(first.len(), 1);
    assert!(matches!(first[0].result, Ok(ImageUpdate::Ready(_))));
    assert!(!first[0].terminal);
    assert!(first[0].validity.as_ref().unwrap().expires.is_some());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), entered.recv())
            .await
            .unwrap()
            .unwrap()
            .1
    );
    source.gate.add_permits(1);
    let rest = events_until(&sink, true).await;
    assert!(matches!(rest[0].result, Ok(ImageUpdate::Validated)));
    assert_eq!(
        service
            .metrics()
            .stage(crate::content::images::ImageStage::Decode)
            .count,
        1
    );
    assert!(
        rest.iter()
            .all(|e| e.validity.as_ref().is_none_or(|v| v.expires.is_none()))
    );
}
