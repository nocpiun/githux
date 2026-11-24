//! Account-scoped single-Issue and paginated-comments data session, reusing
//! [`RepositorySession`]'s credential, auth epoch, concurrency semaphore and cancellation path,
//! plus optional persistent snapshots via [`AccountCache`].

#[cfg(test)]
#[path = "issue_detail_tests.rs"]
mod tests;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{AccountCache, RepoService, RepositoryReadContext, RepositorySession, Scoped};
use crate::domain::cache::{CacheCompleteness, CacheMetadata};
use crate::domain::error::GitHubError;
use crate::domain::issue_detail::{
    IssueAuthor, IssueComment, IssueCommentsCursor, IssueCommentsPage, IssueDetail,
    IssueDetailError, IssueLabel, IssueLocator, IssueState, IssueStateReason,
};
use crate::github::{
    GitHubClient,
    issue_detail::{self, IssueDetailLimits},
};
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{CacheAuthorization, CacheKey, CacheRead, CacheWrite};

const SNAPSHOT_VERSION: u32 = 1;
const ISSUE_REPRESENTATION: &str = "githux-issue-detail-v1";
const COMMENTS_REPRESENTATION: &str = "githux-issue-comments-v1";

/// Authorized cached snapshot of an Issue detail, paired with its [`CacheAuthorization`] token
/// so the UI can reject revoked/retired data with a single atomic load (L022).
#[derive(Clone, Debug)]
pub struct CachedIssueSnapshot {
    pub issue: IssueDetail,
    pub verified_at_ms: i64,
    pub authorization: CacheAuthorization,
}

/// Authorized cached snapshot of an Issue's first page of comments.
#[derive(Clone, Debug)]
pub struct CachedCommentsSnapshot {
    pub page: IssueCommentsPage,
    pub verified_at_ms: i64,
    pub authorization: CacheAuthorization,
}

#[derive(Serialize, Deserialize)]
struct IssueSnapshotDto {
    id: u64,
    number: u64,
    owner: String,
    name: String,
    title: String,
    state: IssueState,
    state_reason: Option<IssueStateReason>,
    author: Option<IssueAuthor>,
    body: Option<String>,
    comments_count: u64,
    labels: Vec<IssueLabel>,
    created_at: String,
    updated_at: String,
    closed_at: Option<String>,
    html_url: Option<String>,
}

impl IssueSnapshotDto {
    fn from_detail(detail: &IssueDetail) -> Self {
        Self {
            id: detail.id,
            number: detail.number,
            owner: detail.repository.owner().to_string(),
            name: detail.repository.name().to_string(),
            title: detail.title.clone(),
            state: detail.state.clone(),
            state_reason: detail.state_reason.clone(),
            author: detail.author.clone(),
            body: detail.body.clone(),
            comments_count: detail.comments_count,
            labels: detail.labels.clone(),
            created_at: detail.created_at.clone(),
            updated_at: detail.updated_at.clone(),
            closed_at: detail.closed_at.clone(),
            html_url: detail.html_url.clone(),
        }
    }

