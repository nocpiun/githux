use super::*;
#[test]
fn performance_http_age_includes_apparent_date_age_and_rejects_invalid_age() {
    let date = "Wed, 21 Oct 2015 07:28:00 GMT";
    let now = 1_445_412_480_000i64;
    assert_eq!(corrected_age_secs(Some(date), Some("20"), now + 90_000), 90);
    assert_eq!(
        corrected_age_secs(Some(date), Some("100"), now + 90_000),
        100
    );
    assert_eq!(corrected_age_secs(Some(date), None, now + 1), 1);
    assert_eq!(corrected_age_secs(Some(date), None, now - 1000), 0);
    assert_eq!(corrected_age_secs(None, Some("bad"), now), u64::MAX);
}
#[tokio::test]
async fn performance_no_store_error_preserves_invalidation_policy() {
    use crate::network::test_server::{LocalHttpServer, ScriptedResponse};
    let server = LocalHttpServer::start(vec![
        ScriptedResponse::json(503, "unavailable").with_header("Cache-Control", "no-store"),
    ]);
    let fetcher = ReqwestExternalImages::with_local_http().unwrap();
    let url = format!("{}image", server.base_url());
    let response = fetcher
        .load_with_metadata(
            &url,
            &ExternalImageLimits::default(),
            ValidationHeaders::default(),
            CancellationToken::default(),
        )
        .await;
    assert_eq!(
        response,
        Err(ExternalImageError::NoStore(Box::new(
            ExternalImageError::Status(503)
        )))
    );
}
