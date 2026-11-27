//! Persistent page snapshots for the repository `Pull requests` list.
//!
//! The repository Issues list established the shape this adapter follows: a first page is
//! stored per account, repository id, filter and request representation. A pull request list
//! is positional, so this adapter also keeps the continuation pages the reader actually paged
//! through, each bound to the generation of the first page it was appended to. Rows loaded
//! under one first page can therefore be restored offline as a known prefix, while pages from
//! another first page (another filter, another refresh) can never be joined to them.
//!
//! Everything about storage, `no-store`, revocation events, atomic publication and replay
//! comes from [`super::CachedPageReadSession`]; this module only names the pull request
//! request identity, its generation and the bounded restore loop.

use super::{
    CachedPageReadSession, CollectionGeneration, PageCacheError, PageReadMode, PageValue,
    metadata_feed_collection, new_collection_generation,
};
use crate::data::Scoped;
use crate::domain::cache::CacheCompleteness;
use crate::domain::error::GitHubError;
use crate::domain::repository::{Completeness, RepositoryError, RepositoryIdentity};
use crate::domain::repository_pulls::{PullFilter, PullListPage, PullPageCursor};
use crate::github::repository_pulls::PullListLimits;
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{AccessEvent, CacheError};

#[cfg(test)]
mod tests;

/// How many stored pages one offline restore may replay. The list is a reading aid, not a
/// mirror of the repository: a handful of pages covers the rows a reader actually visited and
/// keeps one restore bounded in rows, bytes and time.
const MAX_RESTORED_PAGES: usize = 4;
/// A second bound on restored rows, so a small `per_page` cannot multiply the page budget.
const MAX_RESTORED_ROWS: usize = 200;

impl CachedPageReadSession {
    /// Read one page for the repository `Pull requests` tab.
    ///
    /// The first page of a filter starts a collection generation and is stored with it. A
    /// continuation is stored under that same generation, so a later first-page replacement
    /// (a refresh or the other filter) makes the old continuation unreachable instead of
    /// silently mixing two list versions.
    pub(crate) async fn repository_pulls_online(
        &self,
        repository: &RepositoryIdentity,
        filter: PullFilter,
        limits: &PullListLimits,
        cursor: Option<&PullPageCursor>,
        collection: Option<CollectionGeneration>,
        cancel: CancellationToken,
    ) -> Result<PullListPage, PageCacheError<RepositoryError>> {
        let first = cursor.is_none();
        let generation = if first {
            Some(collection.unwrap_or_else(new_collection_generation))
        } else {
            collection
        };
        let cached = if first {
            self.repository_pulls(
                PageReadMode::Online,
                repository,
                filter,
                limits,
                generation,
                cancel.clone(),
            )
            .await
        } else if let (Some(cursor), Some(generation)) = (cursor, generation) {
            self.repository_pulls_page(
                PageReadMode::Online,
                repository,
                filter,
                limits,
                cursor,
                generation,
                cancel.clone(),
            )
            .await
        } else {
            // Without a first-page generation there is nothing safe to append to; the live
            // list keeps working but this page is not persisted.
            self.guard(&cancel)?;
            return self
                .session
                .repository_pulls(repository, filter, cursor, limits, cancel)
                .await
                .map(|value| value.value)
                .map_err(PageCacheError::Read);
        };
        match cached {
            Ok(Scoped {
                value: PageValue::Value { value, .. },
                ..
            }) => return Ok(value),
            Ok(Scoped {
                value: PageValue::Miss(_),
                ..
            }) => {}
            Err(PageCacheError::Cache(error))
                if !matches!(error, CacheError::AccessDenied | CacheError::Stale) => {}
            Err(error) => return Err(error),
        }
        self.guard(&cancel)?;
        let result = self
            .session
            .repository_pulls(repository, filter, cursor, limits, cancel)
            .await;
        if matches!(
            &result,
            Err(RepositoryError::GitHub(GitHubError::Unauthorized))
        ) {
            self.blocking(|store, scope| {
                store.access_event(&scope, AccessEvent::CredentialRejected)
            })
            .await?;
        }
        result
            .map(|value| value.value)
            .map_err(PageCacheError::Read)
    }

