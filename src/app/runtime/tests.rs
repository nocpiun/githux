//! Deterministic, offline tests for the runtime bridge. Real services, real SQLite/CacheStore,
//! in-memory credentials and a mock transport; no GitHub network or real PAT is used.

use super::*;
use crate::auth::credential::CredentialStore;
use crate::auth::memory::InMemoryCredentialStore;
use crate::auth::token::Pat;
use crate::data::AccountService;
use crate::domain::{AccountKey, HostId};
use crate::github::client::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{HttpResponse, MockTransport};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};
use std::sync::Arc;
use std::time::Duration;

fn key(id: u64) -> AccountKey {
    AccountKey::new(id, HostId::github())
}
fn gate() -> ExitGate {
    ExitGate::default()
}
fn db_config(temp: &tempfile::TempDir) -> AccountDatabaseConfig {
    AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3"))
}
async fn user(mock: &MockTransport, id: u64) {
    mock.enqueue(Ok(HttpResponse {
        status: 200,
        headers: vec![],
        body: format!(r#"{{"id":{id},"login":"user{id}"}}"#),
    }))
    .await;
}
async fn repos(mock: &MockTransport, ids: &[u64]) {
    let arr: Vec<_> = ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": id, "name": format!("r{id}"), "full_name": format!("u/r{id}"),
                "owner": {"id": 1, "login": "u"}, "visibility": "public"
            })
        })
        .collect();
    mock.enqueue(Ok(HttpResponse {
        status: 200,
        headers: vec![],
        body: serde_json::json!(arr).to_string(),
    }))
    .await;
}
fn client(mock: Arc<MockTransport>) -> Arc<GitHubClient> {
    Arc::new(GitHubClient::new(GitHubConfig::default(), mock))
}

async fn open_accounts(
    temp: &tempfile::TempDir,
    credentials: Arc<dyn CredentialStore>,
    mock: Arc<MockTransport>,
    executor: Arc<BackgroundExecutor>,
    cached: bool,
) -> AccountService {
    if cached {
        let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        AccountService::open_with_cache(db_config(temp), credentials, client(mock), executor, cache)
            .await
            .unwrap()
    } else {
        AccountService::open(db_config(temp), credentials, client(mock), executor)
            .await
            .unwrap()
    }
}

