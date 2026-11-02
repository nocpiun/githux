use super::tests::*;
use super::*;
use crate::network::transport::json_response;
use crate::{content::PreparedBody, domain::blob::BlobLimits, github::RepositoryLimits};
use serde_json::json;
const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ROOT: &str = "1111111111111111111111111111111111111111";
const BLOB: &str = "3333333333333333333333333333333333333333";
async fn discovery(f: &Fixture, with_readme: bool) {
    f.enqueue(&json!({"id":42,"name":"target","owner":{"id":2,"login":"target"},"visibility":"public","private":false,"default_branch":"main","fork":false,"archived":false,"stargazers_count":0,"forks_count":0,"open_issues_count":0,"size":1,"html_url":"https://github.com/target/target"}).to_string()).await;
    f.enqueue(&json!({"sha":COMMIT,"commit":{"message":"profile","tree":{"sha":ROOT},"author":null,"committer":null}}).to_string()).await;
    f.enqueue(&json!({"sha":ROOT,"truncated":false,"tree":if with_readme { json!([{"path":"README.md","type":"blob","mode":"100644","sha":BLOB,"size":6}]) } else { json!([]) }}).to_string()).await;
    if with_readme {
        f.enqueue(
            &json!({"sha":BLOB,"encoding":"base64","size":6,"content":"aGVsbG8K"}).to_string(),
        )
        .await;
    }
}
#[tokio::test]
async fn optional_readme_missing_prepared_and_fetch_failure_remain_distinct() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    fixture.enqueue(r#"{"id":2,"login":"target"}"#).await;
    let (profile, _, _) = value(
        cached
            .profile(PageReadMode::Online, "target", Default::default())
            .await
            .unwrap(),
    );
    let limits = RepositoryLimits::default();
    let blobs = BlobLimits::default();
    assert!(matches!(
        cached
            .profile_readme(
                PageReadMode::Offline,
                &profile,
                &limits,
                &blobs,
                Default::default()
            )
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));
    discovery(&fixture, false).await;
    let (missing, _, _) = value(
        cached
            .profile_readme(
                PageReadMode::Online,
                &profile,
                &limits,
                &blobs,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(matches!(missing, ProfileReadme::Missing { .. }));
    fixture.mock.enqueue(Ok(json_response(503, "{}"))).await;
    assert!(matches!(
        cached
            .profile_readme(
                PageReadMode::Online,
                &profile,
                &limits,
                &blobs,
                Default::default()
            )
            .await,
        Err(PageCacheError::Read(_))
    ));
    let (missing, _, _) = value(
        cached
            .profile_readme(
                PageReadMode::Offline,
                &profile,
                &limits,
                &blobs,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(matches!(missing, ProfileReadme::Missing { .. }));
    discovery(&fixture, true).await;
    cached
        .profile_readme(
            PageReadMode::Online,
            &profile,
            &limits,
            &blobs,
            Default::default(),
        )
        .await
        .unwrap();
    let (prepared, _, _) = value(
        cached
            .profile_readme(
                PageReadMode::Offline,
                &profile,
                &limits,
                &blobs,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(
        matches!(prepared, ProfileReadme::Prepared { content: crate::content::PreparedContent { body: PreparedBody::Utf8 { ref text, .. }, .. }, .. } if text == "hello\n")
    );
}