    /// Read the stored rows of one list, from the first page through every continuation the
    /// reader actually paged through under that first page's generation.
    ///
    /// Restoring stops at the first missing page, at a partial page (its continuation link is
    /// not trusted) and at the bounded page/row budget. It never issues a request and never
    /// asks for a page that was not read before.
    ///
    /// The stored generation is returned with the rows: only pages stored under that exact
    /// first page may be replayed or appended to, and a caller must not relabel them with a new
    /// refresh generation.
    pub(crate) async fn repository_pulls_restore(
        &self,
        repository: &RepositoryIdentity,
        filter: PullFilter,
        limits: &PullListLimits,
        cancel: CancellationToken,
    ) -> Option<(Vec<PullListPage>, Option<CollectionGeneration>)> {
        let (first, collection) = match self
            .repository_pulls(
                PageReadMode::Offline,
                repository,
                filter,
                limits,
                None,
                cancel.clone(),
            )
            .await
        {
            Ok(Scoped {
                value: PageValue::Value {
                    value, metadata, ..
                },
                ..
            }) => (value, metadata_feed_collection(&metadata)),
            _ => return None,
        };
        let mut pages = vec![first];
        let mut rows = pages[0].pulls.len();
        let generation = collection;
        while pages.len() < MAX_RESTORED_PAGES && rows < MAX_RESTORED_ROWS {
            let Some(page) = pages.last() else {
                break;
            };
            if page.completeness != Completeness::Complete {
                break;
            }
            let (Some(cursor), Some(generation)) = (page.next.clone(), generation) else {
                break;
            };
            match self
                .repository_pulls_page(
                    PageReadMode::Offline,
                    repository,
                    filter,
                    limits,
                    &cursor,
                    generation,
                    cancel.clone(),
                )
                .await
            {
                Ok(Scoped {
                    value: PageValue::Value { value, .. },
                    ..
                }) => {
                    // Never overshoot the row budget: keep the contiguous prefix and leave the
                    // page that would cross the budget unread, so the restored list stays a
                    // bounded, honestly partial prefix even for a larger legal page size.
                    if rows + value.pulls.len() > MAX_RESTORED_ROWS {
                        break;
                    }
                    rows += value.pulls.len();
                    pages.push(value);
                }
                _ => break,
            }
        }
        Some((pages, collection))
    }

    /// Read and store the first stored page of one filter.
    ///
    /// The cache identity includes the numeric repository id, owner/name, filter, page size and
    /// response bound, so a renamed repository, the other filter, another page size or another
    /// representation cannot replay this page. The first-page snapshot also records the
    /// collection generation that its continuations are stored under.
    pub(crate) async fn repository_pulls(
        &self,
        mode: PageReadMode,
        repository: &RepositoryIdentity,
        filter: PullFilter,
        limits: &PullListLimits,
        collection: Option<CollectionGeneration>,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<PullListPage>>, PageCacheError<RepositoryError>> {
        let parameters = serde_json::json!([
            repository.locator().owner(),
            repository.locator().name(),
            filter.api_state(),
            limits.per_page,
            limits.response_bytes
        ]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            Some(repository.id()),
            "repository-pulls",
            parameters,
            cancel,
            |session| async move {
                session
                    .repository_pulls(repository, filter, None, limits, request_cancel)
                    .await
            },
            completeness,
            collection,
        )
        .await
    }

    /// Read and store one continuation of the first page identified by `collection`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn repository_pulls_page(
        &self,
        mode: PageReadMode,
        repository: &RepositoryIdentity,
        filter: PullFilter,
        limits: &PullListLimits,
        cursor: &PullPageCursor,
        collection: CollectionGeneration,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<PullListPage>>, PageCacheError<RepositoryError>> {
        let parameters = serde_json::json!([
            repository.locator().owner(),
            repository.locator().name(),
            filter.api_state(),
            limits.per_page,
            limits.response_bytes,
            cursor.page(),
            collection.suffix()
        ]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            Some(repository.id()),
            "repository-pulls-page",
            parameters,
            cancel,
            |session| async move {
                session
                    .repository_pulls(repository, filter, Some(cursor), limits, request_cancel)
                    .await
            },
            completeness,
            Some(collection),
        )
        .await
    }
}

fn completeness(page: &PullListPage) -> CacheCompleteness {
    match page.completeness {
        Completeness::Complete => CacheCompleteness::Complete,
        Completeness::Partial { .. } => CacheCompleteness::Partial {
            reason: "Pull request page incomplete".into(),
        },
    }
}
