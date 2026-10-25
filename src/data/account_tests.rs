use super::*;
use crate::domain::account::AuthorizationState;
use crate::domain::{AccountKey, HostId};
use crate::github::client::GitHubConfig;
use crate::network::transport::{HttpResponse, MockTransport};
use crate::storage::accounts::{AccountDatabase, AccountDatabaseConfig};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

pub(super) fn key(id: u64) -> AccountKey {
    AccountKey::new(id, HostId::github())
}
pub(super) async fn user(mock: &MockTransport, id: u64) {
    mock.enqueue(Ok(HttpResponse {
        status: 200,
        headers: vec![],
        body: format!(r#"{{"id":{id},"login":"user{id}"}}"#),
    }))
    .await;
}
pub(super) fn config(temp: &tempfile::TempDir) -> AccountDatabaseConfig {
    AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3"))
}
pub(super) async fn open(
    config: AccountDatabaseConfig,
    store: Arc<dyn CredentialStore>,
    mock: Arc<MockTransport>,
    exec: Arc<BackgroundExecutor>,
) -> AccountService {
    AccountService::open(
        config,
        store,
        Arc::new(GitHubClient::new(GitHubConfig::default(), mock)),
        exec,
    )
    .await
    .expect("open")
}

#[tokio::test]
async fn restart_restores_directory_explicit_selection_and_removal_without_secret_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    let store = Arc::new(crate::auth::memory::InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let exec = Arc::new(BackgroundExecutor::new(4));
    let a = open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await;
    for id in [1, 2] {
        user(&mock, id).await;
        a.login(Pat::from_str("ghp_unique_fake_secret_never_sqlite"))
            .await
            .unwrap();
    }
    a.set_active(key(1)).await.unwrap();
    let before = a.active_context().unwrap();
    a.set_active(key(2)).await.unwrap();
    a.set_active(key(1)).await.unwrap();
    assert_ne!(Some(before), a.active_context());
    assert_eq!(
        a.repo_service().set_active(key(2)),
        Err(RegistryError::PersistenceRequired)
    );
    a.close().await.unwrap();
    let b = open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await;
    assert_eq!(b.summaries().len(), 2);
    assert!(
        b.summaries()
            .iter()
            .all(|a| a.authorization == AuthorizationState::PendingVerification)
    );
    assert_eq!(b.active_context().unwrap().account, key(1));
    let pending_epoch = b.summaries()[0].epoch;
    user(&mock, 1).await;
    let verified = b.revalidate(key(1)).await.unwrap();
    assert_eq!(verified.authorization, AuthorizationState::Verified);
    assert!(verified.epoch > pending_epoch);
    b.remove_saved(key(1)).await.unwrap();
    b.close().await.unwrap();
    let c = open(cfg, store, mock, exec).await;
    assert_eq!(c.summaries().len(), 1);
    assert_eq!(c.active_context().unwrap().account, key(2));
    c.close().await.unwrap();
    for entry in std::fs::read_dir(temp.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(
            !bytes
                .windows(b"ghp_unique_fake_secret_never_sqlite".len())
                .any(|w| w == b"ghp_unique_fake_secret_never_sqlite")
        );
    }
}

#[derive(Default)]
struct FaultStore {
    inner: crate::auth::memory::InMemoryCredentialStore,
    fail_delete: AtomicBool,
    fail_save_after_write: AtomicBool,
    lock_after_save: Mutex<Option<std::path::PathBuf>>,
    held: Mutex<Option<rusqlite::Connection>>,
}
impl CredentialStore for FaultStore {
    fn save(&self, r: &CredentialRef, t: &Pat) -> Result<(), CredentialError> {
        self.inner.save(r, t)?;
        if let Some(path) = self.lock_after_save.lock().unwrap().take() {
            let conn = rusqlite::Connection::open(path).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            *self.held.lock().unwrap() = Some(conn);
        }
        if self.fail_save_after_write.load(Ordering::SeqCst) {
            Err(CredentialError::Platform {
                message: "injected after write".into(),
            })
        } else {
            Ok(())
        }
    }
    fn load(&self, r: &CredentialRef) -> Result<Option<Pat>, CredentialError> {
        self.inner.load(r)
    }
    fn delete(&self, r: &CredentialRef) -> Result<(), CredentialError> {
        if self.fail_delete.load(Ordering::SeqCst) {
            Err(CredentialError::Platform {
                message: "injected delete".into(),
            })
        } else {
            self.inner.delete(r)
        }
    }
    fn describe(&self) -> &'static str {
        "fault-injection"
    }
}

