use super::*;
use crate::{
    domain::repository::RepositoryLocator,
    github::GitHubConfig,
    network::transport::{HttpResponse, MockFailure, MockTransport, json_response},
};
use serde_json::json;
use std::sync::Arc;

fn repository(id: u64, name: &str) -> RepositoryIdentity {
    RepositoryIdentity {
        id,
        locator: RepositoryLocator::new("example", name).unwrap(),
    }
}

fn response(rows: serde_json::Value, next: Option<&str>) -> HttpResponse {
    let mut response = json_response(200, &rows.to_string());
    if let Some(next) = next {
        response
            .headers
            .push(("link".into(), format!("<{next}>; rel=\"next\"")));
    }
    response
}

fn issue(title: &str, state: &str) -> serde_json::Value {
    json!({"number":42,"title":title,"state":state,"updated_at":"2026-11-01T12:00:00Z"})
}

#[tokio::test]
async fn repository_issue_pagination_retains_opaque_cursor_and_skips_pr_only_page() {
    let transport = Arc::new(MockTransport::new());
    let second = "https://api.github.com/repositories/12/issues?state=open&sort=created&direction=desc&per_page=30&page=2&after=dGVzdA%3D%3D";
    let third = "https://api.github.com/repositories/12/issues?state=open&sort=created&direction=desc&per_page=30&page=3&after=YW5vdGhlcg%3D";
    transport.enqueue_many(vec![
        Ok(response(json!([issue("First repository", "open"), {"pull_request": {}, "number": 41}]), Some(second))),
        Ok(response(json!([{"pull_request": {}}]), Some(third))),
        Ok(response(json!([{"number":2,"title":"Later issue","state":"open","updated_at":"2026-11-01T12:00:00Z"}]), None)),
        Ok(response(json!([issue("Second repository", "closed")]), None)),
    ]).await;
    let client = GitHubClient::new(GitHubConfig::default(), transport.clone());
    let pat = Pat::from_str("synthetic");
    let repo = repository(12, "first");
    let other = repository(13, "second");
    let cancel = CancellationToken::default();
    let first = fetch(
        &client,
        &pat,
        &repo,
        IssueFilter::Open,
        None,
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(first.rows.len(), 1);
    for (repository, filter) in [(&repo, IssueFilter::Closed), (&other, IssueFilter::Open)] {
        assert!(matches!(
            fetch(
                &client,
                &pat,
                repository,
                filter,
                first.next.as_ref(),
                cancel.clone()
            )
            .await,
            Err(RepositoryError::InvalidInput { .. })
        ));
    }
    let later = fetch(
        &client,
        &pat,
        &repo,
        IssueFilter::Open,
        first.next.as_ref(),
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        later.rows.iter().map(|row| row.number).collect::<Vec<_>>(),
        [2]
    );
    assert!(later.next.is_none());
    let second_repo = fetch(&client, &pat, &other, IssueFilter::Closed, None, cancel)
        .await
        .unwrap();
    assert_eq!(first.rows[0].number, second_repo.rows[0].number);
    assert_ne!(first.rows[0].title, second_repo.rows[0].title);
    let requests = transport.recorded_requests().await;
    assert_eq!(
        requests
            .iter()
            .map(|request| request.url.as_str())
            .collect::<Vec<_>>(),
        [
            "https://api.github.com/repos/example/first/issues?state=open&sort=created&direction=desc&per_page=30&page=1",
            second,
            third,
            "https://api.github.com/repos/example/second/issues?state=closed&sort=created&direction=desc&per_page=30&page=1",
        ]
    );
}

#[tokio::test]
async fn repository_issue_empty_and_read_failures_have_distinct_results() {
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue_many(vec![
            Ok(response(json!([]), None)),
            Ok(json_response(403, "{}")),
            Err(MockFailure::Timeout),
            Ok(response(json!([issue("After retry", "open")]), None)),
        ])
        .await;
    let client = GitHubClient::new(GitHubConfig::default(), transport.clone());
    let pat = Pat::from_str("synthetic");
    let repository = repository(12, "first");
    let read = || {
        fetch(
            &client,
            &pat,
            &repository,
            IssueFilter::Open,
            None,
            CancellationToken::default(),
        )
    };
    let empty = read().await.unwrap();
    assert!(empty.rows.is_empty() && empty.next.is_none());
    assert!(matches!(
        read().await,
        Err(RepositoryError::GitHub(GitHubError::Forbidden))
    ));
    assert!(matches!(
        read().await,
        Err(RepositoryError::GitHub(GitHubError::Timeout { .. }))
    ));
    assert_eq!(read().await.unwrap().rows[0].title, "After retry");
}
