//! Blocking account commits run to completion once submitted, even if the
//! awaiting task is cancelled. Never hold a registry lock across disk/keychain IO.
use super::*;
use crate::domain::account::AuthorizationState;
use crate::domain::{AccountIdentity, AccountKey};
use crate::storage::accounts::{AccountDatabase, AccountDatabaseConfig, StorageError};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountError {
    Storage(StorageError),
    Credential(CredentialError),
    NotFound,
    /// The directory commit succeeded, but secret cleanup remains journalled.
    CleanupPending,
}

pub(super) type Database = Arc<Mutex<Option<AccountDatabase>>>;

impl AccountService {
    /// Open/migrate/restore before publishing the service to callers. No network
    /// access; credential presence is evidence only of local availability.
    pub async fn open(
        config: AccountDatabaseConfig,
        store: Arc<dyn CredentialStore>,
        client: Arc<GitHubClient>,
        executor: Arc<BackgroundExecutor>,
    ) -> Result<Self, StorageError> {
        if config.read_only {
            return Err(StorageError::ReadOnly);
        }
        let store_worker = store.clone();
        let (db, registry) = tokio::task::spawn_blocking(move || {
            let db = AccountDatabase::open(config)?;
            let mut registry = AccountRegistry::restore(db.load()?);
            // Recovery deletes only references that no live account uses.
            // Failure retains the durable route and is observable via recover().
            let _ = cleanup(&db, store_worker.as_ref());
            // recovery_pending() exposes any entries retained by failed cleanup.
            for saved in registry.directory().accounts {
                let state = match store_worker.load(&saved.credential) {
                    Ok(Some(_)) => AuthorizationState::PendingVerification,
                    Ok(None) => AuthorizationState::MissingCredential,
                    Err(_) => AuthorizationState::CredentialUnavailable,
                };
                registry.set_authorization(&saved.identity.key, state);
            }
            Ok::<_, StorageError>((db, registry))
        })
        .await
        .map_err(|_| StorageError::Closed)??;
        Ok(Self {
            registry: Arc::new(Mutex::new(registry)),
            store,
            client,
            executor,
            credential_commit: Arc::new(Mutex::new(())),
            database: Arc::new(Mutex::new(Some(db))),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            persistent: true,
        })
    }