async fn settle(handle: &mut RuntimeHandle) -> Snapshot {
    for _ in 0..2000 {
        let snapshot = handle.snapshot();
        if snapshot.busy.is_none() && snapshot.completion.is_some() {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!(
        "runtime operation did not settle: {:?}",
        handle.snapshot().busy
    );
}

/// Sign in one account and wait for its automatic repository read to settle.
async fn login_and_settle(handle: &mut RuntimeHandle, mock: &MockTransport, id: u64, ids: &[u64]) {
    user(mock, id).await;
    repos(mock, ids).await;
    handle
        .submit(Command::Add(Pat::from_str("ghp_fake")))
        .expect("submit add");
    settle(handle).await;
    assert_eq!(handle.snapshot().active, Some(key(id)));
}

#[test]
fn account_switch_preserves_per_account_tab_sets_without_collision() {
    let mut sessions = Sessions::new();
    let a = key(1);
    let b = key(2);
    assert!(sessions.show(Some(a.clone())));
    let a_home = sessions.workspace().active_id();
    let repo = |name: &str| Route::Repository {
        namespace: "shared".into(),
        name: name.into(),
    };
    sessions.open(repo("one"));
    sessions.open(repo("two"));
    sessions.open(Route::Settings);
    let a_ids: Vec<TabId> = sessions
        .workspace()
        .sessions()
        .iter()
        .map(|s| s.id)
        .collect();
    let a_active = sessions.workspace().active_id();

    assert!(sessions.show(Some(b.clone())));
    assert_eq!(
        sessions.workspace().sessions().len(),
        1,
        "B starts on its own Home"
    );
    assert_ne!(
        sessions.workspace().active_id(),
        a_home,
        "tab ids never collide"
    );
    sessions.open(repo("one"));
    sessions.open(repo("two"));
    let b_ids: Vec<TabId> = sessions
        .workspace()
        .sessions()
        .iter()
        .map(|s| s.id)
        .collect();
    assert!(
        b_ids.iter().all(|id| !a_ids.contains(id)),
        "same repo in two accounts gets two ids"
    );

    assert!(sessions.show(Some(a.clone())));
    assert_eq!(
        sessions
            .workspace()
            .sessions()
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        a_ids
    );
    assert_eq!(sessions.workspace().active_id(), a_active);
    assert_eq!(
        sessions.workspace().active().route.key(),
        RouteKey::Settings
    );
}

#[test]
fn routes_share_case_aliases_and_pull_requests_keep_identity() {
    let mut sessions = Sessions::new();
    let repo = |namespace: &str, name: &str| Route::Repository {
        namespace: namespace.into(),
        name: name.into(),
    };
    let first = sessions.open(repo("Owner", "Repo"));
    assert_eq!(sessions.open(repo("owner", "repo")), first);
    assert_eq!(sessions.workspace().sessions().len(), 2);
    let pr = |namespace: &str, name: &str, number: u64| Route::PullRequest {
        namespace: namespace.into(),
        name: name.into(),
        number,
    };
    let opened = sessions.open(pr("Owner", "Repo", 41));
    assert_eq!(sessions.open(pr("owner", "repo", 41)), opened);
    assert_eq!(
        sessions.workspace().active().route.title(),
        "Owner/Repo #41"
    );
    assert_ne!(sessions.open(pr("owner", "repo", 42)), opened);
    assert_eq!(
        sessions.workspace().active().route.title(),
        "owner/repo #42"
    );
    assert_eq!(
        sessions.workspace().sessions().len(),
        4,
        "Home, repo and two distinct PR sessions"
    );
}

#[tokio::test]
async fn returning_to_an_account_advances_incarnation_and_invalidates_page_tickets() {
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(4));
    let accounts = open_accounts(&temp, credentials, mock.clone(), executor.clone(), true).await;
    let backend = Backend::new(accounts, executor, gate());
    let mut handle = RuntimeHandle::from_backend(backend);

    login_and_settle(&mut handle, &mock, 1, &[1, 2]).await;
    login_and_settle(&mut handle, &mock, 2, &[3]).await;
    handle.submit(Command::Select(key(1))).expect("select 1");
    settle(&mut handle).await;

    let services = handle.services().expect("services after settle");
    let tab = handle.sessions_mut().open(Route::Repository {
        namespace: "u".into(),
        name: "r1".into(),
    });
    let context = services
        .page_context(handle.sessions(), tab, Some(1))
        .expect("page context");
    let mut slot: PageSlot<u32> = PageSlot::default();
    let ticket = slot.begin(context.clone());
    assert!(slot.publish(ticket.clone(), 42, handle.sessions()));

    // Switch to account B: the ticket captured under A must stop being current.
    handle.submit(Command::Select(key(2))).expect("select 2");
    settle(&mut handle).await;
    assert!(
        !slot.is_valid(handle.sessions()),
        "late result must be rejected after a switch"
    );
    assert!(slot.value(handle.sessions()).is_none());

    // Back to A: incarnation advanced, the old identity is still not current.
    handle
        .submit(Command::Select(key(1)))
        .expect("select 1 again");
    settle(&mut handle).await;
    assert!(
        !slot.is_valid(handle.sessions()),
        "A→B→A must not revive the pre-switch ticket"
    );
}

#[tokio::test]
async fn replacement_and_removal_advance_authorization_and_invalidate_a_page_ticket() {
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(4));
    let accounts = open_accounts(&temp, credentials, mock.clone(), executor.clone(), true).await;
    let backend = Backend::new(accounts, executor, gate());
    let mut handle = RuntimeHandle::from_backend(backend);
    login_and_settle(&mut handle, &mock, 1, &[1, 2]).await;

    let services = handle.services().expect("services");
    let tab = handle.sessions_mut().open(Route::Repository {
        namespace: "u".into(),
        name: "r1".into(),
    });
    let context = services
        .page_context(handle.sessions(), tab, Some(1))
        .expect("page context");
    let mut slot: PageSlot<u32> = PageSlot::default();
    let ticket = slot.begin(context);
    slot.publish(ticket, 7, handle.sessions());

    // Replace the PAT of the same account (identity re-verified as user1).
    user(&mock, 1).await;
    repos(&mock, &[1, 2]).await;
    handle
        .submit(Command::Replace {
            account: key(1),
            token: Pat::from_str("ghp_new"),
        })
        .expect("replace");
    settle(&mut handle).await;
    assert!(
        slot.value(handle.sessions()).is_none(),
        "replacement must discard the old ticket"
    );

    // Remove the account entirely.
    handle.submit(Command::Remove(key(1))).expect("remove");
    settle(&mut handle).await;
    assert_eq!(handle.snapshot().active, None);
    assert!(
        handle.services().is_none(),
        "removed account must not hand out services"
    );
}

