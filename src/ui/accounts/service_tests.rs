//! Offline regressions through the real RuntimeHandle, SQLite and CacheStore. The transport is
//! repeatable by request identity; the gate gives cancellation tests a deterministic interleaving.
use super::controller::{AccountsController, Effect, Mode, Tone};
use super::view::operation;
use crate::app::runtime::{Backend, ExitGate, RepositoryList, RuntimeHandle};
use crate::auth::{credential::CredentialStore, memory::InMemoryCredentialStore};
use crate::data::AccountService;
use crate::domain::{AccountKey, HostId};
use crate::github::client::{GitHubClient, GitHubConfig};
use crate::network::{
    cancel::CancellationToken,
    executor::BackgroundExecutor,
    transport::{HttpRequest, HttpResponse, Transport, TransportError, json_response},
};
use crate::storage::{
    accounts::AccountDatabaseConfig,
    cache::{CacheConfig, CacheStore},
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct TransportFixture {
    gate: AtomicBool,
    entered: AtomicUsize,
    release: tokio::sync::Notify,
    reject: AtomicBool,
    fail_repos: AtomicBool,
    requests: AtomicUsize,
}
impl Transport for TransportFixture {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let url = reqwest::Url::parse(&request.url).expect("fixture URL");
            let id = match request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.as_str())
            {
                Some("Bearer preview-1") => 1,
                Some("Bearer preview-2") => 2,
                _ => 3,
            };
            if url.path() == "/user" {
                if self.gate.load(Ordering::SeqCst) {
                    self.entered.fetch_add(1, Ordering::SeqCst);
                    tokio::select! { _ = self.release.notified()=>{}, _=cancel.cancelled()=>return Err(TransportError::Cancelled) }
                }
                if self.reject.load(Ordering::SeqCst) {
                    return Ok(json_response(401, r#"{"message":"Bad credentials"}"#));
                }
                return Ok(json_response(
                    200,
                    &format!(r#"{{"id":{id},"login":"user{id}"}}"#),
                ));
            }
            if url.path() == "/user/repos" {
                if self.fail_repos.load(Ordering::SeqCst) {
                    return Ok(json_response(403, r#"{"message":"Forbidden"}"#));
                }
                return Ok(json_response(
                    200,
                    &format!(
                        r#"[{{"id":{id},"name":"demo","full_name":"user{id}/demo","owner":{{"id":{id},"login":"user{id}"}},"visibility":"private"}}]"#
                    ),
                ));
            }
            Ok(json_response(
                599,
                r#"{"message":"Unmodelled fixture request"}"#,
            ))
        })
    }
}
fn key(id: u64) -> AccountKey {
    AccountKey::new(id, HostId::github())
}
struct Host {
    controller: AccountsController,
    runtime: RuntimeHandle,
    transport: Arc<TransportFixture>,
    gate: ExitGate,
    _directory: tempfile::TempDir,
}
impl Host {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let transport = Arc::new(TransportFixture::default());
        let executor = Arc::new(BackgroundExecutor::new(4));
        let cache = CacheStore::open(CacheConfig::at(directory.path().join("cache"))).unwrap();
        let store = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let accounts = AccountService::open_with_cache(
            AccountDatabaseConfig::at(directory.path().join("accounts.sqlite3")),
            store,
            Arc::new(GitHubClient::new(
                GitHubConfig::default(),
                transport.clone(),
            )),
            executor.clone(),
            cache,
        )
        .await
        .unwrap();
        let gate = ExitGate::default();
        let mut this = Self {
            controller: AccountsController::new(),
            runtime: RuntimeHandle::from_backend(Backend::new(accounts, executor, gate.clone())),
            transport,
            gate,
            _directory: directory,
        };
        this.poll();
        this
    }
    fn poll(&mut self) {
        for effect in self.controller.take_effects() {
            match effect {
                Effect::Submit(command) => {
                    let operation = operation(&command);
                    match self.runtime.submit(command) {
                        Ok(()) => self.controller.submitted(operation),
                        Err(error) => self.controller.note_submit_error(error),
                    }
                }
                Effect::Cancel => self.runtime.cancel(),
                Effect::OpenExternal(_) => {}
            }
        }
        let snapshot = self.runtime.snapshot();
        self.controller.end_poll();
        self.controller.ingest(&snapshot);
    }
    async fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.poll();
            if !self.controller.presentation().busy {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "account operation did not settle"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    async fn login(&mut self, id: u64) {
        self.controller.start_add();
        assert!(self.controller.submit(&format!("preview-{id}")));
        self.settle().await;
        assert_eq!(self.controller.presentation().active, Some(key(id)));
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.transport.release.notify_one();
        self.runtime.shutdown();
        assert!(
            self.gate.wait(Duration::from_secs(5)),
            "background teardown must finish before temporary data is removed"
        );
    }
}
#[tokio::test]
async fn inactive_replacement_rejects_another_identity_then_commits_only_the_target() {
    let mut host = Host::new().await;
    host.login(1).await;
    host.login(2).await;
    host.controller.select(key(1));
    host.settle().await;
    let before = host.controller.presentation().accounts;
    host.controller.start_replace(key(2));
    assert!(host.controller.submit("preview-1"));
    host.settle().await;
    assert_eq!(host.controller.presentation().accounts, before);
    assert_eq!(host.controller.presentation().mode, Mode::Replace(key(2)));
    assert_eq!(
        host.controller.presentation().status.unwrap().tone,
        Tone::Error
    );
    assert!(host.controller.input_text().is_empty());
    host.controller.set_input("correction-being-typed");
    host.controller.revalidate(key(2));
    host.settle().await;
    assert_eq!(host.controller.input_text(), "correction-being-typed");
    assert_eq!(host.controller.presentation().mode, Mode::Replace(key(2)));
    // Commit closes the form even when the automatic repository read fails afterwards.
    host.transport.fail_repos.store(true, Ordering::SeqCst);
    assert!(host.controller.submit("preview-2"));
    host.settle().await;
    let after = host.controller.presentation();
    assert!(!after.form_open);
    assert_eq!(after.active, Some(key(1)));
    assert_eq!(after.accounts[0].epoch, before[0].epoch);
    assert_ne!(after.accounts[1].epoch, before[1].epoch);
    assert!(!after.status.unwrap().text.contains("was not saved"));
    assert!(matches!(after.repositories, RepositoryList::Failed(_)));
    // Opening another replacement while a prior completion is retained cannot close this form.
    host.controller.start_replace(key(2));
    host.poll();
    assert!(host.controller.presentation().form_open);
}
#[tokio::test]
async fn automatic_repository_reads_do_not_replay_completions_or_repeat_on_poll() {
    let mut host = Host::new().await;
    host.login(1).await;
    let count = host.transport.requests.load(Ordering::SeqCst);
    host.controller.start_replace(key(1));
    host.controller.set_input("draft");
    for _ in 0..10 {
        host.poll();
    }
    assert_eq!(host.transport.requests.load(Ordering::SeqCst), count);
    assert_eq!(host.controller.input_text(), "draft");
    assert_eq!(host.controller.presentation().mode, Mode::Replace(key(1)));
}
#[tokio::test]
async fn cancellation_keeps_the_runtime_busy_until_revalidation_reconciles() {
    let mut host = Host::new().await;
    host.login(1).await;
    let before = host.controller.presentation().accounts[0].epoch;
    host.transport.gate.store(true, Ordering::SeqCst);
    host.controller.revalidate(key(1));
    host.poll();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.transport.entered.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    host.controller.cancel();
    host.poll();
    assert!(host.controller.presentation().settling);
    assert!(!host.controller.submit("preview-2"));
    host.transport.release.notify_one();
    host.settle().await;
    assert_ne!(host.controller.presentation().accounts[0].epoch, before);
    assert!(
        host.controller
            .presentation()
            .status
            .unwrap()
            .text
            .contains("cancelled operation settled")
    );
    assert_eq!(host.controller.presentation().active, Some(key(1)));
}
#[tokio::test]
async fn revocation_and_removal_clear_an_already_published_list_in_one_poll() {
    let mut host = Host::new().await;
    host.login(1).await;
    assert!(matches!(
        host.controller.presentation().repositories,
        RepositoryList::Ready { .. }
    ));
    let services = host.runtime.services().unwrap();
    let session = services.repositories().session().unwrap();
    let cache = services.repositories().cache(session.context()).unwrap();
    cache
        .store()
        .revoke_repository(cache.scope(), None)
        .unwrap();
    host.poll();
    assert!(matches!(
        host.controller.presentation().repositories,
        RepositoryList::Failed(_)
    ));
    host.controller.ask_remove(key(1));
    host.controller.confirm_remove(key(1));
    host.settle().await;
    assert!(host.controller.presentation().accounts.is_empty());
    assert!(host.controller.presentation().active.is_none());
    assert!(!matches!(
        host.controller.presentation().repositories,
        RepositoryList::Ready { .. }
    ));
}
