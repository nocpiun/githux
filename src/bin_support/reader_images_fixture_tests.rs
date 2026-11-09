//! Self-checks for the development picture fixture.

use super::*;

#[test]
fn the_fixture_pictures_really_decode() {
    for (label, bytes) in [
        ("banner", picture(1600, 200)),
        ("diagram", picture(320, 180)),
        ("tall", picture(120, 900)),
        ("logo", picture(96, 96)),
    ] {
        let decoded = image::load_from_memory(&bytes).unwrap_or_else(|_| panic!("{label} decodes"));
        assert!(decoded.width() > 0 && decoded.height() > 0, "{label}");
    }
    let oversized = oversized_canvas();
    assert!(oversized.starts_with(b"\x89PNG\r\n\x1a\n"));
    // The declared canvas is enormous even though the payload is tiny.
    let width = u32::from_be_bytes(oversized[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(oversized[20..24].try_into().unwrap());
    assert_eq!((width, height), (60_000, 60_000));
}

#[test]
fn the_fixture_answers_the_same_resource_again_and_again() {
    let fixture = FixtureTransport::new(Scene::Healthy);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let url = format!("https://api.github.test/repos/{OWNER}/{NAME}/git/blobs/{DIAGRAM_BLOB}");
    let body = runtime.block_on(async {
        let first = fixture
            .execute(
                HttpRequest::get(url.clone()),
                crate::network::cancel::CancellationToken::default(),
            )
            .await
            .expect("first");
        let second = fixture
            .execute(
                HttpRequest::get(url.clone()),
                crate::network::cancel::CancellationToken::default(),
            )
            .await
            .expect("second");
        assert_eq!(first.status, 200);
        assert_eq!(first.body, second.body);
        first.body
    });
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    let encoded = value["content"].as_str().expect("content");
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("base64");
    assert!(image::load_from_memory(&bytes).is_ok(), "a real picture");
    assert!(
        fixture.mismatches().is_empty(),
        "{:?}",
        fixture.mismatches()
    );
}

#[test]
fn the_external_source_serves_the_public_addresses_only() {
    let source = MemoryExternalImages::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let limits = ExternalImageLimits::default();
    let result = runtime.block_on(source.load(
        BANNER_URL,
        &limits,
        crate::network::cancel::CancellationToken::default(),
    ));
    assert!(
        result
            .map(|bytes| image::load_from_memory(&bytes).is_ok())
            .unwrap_or(false)
    );
    let missing = runtime.block_on(source.load(
        "https://images.invalid/public/unknown.png",
        &limits,
        crate::network::cancel::CancellationToken::default(),
    ));
    assert!(matches!(missing, Err(ExternalImageError::Status(404))));
}
