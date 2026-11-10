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
    assert!(is_svg_like(b"<svg></svg>"));
    assert!(is_svg_like(
        b"\xEF\xBB\xBF<?xml version=\"1.0\" encoding=\"UTF-8\"?><svg viewBox=\"0 0 10 10\"/>"
    ));
    assert!(is_svg_like(
        b"<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\"><svg/>"
    ));
    assert!(!is_svg_like(
        b"<?xml version=\"1.0\"?><rss version=\"2.0\"/>"
    ));
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
fn static_svg_badge_stats_card_and_chart_rasterize_to_png_with_dimensions() {
    let limits = ImageLimits::default();

    // Representative shields.io-style badge (uses linearGradient, clipPath, text, scale(.1), textLength).
    let badge = br##"<svg xmlns="http://www.w3.org/2000/svg" width="88" height="20" role="img" aria-label="build: passing">
        <linearGradient id="s" x2="0" y2="100%">
            <stop offset="0" stop-color="#bbb" stop-opacity=".1"/>
            <stop offset="1" stop-opacity=".1"/>
        </linearGradient>
        <clipPath id="r"><rect width="88" height="20" rx="3"/></clipPath>
        <g clip-path="url(#r)">
            <rect width="37" height="20" fill="#555"/>
            <rect x="37" width="51" height="20" fill="#4b0"/>
            <rect width="88" height="20" fill="url(#s)"/>
        </g>
        <g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="110">
            <text transform="scale(.1)" x="195" y="140" textLength="270">build</text>
            <text transform="scale(.1)" x="615" y="140" textLength="410">passing</text>
        </g>
    </svg>"##;
    let decoded_badge = decode_image(badge.to_vec(), &limits).expect("badge svg");
    assert_eq!(decoded_badge.format, ImageFormat::Png);
    assert_eq!((decoded_badge.width, decoded_badge.height), (88, 20));
    let badge_png = image::load_from_memory(&decoded_badge.bytes).expect("valid png");
    assert!(badge_png.width() >= 88 && badge_png.height() >= 20);
    // The PNG is supersampled, so the reader budget must charge its real grid
    // rather than only the logical layout size.
    assert!(
        decoded_badge.decoded_bytes()
            == (badge_png.width() as usize) * (badge_png.height() as usize) * 4,
        "decoded budget follows the raster grid: {}",
        decoded_badge.decoded_bytes()
    );
    assert!(decoded_badge.decoded_bytes() > 88 * 20 * 4);
    // Verify non-empty rendered pixels (both left gray and right green badge fills).
    let badge_rgba = badge_png.to_rgba8();
    assert!(badge_rgba.pixels().any(|p| p[3] > 0 && p[1] > p[0]));

    // Representative GitHub README stats-card SVG with XML declaration, viewBox only, and text/progress rings.
    let stats_card = br##"<?xml version="1.0" encoding="UTF-8"?>
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 495 195" fill="none">
        <style>
            .header { font: 600 18px 'Segoe UI', Ubuntu, Sans-Serif; fill: #2f80ed; }
            .stat { font: 600 14px 'Segoe UI', Ubuntu, Sans-Serif; fill: #434d58; }
        </style>
        <rect x="0.5" y="0.5" rx="4.5" height="99%" width="494" fill="#fffefe" stroke="#e4e2e2"/>
        <text x="25" y="35" class="header">Repository Stats</text>
        <text x="25" y="75" class="stat">Total Stars: 1,234</text>
        <text x="25" y="105" class="stat">Total Commits: 567</text>
        <circle cx="420" cy="100" r="40" stroke="#e4e2e2" stroke-width="6"/>
        <circle cx="420" cy="100" r="40" stroke="#2f80ed" stroke-width="6" stroke-dasharray="180 251"/>
    </svg>"##;
    let decoded_card = decode_image(stats_card.to_vec(), &limits).expect("stats card svg");
    assert_eq!(decoded_card.format, ImageFormat::Png);
    assert_eq!((decoded_card.width, decoded_card.height), (495, 195));
    let card_png = image::load_from_memory(&decoded_card.bytes).expect("valid png");
    let card_rgba = card_png.to_rgba8();
    // Verify blue accent pixels from header text / progress ring actually rendered.
    assert!(
        card_rgba
            .pixels()
            .any(|p| p[3] > 0 && p[2] > 180 && p[0] < 100)
    );

    // Representative Star History chart SVG with filter, embedded raster data URI, polyline/path, and axes text.
    let tiny_png_data = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(png(4, 4))
    };
    let chart = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="533.333" style="background:#fff">
            <filter id="xkcdify" width="100%" height="100%" x="-5" y="-5" filterUnits="userSpaceOnUse">
                <feTurbulence baseFrequency=".05" result="noise" type="fractalNoise"/>
                <feDisplacementMap in="SourceGraphic" in2="noise" scale="5" xChannelSelector="R" yChannelSelector="G"/>
            </filter>
            <g transform="translate(80, 60)">
                <path d="M 0 0 L 0 400 L 700 400" stroke="#000" stroke-width="3" fill="none"/>
                <path d="M 10 380 C 120 350, 300 220, 680 20" stroke="#dd4528" stroke-width="3" fill="none" filter="url(#xkcdify)"/>
                <rect width="120" height="32" x="8" y="5" fill="#fff" stroke="#000" rx="5"/>
                <text x="24" y="25" font-family="monospace" font-size="14" fill="#000">example/repo</text>
            </g>
            <image width="20" height="20" x="300" y="12" href="data:image/png;base64,{tiny_png_data}"/>
            <text x="400" y="30" text-anchor="middle" font-size="20" fill="#000">Star History</text>
        </svg>"##
    );
    let decoded_chart = decode_image(chart.into_bytes(), &limits).expect("chart svg");
    assert_eq!(decoded_chart.format, ImageFormat::Png);
    assert_eq!((decoded_chart.width, decoded_chart.height), (800, 533));
    let chart_png = image::load_from_memory(&decoded_chart.bytes).expect("valid png");
    let chart_rgba = chart_png.to_rgba8();
    assert!(
        chart_rgba
            .pixels()
            .any(|p| p[3] > 0 && p[0] > 180 && p[2] < 80)
    );
}

