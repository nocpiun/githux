//! Cache handles are issued only after reconciling the durable account catalog
//! with the cache authorization ledger. Account and cache databases never share
//! a transaction: safety comes from quarantine/retire-before-catalog-commit and
//! startup reconciliation of scopes with no saved account.
use super::*;
use crate::domain::AccountKey;
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{AccessEvent, CacheError, CacheScope, CacheStore};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// The application-owned store and the one currently saved account partition.
/// Consumers must additionally match `scope.account()` with their read session.
#[derive(Clone)]
pub struct AccountCache {
    store: CacheStore,
    scope: CacheScope,
}
impl AccountCache {
    pub fn store(&self) -> &CacheStore {
        &self.store
    }
    pub fn scope(&self) -> &CacheScope {
        &self.scope
    }
    pub fn into_parts(self) -> (CacheStore, CacheScope) {
        (self.store, self.scope)
    }
}

pub(super) struct AccountCaches {
    store: CacheStore,
    scopes: HashMap<AccountKey, CacheScope>,
    recovery_pending: bool,
}
pub(super) type Shared = Arc<Mutex<Option<AccountCaches>>>;

impl AccountCaches {
    pub(super) fn new(store: CacheStore, directory: &crate::storage::accounts::Directory) -> Self {
        let mut this = Self {
            store,
            scopes: HashMap::new(),
            recovery_pending: false,
        };
        this.reconcile(directory);
        this
    }

    /// Unknown live scopes are retired before any saved-account handle is
    /// issued. This covers a catalog commit followed by a crash before cache
    /// retirement, and also prevents remove/re-add from resurrecting a partition.
    fn reconcile(&mut self, directory: &crate::storage::accounts::Directory) {
        self.scopes.clear();
        self.recovery_pending = false;
        let live = match self.store.live_scopes() {
            Ok(scopes) => scopes,
            Err(_) => {
                self.recovery_pending = true;
                return;
            }
        };
        let mut blocked = HashSet::new();
        for scope in live {
            if !directory
                .accounts
                .iter()
                .any(|saved| saved.identity.key == *scope.account())
                && self.store.retire(&scope).is_err()
            {
                self.recovery_pending = true;
                blocked.insert(scope.account().clone());
            }
        }
        for saved in &directory.accounts {
            if blocked.contains(&saved.identity.key) {
                continue;
            }
            match self.store.saved_account(saved.identity.key.clone()) {
                Ok(scope) => {
                    self.scopes.insert(saved.identity.key.clone(), scope);
                }
                Err(_) => self.recovery_pending = true,
            }
        }
        if self.store.recover().is_err() {
            self.recovery_pending = true;
        }
    }

    fn current(&self, account: &AccountKey) -> Option<AccountCache> {
        self.scopes.get(account).map(|scope| AccountCache {
            store: self.store.clone(),
            scope: scope.clone(),
        })
    }

    /// A failed event has an unknown outcome. Withdraw the local handle either
    /// way; a later reconciliation is allowed to issue it only if the persisted
    /// gate still accepts the saved account. The catalog mutation is not attempted.
    fn event(&mut self, account: &AccountKey, event: AccessEvent) -> Result<(), CacheError> {
        let result = match self.scopes.get(account) {
            Some(scope) => self.store.access_event(scope, event),
            // A cache-enabled saved account may have had its handle withdrawn
            // after an uncertain gate outcome. Reconcile its durable partition
            // and still require this event; None here is NOT cache-disabled.
            None => self
                .store
                .saved_account(account.clone())
                .and_then(|scope| self.store.access_event(&scope, event)),
        };
        if result.is_err() {
            if let Some(scope) = self.scopes.get(account) {
                let _ = self.store.suspend(scope);
            }
            self.scopes.remove(account);
            self.recovery_pending = true;
        }
        result
    }

    fn retire(&mut self, account: &AccountKey) -> Result<(), CacheError> {
        let result = match self.scopes.get(account) {
            Some(scope) => self.store.retire(scope),
            // Withdrawn handles are not disabled caching. Resolve the persisted
            // partition and retire it before allowing a remove retry to commit.
            None => self
                .store
                .saved_account(account.clone())
                .and_then(|scope| self.store.retire(&scope)),
        };
        // Never leave an enabled in-memory route after a remove attempt, even
        // when the durable retirement outcome is unknown.
        self.scopes.remove(account);
        if result.is_err() {
            self.recovery_pending = true;
        }
        result
    }

    fn recover(&mut self, directory: &crate::storage::accounts::Directory) -> bool {
        self.reconcile(directory);
        !self.recovery_pending
    }
}

impl AccountService {
    /// Open the real account catalog and opt into private business caching.
    /// Cache/reconciliation failures never fail the account catalog open: they
    /// withhold cache handles and leave ordinary authenticated reads available.
    pub async fn open_with_cache(
        config: AccountDatabaseConfig,
        store: Arc<dyn CredentialStore>,
        client: Arc<GitHubClient>,
        executor: Arc<BackgroundExecutor>,
        cache: CacheStore,
    ) -> Result<Self, StorageError> {
        let service = Self::open(config, store, client, executor).await?;
        let directory = service.registry.lock().expect("registry lock").directory();
        let caches = tokio::task::spawn_blocking(move || AccountCaches::new(cache, &directory))
            .await
            .map_err(|_| StorageError::Closed)?;
        *service.cache.lock().expect("account cache lock") = Some(caches);
        Ok(service)
    }