#[tokio::test]
async fn real_sql_lock_after_secret_save_preserves_old_pat_and_durable_cleanup_route() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = config(&temp);
    cfg.busy_timeout = std::time::Duration::from_millis(20);
    let store = Arc::new(FaultStore::default());
    let mock = Arc::new(MockTransport::new());
    let exec = Arc::new(BackgroundExecutor::new(2));
    let a = open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await;
    user(&mock, 1).await;
    a.login(Pat::from_str("old_fake")).await.unwrap();
    let old = a.registry.lock().unwrap().credential_ref(key(1)).unwrap();
    let epoch = a.summaries()[0].epoch;
    *store.lock_after_save.lock().unwrap() = Some(cfg.path.clone());
    store.fail_delete.store(true, Ordering::SeqCst);
    user(&mock, 1).await;
    assert_eq!(
        a.replace(key(1), Pat::from_str("new_fake")).await,
        Err(LoginError::Storage(StorageError::Busy))
    );
    assert_eq!(a.summaries()[0].epoch, epoch);
    assert_eq!(store.load(&old).unwrap().unwrap().expose(), "old_fake");
    let conn = rusqlite::Connection::open(&cfg.path).unwrap();
    let pending: i64 = conn
        .query_row("SELECT count(*) FROM cleanup", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, 1);
    drop(conn);
    store.held.lock().unwrap().take();
    a.close().await.unwrap();
    let b = open(cfg.clone(), store.clone(), mock, exec).await;
    assert!(b.recovery_pending().await.unwrap());
    assert_eq!(b.recover().await, Err(AccountError::CleanupPending));
    assert_eq!(b.summaries().len(), 1);
    store.fail_delete.store(false, Ordering::SeqCst);
    b.recover().await.unwrap();
    assert_eq!(store.load(&old).unwrap().unwrap().expose(), "old_fake");
    b.close().await.unwrap();
    let db = AccountDatabase::open(cfg).unwrap();
    assert!(db.pending().unwrap().is_empty());
    db.close();
}

#[tokio::test]
async fn save_error_after_write_and_remove_cleanup_failure_remain_recoverable() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    let store = Arc::new(FaultStore::default());
    let mock = Arc::new(MockTransport::new());
    let exec = Arc::new(BackgroundExecutor::new(2));
    let a = open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await;
    store.fail_save_after_write.store(true, Ordering::SeqCst);
    store.fail_delete.store(true, Ordering::SeqCst);
    user(&mock, 1).await;
    assert!(matches!(
        a.login(Pat::from_str("fake")).await,
        Err(LoginError::CredentialSave(_))
    ));
    assert!(a.summaries().is_empty());
    assert_eq!(a.recover().await, Err(AccountError::CleanupPending));
    store.fail_save_after_write.store(false, Ordering::SeqCst);
    store.fail_delete.store(false, Ordering::SeqCst);
    a.recover().await.unwrap();
    user(&mock, 1).await;
    a.login(Pat::from_str("fake")).await.unwrap();
    store.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        a.remove_saved(key(1)).await,
        Err(AccountError::CleanupPending)
    );
    assert!(a.summaries().is_empty());
    a.close().await.unwrap();
    let b = open(cfg, store.clone(), mock, exec).await;
    assert!(b.summaries().is_empty());
    store.fail_delete.store(false, Ordering::SeqCst);
    b.recover().await.unwrap();
}

#[tokio::test]
async fn missing_unavailable_and_failed_revalidation_preserve_saved_identity() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    let store = Arc::new(FaultStore::default());
    let mock = Arc::new(MockTransport::new());
    let exec = Arc::new(BackgroundExecutor::new(2));
    let a = open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await;
    user(&mock, 1).await;
    a.login(Pat::from_str("fake")).await.unwrap();
    a.close().await.unwrap();
    let b = open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await;
    mock.enqueue(Ok(HttpResponse {
        status: 401,
        headers: vec![],
        body: "{}".into(),
    }))
    .await;
    assert_eq!(
        b.revalidate(key(1)).await,
        Err(LoginError::GitHub(GitHubError::Unauthorized))
    );
    assert_eq!(
        b.summaries()[0].authorization,
        AuthorizationState::VerificationFailed(GitHubError::Unauthorized)
    );
    user(&mock, 2).await;
    assert!(matches!(
        b.replace(key(1), Pat::from_str("other_fake")).await,
        Err(LoginError::IdentityMismatch { .. })
    ));
    let credential = b.registry.lock().unwrap().credential_ref(key(1)).unwrap();
    store.delete(&credential).unwrap();
    b.close().await.unwrap();
    let c = open(cfg.clone(), store, mock.clone(), exec.clone()).await;
    assert_eq!(
        c.summaries()[0].authorization,
        AuthorizationState::MissingCredential
    );
    assert_eq!(
        c.revalidate(key(1)).await,
        Err(LoginError::CredentialSave(CredentialError::NotFound))
    );
    c.close().await.unwrap();
    let d = open(
        cfg,
        Arc::new(crate::auth::credential::UnsupportedStore),
        mock,
        exec,
    )
    .await;
    assert_eq!(
        d.summaries()[0].authorization,
        AuthorizationState::CredentialUnavailable
    );
    assert_eq!(d.summaries()[0].identity.key, key(1));
}