#[test]
fn svg_unsupported_active_and_foreign_features_are_rejected_honestly() {
    let limits = ImageLimits::default();
    for unsupported in [
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><script>alert(1)</script></svg>"#.as_slice(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><foreignObject width="20" height="20"><div>hi</div></foreignObject></svg>"#.as_slice(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="10" height="10"><animate attributeName="x" from="0" to="10" dur="1s"/></rect></svg>"#.as_slice(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="10" height="10" onclick="alert(1)"/></svg>"#.as_slice(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><style>@import url("https://evil.invalid/x.css");</style><rect width="10" height="10"/></svg>"#.as_slice(),
    ] {
        assert!(is_svg_like(unsupported));
        assert_eq!(
            decode_image(unsupported.to_vec(), &limits),
            Err(ImageFailure::Unsupported),
            "must reject unsupported SVG construct: {}",
            String::from_utf8_lossy(unsupported)
        );
    }
}

#[test]
fn svg_malformed_and_dtd_payloads_are_rejected() {
    let limits = ImageLimits::default();
    for malformed in [
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect"#.as_slice(),
        br#"<?xml version="1.0"?><svg width="0" height="0"/>"#.as_slice(),
        br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="10" height="10"/></svg>"#.as_slice(),
        br#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY x "hello">]><svg width="20" height="20"><text>&x;</text></svg>"#.as_slice(),
        &[0x3C, b's', b'v', b'g', b' ', 0xFF, 0xFE, b'>'],
    ] {
        assert_eq!(
            decode_image(malformed.to_vec(), &limits),
            Err(ImageFailure::Malformed),
            "must reject malformed SVG: {:?}",
            malformed
        );
    }
}

