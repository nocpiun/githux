//! Cache handles are issued only after reconciling the durable account catalog
//! with the cache authorization ledger. Account and cache databases never share
//! a transaction: safety comes from quarantine/retire-before-catalog-commit and
//! startup reconciliation of scopes with no saved account.
use super::*;
use crate::domain::AccountKey;
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{AccessEvent, CacheError, CacheScope, CacheStore, Revalidation};
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
    #[cfg(test)]
    pub(super) fn from_parts_for_test(store: CacheStore, scope: CacheScope) -> Self {
        Self { store, scope }
    }
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
    /// No scope may be provisioned until a complete durable enumeration has
    /// succeeded. An empty result is not equivalent to an unavailable result.
    reconciled: bool,
    /// Accounts whose old live partition must be durably retired before re-add.
    blocked: HashSet<AccountKey>,
    /// A classified rejection whose durable outcome was uncertain. Replaying it
    /// is a prerequisite for recovery; replacing the credential supersedes it.
    rejected: HashSet<AccountKey>,
    /// Catalog commits made while caching was disabled; acknowledge only after
    /// their durable cache gate has been replayed, before issuing a binding.
    catalog: HashMap<AccountKey, String>,
}
pub(super) type Shared = Arc<Mutex<Option<AccountCaches>>>;

impl AccountCaches {
    pub(super) fn new(
        store: CacheStore,
        directory: &crate::storage::accounts::Directory,
        records: Vec<(AccountKey, String)>,
        database: &crate::storage::accounts::AccountDatabase,
    ) -> Self {
        let mut this = Self {
            store,
            scopes: HashMap::new(),
            recovery_pending: false,
            reconciled: false,
            blocked: HashSet::new(),
            rejected: HashSet::new(),
            catalog: HashMap::new(),
        };
        for (account, kind) in records {
            if directory
                .accounts
                .iter()
                .any(|saved| saved.identity.key == account)
            {
                if matches!(kind.as_str(), "rejected" | "retire_rejected") {
                    this.rejected.insert(account.clone());
                }
                if kind != "rejected" {
                    this.catalog.insert(account, kind);
                }
            }
        }
        this.replay_catalog(database);
        this.reconcile(directory);
        this
    }

    fn replay_catalog(&mut self, database: &crate::storage::accounts::AccountDatabase) {
        for (account, kind) in self.catalog.clone() {
            let applied = if matches!(kind.as_str(), "retire" | "retire_rejected") {
                self.store.live_scopes().and_then(|scopes| {
                    for scope in scopes
                        .into_iter()
                        .filter(|scope| scope.account() == &account)
                    {
                        self.store.retire(&scope)?;
                    }
                    Ok(())
                })
            } else {
                self.store
                    .saved_account(account.clone())
                    .and_then(|scope| self.store.access_event(&scope, AccessEvent::PatReplaced))
            };
            if applied.is_ok() && database.cache_recovered(&account, &kind).is_ok() {
                self.catalog.remove(&account);
            }
        }
    }

