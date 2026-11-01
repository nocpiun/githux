use super::account_tests::{config, key, user};
use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::domain::cache::{CacheCompleteness, CacheMetadata};
use crate::network::transport::MockTransport;
use crate::storage::cache::{CacheConfig, CacheError, CacheKey, CacheRead, CacheStore, ObjectKind};
use std::sync::Arc;

fn cache_config(temp: &tempfile::TempDir) -> CacheConfig {
    CacheConfig::at(temp.path().join("cache"))
}
fn account_summary(service: &AccountService, id: u64) -> AccountSummary {
    service
        .summaries()
        .into_iter()
        .find(|summary| summary.identity.key.id == id)
        .unwrap()
}
fn private_key(repository: u64) -> CacheKey {
    CacheKey::fixed(
        repository,
        true,
        ObjectKind::Blob,
        &"a".repeat(40),
        "raw-v1",
        1,
    )
    .unwrap()
}
fn account_key() -> CacheKey {
    CacheKey::mutable(None, true, "account-feed", "page=1", "json-v1", 1).unwrap()
}
fn metadata() -> CacheMetadata {
    CacheMetadata {
        representation_version: 1,
        fetched_at_ms: 1,
        verified_at_ms: 1,
        completeness: CacheCompleteness::Complete,
        etag: None,
        last_modified: None,
        cache_control: None,
    }
}
fn put(binding: &AccountCache, key: &CacheKey, value: &[u8]) {
    let store = binding.store();
    store
        .put(
            store.begin_write(binding.scope(), key).unwrap(),
            value,
            metadata(),
        )
        .unwrap();
}
fn value(binding: &AccountCache, key: &CacheKey) -> Vec<u8> {
    match binding.store().read(binding.scope(), key).unwrap() {
        CacheRead::Hit(lease) => lease.snapshot().unwrap().unwrap().bytes,
        CacheRead::Miss(reason) => panic!("expected hit, got {reason:?}"),
    }
}
fn denied(binding: &AccountCache, key: &CacheKey) {
    assert!(matches!(
        binding.store().read(binding.scope(), key),
        Err(CacheError::AccessDenied)
    ));
}
async fn open_cached(
    config: crate::storage::accounts::AccountDatabaseConfig,
    store: Arc<dyn CredentialStore>,
    mock: Arc<MockTransport>,
    executor: Arc<BackgroundExecutor>,
    cache: CacheStore,
) -> AccountService {
    AccountService::open_with_cache(
        config,
        store,
        Arc::new(GitHubClient::new(
            crate::github::client::GitHubConfig::default(),
            mock,
        )),
        executor,
        cache,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn saved_restart_replacement_revalidation_remove_and_readd_use_real_catalog_and_cache() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache_config = cache_config(&temp);
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(4));
    let cache = CacheStore::open(cache_config.clone()).unwrap();
    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    for id in [1, 2] {
        user(&mock, id).await;
        service.login(Pat::from_str("initial_fake")).await.unwrap();
    }
    let first_summary = account_summary(&service, 1);
    let first = service.cache(&first_summary).unwrap();
    let sibling = service.cache(&account_summary(&service, 2)).unwrap();
    put(&first, &private_key(10), b"account one repository ten");
    put(&first, &private_key(20), b"account one repository twenty");
    put(&first, &account_key(), b"account aggregate");
    put(&sibling, &private_key(10), b"account two repository ten");
    let old_lease = match first.store().read(first.scope(), &private_key(10)).unwrap() {
        CacheRead::Hit(lease) => lease,
        CacheRead::Miss(_) => panic!(),
    };
    let old_writer = first
        .store()
        .begin_write(first.scope(), &private_key(10))
        .unwrap();
    service.close().await.unwrap();

    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    let restored = service.cache(&account_summary(&service, 1)).unwrap();
    assert_eq!(restored.scope(), first.scope());
    assert_eq!(
        value(&restored, &private_key(10)),
        b"account one repository ten"
    );
    user(&mock, 1).await;
    service
        .replace(key(1), Pat::from_str("replacement_fake"))
        .await
        .unwrap();
    assert!(matches!(
        old_lease.snapshot(),
        Err(CacheError::AccessDenied)
    ));
    assert!(matches!(
        first.store().put(old_writer, b"late old bytes", metadata()),
        Err(CacheError::Stale)
    ));
    denied(&restored, &private_key(10));
    denied(&restored, &private_key(20));
    denied(&restored, &account_key());
    assert_eq!(
        value(
            &service.cache(&account_summary(&service, 2)).unwrap(),
            &private_key(10)
        ),
        b"account two repository ten"
    );
    // Repository proof opens exactly one repository, never the account aggregate.
    let proof = restored
        .store()
        .begin_revalidation(restored.scope(), Some(10))
        .unwrap();
    restored.store().permission_confirmed(proof).unwrap();
    assert_eq!(
        value(&restored, &private_key(10)),
        b"account one repository ten"
    );
    denied(&restored, &private_key(20));
    denied(&restored, &account_key());
    service.close().await.unwrap();

    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    denied(
        &service.cache(&account_summary(&service, 1)).unwrap(),
        &private_key(20),
    );
    service.remove_saved(key(1)).await.unwrap();
    assert!(service.cache(&first_summary).is_none());
    assert_eq!(
        value(
            &service.cache(&account_summary(&service, 2)).unwrap(),
            &private_key(10)
        ),
        b"account two repository ten"
    );
    user(&mock, 1).await;
    service.login(Pat::from_str("readded_fake")).await.unwrap();
    assert!(service.cache(&first_summary).is_none());
    let readded = service.cache(&account_summary(&service, 1)).unwrap();
    assert_ne!(readded.scope().partition(), first.scope().partition());
    assert!(matches!(
        readded.store().read(readded.scope(), &private_key(10)),
        Ok(CacheRead::Miss(_))
    ));
    denied(&restored, &private_key(10));
    service.close().await.unwrap();
}