#[test]
fn svg_oversized_dimensions_pixels_and_node_counts_are_bounded() {
    let limits = ImageLimits::default();
    let too_wide = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20000" height="20"><rect width="10" height="10"/></svg>"#;
    assert_eq!(
        decode_image(too_wide.to_vec(), &limits),
        Err(ImageFailure::TooManyPixels {
            width: 20_000,
            height: 20,
            limit: limits.max_pixels,
        })
    );

    let pixel_limits = ImageLimits {
        max_pixels: 100,
        ..ImageLimits::default()
    };
    let too_many_pixels = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><rect width="20" height="20"/></svg>"#;
    assert_eq!(
        decode_image(too_many_pixels.to_vec(), &pixel_limits),
        Err(ImageFailure::TooManyPixels {
            width: 20,
            height: 20,
            limit: 100,
        })
    );

    let decoded_limits = ImageLimits {
        max_decoded_bytes: 1_000,
        max_pixels: 1_000_000,
        ..ImageLimits::default()
    };
    let too_many_bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40"/></svg>"#;
    assert_eq!(
        decode_image(too_many_bytes.to_vec(), &decoded_limits),
        Err(ImageFailure::TooLarge { limit: 1_000 })
    );

    // Deeply nested SVG exceeds MAX_SVG_XML_DEPTH before usvg conversion.
    let mut deeply_nested =
        String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">"#);
    for _ in 0..300 {
        deeply_nested.push_str("<g>");
    }
    deeply_nested.push_str(r#"<rect width="10" height="10"/>"#);
    for _ in 0..300 {
        deeply_nested.push_str("</g>");
    }
    deeply_nested.push_str("</svg>");
    assert_eq!(
        decode_image(deeply_nested.into_bytes(), &limits),
        Err(ImageFailure::Malformed)
    );
}

#[test]
fn svg_external_file_and_network_references_cannot_read_local_files_or_fetch_network() {
    let limits = ImageLimits::default();
    let dir = tempfile::tempdir().expect("tempdir");
    let secret_png = dir.path().join("secret.png");
    std::fs::write(&secret_png, png(16, 16)).expect("write secret png");
    let secret_path_str = secret_png.to_string_lossy().replace('\\', "/");

    // Probe a local TCP listener to verify SVG `<image href="https://...">` or
    // `<image href="http://...">` never contacts the network.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let addr = listener.local_addr().expect("addr");

    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">
            <rect width="64" height="64" fill="#eeeeee"/>
            <image x="0" y="0" width="32" height="32" href="file:///{secret_path_str}"/>
            <image x="32" y="0" width="32" height="32" href="{secret_path_str}"/>
            <image x="0" y="32" width="32" height="32" href="http://{addr}/leak.png"/>
            <image x="32" y="32" width="32" height="32" href="https://{addr}/leak.png"/>
        </svg>"##
    );

    let decoded = decode_image(svg.into_bytes(), &limits).expect("svg without external images");
    let rendered = image::load_from_memory(&decoded.bytes)
        .expect("valid png")
        .to_rgba8();
    // Every pixel must come solely from the gray background rect (#eeeeee), never
    // from `secret.png` (which contains blue-tinted gradients).
    assert!(
        rendered
            .pixels()
            .all(|p| p[0] == 238 && p[1] == 238 && p[2] == 238)
    );
    assert!(
        matches!(listener.accept(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "SVG decoding must never open any network connection"
    );
}

#[test]
fn svg_non_ascii_attribute_names_do_not_panic_the_decoder() {
    let limits = ImageLimits::default();
    // A valid XML name can contain multi-byte characters. The event-attribute
    // guard must inspect bytes safely instead of slicing `name[..2]`.
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\">\
        <rect width=\"20\" height=\"20\" fill=\"#123456\" 中=\"x\"/>\
    </svg>";
    let decoded = decode_image(svg.as_bytes().to_vec(), &limits).expect("valid SVG renders");
    assert_eq!((decoded.width, decoded.height), (20, 20));
    assert!(image::load_from_memory(&decoded.bytes).is_ok());
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
