//! Disposable content with a durable authorization ledger. All methods perform
//! blocking I/O: invoke on an existing blocking worker, never the UI thread.
#![allow(dead_code)] // Consumer/account-event wiring is a subsequent task.

mod keys;
mod objects;
mod schema;
#[cfg(test)]
mod tests;

use crate::domain::{
    AccountKey,
    cache::{CacheMetadata, CacheMiss, CacheSnapshot},
};
use keys::repository_key;
#[allow(unused_imports)] // Public consumer surface; account/repository wiring lands later.
pub use keys::{CacheKey, CacheScope, ObjectKind};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashMap,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, CacheError>;
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("invalid cache input or configuration")]
    InvalidInput,
    #[error("cache directory is unavailable")]
    PathUnavailable,
    #[error("cache is already open")]
    InUse,
    #[error("cache I/O failed")]
    Io,
    #[error("cache database operation failed")]
    Database,
    #[error("cache schema {0} is newer than supported")]
    FutureSchema(i64),
    #[error("cache metadata is corrupt")]
    Corrupt,
    #[error("cache authorization gate denies access")]
    AccessDenied,
    #[error("cache operation belongs to an older context")]
    Stale,
    #[error("cache capacity is occupied by active reads")]
    Capacity,
    #[error("cache payload is too large")]
    TooLarge,
}
impl From<rusqlite::Error> for CacheError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Database
    }
}
impl From<std::io::Error> for CacheError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// A dedicated cache directory, never the application's data root.
    pub root: PathBuf,
    pub capacity_bytes: u64,
    pub max_entry_bytes: u64,
    pub inline_bytes: u64,
    pub max_entries: usize,
    pub database_bytes: u64,
}
impl CacheConfig {
    pub fn at(root: PathBuf) -> Self {
        Self {
            root,
            capacity_bytes: 256 * 1024 * 1024,
            max_entry_bytes: 16 * 1024 * 1024,
            inline_bytes: 64 * 1024,
            max_entries: 8192,
            database_bytes: 64 * 1024 * 1024,
        }
    }
    pub fn application_cache() -> Result<Self> {
        Ok(Self::at(
            dirs::cache_dir()
                .ok_or(CacheError::PathUnavailable)?
                .join("Githux")
                .join("business-v1"),
        ))
    }
}
struct State {
    db: Connection,
    pins: HashMap<String, usize>,
}
struct Inner {
    state: Mutex<State>,
    config: CacheConfig,
    instance: String,
    _lease: File,
}
#[derive(Clone)]
pub struct CacheStore {
    inner: Arc<Inner>,
}

/// Captured immediately before a repository-specific permission request.
/// GET /user alone must never be used to complete this ticket.
pub struct Revalidation {
    scope: CacheScope,
    repository: String,
    epoch: String,
    revision: String,
    instance: String,
}
/// Captured before a network read; newer writers and access events invalidate it.
pub struct CacheWrite {
    scope: CacheScope,
    key: CacheKey,
    epoch: String,
    gate: String,
    revision: String,
    instance: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Stored,
    NotStored,
}
#[derive(Debug, Clone, Copy)]
pub enum AccessEvent {
    PatReplaced,
    CredentialRejected,
    Offline,
    Timeout,
    ServerFailure,
    RateLimited,
    AmbiguousDenial,
}

pub enum CacheRead {
    Hit(CacheLease),
    Miss(CacheMiss),
}
/// Temporary eviction protection, not permission. snapshot() rechecks the gate.
/// Dropping the final lease permits ordinary eviction; retirement/clear/revocation
/// immediately invalidate it. Already copied bytes cannot be recalled by storage.
pub struct CacheLease {
    store: CacheStore,
    scope: CacheScope,
    key: CacheKey,
    id: String,
}
impl Drop for CacheLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.store.inner.state.lock()
            && let Some(count) = state.pins.get_mut(&self.id)
        {
            *count -= 1;
            if *count == 0 {
                state.pins.remove(&self.id);
            }
        }
    }
}
impl CacheLease {
    pub fn snapshot(&self) -> Result<Option<CacheSnapshot>> {
        let state = self.store.lock()?;
        check_access(&state.db, &self.scope, &self.key)?;
        self.store
            .load(&state.db, &self.id, &self.key)
            .map(|value| value.ok())
    }
}

