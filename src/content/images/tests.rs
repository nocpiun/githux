//! Bounded decode, format sniffing and the request contract.

use super::*;

fn scope(generation: u64) -> ImageScope {
    ImageScope {
        reader: 1,
        generation,
    }
}

fn encoded(format: image::ImageFormat, width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
    });
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, format)
        .expect("encode fixture");
    cursor.into_inner()
}

fn png(width: u32, height: u32) -> Vec<u8> {
    encoded(image::ImageFormat::Png, width, height)
}

#[test]
fn formats_are_detected_from_content_not_extension() {
    assert_eq!(detect_format(&png(2, 2)), Some(ImageFormat::Png));
    assert_eq!(
        detect_format(&encoded(image::ImageFormat::Jpeg, 2, 2)),
        Some(ImageFormat::Jpeg)
    );
    assert_eq!(
        detect_format(&encoded(image::ImageFormat::Gif, 2, 2)),
        Some(ImageFormat::Gif)
    );
    assert_eq!(
        detect_format(&encoded(image::ImageFormat::WebP, 2, 2)),
        Some(ImageFormat::Webp)
    );
    assert_eq!(detect_format(b"<svg></svg>"), None);
    assert_eq!(detect_format(b""), None);
}

#[test]
fn a_real_png_decodes_with_its_intrinsic_size() {
    let limits = ImageLimits::default();
    let decoded = decode_image(png(37, 19), &limits).expect("valid png");
    assert_eq!(decoded.format, ImageFormat::Png);
    assert_eq!((decoded.width, decoded.height), (37, 19));
    assert_eq!(decoded.bytes.len(), png(37, 19).len());
    assert_eq!(decoded.decoded_bytes(), 37 * 19 * 4);
}

#[test]
fn every_supported_format_decodes() {
    let limits = ImageLimits::default();
    for format in [
        image::ImageFormat::Png,
        image::ImageFormat::Jpeg,
        image::ImageFormat::WebP,
        image::ImageFormat::Gif,
    ] {
        let decoded = decode_image(encoded(format, 8, 8), &limits)
            .unwrap_or_else(|e| panic!("{format:?}: {e:?}"));
        assert_eq!((decoded.width, decoded.height), (8, 8), "{format:?}");
    }
}

#[test]
fn svg_is_unsupported_and_never_executed() {
    let limits = ImageLimits::default();
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec();
    assert!(is_svg_like(&svg));
    assert_eq!(decode_image(svg, &limits), Err(ImageFailure::Unsupported));
    assert_eq!(
        decode_image(b"<?xml version=\"1.0\"?><svg/>".to_vec(), &limits),
        Err(ImageFailure::Unsupported)
    );
}

#[test]
fn garbage_and_truncated_payloads_are_malformed() {
    let limits = ImageLimits::default();
    assert_eq!(
        decode_image(b"not an image".to_vec(), &limits),
        Err(ImageFailure::Unsupported)
    );
    // A correct PNG signature with no decodable body.
    let mut signature = png(4, 4);
    signature.truncate(12);
    assert_eq!(
        decode_image(signature, &limits),
        Err(ImageFailure::Malformed)
    );
}

#[test]
fn an_oversized_encoded_body_is_rejected_before_decode() {
    let limits = ImageLimits {
        max_response_bytes: 32,
        ..ImageLimits::default()
    };
    assert_eq!(
        decode_image(png(8, 8), &limits),
        Err(ImageFailure::TooLarge { limit: 32 })
    );
}

