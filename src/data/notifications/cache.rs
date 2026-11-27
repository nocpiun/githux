//! Bounded, private projection of previously observed notification rows.
use super::*;
use crate::data::AccountCache;
use crate::domain::cache::{CacheCompleteness, CacheMetadata};
use crate::domain::repository::{RepositoryIdentity, RepositoryLocator};
use crate::github::client::GitHubConfig;
use crate::storage::cache::{CacheAuthorization, CacheError, CacheKey, CacheRead, CacheWrite};
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
pub(super) const MAX_ROWS: usize = 100;
const MAX_ROW_BYTES: usize = 4096;
const MAX_BYTES: usize = 256 * 1024;

#[derive(Clone)]
pub(super) struct InboxCache {
    account: AccountCache,
    key: CacheKey,
}

#[derive(Serialize, Deserialize)]
struct Row {
    id: String,
    repository_id: u64,
    owner: String,
    repository: String,
    title: String,
    kind: SubjectKind,
    subject_route: Option<String>,
    reason: String,
    // Earlier projections contained only read rows.
    #[serde(default)]
    unread: bool,
    updated_at: String,
}

impl From<NotificationThread> for Row {
    fn from(item: NotificationThread) -> Self {
        let subject_route = item.target().and(item.subject.url);
        let locator = item.repository.locator();
        Self {
            id: item.id,
            repository_id: item.repository.id(),
            owner: locator.owner().to_string(),
            repository: locator.name().to_string(),
            title: item.subject.title,
            kind: item.subject.kind,
            subject_route,
            reason: item.reason,
            unread: item.unread,
            updated_at: item.updated_at,
        }
    }
}

impl Row {
    fn into_thread(self) -> Option<NotificationThread> {
        let locator = RepositoryLocator::new(&self.owner, &self.repository).ok()?;
        Some(NotificationThread {
            id: self.id,
            repository: RepositoryIdentity {
                id: self.repository_id,
                locator,
            },
            subject: NotificationSubject {
                title: self.title,
                kind: self.kind,
                url: self.subject_route,
                latest_comment_url: None,
            },
            reason: self.reason,
            unread: self.unread,
            updated_at: self.updated_at,
            last_read_at: None,
        })
    }
}

impl InboxCache {
    pub(super) fn new(account: AccountCache, config: &GitHubConfig) -> Option<Self> {
        let request = serde_json::json!([
            config.base_url,
            config.accept,
            config.api_version,
            "all=false&participating=false&per_page=50"
        ]);
        let key = CacheKey::mutable(
            None,
            true,
            "notifications",
            &request.to_string(),
            "read-rows",
            VERSION,
        )
        .ok()?;
        Some(Self { account, key })
    }

    pub(super) fn authorization(&self) -> Option<CacheAuthorization> {
        self.account
            .store()
            .authorization(self.account.scope(), None)
    }

    pub(super) fn begin_write(&self) -> Option<CacheWrite> {
        self.account
            .store()
            .begin_write(self.account.scope(), &self.key)
            .ok()
    }

    pub(super) fn invalidate(&self) {
        let _ = self
            .account
            .store()
            .invalidate(self.account.scope(), &self.key);
    }

    // An explicit repository denial must also exclude new aggregate projections
    // containing that repository. A miss still means the repository gate allowed
    // the read. This check neither records permission nor writes a repository slot.
    fn check_repositories(&self, ids: impl Iterator<Item = u64>) -> Result<(), CacheError> {
        for id in ids.collect::<HashSet<_>>() {
            let key = CacheKey::mutable(Some(id), true, "notifications", "", "read-rows", VERSION)?;
            self.account.store().read(self.account.scope(), &key)?;
        }
        Ok(())
    }

    pub(super) fn read(&self) -> Option<(Vec<NotificationThread>, i64)> {
        let CacheRead::Hit(lease) = self
            .account
            .store()
            .read(self.account.scope(), &self.key)
            .ok()?
        else {
            return None;
        };
        let snapshot = lease.snapshot().ok()??;
        if snapshot.bytes.len() > MAX_BYTES {
            return None;
        }
        let rows: Vec<Row> = serde_json::from_slice(&snapshot.bytes).ok()?;
        if rows.is_empty() || rows.len() > MAX_ROWS {
            return None;
        }
        self.check_repositories(rows.iter().map(|row| row.repository_id))
            .ok()?;
        let items = rows
            .into_iter()
            .map(Row::into_thread)
            .collect::<Option<_>>()?;
        Some((items, snapshot.metadata.fetched_at_ms))
    }

    pub(super) fn write(&self, ticket: CacheWrite, items: Option<Vec<NotificationThread>>) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let mut metadata = CacheMetadata {
            representation_version: VERSION,
            fetched_at_ms: stamp,
            verified_at_ms: stamp,
            completeness: CacheCompleteness::Partial {
                reason: "recent loaded rows".into(),
            },
            etag: None,
            last_modified: None,
            cache_control: Some("no-store".into()),
        };
        let mut bytes = Vec::new();
        if let Some(items) = items {
            let mut rows = Vec::new();
            let mut size = 2;
            for item in items.into_iter().take(MAX_ROWS) {
                let row = Row::from(item);
                let Ok(encoded) = serde_json::to_vec(&row) else {
                    continue;
                };
                if encoded.len() > MAX_ROW_BYTES || size + encoded.len() + 1 > MAX_BYTES {
                    continue;
                }
                size += encoded.len() + 1;
                rows.push(row);
            }
            if !rows.is_empty()
                && self
                    .check_repositories(rows.iter().map(|row| row.repository_id))
                    .is_ok()
                && let Ok(encoded) = serde_json::to_vec(&rows)
            {
                bytes = encoded;
                metadata.cache_control = None;
            }
        }
        let _ = self.account.store().put(ticket, &bytes, metadata);
    }
}