#[tokio::test]
async fn cancelled_login_closed_service_and_second_instance_do_not_claim_success() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    let mock = Arc::new(MockTransport::new());
    let store = Arc::new(FaultStore::default());
    let exec = Arc::new(BackgroundExecutor::new(2));
    let a = open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await;
    let second = AccountService::open(
        cfg,
        store,
        Arc::new(GitHubClient::new(GitHubConfig::default(), mock.clone())),
        exec,
    )
    .await;
    assert!(matches!(second, Err(StorageError::InUse)));
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    assert_eq!(
        a.login_cancellable(Pat::from_str("fake"), cancel).await,
        Err(LoginError::GitHub(GitHubError::Cancelled))
    );
    assert!(a.summaries().is_empty());
    a.close().await.unwrap();
    assert_eq!(
        a.set_active(key(1)).await,
        Err(AccountError::Storage(StorageError::Closed))
    );
    assert_eq!(
        a.login(Pat::from_str("fake")).await,
        Err(LoginError::Storage(StorageError::Closed))
    );
}

#[tokio::test]
async fn metadata_trigger_failure_does_not_publish_selection_or_remove_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    let store = Arc::new(FaultStore::default());
    let mock = Arc::new(MockTransport::new());
    let exec = Arc::new(BackgroundExecutor::new(2));
    let a = open(cfg.clone(), store.clone(), mock.clone(), exec).await;
    for id in [1, 2] {
        user(&mock, id).await;
        a.login(Pat::from_str("fake")).await.unwrap();
    }
    let before = a.active_context();
    let reference = a.registry.lock().unwrap().credential_ref(key(2)).unwrap();
    let conn = rusqlite::Connection::open(&cfg.path).unwrap();
    conn.execute_batch("CREATE TRIGGER injected_failure BEFORE UPDATE ON selection BEGIN SELECT RAISE(ABORT,'injected metadata failure'); END;").unwrap();
    assert_eq!(
        a.set_active(key(1)).await,
        Err(AccountError::Storage(StorageError::Database))
    );
    assert_eq!(
        a.remove_saved(key(2)).await,
        Err(AccountError::Storage(StorageError::Database))
    );
    assert_eq!(a.active_context(), before);
    assert_eq!(a.summaries().len(), 2);
    assert!(store.load(&reference).unwrap().is_some());
    conn.execute_batch("DROP TRIGGER injected_failure").unwrap();
    drop(conn);
    a.remove_saved(key(2)).await.unwrap();
    a.close().await.unwrap();
}

#[tokio::test]
async fn interrupted_staged_save_is_cleaned_on_reopen_without_creating_account() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    let store = Arc::new(FaultStore::default());
    let reference = CredentialRef::versioned(HostId::github(), 7, uuid::Uuid::new_v4().to_string());
    let db = AccountDatabase::open(cfg.clone()).unwrap();
    db.stage(&reference).unwrap();
    store
        .save(&reference, &Pat::from_str("interrupted_fake"))
        .unwrap();
    db.close();
    let a = open(
        cfg,
        store.clone(),
        Arc::new(MockTransport::new()),
        Arc::new(BackgroundExecutor::new(1)),
    )
    .await;
    assert!(a.summaries().is_empty());
    assert!(!a.recovery_pending().await.unwrap());
    assert!(store.load(&reference).unwrap().is_none());
    a.close().await.unwrap();
}

#[test]
fn schema_migration_future_corrupt_readonly_and_real_busy_errors_preserve_file() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    AccountDatabase::open(cfg.clone()).unwrap().close();
    let conn = rusqlite::Connection::open(&cfg.path).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    conn.pragma_update(None, "user_version", 2).unwrap();
    drop(conn);
    let before = std::fs::read(&cfg.path).unwrap();
    assert!(matches!(
        AccountDatabase::open(cfg.clone()),
        Err(StorageError::FutureSchema(2))
    ));
    assert_eq!(std::fs::read(&cfg.path).unwrap(), before);
    std::fs::write(&cfg.path, b"invalid sqlite fixture").unwrap();
    assert!(matches!(
        AccountDatabase::open(cfg.clone()),
        Err(StorageError::Corrupt)
    ));
    assert_eq!(std::fs::read(&cfg.path).unwrap(), b"invalid sqlite fixture");
    std::fs::remove_file(&cfg.path).unwrap();
    AccountDatabase::open(cfg.clone()).unwrap().close();
    let mut ro = cfg.clone();
    ro.read_only = true;
    let mut db = AccountDatabase::open(ro).unwrap();
    assert_eq!(
        db.commit(&Default::default(), None, None),
        Err(StorageError::ReadOnly)
    );
    db.close();
    let conn = rusqlite::Connection::open(&cfg.path).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut busy = cfg;
    busy.busy_timeout = std::time::Duration::from_millis(10);
    assert!(matches!(
        AccountDatabase::open(busy),
        Err(StorageError::Busy)
    ));
}
