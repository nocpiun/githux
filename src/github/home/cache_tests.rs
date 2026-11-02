use super::cache::*;
use crate::{
    auth::token::Pat,
    github::client::GitHubClient,
    network::transport::{MockTransport, json_response},
};
use std::sync::Arc;
#[tokio::test]
async fn serialized_response_set_excludes_credentials_error_echo_and_download_locators() {
    let mock = Arc::new(MockTransport::new());
    let mut response = json_response(
        200,
        r#"{"data":{"download_url":"https://private.invalid/?signed=secret"},"errors":[{"type":"FORBIDDEN","path":["user"],"message":"server-secret"}]}"#,
    );
    response
        .headers
        .push(("Set-Cookie".into(), "cookie-secret".into()));
    mock.enqueue(Ok(response)).await;
    let recording = Arc::new(PageTransport::recording(mock));
    let client = GitHubClient::new(Default::default(), recording.clone());
    let request = client.build_post(
        "graphql",
        &Pat::from_str("pat-secret"),
        r#"{"query":"query { viewer { login } }"}"#.into(),
    );
    client
        .transport()
        .execute(request, Default::default())
        .await
        .unwrap();
    let persisted = serde_json::to_string(&recording.exchanges()).unwrap();
    for secret in [
        "pat-secret",
        "cookie-secret",
        "server-secret",
        "signed=secret",
        "Authorization",
        "download_url",
    ] {
        assert!(!persisted.contains(secret));
    }
    assert!(persisted.contains("FORBIDDEN"));
}
