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
fn test_binding(cache: &CacheStore, scope: &crate::storage::cache::CacheScope) -> AccountCache {
    AccountCache::from_parts_for_test(cache.clone(), scope.clone())
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
    service.set_active(key(1)).await.unwrap();
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
    // Re-acquire the binding through the new session. The persistent partition
    // is intentionally retained across PAT replacement; its cache epoch/gates
    // remain quarantined, and proof publication must pass the service context.
    let replacement_session = service.repo_service().session().unwrap();
    let current = service
        .repo_service()
        .cache(replacement_session.context())
        .unwrap();
    denied(&current, &private_key(10));
    let proof = service
        .repo_service()
        .begin_revalidation(replacement_session.context(), Some(10))
        .unwrap();
    service
        .repo_service()
        .permission_confirmed(replacement_session.context(), proof)
        .await
        .unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_session_proof_cannot_reopen_repository_during_replacement_commit() {
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
    put(&binding, &private_key(6), b"private");
    // Quarantine before the old proof begins, while the old registry epoch is
    // still published. This is the exact replacement commit window.
    cache
        .access_event(
            binding.scope(),
            crate::storage::cache::AccessEvent::PatReplaced,
        )
        .unwrap();
    let session = service.repo_service().session().unwrap();
    let context = session.context().clone();
    let proof = service
        .repo_service()
        .begin_revalidation(&context, Some(6))
        .unwrap();
    let repos = service.repo_service();
    let (entered, release) = repos.pause_rejection();
    let confirmed = tokio::spawn(async move { repos.permission_confirmed(&context, proof).await });
    entered.await.unwrap();
    user(&mock, 1).await;
    service
        .replace(key(1), Pat::from_str("new_fake"))
        .await
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(confirmed.await.unwrap(), Err(CacheError::Stale));
    denied(
        &service.cache(&account_summary(&service, 1)).unwrap(),
        &private_key(6),
    );
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
    let session = service.repo_service().session().unwrap();
    let proof = service
        .repo_service()
        .begin_revalidation(session.context(), Some(7))
        .unwrap();
    service
        .repo_service()
        .permission_confirmed(session.context(), proof)
        .await
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
    // The partition is already durably retired, so a direct remove retry does
    // not need to provision or retire another partition before the catalog.
    service.remove_saved(key(1)).await.unwrap();
    assert!(service.summaries().is_empty());
    service.close().await.unwrap();
    let service = open_cached(accounts, credentials, mock, executor, cache).await;
    assert!(service.summaries().is_empty());
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
        let objects = cache_config.root.join("objects");
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
        let objects = cache_config.root.join("objects");
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
    // Retry WITHOUT recover: the withdrawn map entry must not be mistaken for
    // a cache-disabled account. It still has to quarantine the existing scope.
    user(&mock, 1).await;
    service
        .replace(key(1), Pat::from_str("new_fake"))
        .await
        .unwrap();
    let replaced = service.cache(&account_summary(&service, 1)).unwrap();
    denied(&replaced, &private_key(1));
    let session = service.repo_service().session().unwrap();
    let proof = service
        .repo_service()
        .begin_revalidation(session.context(), Some(1))
        .unwrap();
    service
        .repo_service()
        .permission_confirmed(session.context(), proof)
        .await
        .unwrap();
    assert_eq!(value(&replaced, &private_key(1)), b"old private");
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
    let session = service.repo_service().session().unwrap();
    let proof = service
        .repo_service()
        .begin_revalidation(session.context(), Some(4))
        .unwrap();
    service
        .repo_service()
        .permission_confirmed(session.context(), proof)
        .await
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
    let session = service.repo_service().session().unwrap();
    let proof = service
        .repo_service()
        .begin_revalidation(session.context(), Some(3))
        .unwrap();
    service
        .repo_service()
        .permission_confirmed(session.context(), proof)
        .await
        .unwrap();
    assert_eq!(value(&binding, &private_key(3)), b"private");
    service.close().await.unwrap();
}

#[tokio::test]
async fn failed_rejection_is_replayed_before_recovery_reissues_scope() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(accounts, credentials, mock.clone(), executor, cache.clone()).await;
    user(&mock, 1).await;
    service.login(Pat::from_str("fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(1), b"private");
    let context = service.repo_service().session().unwrap().context().clone();
    let connection =
        rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_reject BEFORE UPDATE ON scopes BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        service
            .repo_service()
            .credential_rejected(&context)
            .await
            .is_err()
    );
    connection
        .execute_batch("DROP TRIGGER fail_reject")
        .unwrap();
    drop(connection);
    service.recover().await.unwrap();
    denied(
        &service.cache(&account_summary(&service, 1)).unwrap(),
        &private_key(1),
    );
    assert!(!service.recovery_pending().await.unwrap());
    service.close().await.unwrap();
}

#[tokio::test]
async fn user_revalidation_rejects_only_unauthorized_and_identity_success_does_not_unlock() {
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
    service.login(Pat::from_str("fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(1), b"private");
    for status in [500, 403, 404, 429] {
        mock.enqueue(Ok(crate::network::transport::HttpResponse {
            status,
            headers: vec![],
            body: "{}".into(),
        }))
        .await;
        assert!(service.revalidate(key(1)).await.is_err());
        assert_eq!(value(&binding, &private_key(1)), b"private");
    }
    mock.enqueue(Ok(crate::network::transport::HttpResponse {
        status: 401,
        headers: vec![],
        body: "{}".into(),
    }))
    .await;
    assert_eq!(
        service.revalidate(key(1)).await,
        Err(LoginError::GitHub(GitHubError::Unauthorized))
    );
    denied(&binding, &private_key(1));
    user(&mock, 1).await;
    service.revalidate(key(1)).await.unwrap();
    denied(&binding, &private_key(1));
    service.close().await.unwrap();
    let service = open_cached(accounts, credentials, mock, executor, cache).await;
    denied(
        &service.cache(&account_summary(&service, 1)).unwrap(),
        &private_key(1),
    );
    service.close().await.unwrap();
}

#[tokio::test]
async fn failed_rejection_journal_survives_restart_and_replays_before_cache_access() {
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
    service.login(Pat::from_str("fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    put(&binding, &private_key(1), b"private");
    let context = service.repo_service().session().unwrap().context().clone();
    let connection =
        rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_reject BEFORE UPDATE ON scopes BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        service
            .repo_service()
            .credential_rejected(&context)
            .await
            .is_err()
    );
    service.close().await.unwrap();
    connection
        .execute_batch("DROP TRIGGER fail_reject")
        .unwrap();
    drop(connection);
    // Release every store clone: this exercises a fresh store instance, where
    // the process-local suspension map no longer exists.
    drop(binding);
    drop(service);
    drop(cache);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
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
        &private_key(1),
    );
    let connection = rusqlite::Connection::open(&accounts.path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM cache_recovery", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(connection);
    user(&mock, 1).await;
    service
        .replace(key(1), Pat::from_str("new_fake"))
        .await
        .unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    denied(&binding, &private_key(1));
    let session = service.repo_service().session().unwrap();
    let proof = service
        .repo_service()
        .begin_revalidation(session.context(), Some(1))
        .unwrap();
    service
        .repo_service()
        .permission_confirmed(session.context(), proof)
        .await
        .unwrap();
    assert_eq!(value(&binding, &private_key(1)), b"private");
    service.close().await.unwrap();
}

#[tokio::test]
async fn blocked_orphan_is_retired_before_readd_gets_a_fresh_partition() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let orphan = cache.saved_account(key(1)).unwrap();
    put(&test_binding(&cache, &orphan), &private_key(1), b"orphan");
    let connection =
        rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_orphan BEFORE UPDATE ON scopes BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(accounts, credentials, mock.clone(), executor, cache.clone()).await;
    assert!(service.recovery_pending().await.unwrap());
    connection
        .execute_batch("DROP TRIGGER fail_orphan")
        .unwrap();
    drop(connection);
    user(&mock, 1).await;
    service.login(Pat::from_str("new_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    assert_ne!(binding.scope().partition(), orphan.partition());
    assert!(matches!(
        binding.store().read(binding.scope(), &private_key(1)),
        Ok(CacheRead::Miss(_))
    ));
    denied(&test_binding(&cache, &orphan), &private_key(1));
    service.close().await.unwrap();
}

#[tokio::test]
async fn recovery_retires_blocked_partition_after_readd_before_issuing_binding() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let orphan = cache.saved_account(key(1)).unwrap();
    put(&test_binding(&cache, &orphan), &private_key(1), b"orphan");
    let connection =
        rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_orphan BEFORE UPDATE ON scopes BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(accounts, credentials, mock.clone(), executor, cache.clone()).await;
    user(&mock, 1).await;
    // Do not commit a new account whose old partition still cannot be retired.
    // This is what makes a later restart safe without an in-memory marker.
    assert!(matches!(
        service.login(Pat::from_str("new_fake")).await,
        Err(LoginError::CredentialSave(_))
    ));
    assert!(service.summaries().is_empty());
    connection
        .execute_batch("DROP TRIGGER fail_orphan")
        .unwrap();
    drop(connection);
    user(&mock, 1).await;
    service.login(Pat::from_str("new_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    assert_ne!(binding.scope().partition(), orphan.partition());
    assert!(matches!(
        binding.store().read(binding.scope(), &private_key(1)),
        Ok(CacheRead::Miss(_))
    ));
    denied(&test_binding(&cache, &orphan), &private_key(1));
    service.close().await.unwrap();
}

#[tokio::test]
async fn failed_retire_remove_retry_and_credential_cleanup_cannot_reuse_live_partition() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(super::account_tests::FaultStore::default());
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
    put(&binding, &private_key(1), b"private");
    let connection =
        rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_retire BEFORE UPDATE ON scopes BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert_eq!(
        service.remove_saved(key(1)).await,
        Err(AccountError::CleanupPending)
    );
    connection
        .execute_batch("DROP TRIGGER fail_retire")
        .unwrap();
    drop(connection);
    credentials
        .fail_delete
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        service.remove_saved(key(1)).await,
        Err(AccountError::CleanupPending)
    );
    assert!(service.recovery_pending().await.unwrap());
    credentials
        .fail_delete
        .store(false, std::sync::atomic::Ordering::SeqCst);
    user(&mock, 1).await;
    service.login(Pat::from_str("new_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    assert!(matches!(
        binding.store().read(binding.scope(), &private_key(1)),
        Ok(CacheRead::Miss(_))
    ));
    service.close().await.unwrap();
}

#[tokio::test]
async fn failed_scope_enumeration_blocks_provisioning_until_reconciliation_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let orphan = cache.saved_account(key(1)).unwrap();
    put(&test_binding(&cache, &orphan), &private_key(1), b"orphan");
    let locked =
        rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    locked.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(accounts, credentials, mock.clone(), executor, cache.clone()).await;
    assert!(service.recovery_pending().await.unwrap());
    user(&mock, 1).await;
    assert!(matches!(
        service.login(Pat::from_str("new_fake")).await,
        Err(LoginError::CredentialSave(_))
    ));
    assert!(service.summaries().is_empty());
    drop(locked);
    user(&mock, 1).await;
    service.login(Pat::from_str("new_fake")).await.unwrap();
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    assert_ne!(binding.scope().partition(), orphan.partition());
    assert!(matches!(
        binding.store().read(binding.scope(), &private_key(1)),
        Ok(CacheRead::Miss(_))
    ));
    denied(&test_binding(&cache, &orphan), &private_key(1));
    service.close().await.unwrap();
}

#[tokio::test]
async fn network_only_catalog_changes_replay_before_later_cache_enabled_open() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let orphan = cache.saved_account(key(1)).unwrap();
    put(&test_binding(&cache, &orphan), &private_key(1), b"orphan");
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let online = super::account_tests::open(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
    )
    .await;
    user(&mock, 1).await;
    online.login(Pat::from_str("first_fake")).await.unwrap();
    // A second network-only save before any cache binding must preserve the
    // stronger retirement barrier, rather than downgrade it to replacement.
    user(&mock, 1).await;
    online
        .replace(key(1), Pat::from_str("first_updated_fake"))
        .await
        .unwrap();
    online.close().await.unwrap();
    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    let first = service.cache(&account_summary(&service, 1)).unwrap();
    assert_ne!(first.scope().partition(), orphan.partition());
    put(&first, &private_key(1), b"first");
    service.close().await.unwrap();
    let online = super::account_tests::open(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
    )
    .await;
    user(&mock, 1).await;
    online
        .replace(key(1), Pat::from_str("second_fake"))
        .await
        .unwrap();
    online.close().await.unwrap();
    let service = open_cached(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
        cache.clone(),
    )
    .await;
    let second = service.cache(&account_summary(&service, 1)).unwrap();
    assert_eq!(first.scope(), second.scope());
    denied(&second, &private_key(1));
    let repos = service.repo_service();
    let session = repos.session().unwrap();
    let proof = repos
        .begin_revalidation(session.context(), Some(1))
        .unwrap();
    repos
        .permission_confirmed(session.context(), proof)
        .await
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
    assert_eq!(
        value(
            &service.cache(&account_summary(&service, 1)).unwrap(),
            &private_key(1)
        ),
        b"first"
    );
    service.close().await.unwrap();
    let online = super::account_tests::open(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
    )
    .await;
    online.remove_saved(key(1)).await.unwrap();
    user(&mock, 1).await;
    online.login(Pat::from_str("third_fake")).await.unwrap();
    online.close().await.unwrap();
    let service = open_cached(accounts, credentials, mock, executor, cache).await;
    let third = service.cache(&account_summary(&service, 1)).unwrap();
    assert_ne!(second.scope().partition(), third.scope().partition());
    assert!(matches!(
        third.store().read(third.scope(), &private_key(1)),
        Ok(CacheRead::Miss(_))
    ));
    denied(&second, &private_key(1));
    service.close().await.unwrap();
}

#[tokio::test]
async fn offline_constructor_rejection_preserves_pending_retirement_until_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let orphan = cache.saved_account(key(1)).unwrap();
    put(&test_binding(&cache, &orphan), &private_key(1), b"orphan");
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let online = super::account_tests::open(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
    )
    .await;
    user(&mock, 1).await;
    online.login(Pat::from_str("fake")).await.unwrap();
    mock.enqueue(Ok(crate::network::transport::HttpResponse {
        status: 401,
        headers: vec![],
        body: "{}".into(),
    }))
    .await;
    assert_eq!(
        online.revalidate(key(1)).await,
        Err(LoginError::GitHub(GitHubError::Unauthorized))
    );
    online.close().await.unwrap();
    let service = open_cached(accounts.clone(), credentials, mock.clone(), executor, cache).await;
    let binding = service.cache(&account_summary(&service, 1)).unwrap();
    assert_ne!(binding.scope().partition(), orphan.partition());
    denied(&binding, &private_key(1));
    let connection = rusqlite::Connection::open(&accounts.path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT kind FROM cache_recovery", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "rejected"
    );
    drop(connection);
    user(&mock, 1).await;
    service
        .replace(key(1), Pat::from_str("new_fake"))
        .await
        .unwrap();
    let repos = service.repo_service();
    let session = repos.session().unwrap();
    let proof = repos
        .begin_revalidation(session.context(), Some(1))
        .unwrap();
    repos
        .permission_confirmed(session.context(), proof)
        .await
        .unwrap();
    let binding = repos.cache(session.context()).unwrap();
    assert!(matches!(
        binding.store().read(binding.scope(), &private_key(1)),
        Ok(CacheRead::Miss(_))
    ));
    service.close().await.unwrap();
}

#[tokio::test]
async fn removal_clears_failed_catalog_replay_and_allows_immediate_readd() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let online = super::account_tests::open(
        accounts.clone(),
        credentials.clone(),
        mock.clone(),
        executor.clone(),
    )
    .await;
    user(&mock, 1).await;
    online.login(Pat::from_str("first_fake")).await.unwrap();
    online.close().await.unwrap();
    let scope = cache.saved_account(key(1)).unwrap();
    put(&test_binding(&cache, &scope), &private_key(1), b"old");
    let connection =
        rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_catalog BEFORE UPDATE ON scopes BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let service = open_cached(
        accounts.clone(),
        credentials,
        mock.clone(),
        executor,
        cache.clone(),
    )
    .await;
    assert!(service.cache(&account_summary(&service, 1)).is_none());
    assert!(service.recovery_pending().await.unwrap());
    connection
        .execute_batch("DROP TRIGGER fail_catalog")
        .unwrap();
    drop(connection);
    service.remove_saved(key(1)).await.unwrap();
    assert!(!service.recovery_pending().await.unwrap());
    user(&mock, 1).await;
    service.login(Pat::from_str("second_fake")).await.unwrap();
    let readded = service.cache(&account_summary(&service, 1)).unwrap();
    assert_ne!(readded.scope().partition(), scope.partition());
    assert!(matches!(
        cache.read(readded.scope(), &private_key(1)),
        Ok(CacheRead::Miss(_))
    ));
    service.close().await.unwrap();
}