    /// Unknown live scopes are retired before any saved-account handle is
    /// issued. This covers a catalog commit followed by a crash before cache
    /// retirement, and also prevents remove/re-add from resurrecting a partition.
    fn reconcile(&mut self, directory: &crate::storage::accounts::Directory) {
        self.scopes.clear();
        self.recovery_pending = false;
        self.reconciled = false;
        let live = match self.store.live_scopes() {
            Ok(scopes) => scopes,
            Err(_) => {
                self.recovery_pending = true;
                return;
            }
        };
        let mut orphans = HashSet::new();
        for scope in live {
            let account = scope.account().clone();
            let unsaved = !directory
                .accounts
                .iter()
                .any(|saved| saved.identity.key == account);
            // Retirement follows the durable directory, not the in-memory
            // binding map. A saved account's replacement must quarantine rather
            // than rotate its partition; an absent account's scope is an orphan.
            if unsaved || self.blocked.contains(&account) {
                if self.store.retire(&scope).is_err() {
                    self.recovery_pending = true;
                    self.blocked.insert(account.clone());
                    orphans.insert(account);
                } else {
                    self.blocked.remove(&account);
                }
            }
        }
        for saved in &directory.accounts {
            if orphans.contains(&saved.identity.key)
                || self.catalog.contains_key(&saved.identity.key)
            {
                continue;
            }
            if self.rejected.contains(&saved.identity.key) {
                // The account journal remains authoritative even if the cache
                // ledger was unavailable (or replaced by an empty cache).
                let rejection = self
                    .store
                    .saved_account(saved.identity.key.clone())
                    .and_then(|scope| {
                        self.store
                            .access_event(&scope, AccessEvent::CredentialRejected)
                    });
                if rejection.is_err() {
                    self.recovery_pending = true;
                    continue;
                }
            }
            match self.store.saved_account(saved.identity.key.clone()) {
                Ok(scope) => {
                    self.scopes.insert(saved.identity.key.clone(), scope);
                }
                Err(_) => self.recovery_pending = true,
            }
        }
        self.rejected.retain(|account| {
            directory
                .accounts
                .iter()
                .any(|saved| saved.identity.key == *account)
        });
        // Do not erase a just-observed failed orphan retirement. Only a
        // successful durable operation above may clear this protection.
        self.blocked.extend(orphans);
        if self.store.recover().is_err() {
            self.recovery_pending = true;
        }
        self.recovery_pending |= !self.catalog.is_empty();
        self.reconciled = true;
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
        if matches!(event, AccessEvent::PatReplaced) && self.catalog.contains_key(account) {
            // A pending retirement cannot be superseded by a new credential:
            // complete catalog replay first, including its durable acknowledgement.
            return Err(CacheError::AccessDenied);
        }
        let result = match self.scopes.get(account) {
            Some(scope) => self.store.access_event(scope, event),
            // A cache-enabled saved account may have had its handle withdrawn
            // after an uncertain gate outcome. Reconcile its durable partition
            // and still require this event; None here is NOT cache-disabled.
            None => self
                .store
                .live_scopes()
                .and_then(|scopes| {
                    scopes
                        .into_iter()
                        .find(|scope| scope.account() == account)
                        .ok_or(CacheError::AccessDenied)
                })
                .and_then(|scope| self.store.access_event(&scope, event)),
        };
        if result.is_err() {
            if let Some(scope) = self.scopes.get(account) {
                let _ = self.store.suspend(scope);
            }
            self.scopes.remove(account);
            self.recovery_pending = true;
            if matches!(event, AccessEvent::CredentialRejected) {
                self.rejected.insert(account.clone());
            }
        }
        result
    }

