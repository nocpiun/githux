//! Account-bound repository pull list reads against delayed offline transports.

use super::*;
use crate::auth::{InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::domain::error::GitHubError;
use crate::domain::repository::RepositoryLocator;
use crate::domain::repository_pulls::{PullFilter, PullState};
use crate::domain::{AccountKey, HostId};
use crate::github::{GitHubClient, repository_pulls::PullListLimits};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{
    HttpRequest, HttpResponse, MockTransport, Transport, TransportError, json_response,
};
use crate::storage::accounts::AccountDatabaseConfig;
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

fn identity(owner: &str, name: &str, id: u64) -> RepositoryIdentity {
    RepositoryIdentity {
        id,
        locator: RepositoryLocator::new(owner, name).unwrap(),
    }
}

fn pull(number: u64, state: &str, merged_at: Option<&str>) -> Value {
    json!({
        "id": number * 100,
        "number": number,
        "title": format!("Change {number}"),
        "state": state,
        "draft": false,
        "merged_at": merged_at,
        "user": {"login": "octocat"},
        "head": {"ref": "feature/x"},
        "base": {"ref": "main"},
        "created_at": "2026-11-01T00:00:00Z",
        "updated_at": "2026-11-20T00:00:00Z"
    })
}

fn page(pulls: Vec<Value>, next: Option<&str>) -> HttpResponse {
    let mut response = json_response(200, &Value::Array(pulls).to_string());
    if let Some(next) = next {
        response
            .headers
            .push(("link".into(), format!("<{next}>; rel=\"next\"")));
    }
    response
}

async fn service(temp: &tempfile::TempDir, transport: Arc<dyn Transport>) -> Arc<AccountService> {
    Arc::new(
        AccountService::open(
            AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(Default::default(), transport)),
            Arc::new(BackgroundExecutor::new(2)),
        )
        .await
        .unwrap(),
    )
}

#[tokio::test]
async fn the_session_reads_its_own_repository_and_keeps_pages_separate() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue(Ok(json_response(
            200,
            &json!({"id": 1, "login": "account-a"}).to_string(),
        )))
        .await;
    let service = service(&temp, transport.clone() as Arc<dyn Transport>).await;
    service
        .login(Pat::from_str("account-a"))
        .await
        .expect("synthetic login");

    let repo_a = identity("octo", "repo", 42);
    transport
        .enqueue(Ok(page(vec![pull(7, "open", None)], None)))
        .await;
    let session = service
        .repo_service()
        .pull_list_session(repo_a.clone())
        .expect("session");
    assert_eq!(session.identity().id(), 42);
    assert_eq!(session.identity().locator(), repo_a.locator());
    assert_eq!(session.query(PullFilter::Open).filter(), PullFilter::Open);

    let first = session
        .page(
            PullFilter::Open,
            None,
            &PullListLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
    assert_eq!(first.value.pulls.len(), 1);
    assert_eq!(first.value.pulls[0].number, 7);
    assert_eq!(first.value.pulls[0].state, PullState::Open);

    // The same visible number in another repository is a different row read from that
    // repository's own endpoint.
    let repo_b = identity("octo", "other", 43);
    transport
        .enqueue(Ok(page(vec![pull(7, "closed", None)], None)))
        .await;
    let other_session = service
        .repo_service()
        .pull_list_session(repo_b.clone())
        .expect("session");
    let second = other_session
        .page(
            PullFilter::Closed,
            None,
            &PullListLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
    assert_eq!(second.value.pulls[0].number, first.value.pulls[0].number);
    assert_eq!(second.value.pulls[0].state, PullState::Closed);
    assert_eq!(second.value.pulls[0].id, 700);

    let recorded = transport.recorded_requests().await;
    let paths: Vec<_> = recorded
        .iter()
        .map(|request| {
            reqwest::Url::parse(&request.url)
                .unwrap()
                .path()
                .to_string()
        })
        .collect();
    assert_eq!(
        paths,
        vec!["/user", "/repos/octo/repo/pulls", "/repos/octo/other/pulls"]
    );
    // The repository list is never the personal search.
    assert!(
        recorded
            .iter()
            .all(|request| !request.url.contains("search/issues"))
    );
}

#[tokio::test]
async fn the_session_rejects_a_cursor_issued_for_another_repository() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue(Ok(json_response(
            200,
            &json!({"id": 1, "login": "account-a"}).to_string(),
        )))
        .await;
    let service = service(&temp, transport.clone() as Arc<dyn Transport>).await;
    service.login(Pat::from_str("account-a")).await.unwrap();
    transport
        .enqueue(Ok(page(
            vec![pull(7, "open", None)],
            Some("https://api.github.com/repositories/42/pulls?state=open&page=2"),
        )))
        .await;
    let session = service
        .repo_service()
        .pull_list_session(identity("octo", "repo", 42))
        .unwrap();
    let first = session
        .page(
            PullFilter::Open,
            None,
            &PullListLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
    let cursor = first.value.next.expect("next page");
    // Same name, different numeric repository: still another repository.
    let other = service
        .repo_service()
        .pull_list_session(identity("octo", "repo", 43))
        .unwrap();
    assert_eq!(
        other
            .page(
                PullFilter::Open,
                Some(&cursor),
                &PullListLimits::default(),
                CancellationToken::default(),
            )
            .await,
        Err(RepositoryError::InvalidInput {
            field: "pull cursor identity"
        })
    );
}

type Gate = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
    u16,
);
#[derive(Default)]
struct Delayed {
    gate: Mutex<Option<Gate>>,
}