/// A GIF logical screen descriptor carries the canvas size without a CRC, so a
/// hostile header can be built cheaply. The reader must reject it from the
/// header, before any pixel buffer exists.
#[test]
fn a_huge_header_is_rejected_before_any_allocation() {
    fn gif_with_canvas(width: u16, height: u16) -> Vec<u8> {
        // A real 2x2 GIF, then the logical-screen size is rewritten: a GIF
        // header carries no CRC, so a hostile canvas size costs nothing to
        // send. The rest of the payload stays valid.
        let mut bytes = encoded(image::ImageFormat::Gif, 2, 2);
        bytes[6..8].copy_from_slice(&width.to_le_bytes());
        bytes[8..10].copy_from_slice(&height.to_le_bytes());
        bytes
    }

    let limits = ImageLimits::default();
    assert_eq!(
        decode_image(gif_with_canvas(20_000, 20_000), &limits),
        Err(ImageFailure::TooManyPixels {
            width: 20_000,
            height: 20_000,
            limit: limits.max_pixels,
        })
    );
    // Past the single-edge limit as well.
    assert_eq!(
        decode_image(gif_with_canvas(9_000, 10), &limits),
        Err(ImageFailure::TooManyPixels {
            width: 9_000,
            height: 10,
            limit: limits.max_pixels,
        })
    );
}

#[test]
fn the_decoded_buffer_budget_is_enforced() {
    let limits = ImageLimits {
        max_decoded_bytes: 4_000,
        max_pixels: 1_000_000,
        ..ImageLimits::default()
    };
    assert_eq!(
        decode_image(png(64, 64), &limits),
        Err(ImageFailure::TooLarge { limit: 4_000 })
    );
}

#[test]
fn the_pixel_budget_is_enforced() {
    let limits = ImageLimits {
        max_pixels: 100,
        ..ImageLimits::default()
    };
    assert_eq!(
        decode_image(png(20, 20), &limits),
        Err(ImageFailure::TooManyPixels {
            width: 20,
            height: 20,
            limit: 100,
        })
    );
}

#[test]
fn keys_are_stable_and_separate_commit_and_source() {
    let a = ImageTarget::Repository {
        owner: "o".into(),
        name: "n".into(),
        commit: "a".repeat(40),
        path: "docs/a.png".into(),
    };
    let same = a.clone();
    assert_eq!(a.key(), same.key());
    let other_commit = ImageTarget::Repository {
        owner: "o".into(),
        name: "n".into(),
        commit: "b".repeat(40),
        path: "docs/a.png".into(),
    };
    assert_ne!(a.key(), other_commit.key());
    let external = ImageTarget::External {
        url: "https://example.com/a.png".into(),
    };
    assert_ne!(a.key(), external.key());
    assert_eq!(external.key().as_str(), "url:https://example.com/a.png");
}

#[test]
fn refusal_and_failure_labels_are_stable() {
    assert_eq!(
        ImageFailure::Refused(ImageRefusal::InsecureExternal).label(),
        "only HTTPS pictures load"
    );
    assert!(ImageFailure::Network.is_retryable());
    assert!(!ImageFailure::Unsupported.is_retryable());
}

#[test]
fn the_sink_keeps_completions_until_drained() {
    let sink = ImageSink::new();
    let key = ImageKey::new("url:https://example.com/a.png");
    sink.push(ImageCompletion {
        scope: scope(3),
        key: key.clone(),
        result: Err(ImageFailure::Network),
    });
    // A second producer on another thread is not lost either.
    let other = sink.clone();
    std::thread::spawn(move || {
        other.push(ImageCompletion {
            scope: scope(3),
            key,
            result: Err(ImageFailure::Timeout),
        });
    })
    .join()
    .expect("producer");
    let mut drained = sink.drain();
    drained.sort_by_key(|completion| format!("{:?}", completion.result));
    assert_eq!(drained.len(), 2);
    assert!(
        drained
            .iter()
            .all(|completion| completion.scope == scope(3))
    );
    assert!(sink.drain().is_empty());
}

#[test]
fn pending_requests_are_bounded_and_keep_order() {
    let mut pending = PendingRequests::default();
    for index in 0..5 {
        pending.push(ImageRequest {
            scope: scope(1),
            key: ImageKey::new(format!("k{index}")),
            target: ImageTarget::External {
                url: format!("https://example.com/{index}.png"),
            },
        });
    }
    let taken = pending.take(2);
    assert_eq!(
        taken.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        ["k0", "k1"]
    );
    assert!(!pending.is_empty());
    let rest = pending.take(10);
    assert_eq!(rest.len(), 3);
    assert!(pending.is_empty());
}