    fn retire(&mut self, account: &AccountKey) -> Result<(), CacheError> {
        let blocked = self.blocked.contains(account);
        let result = if blocked {
            // A previous failed/local-suspended scope cannot prove retirement.
            // Re-read the current durable generation and retry the live row.
            self.store.live_scopes().and_then(|scopes| {
                match scopes.into_iter().find(|scope| scope.account() == account) {
                    Some(scope) => self.store.retire(&scope),
                    None => Ok(()),
                }
            })
        } else {
            match self.scopes.get(account) {
                Some(scope) => self.store.retire(scope),
                // Empty map state is not evidence of durable retirement. Retire a
                // still-live persisted partition; only no live partition (already
                // retired) satisfies the prerequisite without another UPDATE.
                None => self.store.live_scopes().and_then(|scopes| {
                    match scopes.into_iter().find(|scope| scope.account() == account) {
                        Some(scope) => self.store.retire(&scope),
                        None => Ok(()),
                    }
                }),
            }
        };
        // Never leave an enabled in-memory route after a remove attempt, even
        // when the durable retirement outcome is unknown.
        self.scopes.remove(account);
        if result.is_err() {
            self.recovery_pending = true;
            self.blocked.insert(account.clone());
        } else if blocked {
            self.blocked.remove(account);
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
        let database = service.database.clone();
        let caches = tokio::task::spawn_blocking(move || {
            let database = database.lock().expect("database lock");
            let database = database.as_ref().ok_or(StorageError::Closed)?;
            let records = database.cache_recovery()?;
            Ok::<_, StorageError>(AccountCaches::new(cache, &directory, records, database))
        })
        .await
        .map_err(|_| StorageError::Closed)?;
        *service.cache.lock().expect("account cache lock") = Some(caches?);
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

impl RepoService {
    /// Begin the cache ticket for a repository-specific proof. The actual
    /// network read still belongs to the consumer's existing session; finish
    /// with `permission_confirmed`, which serializes against account commits.
    pub fn begin_revalidation(
        &self,
        context: &RepositoryReadContext,
        repository: Option<u64>,
    ) -> Result<Revalidation, CacheError> {
        let binding = self.cache(context).ok_or(CacheError::AccessDenied)?;
        binding
            .store()
            .begin_revalidation(binding.scope(), repository)
    }

    /// Publish repository proof only while its session and cache partition are
    /// still current. In particular, an old PAT cannot prove the cache epoch
    /// allocated by a concurrent replacement before that replacement commits.
    pub async fn permission_confirmed(
        &self,
        context: &RepositoryReadContext,
        proof: Revalidation,
    ) -> Result<(), CacheError> {
        if !self.is_current(context) {
            return Err(CacheError::Stale);
        }
        let gate = self.credential_commit.clone();
        let registry = self.registry.clone();
        let cache = self.cache.clone();
        let closed = self.closed.clone();
        let expected = context.clone();
        let proof_scope = proof.scope().clone();
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
                return Err(CacheError::Stale);
            }
            let binding = cache
                .lock()
                .expect("account cache lock")
                .as_ref()
                .and_then(|cache| cache.current(&expected.auth().account))
                .ok_or(CacheError::AccessDenied)?;
            if binding.scope() != &proof_scope {
                return Err(CacheError::Stale);
            }
            if cache
                .lock()
                .expect("account cache lock")
                .as_ref()
                .is_some_and(|cache| cache.rejected.contains(&expected.auth().account))
            {
                return Err(CacheError::AccessDenied);
            }
            binding.store().permission_confirmed(proof)
        })
        .await
        .map_err(|_| CacheError::Stale)?
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
        let database = self.database.clone();
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
            reject(&cache, &database, &account).map_err(|_| GitHubError::CredentialStore {
                message: "private cache authorization update failed".into(),
            })
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

pub(super) fn prepare_new(
    cache: &Shared,
    account: &AccountKey,
    directory: &crate::storage::accounts::Directory,
) -> Result<(), CacheError> {
    let mut cache = cache.lock().expect("account cache lock");
    let Some(cache) = cache.as_mut() else {
        return Ok(());
    };
    if !cache.reconciled {
        cache.reconcile(directory);
    }
    if !cache.reconciled || cache.catalog.contains_key(account) {
        cache.blocked.insert(account.clone());
        return Err(CacheError::AccessDenied);
    }
    if cache.blocked.contains(account)
        || cache
            .store
            .live_scopes()?
            .iter()
            .any(|scope| scope.account() == account)
    {
        cache.blocked.insert(account.clone());
        cache.retire(account)?;
    }
    Ok(())
}

pub(super) fn provision(
    cache: &Shared,
    account: &AccountKey,
    directory: &crate::storage::accounts::Directory,
) {
    let mut cache = cache.lock().expect("account cache lock");
    if let Some(cache) = cache.as_mut() {
        if !cache.reconciled {
            // Enumeration failure is a closed barrier, not an empty ledger.
            // Remember this just-committed account for a later recovery pass;
            // an unsuccessful enumeration must not provision its old partition.
            cache.blocked.insert(account.clone());
            cache.reconcile(directory);
        }
        if cache.blocked.contains(account) {
            // A failed orphan retirement must complete before re-add. Retiring
            // first makes saved_account allocate a fresh UUID partition instead
            // of reviving the orphan's old bytes under the newly supplied PAT.
            if cache.retire(account).is_err() {
                return;
            }
        }
        if cache.scopes.contains_key(account) {
            return;
        }
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
    if let Some(cache) = cache.as_mut()
        && !cache.recover(directory)
    {
        return Err(AccountError::CleanupPending);
    }
    Ok(())
}

/// Caller holds the account commit lock and has checked credential ownership.
/// Journal first; still deny cache handles if the journal operation itself fails.
pub(super) fn reject(
    cache: &Shared,
    database: &accounts::Database,
    account: &AccountKey,
) -> Result<(), AccountError> {
    let journal = match database.lock().expect("database lock").as_ref() {
        Some(database) => database
            .stage_cache_rejection(account)
            .map_err(AccountError::Storage),
        None => Ok(()),
    };
    let mut cache = cache.lock().expect("account cache lock");
    let gate = match cache.as_mut() {
        Some(cache) => {
            cache.rejected.insert(account.clone());
            cache
                .event(account, AccessEvent::CredentialRejected)
                .map_err(|_| AccountError::CleanupPending)
        }
        None => Ok(()),
    };
    journal.and(gate)
}

/// Only after the account transaction has published replacement credentials.
pub(super) fn published(cache: &Shared, account: &AccountKey) {
    if let Some(cache) = cache.lock().expect("account cache lock").as_mut() {
        cache.rejected.remove(account);
        cache.catalog.remove(account);
    }
}

pub(super) fn recover_with_catalog(
    cache: &Shared,
    directory: &crate::storage::accounts::Directory,
    database: &accounts::Database,
) -> Result<(), AccountError> {
    {
        let database = database.lock().expect("database lock");
        if let Some(database) = database.as_ref()
            && let Some(cache) = cache.lock().expect("account cache lock").as_mut()
        {
            cache.replay_catalog(database);
        }
    }
    recover(cache, directory)
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