#[tokio::test]
async fn cache_open_failure_falls_back_to_online_reads() {
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(4));
    let backend = super::backend::open_with_cache(
        db_config(&temp),
        credentials,
        client(mock.clone()),
        executor,
        Err(crate::storage::cache::CacheError::Io),
        gate(),
    )
    .await
    .expect("online fallback");
    assert!(backend.cache_open_error.is_some());
    let mut handle = RuntimeHandle::from_backend(backend);
    login_and_settle(&mut handle, &mock, 1, &[1, 2]).await;
    match handle.snapshot().repositories {
        RepositoryList::Ready { outcome, .. } => assert_eq!(outcome.repositories.len(), 2),
        other => panic!("expected a ready online list, got {other:?}"),
    }
}

#[tokio::test]
async fn revocation_clears_a_displayed_page_in_one_drain() {
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(4));
    let accounts = open_accounts(&temp, credentials, mock.clone(), executor.clone(), true).await;
    let backend = Backend::new(accounts, executor, gate());
    let mut handle = RuntimeHandle::from_backend(backend);
    login_and_settle(&mut handle, &mock, 1, &[1, 2]).await;

    let services = handle.services().expect("services");
    let tab = handle.sessions_mut().open(Route::Repository {
        namespace: "u".into(),
        name: "r1".into(),
    });
    let context = services
        .page_context(handle.sessions(), tab, Some(1))
        .expect("page context");
    let binding = context.cache().cloned().expect("cache binding");
    let mut slot: PageSlot<u32> = PageSlot::default();
    let ticket = slot.begin(context);
    assert!(slot.publish(ticket, 9, handle.sessions()));
    assert_eq!(slot.value(handle.sessions()), Some(&9));

    // Revoke this repository's private scope: the captured authorization token is now stale.
    binding
        .store()
        .revoke_repository(binding.scope(), Some(1))
        .expect("revoke");
    assert!(
        slot.clear_invalid(handle.sessions()),
        "single drain must clear revoked content"
    );
    assert!(
        slot.value(handle.sessions()).is_none(),
        "revoked content must not come back"
    );
}

#[test]
fn exit_gate_waits_for_background_teardown_and_times_out_honestly() {
    let gate = ExitGate::default();
    assert!(
        gate.wait(Duration::from_millis(5)),
        "no pending work returns immediately"
    );
    let guard = gate.enter();
    assert_eq!(gate.pending(), 1);
    assert!(
        !gate.wait(Duration::from_millis(20)),
        "pending work must not report done"
    );
    drop(guard);
    assert!(gate.wait(Duration::from_secs(5)));
}

#[tokio::test]
async fn shutdown_registers_the_exit_gate_until_accounts_close() {
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(4));
    let accounts = open_accounts(&temp, credentials, mock, executor.clone(), false).await;
    let gate = ExitGate::default();
    let backend = Backend::new(accounts, executor, gate.clone());
    backend.shutdown();
    assert!(
        gate.wait(Duration::from_secs(5)),
        "close and executor join must release the gate"
    );
}