    pub(super) async fn commit_login(
        &self,
        identity: AccountIdentity,
        pat: Pat,
        replacement: Option<crate::domain::AuthEpoch>,
    ) -> Result<LoginOutcome, LoginError> {
        let gate = self.credential_commit.clone();
        let registry = self.registry.clone();
        let database = self.database.clone();
        let store = self.store.clone();
        let closed = self.closed.clone();
        tokio::task::spawn_blocking(move || {
            let _gate = gate.lock().expect("account commit lock");
            if closed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(LoginError::Storage(StorageError::Closed));
            }
            let mut next = registry.lock().expect("registry lock").clone();
            let old = next.credential_ref(identity.key.clone());
            if let Some(epoch) = replacement {
                if old.is_none() {
                    return Err(LoginError::NotFound);
                }
                if next.incarnation(&identity.key) != Some(epoch) {
                    return Err(LoginError::GitHub(GitHubError::StaleContext));
                }
            }
            let mut db = database.lock().expect("database lock");
            let credential = if db.is_some() {
                CredentialRef::versioned(
                    identity.key.host.clone(),
                    identity.key.id,
                    uuid::Uuid::new_v4().to_string(),
                )
            } else {
                CredentialRef::new(identity.key.host.clone(), identity.key.id)
            };
            if let Some(db) = db.as_ref() {
                db.stage(&credential).map_err(LoginError::Storage)?;
            }
            // Even a platform save error may have written a secret. The staged
            // reference remains until a successful idempotent delete + SQL ack.
            if let Err(error) = store.save(&credential, &pat) {
                if let Some(db) = db.as_ref() {
                    let _ = cleanup(db, store.as_ref());
                }
                return Err(LoginError::CredentialSave(error));
            }
            let summary = next.upsert_verified(identity, credential.clone());
            if let Some(db) = db.as_mut()
                && let Err(error) = db.commit(&next.directory(), Some(&credential), old.as_ref())
            {
                // Old credential/registry stay authoritative. New slot is
                // still journalled even if rollback cleanup itself fails.
                let _ = cleanup(db, store.as_ref());
                return Err(LoginError::Storage(error));
            }
            *registry.lock().expect("registry lock") = next;
            if let Some(db) = db.as_ref()
                && cleanup(db, store.as_ref()).is_err()
            {
                return Err(LoginError::CleanupPending(Box::new(summary)));
            }
            Ok(if old.is_some() {
                LoginOutcome::Replaced(summary)
            } else {
                LoginOutcome::SignedIn(summary)
            })
        })
        .await
        .map_err(|_| LoginError::Storage(StorageError::Closed))?
    }

    /// Persist selection before making it visible. All mutations share this
    /// serial commit boundary, ordered by acquisition after network validation.
    pub async fn set_active(&self, account: AccountKey) -> Result<(), AccountError> {
        self.mutate(Some(account), None).await
    }
    pub async fn remove_saved(&self, account: AccountKey) -> Result<(), AccountError> {
        self.mutate(None, Some(account)).await
    }
    async fn mutate(
        &self,
        active: Option<AccountKey>,
        removed: Option<AccountKey>,
    ) -> Result<(), AccountError> {
        let gate = self.credential_commit.clone();
        let registry = self.registry.clone();
        let database = self.database.clone();
        let store = self.store.clone();
        let closed = self.closed.clone();
        tokio::task::spawn_blocking(move || {
            let _gate = gate.lock().expect("account commit lock");
            if closed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(AccountError::Storage(StorageError::Closed));
            }
            let mut next = registry.lock().expect("registry lock").clone();
            let retired = if let Some(key) = removed {
                let r = next
                    .credential_ref(key.clone())
                    .ok_or(AccountError::NotFound)?;
                next.remove(key).map_err(|_| AccountError::NotFound)?;
                Some(r)
            } else {
                next.set_active(active.expect("selection"))
                    .map_err(|_| AccountError::NotFound)?;
                None
            };
            let mut db = database.lock().expect("database lock");
            if let Some(db) = db.as_mut() {
                db.commit(&next.directory(), None, retired.as_ref())
                    .map_err(AccountError::Storage)?;
            }
            *registry.lock().expect("registry lock") = next;
            if let Some(db) = db.as_ref() {
                cleanup(db, store.as_ref())
            } else if let Some(r) = retired {
                store.delete(&r).map_err(AccountError::Credential)
            } else {
                Ok(())
            }
        })
        .await
        .map_err(|_| AccountError::Storage(StorageError::Closed))?
    }

    /// Observe startup or later recovery failures without hiding the directory.
    pub async fn recovery_pending(&self) -> Result<bool, StorageError> {
        let database = self.database.clone();
        let closed = self.closed.clone();
        tokio::task::spawn_blocking(move || {
            if closed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(StorageError::Closed);
            }
            match database.lock().expect("database lock").as_ref() {
                Some(db) => Ok(!db.pending()?.is_empty()),
                None => Ok(false),
            }
        })
        .await
        .map_err(|_| StorageError::Closed)?
    }

    /// Explicit retry of journalled secret deletion. No automatic retry loop.
    pub async fn recover(&self) -> Result<(), AccountError> {
        let gate = self.credential_commit.clone();
        let database = self.database.clone();
        let store = self.store.clone();
        let closed = self.closed.clone();
        tokio::task::spawn_blocking(move || {
            let _gate = gate.lock().expect("account commit lock");
            if closed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(AccountError::Storage(StorageError::Closed));
            }
            match database.lock().expect("database lock").as_ref() {
                Some(db) => cleanup(db, store.as_ref()),
                None => Ok(()),
            }
        })
        .await
        .map_err(|_| AccountError::Storage(StorageError::Closed))?
    }

    /// Drain submitted commits and release the database lease off the UI thread.
    /// Stop/await network tasks before closing the network executor separately.
    pub async fn close(&self) -> Result<(), StorageError> {
        let gate = self.credential_commit.clone();
        let database = self.database.clone();
        let closed = self.closed.clone();
        // Reject future commits immediately; already running commits finish.
        closed.store(true, std::sync::atomic::Ordering::Release);
        tokio::task::spawn_blocking(move || {
            let _gate = gate.lock().expect("account commit lock");
            if let Some(db) = database.lock().expect("database lock").take() {
                db.close();
            }
        })
        .await
        .map_err(|_| StorageError::Closed)
    }

    /// Revalidate a saved credential without treating its mere presence as
    /// authorization. Both success and errors are rejected if the slot changed.
    pub async fn revalidate(&self, account: AccountKey) -> Result<AccountSummary, LoginError> {
        let (credential, epoch, selection) = {
            let registry = self.registry.lock().expect("registry lock");
            let credential = registry
                .credential_ref(account.clone())
                .ok_or(LoginError::NotFound)?;
            let epoch = registry
                .summaries()
                .into_iter()
                .find(|s| s.identity.key == account)
                .ok_or(LoginError::NotFound)?
                .epoch;
            (credential, epoch, registry.active_context())
        };
        let store = self.store.clone();
        let r = credential.clone();
        let loaded = tokio::task::spawn_blocking(move || store.load(&r))
            .await
            .map_err(|_| LoginError::Storage(StorageError::Closed))?;
        let (result, state) = match loaded {
            Ok(Some(pat)) => {
                let result = self.verify(&pat).await.and_then(|v| {
                    if v.identity.key == account {
                        Ok(v.identity)
                    } else {
                        Err(LoginError::IdentityMismatch {
                            expected_account: account.id,
                            actual_account: v.identity.key.id,
                        })
                    }
                });
                let state = match &result {
                    Ok(_) => AuthorizationState::Verified,
                    Err(LoginError::GitHub(e)) => AuthorizationState::VerificationFailed(e.clone()),
                    _ => AuthorizationState::VerificationFailed(GitHubError::Unauthorized),
                };
                (result, state)
            }
            Ok(None) => (
                Err(LoginError::CredentialSave(CredentialError::NotFound)),
                AuthorizationState::MissingCredential,
            ),
            Err(e) => (
                Err(LoginError::CredentialSave(e)),
                AuthorizationState::CredentialUnavailable,
            ),
        };
        let gate = self.credential_commit.clone();
        let registry = self.registry.clone();
        let closed = self.closed.clone();
        tokio::task::spawn_blocking(move || {
            let _gate = gate.lock().expect("account commit lock");
            if closed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(LoginError::Storage(StorageError::Closed));
            }
            let mut reg = registry.lock().expect("registry lock");
            if reg.active_context() != selection
                || reg.credential_ref(account.clone()).as_ref() != Some(&credential)
                || !reg
                    .summaries()
                    .iter()
                    .any(|s| s.identity.key == account && s.epoch == epoch)
            {
                return Err(LoginError::GitHub(GitHubError::StaleContext));
            }
            // This operation changes runtime evidence only; display refresh is
            // persisted on the next explicit login/replacement.
            reg.revalidated(&account, state);
            result?;
            Ok(reg
                .summaries()
                .into_iter()
                .find(|s| s.identity.key == account)
                .expect("existing"))
        })
        .await
        .map_err(|_| LoginError::Storage(StorageError::Closed))?
    }
}
fn cleanup(db: &AccountDatabase, store: &dyn CredentialStore) -> Result<(), AccountError> {
    let refs = db.pending().map_err(AccountError::Storage)?;
    let mut failed = false;
    for r in refs {
        if store.delete(&r).is_err() || db.cleaned(&r).is_err() {
            failed = true;
        }
    }
    if failed {
        Err(AccountError::CleanupPending)
    } else {
        Ok(())
    }
}