    fn into_detail(self) -> Option<IssueDetail> {
        let repository =
            crate::domain::repository::RepositoryLocator::new(self.owner, self.name).ok()?;
        Some(IssueDetail {
            id: self.id,
            number: self.number,
            repository,
            title: self.title,
            state: self.state,
            state_reason: self.state_reason,
            author: self.author,
            body: self.body,
            comments_count: self.comments_count,
            labels: self.labels,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
            html_url: self.html_url,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct CommentsPageSnapshotDto {
    comments: Vec<IssueComment>,
    has_next: bool,
    per_page: u32,
}

/// Account-bound session for reading a single Issue and its paginated comments.
#[derive(Clone)]
pub struct IssueDetailSession {
    repository: RepositorySession,
    client: Arc<GitHubClient>,
    cache: Option<AccountCache>,
}

impl std::fmt::Debug for IssueDetailSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssueDetailSession")
            .field("context", self.context())
            .field("has_cache", &self.cache.is_some())
            .finish_non_exhaustive()
    }
}

impl RepoService {
    /// Capture an account-bound session for reading Issue details and comments.
    pub fn issue_detail_session(&self) -> Result<IssueDetailSession, IssueDetailError> {
        let repository = self.session().map_err(|_| GitHubError::StaleContext)?;
        let cache = self.cache(repository.context());
        Ok(IssueDetailSession {
            repository,
            client: self.client.clone(),
            cache,
        })
    }
}

impl IssueDetailSession {
    pub fn context(&self) -> &RepositoryReadContext {
        self.repository.context()
    }

    pub fn verify(&self) -> bool {
        self.repository.verify()
    }

    fn issue_cache_key(&self, locator: &IssueLocator) -> Option<CacheKey> {
        let (owner, name, number) = locator.normalized_key();
        let config = self.client.config();
        let request = serde_json::json!({
            "owner": owner,
            "repo": name,
            "number": number,
            "base": config.base_url,
            "accept": config.accept,
            "api": config.api_version,
        })
        .to_string();
        CacheKey::mutable(
            None,
            true,
            "issue-detail",
            &request,
            ISSUE_REPRESENTATION,
            SNAPSHOT_VERSION,
        )
        .ok()
    }

    fn comments_p1_cache_key(&self, locator: &IssueLocator, per_page: u32) -> Option<CacheKey> {
        let (owner, name, number) = locator.normalized_key();
        let config = self.client.config();
        let request = serde_json::json!({
            "owner": owner,
            "repo": name,
            "number": number,
            "per_page": per_page,
            "base": config.base_url,
            "accept": config.accept,
            "api": config.api_version,
        })
        .to_string();
        CacheKey::mutable(
            None,
            true,
            "issue-comments-p1",
            &request,
            COMMENTS_REPRESENTATION,
            SNAPSHOT_VERSION,
        )
        .ok()
    }

    /// Best-effort read of a previously cached authorized snapshot for `locator`.
    /// Never blocks the caller thread; returns `Ok(None)` when uncached, unauthorized or corrupt.
    pub async fn read_cached_issue(
        &self,
        locator: &IssueLocator,
        cancel: CancellationToken,
    ) -> Result<Option<Scoped<CachedIssueSnapshot>>, IssueDetailError> {
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let Some(cache) = self.cache.clone() else {
            return Ok(None);
        };
        let Some(key) = self.issue_cache_key(locator) else {
            return Ok(None);
        };
        let context = self.context().clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            let store = cache.store();
            let scope = cache.scope();
            let authorization = store.authorization(scope, None)?;
            if !authorization.is_current() {
                return None;
            }
            match store.read(scope, &key).ok()? {
                CacheRead::Hit(lease) => {
                    if !authorization.is_current() {
                        return None;
                    }
                    let hit = lease.snapshot().ok().flatten()?;
                    let dto: IssueSnapshotDto = serde_json::from_slice(&hit.bytes).ok()?;
                    let issue = dto.into_detail()?;
                    Some(CachedIssueSnapshot {
                        issue,
                        verified_at_ms: hit.metadata.verified_at_ms,
                        authorization,
                    })
                }
                CacheRead::Miss(_) => None,
            }
        })
        .await
        .ok()
        .flatten();

        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        Ok(snapshot
            .filter(|snap| snap.authorization.is_current())
            .map(|value| Scoped { context, value }))
    }