#[tokio::test]
async fn rejection_suspends_outstanding_handles_when_reconciliation_enumeration_fails() {
    let temp = tempfile::tempdir().unwrap();
    let accounts = config(&temp);
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let mock = Arc::new(MockTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(2));
    let service = open_cached(
        accounts.clone(),
        credentials,
        mock.clone(),
        executor,
        cache.clone(),
    )
    .await;
    user(&mock, 1).await;
    service.login(Pat::from_str("fake")).await.unwrap();
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
    let context = service.repo_service().session().unwrap().context().clone();

    let lock = rusqlite::Connection::open(cache_config(&temp).root.join("cache.sqlite3")).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(service.recover().await.is_err());
    assert!(service.recovery_pending().await.unwrap());
    // The map was cleared by the failed reconciliation, but the previously
    // issued binding must still be locally suspended on rejection.
    assert!(
        service
            .repo_service()
            .credential_rejected(&context)
            .await
            .is_err()
    );
    assert!(matches!(lease.snapshot(), Err(CacheError::AccessDenied)));
    drop(lock);
    service.recover().await.unwrap();
    denied(&binding, &private_key(1));
    let restored = service.cache(&account_summary(&service, 1)).unwrap();
    denied(&restored, &private_key(1));
    service.close().await.unwrap();
}

#[test]
fn scope_enumeration_is_read_only_and_validates_persisted_account_ids() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CacheStore::open(cache_config(&temp)).unwrap();
    let first = cache.saved_account(key(1)).unwrap();
    assert_eq!(cache.live_scopes().unwrap(), vec![first]);
}
