use super::{CacheError, Result};
use rusqlite::Connection;

pub(super) fn initialize(db: &mut Connection, max_pages: u64) -> Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > 1 {
        return Err(CacheError::FutureSchema(version));
    }
    if db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? != "ok" {
        return Err(CacheError::Corrupt);
    }
    db.pragma_update(None, "foreign_keys", "ON")?;
    db.pragma_update(None, "synchronous", "FULL")?;
    db.pragma_update(None, "journal_mode", "DELETE")?;
    if version == 0 {
        let count: i64 = db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if count != 0 {
            return Err(CacheError::Corrupt);
        }
        db.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        let tx = db.transaction()?;
        tx.execute_batch(
            "
CREATE TABLE scopes(partition TEXT PRIMARY KEY, host TEXT NOT NULL, account TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('active','rejected','retired')), epoch TEXT NOT NULL,
 restricted INTEGER NOT NULL CHECK(restricted IN (0,1)));
CREATE UNIQUE INDEX live_account ON scopes(host,account) WHERE state != 'retired';
CREATE TABLE gates(partition TEXT NOT NULL REFERENCES scopes(partition), repository TEXT NOT NULL,
 epoch TEXT NOT NULL, revision TEXT NOT NULL, allowed INTEGER NOT NULL CHECK(allowed IN (0,1)),
 PRIMARY KEY(partition,repository));
CREATE TABLE slots(partition TEXT NOT NULL REFERENCES scopes(partition), key TEXT NOT NULL,
 revision TEXT NOT NULL, PRIMARY KEY(partition,key));
CREATE TABLE entries(id TEXT PRIMARY KEY, partition TEXT NOT NULL REFERENCES scopes(partition),
 key TEXT NOT NULL, repository TEXT NOT NULL, private INTEGER NOT NULL CHECK(private IN (0,1)),
 metadata TEXT NOT NULL, inline BLOB, object TEXT, bytes INTEGER NOT NULL CHECK(bytes>=0),
 checksum TEXT NOT NULL, used INTEGER NOT NULL,
 CHECK((inline IS NULL) != (object IS NULL)), UNIQUE(partition,key));
CREATE INDEX entry_lru ON entries(used);
CREATE INDEX entry_scope ON entries(partition,repository);
PRAGMA user_version=1;",
        )?;
        tx.commit()?;
    }
    // A hard database ceiling also bounds key/validator/gate bookkeeping. SQLITE_FULL
    // is an explicit failure, never grounds to rebuild the authorization ledger.
    db.pragma_update(None, "max_page_count", max_pages)?;
    Ok(())
}