    /// Best-effort read of a previously cached authorized first page of comments for `locator`.
    pub async fn read_cached_comments_first_page(
        &self,
        locator: &IssueLocator,
        limits: &IssueDetailLimits,
        cancel: CancellationToken,
    ) -> Result<Option<Scoped<CachedCommentsSnapshot>>, IssueDetailError> {
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let Some(cache) = self.cache.clone() else {
            return Ok(None);
        };
        let Some(key) = self.comments_p1_cache_key(locator, limits.comments_per_page) else {
            return Ok(None);
        };
        let locator_clone = locator.clone();
        let origin = comments_origin(&self.client, locator);
        let context = self.context().clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            let store = cache.store();
            let scope = cache.scope();
            let authorization = store.authorization(scope, None)?;
            if !authorization.is_current() {
                return None;
            }
            match store.read(scope, &key).ok()? {
                CacheRead::Hit(lease) => {
                    if !authorization.is_current() {
                        return None;
                    }
                    let hit = lease.snapshot().ok().flatten()?;
                    let dto: CommentsPageSnapshotDto = serde_json::from_slice(&hit.bytes).ok()?;
                    let next = if dto.has_next {
                        Some(IssueCommentsCursor {
                            locator: locator_clone.clone(),
                            page: 2,
                            per_page: dto.per_page,
                            origin: origin?,
                        })
                    } else {
                        None
                    };
                    Some(CachedCommentsSnapshot {
                        page: IssueCommentsPage {
                            comments: dto.comments,
                            next,
                            page: 1,
                        },
                        verified_at_ms: hit.metadata.verified_at_ms,
                        authorization,
                    })
                }
                CacheRead::Miss(_) => None,
            }
        })
        .await
        .ok()
        .flatten();

        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        Ok(snapshot
            .filter(|snap| snap.authorization.is_current())
            .map(|value| Scoped { context, value }))
    }

    /// Fetch the authoritative Issue detail from GitHub and update/invalidate the cache accordingly.
    pub async fn fetch_issue(
        &self,
        locator: &IssueLocator,
        limits: &IssueDetailLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<IssueDetail>, IssueDetailError> {
        // Acquire the write ticket BEFORE the network read so any concurrent invalidation or
        // newer write supersedes this in-flight response.
        let write_ticket = self.begin_cache_write(self.issue_cache_key(locator)).await;

        let result = self
            .repository
            .read(cancel, async move |pat, cancel| {
                issue_detail::fetch_issue(&self.client, &pat, locator, limits, cancel).await
            })
            .await;

        match result {
            Ok(scoped) => {
                let response = scoped.value;
                if self.verify()
                    && let (Some(cache), Some(ticket)) = (self.cache.clone(), write_ticket)
                {
                    let dto = IssueSnapshotDto::from_detail(&response.value);
                    let no_store = response.no_store;
                    let etag = response.etag.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        put_snapshot(&cache, ticket, &dto, no_store, etag);
                    })
                    .await;
                }
                if !self.verify() {
                    return Err(GitHubError::StaleContext.into());
                }
                Ok(Scoped {
                    context: scoped.context,
                    value: response.value,
                })
            }
            Err(error) => {
                if error.is_missing()
                    || error.is_auth_or_permission()
                    || matches!(error, IssueDetailError::NotAnIssue)
                {
                    self.invalidate_issue_caches(locator, limits.comments_per_page)
                        .await;
                }
                Err(error)
            }
        }
    }

    /// Fetch one bounded page of plain comments from GitHub and update/invalidate page-1 cache.
    pub async fn fetch_comments_page(
        &self,
        locator: &IssueLocator,
        cursor: Option<&IssueCommentsCursor>,
        limits: &IssueDetailLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<IssueCommentsPage>, IssueDetailError> {
        let is_first_page = cursor.is_none();
        let write_ticket = if is_first_page {
            self.begin_cache_write(self.comments_p1_cache_key(locator, limits.comments_per_page))
                .await
        } else {
            None
        };

        let result = self
            .repository
            .read(cancel, async move |pat, cancel| {
                issue_detail::fetch_comments_page(
                    &self.client,
                    &pat,
                    locator,
                    cursor,
                    limits,
                    cancel,
                )
                .await
            })
            .await;

        match result {
            Ok(scoped) => {
                let response = scoped.value;
                if is_first_page
                    && self.verify()
                    && let (Some(cache), Some(ticket)) = (self.cache.clone(), write_ticket)
                {
                    let dto = CommentsPageSnapshotDto {
                        comments: response.value.comments.clone(),
                        has_next: response.value.next.is_some(),
                        per_page: limits.comments_per_page,
                    };
                    let no_store = response.no_store;
                    let etag = response.etag.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        put_snapshot(&cache, ticket, &dto, no_store, etag);
                    })
                    .await;
                }
                if !self.verify() {
                    return Err(GitHubError::StaleContext.into());
                }
                Ok(Scoped {
                    context: scoped.context,
                    value: response.value,
                })
            }
            Err(error) => {
                if (error.is_missing() || error.is_auth_or_permission())
                    && let Some(cache) = self.cache.clone()
                    && let Some(key) = self.comments_p1_cache_key(locator, limits.comments_per_page)
                {
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = cache.store().invalidate(cache.scope(), &key);
                    })
                    .await;
                }
                Err(error)
            }
        }
    }

    async fn begin_cache_write(&self, key: Option<CacheKey>) -> Option<CacheWrite> {
        if !self.verify() {
            return None;
        }
        let cache = self.cache.clone()?;
        let key = key?;
        tokio::task::spawn_blocking(move || cache.store().begin_write(cache.scope(), &key).ok())
            .await
            .ok()
            .flatten()
    }

    async fn invalidate_issue_caches(&self, locator: &IssueLocator, per_page: u32) {
        let Some(cache) = self.cache.clone() else {
            return;
        };
        let issue_key = self.issue_cache_key(locator);
        let comments_key = self.comments_p1_cache_key(locator, per_page);
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(key) = issue_key {
                let _ = cache.store().invalidate(cache.scope(), &key);
            }
            if let Some(key) = comments_key {
                let _ = cache.store().invalidate(cache.scope(), &key);
            }
        })
        .await;
    }
}

fn comments_origin(client: &GitHubClient, locator: &IssueLocator) -> Option<String> {
    let mut url = reqwest::Url::parse(&client.config().base_url).ok()?;
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.pop_if_empty();
        segments.push("repos");
        segments.push(locator.owner());
        segments.push(locator.name());
        segments.push("issues");
        segments.push(&locator.number().to_string());
        segments.push("comments");
    }
    Some(url.as_str().to_string())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn put_snapshot<T: Serialize>(
    cache: &AccountCache,
    ticket: CacheWrite,
    dto: &T,
    no_store: bool,
    etag: Option<String>,
) {
    let Ok(bytes) = serde_json::to_vec(dto) else {
        return;
    };
    let now = now_ms();
    let metadata = CacheMetadata {
        representation_version: SNAPSHOT_VERSION,
        fetched_at_ms: now,
        verified_at_ms: now,
        completeness: CacheCompleteness::Complete,
        etag,
        last_modified: None,
        cache_control: no_store.then(|| "no-store".to_string()),
    };
    let _ = cache.store().put(ticket, &bytes, metadata);
}