impl Delayed {
    /// Hold the next pull page at the transport boundary and expose the entry signal plus
    /// the release sender.
    fn block(
        &self,
        status: u16,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, resume) = tokio::sync::oneshot::channel();
        *self.gate.lock().unwrap() = Some((entered, resume, status));
        (ready, release)
    }
}

impl Transport for Delayed {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max {
                return Err(TransportError::ResponseTooLarge { limit: max });
            }
            Ok(response)
        })
    }

    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let url = reqwest::Url::parse(&request.url).unwrap();
            if url.path() == "/user" {
                let second = request
                    .headers
                    .iter()
                    .any(|(_, value)| value.ends_with("account-b"));
                return Ok(json_response(
                    200,
                    &json!({
                        "id": if second { 2 } else { 1 },
                        "login": if second { "account-b" } else { "account-a" }
                    })
                    .to_string(),
                ));
            }
            let gate = self.gate.lock().unwrap().take();
            if let Some((entered, resume, status)) = gate {
                let _ = entered.send(());
                let _ = resume.await;
                if status != 200 {
                    return Ok(json_response(status, "{}"));
                }
            }
            Ok(page(vec![pull(7, "open", None)], None))
        })
    }
}

async fn ready(signal: tokio::sync::oneshot::Receiver<()>, case: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), signal)
        .await
        .unwrap_or_else(|_| panic!("boundary reached for {case}"))
        .unwrap();
}

#[tokio::test]
async fn delayed_pull_pages_and_errors_are_stale_after_every_account_transition() {
    for change in ["aba", "replace", "remove-readd", "close"] {
        for status in [200, 401] {
            let temp = tempfile::tempdir().unwrap();
            let transport = Arc::new(Delayed::default());
            let service = service(&temp, transport.clone() as Arc<dyn Transport>).await;
            service.login(Pat::from_str("account-a")).await.unwrap();
            let account_a = AccountKey::new(1, HostId::github());
            let key = account_a.clone();
            service.login(Pat::from_str("account-b")).await.unwrap();
            service.set_active(account_a).await.unwrap();

            let session = service
                .repo_service()
                .pull_list_session(identity("octo", "repo", 42))
                .unwrap();
            let context = session.context().clone();
            let (entered, release) = transport.block(status);
            let task = tokio::spawn(async move {
                session
                    .page(
                        PullFilter::Open,
                        None,
                        &PullListLimits::default(),
                        CancellationToken::default(),
                    )
                    .await
            });
            ready(entered, &format!("{change}/{status}")).await;
            match change {
                "aba" => {
                    service
                        .set_active(AccountKey::new(2, HostId::github()))
                        .await
                        .unwrap();
                    service.set_active(key).await.unwrap();
                }
                "replace" => {
                    service
                        .replace(key, Pat::from_str("account-a-new"))
                        .await
                        .unwrap();
                }
                "remove-readd" => {
                    service.remove_saved(key).await.unwrap();
                    service.login(Pat::from_str("account-a")).await.unwrap();
                }
                "close" => service.close().await.unwrap(),
                _ => unreachable!(),
            }
            release.send(()).unwrap();
            assert_eq!(
                task.await.unwrap(),
                Err(RepositoryError::GitHub(GitHubError::StaleContext)),
                "{change}/{status}"
            );
            assert!(!service.repo_service().is_current(&context));
        }
    }
}
