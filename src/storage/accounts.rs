//! SQLite stores only display metadata and secret-free credential references.
use crate::auth::CredentialRef;
use crate::domain::{AccountIdentity, AccountKey, HostId};
use rusqlite::{Connection, OpenFlags, params};
use std::{fs::File, path::PathBuf, time::Duration};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StorageError {
    #[error("account storage path unavailable")]
    PathUnavailable,
    #[error("account database already in use")]
    InUse,
    #[error("account storage I/O failed")]
    Io,
    #[error("account database is read only")]
    ReadOnly,
    #[error("account database is busy")]
    Busy,
    #[error("account database is corrupt or has invalid records")]
    Corrupt,
    #[error("unsupported account schema {0}")]
    FutureSchema(i64),
    #[error("account database operation failed")]
    Database,
    #[error("account service is closed")]
    Closed,
}
impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode::*;
        match error.sqlite_error_code() {
            Some(DatabaseBusy | DatabaseLocked) => Self::Busy,
            Some(ReadOnly) => Self::ReadOnly,
            Some(DatabaseCorrupt | NotADatabase) => Self::Corrupt,
            _ => Self::Database,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountDatabaseConfig {
    pub path: PathBuf,
    pub busy_timeout: Duration,
    /// Explicit read-only opens never migrate or perform credential recovery.
    pub read_only: bool,
}
impl AccountDatabaseConfig {
    pub fn application_data() -> Result<Self, StorageError> {
        let root = dirs::data_local_dir().ok_or(StorageError::PathUnavailable)?;
        Ok(Self::at(root.join("Githux").join("accounts.sqlite3")))
    }
    pub fn at(path: PathBuf) -> Self {
        Self {
            path,
            busy_timeout: Duration::from_millis(500),
            read_only: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SavedAccount {
    pub identity: AccountIdentity,
    pub credential: CredentialRef,
}
#[derive(Debug, Clone, Default)]
pub struct Directory {
    pub accounts: Vec<SavedAccount>,
    pub active: Option<AccountKey>,
}

pub struct AccountDatabase {
    connection: Option<Connection>,
    // An OS advisory lock also excludes a second service/process whose memory
    // snapshot would otherwise overwrite a newer directory. Never unlink it.
    lease: Option<File>,
    read_only: bool,
}
impl Drop for AccountDatabase {
    fn drop(&mut self) {
        // Explicit close also runs on a blocking worker. The fallback covers a
        // caller dropping an unclosed service on the GPUI thread.
        if let Some(connection) = self.connection.take() {
            let lease = self.lease.take();
            std::thread::spawn(move || {
                drop(connection);
                drop(lease);
            });
        }
    }
}
impl AccountDatabase {
    pub fn close(mut self) {
        // Blocking caller owns the shutdown boundary and observes lease release.
        drop(self.connection.take());
        drop(self.lease.take());
    }
    fn connection(&self) -> &Connection {
        self.connection.as_ref().expect("open database")
    }
    /// Blocking: callers must use a blocking worker, including open/migration.
    pub fn open(config: AccountDatabaseConfig) -> Result<Self, StorageError> {
        if !config.path.is_absolute() {
            return Err(StorageError::PathUnavailable);
        }
        if !config.read_only {
            std::fs::create_dir_all(config.path.parent().ok_or(StorageError::PathUnavailable)?)
                .map_err(|_| StorageError::Io)?;
        }
        let lease = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(config.path.with_extension("sqlite3.lock"))
            .map_err(|_| StorageError::Io)?;
        lease.try_lock().map_err(|_| StorageError::InUse)?;
        let flags = if config.read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        };
        let mut connection = Connection::open_with_flags(&config.path, flags)?;
        connection.busy_timeout(config.busy_timeout.min(Duration::from_secs(5)))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 1 {
            return Err(StorageError::FutureSchema(version));
        }
        let integrity: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(StorageError::Corrupt);
        }
        if version == 0 {
            if config.read_only {
                return Err(StorageError::ReadOnly);
            }
            let count: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if count != 0 {
                return Err(StorageError::Corrupt);
            }
            let tx = connection.transaction()?;
            tx.execute_batch("CREATE TABLE accounts(position INTEGER PRIMARY KEY, host TEXT NOT NULL, id TEXT NOT NULL, login TEXT NOT NULL, name TEXT, avatar TEXT, url TEXT, revision TEXT, UNIQUE(host,id));
                CREATE TABLE selection(singleton INTEGER PRIMARY KEY CHECK(singleton=1), host TEXT, id TEXT);
                INSERT INTO selection VALUES(1,NULL,NULL);
                CREATE TABLE cleanup(host TEXT NOT NULL, id TEXT NOT NULL, revision TEXT NOT NULL, PRIMARY KEY(host,id,revision));
                PRAGMA user_version=1;")?;
            tx.commit()?;
        }
        if !config.read_only {
            connection.pragma_update(None, "synchronous", "FULL")?;
            // Obtain a write transaction even for an existing v1 database so
            // startup reports read-only/locked storage before accepting writes.
            connection.execute_batch("BEGIN IMMEDIATE; COMMIT;")?;
        }
        let db = Self {
            connection: Some(connection),
            lease: Some(lease),
            read_only: config.read_only,
        };
        db.load()?;
        db.pending()?;
        Ok(db)
    }
    pub fn load(&self) -> Result<Directory, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT host,id,login,name,avatar,url,revision FROM accounts ORDER BY position",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut accounts = Vec::new();
        for row in rows {
            let (host, id, login, display_name, avatar_url, html_url, revision) = row?;
            let key = key(host, id)?;
            let credential = reference(&key, revision)?;
            accounts.push(SavedAccount {
                identity: AccountIdentity {
                    key,
                    login,
                    display_name,
                    avatar_url,
                    html_url,
                },
                credential,
            });
        }
        let (host, id): (Option<String>, Option<String>) = self.connection().query_row(
            "SELECT host,id FROM selection WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let active = match (host, id) {
            (None, None) => None,
            (Some(h), Some(i)) => Some(key(h, i)?),
            _ => return Err(StorageError::Corrupt),
        };
        if active
            .as_ref()
            .is_some_and(|k| !accounts.iter().any(|a| &a.identity.key == k))
        {
            return Err(StorageError::Corrupt);
        }
        Ok(Directory { accounts, active })
    }
    /// Record a cleanup route BEFORE any new secret can be created.
    pub fn stage(&self, credential: &CredentialRef) -> Result<(), StorageError> {
        self.writable()?;
        self.connection().execute(
            "INSERT OR IGNORE INTO cleanup VALUES(?1,?2,?3)",
            params![
                credential.host().as_str(),
                credential.account_id().to_string(),
                credential.revision().unwrap_or("")
            ],
        )?;
        Ok(())
    }
    /// Atomically publish the directory and retain all retired secret references.
    pub fn commit(
        &mut self,
        directory: &Directory,
        published: Option<&CredentialRef>,
        retired: Option<&CredentialRef>,
    ) -> Result<(), StorageError> {
        self.writable()?;
        let tx = self
            .connection
            .as_mut()
            .expect("open database")
            .transaction()?;
        tx.execute("DELETE FROM accounts", [])?;
        for (position, a) in directory.accounts.iter().enumerate() {
            tx.execute(
                "INSERT INTO accounts VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    position as i64,
                    a.identity.key.host.as_str(),
                    a.identity.key.id.to_string(),
                    a.identity.login,
                    a.identity.display_name,
                    a.identity.avatar_url,
                    a.identity.html_url,
                    a.credential.revision()
                ],
            )?;
        }
        tx.execute(
            "UPDATE selection SET host=?1,id=?2 WHERE singleton=1",
            params![
                directory.active.as_ref().map(|a| a.host.as_str()),
                directory.active.as_ref().map(|a| a.id.to_string())
            ],
        )?;
        if let Some(r) = retired {
            tx.execute(
                "INSERT OR IGNORE INTO cleanup VALUES(?1,?2,?3)",
                params![
                    r.host().as_str(),
                    r.account_id().to_string(),
                    r.revision().unwrap_or("")
                ],
            )?;
        }
        if let Some(r) = published {
            tx.execute(
                "DELETE FROM cleanup WHERE host=?1 AND id=?2 AND revision=?3",
                params![
                    r.host().as_str(),
                    r.account_id().to_string(),
                    r.revision().unwrap_or("")
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn pending(&self) -> Result<Vec<CredentialRef>, StorageError> {
        let mut stmt = self
            .connection()
            .prepare("SELECT host,id,revision FROM cleanup")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut refs = Vec::new();
        for row in rows {
            let (h, i, r) = row?;
            let key = key(h, i)?;
            refs.push(reference(&key, if r.is_empty() { None } else { Some(r) })?);
        }
        // A malformed recovery entry must never cause deletion of a live PAT.
        let directory = self.load()?;
        if refs
            .iter()
            .any(|r| directory.accounts.iter().any(|a| &a.credential == r))
        {
            return Err(StorageError::Corrupt);
        }
        Ok(refs)
    }
    pub fn cleaned(&self, r: &CredentialRef) -> Result<(), StorageError> {
        self.writable()?;
        self.connection().execute(
            "DELETE FROM cleanup WHERE host=?1 AND id=?2 AND revision=?3",
            params![
                r.host().as_str(),
                r.account_id().to_string(),
                r.revision().unwrap_or("")
            ],
        )?;
        Ok(())
    }
    fn writable(&self) -> Result<(), StorageError> {
        if self.read_only {
            Err(StorageError::ReadOnly)
        } else {
            Ok(())
        }
    }
}
fn key(host: String, id: String) -> Result<AccountKey, StorageError> {
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err(StorageError::Corrupt);
    }
    let parsed = id.parse::<u64>().map_err(|_| StorageError::Corrupt)?;
    if parsed.to_string() != id {
        return Err(StorageError::Corrupt);
    }
    Ok(AccountKey::new(parsed, HostId::new(host)))
}
fn reference(key: &AccountKey, revision: Option<String>) -> Result<CredentialRef, StorageError> {
    match revision {
        None => Ok(CredentialRef::new(key.host.clone(), key.id)),
        Some(r) if uuid::Uuid::parse_str(&r).is_ok() => {
            Ok(CredentialRef::versioned(key.host.clone(), key.id, r))
        }
        _ => Err(StorageError::Corrupt),
    }
}