impl CacheStore {
    pub fn open(config: CacheConfig) -> Result<Self> {
        if !config.root.is_absolute()
            || config.capacity_bytes == 0
            || config.max_entries == 0
            || config.max_entries > i64::MAX as usize
            || config.max_entry_bytes > config.capacity_bytes
            || config.inline_bytes > config.max_entry_bytes
            || config.database_bytes < 64 * 1024
            || config.database_bytes > i64::MAX as u64
            || config.capacity_bytes > i64::MAX as u64
        {
            return Err(CacheError::InvalidInput);
        }
        std::fs::create_dir_all(&config.root)?;
        let lease = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(config.root.join("cache.lock"))?;
        lease.try_lock().map_err(|_| CacheError::InUse)?;
        objects::ensure_directory(&config.root.join("objects"))?;
        let mut db = Connection::open(config.root.join("cache.sqlite3"))?;
        db.busy_timeout(Duration::from_millis(500))?;
        let page_size: u64 = db.pragma_query_value(None, "page_size", |r| r.get(0))?;
        schema::initialize(&mut db, config.database_bytes / page_size)?;
        let store = Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    db,
                    pins: HashMap::new(),
                }),
                config,
                instance: nonce(),
                _lease: lease,
            }),
        };
        store.recover()?;
        Ok(store)
    }
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.inner.state.lock().map_err(|_| CacheError::Database)
    }
    /// Only call for an account present in the durable account catalog. This does
    /// not mark it network-verified and never resets an existing quarantine.
    pub fn saved_account(&self, account: AccountKey) -> Result<CacheScope> {
        if account.host.as_str().is_empty() || account.host.as_str().len() > 253 {
            return Err(CacheError::InvalidInput);
        }
        let mut state = self.lock()?;
        let tx = state.db.transaction()?;
        let partition: Option<String> = tx
            .query_row(
                "SELECT partition FROM scopes WHERE host=?1 AND account=?2 AND state!='retired'",
                params![account.host.as_str(), account.id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let partition = if let Some(partition) = partition {
            partition
        } else {
            let partition = nonce();
            tx.execute(
                "INSERT INTO scopes VALUES(?1,?2,?3,'active',?4,0)",
                params![
                    partition,
                    account.host.as_str(),
                    account.id.to_string(),
                    nonce()
                ],
            )?;
            partition
        };
        keys::object_name(&partition)?;
        tx.commit()?;
        Ok(CacheScope { account, partition })
    }
    /// Durable logical retirement first. recover() can finish physical deletion
    /// later, including after a crash. Re-adding allocates another partition.
    pub fn retire(&self, scope: &CacheScope) -> Result<()> {
        let state = self.lock()?;
        validate_scope(&state.db, scope)?;
        state.db.execute(
            "UPDATE scopes SET state='retired',epoch=?1 WHERE partition=?2",
            params![nonce(), scope.partition],
        )?;
        Ok(())
    }
    pub fn access_event(&self, scope: &CacheScope, event: AccessEvent) -> Result<()> {
        let state = self.lock()?;
        validate_scope(&state.db, scope)?;
        match event {
            AccessEvent::PatReplaced | AccessEvent::CredentialRejected => {
                let status = if matches!(event, AccessEvent::CredentialRejected) {
                    "rejected"
                } else {
                    "active"
                };
                state.db.execute(
                    "UPDATE scopes SET state=?1,epoch=?2,restricted=1 WHERE partition=?3",
                    params![status, nonce(), scope.partition],
                )?;
            }
            _ => {}
        }
        Ok(())
    }
    pub fn begin_revalidation(
        &self,
        scope: &CacheScope,
        repository: Option<u64>,
    ) -> Result<Revalidation> {
        let mut state = self.lock()?;
        let tx = state.db.transaction()?;
        let (status, epoch, _) = validate_scope(&tx, scope)?;
        if status != "active" {
            return Err(CacheError::AccessDenied);
        }
        let repository = repository_key(repository);
        ensure_gate(&tx, scope, &repository, &epoch)?;
        let revision = gate(&tx, scope, &repository)?.1;
        tx.commit()?;
        Ok(Revalidation {
            scope: scope.clone(),
            repository,
            epoch,
            revision,
            instance: self.inner.instance.clone(),
        })
    }
    pub fn permission_confirmed(&self, ticket: Revalidation) -> Result<()> {
        let state = self.lock()?;
        if ticket.instance != self.inner.instance {
            return Err(CacheError::Stale);
        }
        let (status, epoch, _) = validate_scope(&state.db, &ticket.scope)?;
        if status != "active"
            || epoch != ticket.epoch
            || gate(&state.db, &ticket.scope, &ticket.repository)?.1 != ticket.revision
        {
            return Err(CacheError::Stale);
        }
        state.db.execute(
            "UPDATE gates SET epoch=?1,allowed=1 WHERE partition=?2 AND repository=?3",
            params![epoch, ticket.scope.partition, ticket.repository],
        )?;
        Ok(())
    }
    /// Explicit evidence of repository revocation, never a raw ambiguous 403/404.
    pub fn revoke_repository(&self, scope: &CacheScope, repository: Option<u64>) -> Result<()> {
        let mut state = self.lock()?;
        let tx = state.db.transaction()?;
        let (_, epoch, _) = validate_scope(&tx, scope)?;
        let repository = repository_key(repository);
        ensure_gate(&tx, scope, &repository, &epoch)?;
        tx.execute(
            "UPDATE gates SET allowed=0,revision=?1,epoch=?2 WHERE partition=?3 AND repository=?4",
            params![nonce(), epoch, scope.partition, repository],
        )?;
        tx.execute(
            "DELETE FROM entries WHERE partition=?1 AND repository=?2",
            params![scope.partition, repository],
        )?;
        // Revoking one concrete repository also invalidates this account's
        // aggregate (repository=None) snapshots in the same durable transaction.
        // Existing leases, in-flight write tickets and earlier revalidation
        // proofs are all bound to the aggregate gate revision and therefore stop
        // reading/repopulating/reopening old bytes. The aggregate's allowed and
        // epoch columns are deliberately untouched: a fresh online read may
        // still publish under the pre-existing authorization (or stay
        // quarantined if a PAT replacement/rejection is still in effect).
        if repository != "account" {
            let account = repository_key(None);
            ensure_gate(&tx, scope, &account, &epoch)?;
            tx.execute(
                "UPDATE gates SET revision=?1 WHERE partition=?2 AND repository=?3",
                params![nonce(), scope.partition, account],
            )?;
            tx.execute(
                "DELETE FROM entries WHERE partition=?1 AND repository=?2",
                params![scope.partition, account],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn begin_write(&self, scope: &CacheScope, key: &CacheKey) -> Result<CacheWrite> {
        let mut state = self.lock()?;
        let tx = state.db.transaction()?;
        let (_, epoch, _) = validate_scope(&tx, scope)?;
        ensure_gate(&tx, scope, &key.repository_key(), &epoch)?;
        check_access(&tx, scope, key)?;
        let gate = gate(&tx, scope, &key.repository_key())?.1;
        let revision = nonce();
        tx.execute(
            "INSERT OR REPLACE INTO slots VALUES(?1,?2,?3)",
            params![scope.partition, key.encoded()?, revision],
        )?;
        // Slot eviction can cancel a very old in-flight write; it never publishes
        // it. The ledger itself has a separate hard database page ceiling.
        tx.execute("DELETE FROM slots WHERE rowid IN (SELECT rowid FROM slots ORDER BY rowid DESC LIMIT -1 OFFSET ?1)", [self.inner.config.max_entries as i64])?;
        tx.commit()?;
        Ok(CacheWrite {
            scope: scope.clone(),
            key: key.clone(),
            epoch,
            gate,
            revision,
            instance: self.inner.instance.clone(),
        })
    }
    pub fn put(
        &self,
        ticket: CacheWrite,
        bytes: &[u8],
        metadata: CacheMetadata,
    ) -> Result<WriteOutcome> {
        metadata_valid(&metadata)?;
        if metadata.representation_version != ticket.key.version() {
            return Err(CacheError::InvalidInput);
        }
        let mut state = self.lock()?;
        objects::collect(&self.inner.config.root, &state.db)?;
        let State { db, pins } = &mut *state;
        let tx = db.transaction()?;
        if ticket.instance != self.inner.instance
            || validate_scope(&tx, &ticket.scope)?.1 != ticket.epoch
            || gate(&tx, &ticket.scope, &ticket.key.repository_key())?.1 != ticket.gate
            || tx
                .query_row(
                    "SELECT revision FROM slots WHERE partition=?1 AND key=?2",
                    params![ticket.scope.partition, ticket.key.encoded()?],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .as_deref()
                != Some(&ticket.revision)
        {
            return Err(CacheError::Stale);
        }
        check_access(&tx, &ticket.scope, &ticket.key)?;
        // no-store also invalidates the previously reusable representation.
        if !metadata.permits_storage() {
            tx.execute(
                "DELETE FROM entries WHERE partition=?1 AND key=?2",
                params![ticket.scope.partition, ticket.key.encoded()?],
            )?;
            tx.commit()?;
            return Ok(WriteOutcome::NotStored);
        }
        if bytes.len() as u64 > self.inner.config.max_entry_bytes {
            return Err(CacheError::TooLarge);
        }
        let id = nonce();
        let object = if bytes.len() as u64 > self.inner.config.inline_bytes {
            objects::publish(&self.inner.config.root, &id, bytes)?;
            Some(id.clone())
        } else {
            None
        };
        let inline = if object.is_none() { Some(bytes) } else { None };
        tx.execute("INSERT INTO entries VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
            ON CONFLICT(partition,key) DO UPDATE SET id=excluded.id,metadata=excluded.metadata,inline=excluded.inline,
            object=excluded.object,bytes=excluded.bytes,checksum=excluded.checksum,used=excluded.used",
            params![id, ticket.scope.partition, ticket.key.encoded()?, ticket.key.repository_key(), ticket.key.private,
                serde_json::to_string(&metadata).map_err(|_| CacheError::InvalidInput)?, inline, object, bytes.len() as i64,
                objects::checksum(bytes), now()])?;
        evict(&tx, pins, &self.inner.config)?;
        // If a single insertion would evict itself there is no useful publication.
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM entries WHERE id=?1)",
            [&id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(CacheError::Capacity);
        }
        tx.commit()?;
        Ok(WriteOutcome::Stored)
    }
    pub fn read(&self, scope: &CacheScope, key: &CacheKey) -> Result<CacheRead> {
        let mut state = self.lock()?;
        check_access(&state.db, scope, key)?;
        let id: Option<String> = state
            .db
            .query_row(
                "SELECT id FROM entries WHERE partition=?1 AND key=?2",
                params![scope.partition, key.encoded()?],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else {
            return Ok(CacheRead::Miss(CacheMiss::NotCached));
        };
        if let Err(reason) = self.load(&state.db, &id, key)? {
            state.db.execute("DELETE FROM entries WHERE id=?1", [&id])?;
            return Ok(CacheRead::Miss(reason));
        }
        state
            .db
            .execute("UPDATE entries SET used=?1 WHERE id=?2", params![now(), id])?;
        *state.pins.entry(id.clone()).or_default() += 1;
        Ok(CacheRead::Hit(CacheLease {
            store: self.clone(),
            scope: scope.clone(),
            key: key.clone(),
            id,
        }))
    }
    fn load(
        &self,
        db: &Connection,
        id: &str,
        key: &CacheKey,
    ) -> Result<std::result::Result<CacheSnapshot, CacheMiss>> {
        let row = db
            .query_row(
                "SELECT substr(metadata,1,16385),CASE WHEN length(inline)=0 THEN inline ELSE substr(inline,1,?2) END,object,bytes,checksum FROM entries WHERE id=?1",
                params![id, self.inner.config.max_entry_bytes + 1],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<Vec<u8>>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, u64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((metadata, inline, object, size, checksum)) = row else {
            return Ok(Err(CacheMiss::NotCached));
        };
        if size > self.inner.config.max_entry_bytes {
            return Ok(Err(CacheMiss::Corrupt));
        }
        let metadata: CacheMetadata = match serde_json::from_str(&metadata) {
            Ok(value) => value,
            Err(_) => return Ok(Err(CacheMiss::Corrupt)),
        };
        if metadata_valid(&metadata).is_err() || !metadata.permits_storage() {
            return Ok(Err(CacheMiss::Corrupt));
        }
        if metadata.representation_version != key.version() {
            return Ok(Err(CacheMiss::RepresentationVersion));
        }
        let bytes = match (inline, object) {
            (Some(value), None) => value,
            (None, Some(name)) => match objects::read(&self.inner.config.root, &name, size) {
                Ok(value) => value,
                Err(_) => return Ok(Err(CacheMiss::Corrupt)),
            },
            _ => return Ok(Err(CacheMiss::Corrupt)),
        };
        if bytes.len() as u64 != size || objects::checksum(&bytes) != checksum {
            return Ok(Err(CacheMiss::Corrupt));
        }
        Ok(Ok(CacheSnapshot { bytes, metadata }))
    }
    /// Forget one representation after a successful mutation/explicit invalidation.
    /// Older network writers cannot repopulate it; the authorization gate remains.
    pub fn invalidate(&self, scope: &CacheScope, key: &CacheKey) -> Result<()> {
        let mut state = self.lock()?;
        let tx = state.db.transaction()?;
        validate_scope(&tx, scope)?;
        tx.execute(
            "DELETE FROM entries WHERE partition=?1 AND key=?2",
            params![scope.partition, key.encoded()?],
        )?;
        tx.execute(
            "DELETE FROM slots WHERE partition=?1 AND key=?2",
            params![scope.partition, key.encoded()?],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Removes only business content. Gate/retirement history survives, and all
    /// pre-clear write tickets are invalidated. Physical deletion via recover().
    pub fn clear(&self) -> Result<()> {
        let mut state = self.lock()?;
        let tx = state.db.transaction()?;
        tx.execute("DELETE FROM entries", [])?;
        tx.execute("DELETE FROM slots", [])?;
        tx.commit()?;
        Ok(())
    }
    pub fn usage(&self) -> Result<(u64, usize)> {
        Ok(self.lock()?.db.query_row(
            "SELECT coalesce(sum(bytes),0),count(*) FROM entries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }
    /// Retryable cleanup. Exclusive store lease prevents another publisher racing
    /// orphan removal. Never follows object-directory symlinks or recursive paths.
    pub fn recover(&self) -> Result<()> {
        let mut state = self.lock()?;
        let State { db, pins } = &mut *state;
        let tx = db.transaction()?;
        tx.execute("DELETE FROM entries WHERE partition IN (SELECT partition FROM scopes WHERE state='retired')", [])?;
        tx.execute("DELETE FROM slots WHERE partition IN (SELECT partition FROM scopes WHERE state='retired')", [])?;
        // Malformed sizes must not count against capacity and evict healthy peers.
        tx.execute(
            "DELETE FROM entries WHERE bytes>?1",
            [self.inner.config.max_entry_bytes],
        )?;
        evict(&tx, pins, &self.inner.config)?;
        tx.commit()?;
        objects::collect(&self.inner.config.root, &state.db)?;
        state.db.execute_batch("PRAGMA incremental_vacuum(256)")?;
        Ok(())
    }
}
fn nonce() -> String {
    Uuid::new_v4().to_string()
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn metadata_valid(value: &CacheMetadata) -> Result<()> {
    if value.representation_version == 0
        || value.fetched_at_ms < 0
        || value.verified_at_ms < value.fetched_at_ms
        || serde_json::to_vec(value)
            .map_err(|_| CacheError::InvalidInput)?
            .len()
            > 16384
    {
        return Err(CacheError::InvalidInput);
    }
    Ok(())
}
fn validate_scope(db: &Connection, scope: &CacheScope) -> Result<(String, String, bool)> {
    let row = db.query_row("SELECT state,epoch,restricted FROM scopes WHERE partition=?1 AND host=?2 AND account=?3",
        params![scope.partition, scope.account.host.as_str(), scope.account.id.to_string()],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?))).optional()?;
    match row {
        Some((state, _, _)) if state == "retired" => Err(CacheError::AccessDenied),
        Some(row) => Ok(row),
        None => Err(CacheError::AccessDenied),
    }
}
fn ensure_gate(db: &Connection, scope: &CacheScope, repository: &str, epoch: &str) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO gates SELECT ?1,?2,?3,?4,1-restricted FROM scopes WHERE partition=?1", params![scope.partition, repository, epoch, nonce()])?;
    Ok(())
}
fn gate(db: &Connection, scope: &CacheScope, repository: &str) -> Result<(String, String, bool)> {
    Ok(db.query_row(
        "SELECT epoch,revision,allowed FROM gates WHERE partition=?1 AND repository=?2",
        params![scope.partition, repository],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}
fn check_access(db: &Connection, scope: &CacheScope, key: &CacheKey) -> Result<()> {
    let (state, epoch, restricted) = validate_scope(db, scope)?;
    if !key.private {
        return Ok(());
    }
    if state != "active" {
        return Err(CacheError::AccessDenied);
    }
    let permission = db
        .query_row(
            "SELECT epoch,allowed FROM gates WHERE partition=?1 AND repository=?2",
            params![scope.partition, key.repository_key()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional()?;
    match permission {
        Some((observed, allowed)) if allowed && (!restricted || observed == epoch) => Ok(()),
        None if !restricted => Ok(()),
        _ => Err(CacheError::AccessDenied),
    }
}
fn evict(db: &Connection, pins: &HashMap<String, usize>, config: &CacheConfig) -> Result<()> {
    let (mut bytes, mut count): (u64, usize) = db.query_row(
        "SELECT coalesce(sum(bytes),0),count(*) FROM entries",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut stmt = db.prepare("SELECT id,bytes FROM entries ORDER BY used,rowid")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (id, size) in rows {
        if bytes <= config.capacity_bytes && count <= config.max_entries {
            break;
        }
        if pins.contains_key(&id) {
            continue;
        }
        db.execute("DELETE FROM entries WHERE id=?1", [&id])?;
        bytes -= size;
        count -= 1;
    }
    if bytes > config.capacity_bytes || count > config.max_entries {
        return Err(CacheError::Capacity);
    }
    Ok(())
}