    /// Bind account-level consumers to the current saved incarnation, not just
    /// a caller-provided id. `None` is fail-closed, not an empty cache hit.
    pub fn cache(&self, account: &AccountSummary) -> Option<AccountCache> {
        let current = self
            .registry
            .lock()
            .expect("registry lock")
            .summaries()
            .into_iter()
            .find(|summary| summary.identity.key == account.identity.key)?;
        if current != *account {
            return None;
        }
        self.cache
            .lock()
            .expect("account cache lock")
            .as_ref()
            .and_then(|cache| cache.current(&account.identity.key))
    }
}

#[cfg(test)]
pub(super) struct RejectionProbe {
    pub entered: tokio::sync::oneshot::Sender<()>,
    pub resume: std::sync::mpsc::Receiver<()>,
}

impl RepoService {
    #[cfg(test)]
    pub(super) fn pause_rejection(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        *self.rejection_probe.lock().expect("rejection probe") =
            Some(RejectionProbe { entered, resume });
        (ready, release)
    }

    /// Repository/page consumers must use their fixed read context. This ties
    /// the partition to both the saved account and its current runtime epoch.
    pub fn cache(&self, context: &RepositoryReadContext) -> Option<AccountCache> {
        if !self.is_current(context) {
            return None;
        }
        self.cache
            .lock()
            .expect("account cache lock")
            .as_ref()
            .and_then(|cache| cache.current(&context.auth().account))
    }

    /// Record only an already-classified credential rejection. Transient HTTP
    /// and transport failures must not call this API. Both sides of the blocking
    /// boundary recheck the original context, so a late 401 cannot quarantine a
    /// replacement credential or a remove/re-added account.
    pub async fn credential_rejected(
        &self,
        context: &RepositoryReadContext,
    ) -> Result<(), GitHubError> {
        if !self.is_current(context) {
            return Err(GitHubError::StaleContext);
        }
        let gate = self.credential_commit.clone();
        let registry = self.registry.clone();
        let cache = self.cache.clone();
        let closed = self.closed.clone();
        let expected = context.clone();
        let account = context.auth().account.clone();
        #[cfg(test)]
        let probe = self.rejection_probe.lock().expect("rejection probe").take();
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some(probe) = probe {
                probe.entered.send(()).unwrap();
                probe.resume.recv().unwrap();
            }
            let _gate = gate.lock().expect("account commit lock");
            if closed.load(std::sync::atomic::Ordering::Acquire)
                || registry
                    .lock()
                    .expect("registry lock")
                    .active_context()
                    .as_ref()
                    != Some(expected.auth())
            {
                return Err(GitHubError::StaleContext);
            }
            let mut cache = cache.lock().expect("account cache lock");
            match cache.as_mut() {
                Some(cache) => cache
                    .event(&account, AccessEvent::CredentialRejected)
                    .map_err(|_| GitHubError::CredentialStore {
                        message: "private cache authorization update failed".into(),
                    }),
                None => Ok(()),
            }
        })
        .await
        .map_err(|_| GitHubError::StaleContext)?
    }
}

pub(super) fn quarantine(
    cache: &Shared,
    account: &AccountKey,
    event: AccessEvent,
) -> Result<(), CacheError> {
    match cache.lock().expect("account cache lock").as_mut() {
        Some(cache) => cache.event(account, event),
        None => Ok(()),
    }
}

pub(super) fn retire(cache: &Shared, account: &AccountKey) -> Result<(), CacheError> {
    match cache.lock().expect("account cache lock").as_mut() {
        Some(cache) => cache.retire(account),
        None => Ok(()),
    }
}

pub(super) fn provision(cache: &Shared, account: &AccountKey) {
    let mut cache = cache.lock().expect("account cache lock");
    if let Some(cache) = cache.as_mut()
        && !cache.scopes.contains_key(account)
    {
        match cache.store.saved_account(account.clone()) {
            Ok(scope) => {
                cache.scopes.insert(account.clone(), scope);
            }
            Err(_) => cache.recovery_pending = true,
        }
    }
}

pub(super) fn recover(
    cache: &Shared,
    directory: &crate::storage::accounts::Directory,
) -> Result<(), AccountError> {
    let mut cache = cache.lock().expect("account cache lock");
    match cache.as_mut() {
        Some(cache) => {
            if cache.recover(directory) {
                Ok(())
            } else {
                Err(AccountError::CleanupPending)
            }
        }
        None => Ok(()),
    }
}

pub(super) fn mark_recovery(cache: &Shared) {
    if let Some(cache) = cache.lock().expect("account cache lock").as_mut() {
        cache.recovery_pending = true;
    }
}

pub(super) fn recovery_pending(cache: &Shared) -> bool {
    cache
        .lock()
        .expect("account cache lock")
        .as_ref()
        .is_some_and(|cache| cache.recovery_pending)
}
