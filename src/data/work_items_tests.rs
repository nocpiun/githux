//! Actual delayed transport races through the shared authorization path.
use super::*;
use crate::{
    auth::{InMemoryCredentialStore, token::Pat},
    data::AccountService,
    domain::{AccountKey, HostId},
    github::GitHubClient,
    network::{
        executor::BackgroundExecutor,
        transport::{HttpRequest, HttpResponse, Transport, TransportError, json_response},
    },
    storage::accounts::AccountDatabaseConfig,
};
use serde_json::json;
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};
type Gate = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
    u16,
);
#[derive(Default)]
struct Delayed {
    gate: Mutex<Option<Gate>>,
}
impl Transport for Delayed {
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
                return Ok(json_response(200, &json!({"id":if second { 2 } else { 1 },"login":if second { "account-b" } else { "account-a" }}).to_string()));
            }
            let gate = self.gate.lock().unwrap().take();
            if let Some((entered, resume, status)) = gate {
                entered.send(()).unwrap();
                resume.await.unwrap();
                if status != 200 {
                    return Ok(json_response(status, "{}"));
                }
            }
            Ok(json_response(
                200,
                "{\"total_count\":0,\"incomplete_results\":false,\"items\":[]}",
            ))
        })
    }
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
}
#[tokio::test]
async fn delayed_success_and_error_are_stale_after_every_account_transition() {
    for change in ["aba", "replace", "remove-readd", "close"] {
        for status in [200, 401] {
            let temp = tempfile::tempdir().unwrap();
            let transport = Arc::new(Delayed::default());
            let service = AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(Default::default(), transport.clone())),
                Arc::new(BackgroundExecutor::new(2)),
            )
            .await
            .unwrap();
            service.login(Pat::from_str("account-b")).await.unwrap();
            service.login(Pat::from_str("account-a")).await.unwrap();
            let session = service.repo_service().work_item_session().unwrap();
            let context = session.context().clone();
            let q = session.query(WorkItemKind::Issue, Relation::Assigned, StateFilter::Open);
            let (entered, ready) = tokio::sync::oneshot::channel();
            let (release, resume) = tokio::sync::oneshot::channel();
            *transport.gate.lock().unwrap() = Some((entered, resume, status));
            let task = tokio::spawn(async move {
                session
                    .page(&q, None, &Default::default(), Default::default())
                    .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), ready)
                .await
                .unwrap()
                .unwrap();
            let key = AccountKey::new(1, HostId::github());
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
                Err(WorkItemError::GitHub(GitHubError::StaleContext)),
                "{change}/{status}"
            );
            assert!(!service.repo_service().is_current(&context));
        }
    }
}