#[tokio::test]
async fn rejected_credentials_stay_closed_across_restart_and_late_rejection_is_stale() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    user(&mock, 1).await;
    service.login(Pat::from_str("old_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(7), b"private");
    let session = service.repo_service().session().unwrap();
    service
        .repo_service()
        .credential_rejected(session.context())
        .await
        .unwrap();
    denied(&binding, &private_key(7));
    let stale = session.context().clone();
    user(&mock, 1).await;
    service
        .replace(key(1), Pat::from_str("new_fake"))
        .await
        .unwrap();
    assert_eq!(
        service.repo_service().credential_rejected(&stale).await,
        Err(GitHubError::StaleContext)
    );
    // A late rejection did not turn the replacement's active quarantine into a
    // rejected scope; the same repository proof is still required and sufficient.
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    binding
        .store()
        .permission_confirmed(
            binding
                .store()
                .begin_revalidation(binding.scope(), Some(7))
                .unwrap(),
        )
        .unwrap();
    assert_eq!(value(&binding, &private_key(7)), b"private");
    service.close().await.unwrap();

    let service = open_cached(accounts, credentials, mock, executor, cache).await;
    assert_eq!(
        value(
            &service.cache(&account_summary(&service, 1)).unwrap(),
            &private_key(7)
        ),
        b"private"
    );
    service.close().await.unwrap();
}

#[tokio::test]
async fn retirement_before_catalog_failure_is_fail_closed_and_recovers_on_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    user(&mock, 1).await;
    service.login(Pat::from_str("old_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(1), b"private");
    let lease = match binding
        .store()
        .read(binding.scope(), &private_key(1))
        .unwrap()
    {
        CacheRead::Hit(lease) => lease,
        CacheRead::Miss(_) => panic!(),
    };
    let connection = rusqlite::Connection::open(&accounts.path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_remove BEFORE DELETE ON accounts BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(matches!(
        service.remove_saved(key(1)).await,
        Err(AccountError::Storage(_))
    ));
    denied(&binding, &private_key(1));
    assert!(matches!(lease.snapshot(), Err(CacheError::AccessDenied)));
    assert!(service.cache(&account_summary(&service, 1)).is_none());
    assert!(service.recovery_pending().await.unwrap());
    assert_eq!(service.summaries().len(), 1);
    connection
        .execute_batch("DROP TRIGGER fail_remove")
        .unwrap();
    drop(connection);
    service.close().await.unwrap();
    let service = open_cached(accounts, credentials, mock, executor, cache).await;
    assert!(service.cache(&account_summary(&service, 1)).is_some());
    service.close().await.unwrap();
}

#[tokio::test]
async fn committed_removal_reconciles_orphan_on_restart_and_physical_cleanup_failure_is_retryable()
{
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let mut cache_config = cache_config(&temp);
    cache_config.inline_bytes = 0;
    let cache = CacheStore::open(cache_config.clone()).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    for id in [1, 2] {
        user(&mock, id).await;
        service.login(Pat::from_str("fake")).await.unwrap();
    }
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(1), b"private");
    // Simulate a crash after the catalog commit: the cache still contains a live
    // partition and object when the next process begins.
    let mut directory = service.registry.lock().unwrap().directory();
    directory
        .accounts
        .retain(|saved| saved.identity.key != key(1));
    service
        .database
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .commit(&directory, None, None)
        .unwrap();
    drop(leaseless(&binding, &private_key(1)));
    service.close().await.unwrap();
    assert_eq!(cache.usage().unwrap().1, 1);
    let service = open_cached(accounts, credentials, mock, executor, cache.clone()).await;
    assert!(
        service
            .summaries()
            .iter()
            .all(|summary| summary.identity.key != key(1))
    );
    denied(&binding, &private_key(1));
    assert_eq!(cache.usage().unwrap().1, 0);
    service.close().await.unwrap();
}
fn leaseless(binding: &AccountCache, key: &CacheKey) -> CacheRead {
    binding.store().read(binding.scope(), key).unwrap()
}

#[tokio::test]
async fn legacy_open_and_broken_reconciliation_keep_account_reads_without_cache() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let mut cache_config = cache_config(&temp);
    cache_config.inline_bytes = 0;
    let cache = CacheStore::open(cache_config.clone()).unwrap();
    // A live scope with no saved account makes reconciliation fail closed even
    // when its physical cleanup later encounters an unknown filesystem error.
    let orphan = cache.saved_account(key(1)).unwrap();
    let writer = cache.begin_write(&orphan, &private_key(1)).unwrap();
    cache.put(writer, b"orphan", metadata()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let objects = cache_config(&temp).root.join("objects");
        let mut permissions = std::fs::metadata(&objects).unwrap().permissions();
        permissions.set_mode(0o500);
        std::fs::set_permissions(&objects, permissions).unwrap();
    }
    #[cfg(windows)]
    let object_guard = {
        use std::os::windows::fs::OpenOptionsExt;
        let object = std::fs::read_dir(cache_config.root.join("objects"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(object)
            .unwrap()
    };
    let service = open_cached(accounts, credentials, mock, executor, cache).await;
    assert!(service.summaries().is_empty());
    assert!(service.active_context().is_none());
    assert!(service.recovery_pending().await.unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let objects = cache_config(&temp).root.join("objects");
        let mut permissions = std::fs::metadata(&objects).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&objects, permissions).unwrap();
    }
    #[cfg(windows)]
    drop(object_guard);
    service.recover().await.unwrap();
    assert!(!service.recovery_pending().await.unwrap());
    service.close().await.unwrap();
}

#[tokio::test]
async fn cache_gate_failure_aborts_replacement_and_preserves_old_credential() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(
        accounts,
        credentials.clone(),
        mock.clone(),
        executor,
        cache.clone(),
    )
    .await;
    user(&mock, 1).await;
    service.login(Pat::from_str("old_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(1), b"old private");
    let old_credential = service
        .registry
        .lock()
        .unwrap()
        .credential_ref(key(1))
        .unwrap();
    // A real SQLite trigger fails after access_event has assigned its new epoch
    // value but before commit. SQLite rolls the update back; the account commit
    // must not proceed, and reconciliation can safely reissue the old partition.
    let lease = match binding
        .store()
        .read(binding.scope(), &private_key(1))
        .unwrap()
    {
        CacheRead::Hit(lease) => lease,
        CacheRead::Miss(_) => panic!(),
    };
    let writer = binding
        .store()
        .begin_write(binding.scope(), &private_key(1))
        .unwrap();
    let connection =
        rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_gate BEFORE UPDATE ON scopes BEGIN SELECT RAISE(ABORT,'injected gate failure'); END;").unwrap();
    user(&mock, 1).await;
    assert!(matches!(
        service.replace(key(1), Pat::from_str("new_fake")).await,
        Err(LoginError::CredentialSave(_))
    ));
    assert!(service.cache(&account_summary(&service, 1)).is_none());
    denied(&binding, &private_key(1));
    assert!(matches!(lease.snapshot(), Err(CacheError::AccessDenied)));
    assert!(matches!(
        binding.store().put(writer, b"late", metadata()),
        Err(CacheError::AccessDenied) | Err(CacheError::Stale)
    ));
    assert_eq!(
        credentials.load(&old_credential).unwrap().unwrap().expose(),
        "old_fake"
    );
    connection.execute_batch("DROP TRIGGER fail_gate").unwrap();
    drop(connection);
    service.recover().await.unwrap();
    assert_eq!(
        value(
            &service.cache(&account_summary(&service, 1)).unwrap(),
            &private_key(1)
        ),
        b"old private"
    );
    service.close().await.unwrap();
}

#[tokio::test]
async fn replacement_quarantine_survives_catalog_failure_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    user(&mock, 1).await;
    service.login(Pat::from_str("old_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(4), b"private");
    let old_credential = service
        .registry
        .lock()
        .unwrap()
        .credential_ref(key(1))
        .unwrap();
    let connection = rusqlite::Connection::open(&accounts.path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_replace BEFORE DELETE ON accounts BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    user(&mock, 1).await;
    assert!(matches!(
        service.replace(key(1), Pat::from_str("new_fake")).await,
        Err(LoginError::Storage(_))
    ));
    denied(&binding, &private_key(4));
    assert_eq!(
        credentials.load(&old_credential).unwrap().unwrap().expose(),
        "old_fake"
    );
    connection
        .execute_batch("DROP TRIGGER fail_replace")
        .unwrap();
    drop(connection);
    service.close().await.unwrap();
    let service = open_cached(accounts, credentials, mock, executor, cache).await;
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    denied(&binding, &private_key(4));
    binding
        .store()
        .permission_confirmed(
            binding
                .store()
                .begin_revalidation(binding.scope(), Some(4))
                .unwrap(),
        )
        .unwrap();
    assert_eq!(value(&binding, &private_key(4)), b"private");
    service.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_rejection_waits_for_replacement_and_cannot_poison_the_new_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(4));
    let service =
        Arc::new(open_cached(accounts, credentials, mock.clone(), executor, cache.clone()).await);
    user(&mock, 1).await;
    service.login(Pat::from_str("old_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(3), b"private");
    let old_context = service.repo_service().session().unwrap().context().clone();
    let old_epoch = old_context.auth().epoch;
    // The old rejection has reached its blocking worker. Let replacement win
    // the real commit lock, then release the old worker: its in-worker context
    // check must reject the result instead of poisoning the replacement epoch.
    let repos = service.repo_service();
    let (entered, release) = repos.pause_rejection();
    let rejected = tokio::spawn(async move { repos.credential_rejected(&old_context).await });
    entered.await.unwrap();
    user(&mock, 1).await;
    service
        .replace(key(1), Pat::from_str("new_fake"))
        .await
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(rejected.await.unwrap(), Err(GitHubError::StaleContext));
    let current = service.active_context().unwrap();
    assert!(current.epoch > old_epoch);
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    binding
        .store()
        .permission_confirmed(
            binding
                .store()
                .begin_revalidation(binding.scope(), Some(3))
                .unwrap(),
        )
        .unwrap();
    assert_eq!(value(&binding, &private_key(3)), b"private");
    service.close().await.unwrap();
}

#[test]
fn scope_enumeration_is_read_only_and_validates_persisted_account_ids() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let first = cache.saved_account(key(1)).unwrap();
    assert_eq!(cache.live_scopes().unwrap(), vec![first]);
}
