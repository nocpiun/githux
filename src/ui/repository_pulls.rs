//! The repository Pull requests tab's list model: filter, pages, retry and retained state.
//!
//! This is the PR-specific half of the repository "Pull requests" tab. The page
//! container, tab strip and shared row presentation come from the repository page; this
//! module owns the list's own promises so they can be verified without a window:
//!
//! - `Open`/`Closed` are the only filters. GitHub's `closed` set contains both merged and
//!   closed-without-merge pull requests, so the row state is derived from `state`,
//!   `draft` and `merged_at` instead of from the filter.
//! - A continuation failure keeps every loaded row and the same cursor, so a retry
//!   continues the list instead of restarting it or silently ending it.
//! - A successful empty page is a real empty result. A failed page, a partial (SSO or
//!   rejected pagination) page and a page still loading are not.
//! - Every request carries its account context, repository identity, filter, page and
//!   generation. A reply from another account, repository, filter or generation is
//!   rejected, success and failure alike.
//! - Filter, loaded rows and the remembered position survive leaving the tab for a pull
//!   request and coming back. Only an explicit filter change or an account change clears
//!   them.

#[cfg(test)]
#[path = "repository_pulls_tests.rs"]
mod tests;

use crate::data::page_cache::{
    CachedPageReadSession, CollectionGeneration, PageCacheError, new_collection_generation,
};
use crate::data::repository_pulls::PullListSession;
use crate::domain::error::GitHubError;
use crate::domain::repository::{
    Completeness, IncompleteReason, RepositoryError, RepositoryIdentity,
};
use crate::domain::repository_pulls::{
    PullFilter, PullListPage, PullListQuery, PullPageCursor, PullState, PullSummary,
};
use crate::github::repository_pulls::PullListLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::storage::cache::CacheError;
use crate::ui::relative_time;
use crate::ui::repository::PageError;
use std::collections::HashMap;
use std::time::SystemTime;

/// Whether a request replaces the current list or continues it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullRequestMode {
    Replace,
    Append,
}

/// Why a pull request list read failed.
///
/// The list deliberately keeps the cache layer's distinction: an unusable local snapshot, a
/// refused authorization and a failed GitHub read are different facts, and only the last one
/// (or a transient cache failure) may leave already displayed rows on screen.
#[derive(Debug)]
pub enum PullListError {
    /// The list read itself failed (network, server, rejected credential, partial).
    Read(RepositoryError),
    /// The persistent page could not be read or written.
    Cache(CacheError),
    /// The read is refused because its captured context is no longer current.
    Context(GitHubError),
}

impl From<RepositoryError> for PullListError {
    fn from(error: RepositoryError) -> Self {
        Self::Read(error)
    }
}

impl PullListError {
    fn from_page_cache(error: PageCacheError<RepositoryError>) -> Self {
        match error {
            PageCacheError::Read(error) => Self::Read(error),
            PageCacheError::Cache(error) => Self::Cache(error),
            PageCacheError::Context(error) => Self::Context(error),
        }
    }

    /// Whether this failure proves the current authorization may no longer read the list, as
    /// opposed to a transient cache/transport/server failure. Mirrors the repository Issues
    /// tab's access-failure set, including a revoked cache partition.
    pub fn access_failure(&self) -> bool {
        match self {
            Self::Read(error) => access_failure(error),
            Self::Cache(CacheError::AccessDenied | CacheError::Stale) => true,
            Self::Cache(_) => false,
            Self::Context(error) => matches!(
                error,
                GitHubError::Unauthorized
                    | GitHubError::Forbidden
                    | GitHubError::NotFound
                    | GitHubError::SsoRequired { .. }
                    | GitHubError::StaleContext
            ),
        }
    }

    /// The header failure this list may show.
    pub fn page_error(&self) -> PageError {
        match self {
            Self::Read(error) => PageError::from_repository(error),
            Self::Context(error) => {
                PageError::from_repository(&RepositoryError::GitHub(error.clone()))
            }
            Self::Cache(CacheError::AccessDenied | CacheError::Stale) => PageError::new(
                "Pull request access could not be verified.",
                Some(
                    "The cached list is unavailable under the current authorization. Retry while online to verify it; no empty result is inferred."
                        .to_string(),
                ),
                true,
            ),
            Self::Cache(_) => PageError::new(
                "The pull request cache could not be read.",
                Some("Retry to read the list from GitHub; no empty result is inferred.".to_string()),
                true,
            ),
        }
    }
}

/// One issued request. It owns the cancellation source, so dropping the request is not
/// enough to stop it: the host keeps it until `finish` or `cancel`.
#[derive(Clone)]
pub struct PullListRequest {
    mode: PullRequestMode,
    filter: PullFilter,
    cursor: Option<PullPageCursor>,
    page: u32,
    limits: PullListLimits,
    generation: u64,
    /// The first-page generation this page belongs to, when the page is persisted. A
    /// continuation under no generation is read live but never stored.
    collection: Option<CollectionGeneration>,
    session: PullListSession,
    /// The persistent page reader for this read, when the page has a cache partition.
    cached: Option<CachedPageReadSession>,
    cancel: CancellationToken,
    source: CancellationTokenSource,
}

impl std::fmt::Debug for PullListRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PullListRequest")
            .field("mode", &self.mode)
            .field("filter", &self.filter)
            .field("page", &self.page)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl PullListRequest {
    #[cfg(test)]
    pub fn mode(&self) -> PullRequestMode {
        self.mode
    }

    pub fn filter(&self) -> PullFilter {
        self.filter
    }

    #[cfg(test)]
    pub fn cursor(&self) -> Option<&PullPageCursor> {
        self.cursor.as_ref()
    }

    /// The page number this request asks for. A reply for another page is rejected.
    pub fn page(&self) -> u32 {
        self.page
    }

    /// The model generation that issued this request. A reply is only applied to the
    /// generation that asked for it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Stop the read at its next await point. The host calls this when the tab, filter,
    /// repository or account changes, and the model calls it when it is dropped.
    pub fn abort(&self) {
        self.source.cancel();
    }

    #[cfg(test)]
    pub fn is_aborted(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The first-page generation this request belongs to, if it is persisted.
    #[cfg(test)]
    pub fn collection(&self) -> Option<CollectionGeneration> {
        self.collection
    }

    pub async fn execute(&self) -> Result<PullListPage, PullListError> {
        if let Some(cached) = &self.cached {
            return cached
                .repository_pulls_online(
                    self.session.identity(),
                    self.filter,
                    &self.limits,
                    self.cursor.as_ref(),
                    self.collection,
                    self.cancel.clone(),
                )
                .await
                .map_err(PullListError::from_page_cache);
        }
        self.session
            .page(
                self.filter,
                self.cursor.as_ref(),
                &self.limits,
                self.cancel.clone(),
            )
            .await
            .map(|scoped| scoped.value)
            .map_err(PullListError::Read)
    }

    /// The stored pages of this list, for a cache-first paint that must not wait for the
    /// network.
    ///
    /// Only a replacement restores. The result is every page the reader actually loaded under
    /// the stored first page's generation, so a pull request first seen on page two is still
    /// there after a restart. A live continuation is never appended to that prefix.
    pub async fn restore(&self) -> Option<(Vec<PullListPage>, Option<CollectionGeneration>)> {
        let cached = self.cached.as_ref()?;
        if self.mode != PullRequestMode::Replace || self.cursor.is_some() {
            return None;
        }
        cached
            .repository_pulls_restore(
                self.session.identity(),
                self.filter,
                &self.limits,
                self.cancel.clone(),
            )
            .await
    }
}

/// The position the list was left at, keyed by GitHub's stable pull request id.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PullScrollAnchor {
    pub pull_id: u64,
    /// Pixels from the top of the list's content.
    pub offset: f32,
}

/// Whether a finished request published its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullFinish {
    Applied,
    /// Superseded, stale for the current account, or not this model's reply.
    Rejected,
}

/// Display fields one row hands to the shared work-item row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRow {
    pub number: u64,
    pub title: String,
    pub state: PullState,
    pub state_label: &'static str,
    pub author: Option<String>,
    pub head: String,
    pub base: String,
    /// Relative update time at render time; an unavailable timestamp stays honest.
    pub updated: String,
    /// Exact timestamp for the tooltip and accessible name.
    pub updated_tooltip: String,
}

impl PullSummary {
    /// Map the loaded fields to the shared row's display strings. An absent author stays
    /// absent and an empty title falls back, so the row never renders an empty slot.
    pub fn row(&self, now: SystemTime) -> PullRow {
        let title = if self.title.trim().is_empty() {
            "Untitled pull request".to_string()
        } else {
            self.title.clone()
        };
        PullRow {
            number: self.number,
            title,
            state: self.state,
            state_label: self.state.label(),
            author: self.author.clone(),
            head: self.head.as_str().to_string(),
            base: self.base.as_str().to_string(),
            updated: relative_time::relative(Some(&self.updated_at), now),
            updated_tooltip: self.updated_at.clone(),
        }
    }
}

/// Whether the failure proves this authorization may no longer read the list, as opposed to a
/// transient transport/server failure. Mirrors the repository Issues tab's access-failure set.
fn access_failure(error: &RepositoryError) -> bool {
    matches!(
        error,
        RepositoryError::GitHub(
            GitHubError::Unauthorized
                | GitHubError::Forbidden
                | GitHubError::NotFound
                | GitHubError::SsoRequired { .. }
                | GitHubError::StaleContext
        ) | RepositoryError::SsoPartial { .. }
    )
}

/// Fold a page's completeness into the reasons an earlier page already reported, without
/// duplicating identical causes.
fn merge_reasons(reasons: &mut Vec<IncompleteReason>, page: Completeness) {
    let Completeness::Partial { reasons: page } = page else {
        return;
    };
    for reason in page {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
}

impl Drop for PullListModel {
    /// A closed page or replaced model must not leave its read occupying a shared request
    /// slot until the HTTP timeout. Dropping the source alone does not cancel, so abort here.
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The filter, loaded rows, cursors, errors and retained view state of one repository
/// Pull requests tab.
pub struct PullListModel {
    session: PullListSession,
    /// The persistent first-page reader shared by this model's requests, when the page has a
    /// cache partition. It carries the same account/repository context as `session`.
    cached: Option<CachedPageReadSession>,
    filter: PullFilter,
    rows: Vec<PullSummary>,
    next: Option<PullPageCursor>,
    completeness: Option<Completeness>,
    error: Option<PullListError>,
    failed_mode: Option<PullRequestMode>,
    busy: Option<PullRequestMode>,
    loaded: bool,
    /// The rows currently drawn came from a stored first page and no network result has
    /// replaced them yet. The prefix is bounded: its continuation is not stored.
    historical: bool,
    /// The stored first page has a real continuation that is not available offline. It is
    /// visible as a partial list, never as a complete one.
    cached_more: bool,
    /// The first-page generation of the rows on screen. A continuation must belong to it, and
    /// only a fresh first page starts a new one.
    collection: Option<CollectionGeneration>,
    /// The generation whose replacement may still publish its cache-first paint.
    restore_generation: Option<u64>,
    generation: u64,
    flight: Option<PullListRequest>,
    scroll: Option<PullScrollAnchor>,
    limits: PullListLimits,
}

impl PullListModel {
    pub fn new(session: PullListSession, filter: PullFilter) -> Self {
        Self {
            session,
            cached: None,
            filter,
            rows: Vec::new(),
            next: None,
            completeness: None,
            error: None,
            failed_mode: None,
            busy: None,
            loaded: false,
            historical: false,
            cached_more: false,
            collection: None,
            restore_generation: None,
            generation: 0,
            flight: None,
            scroll: None,
            limits: PullListLimits::default(),
        }
    }

    /// Attach the persistent page reader. It must belong to the same account context and
    /// repository as the plain session; otherwise it is ignored.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_cache(mut self, cached: Option<CachedPageReadSession>) -> Self {
        self.set_cache(cached);
        self
    }

    pub fn set_cache(&mut self, cached: Option<CachedPageReadSession>) {
        self.cached =
            cached.filter(|cached| cached.context() == self.session.context() && cached.verify());
    }

    pub fn with_limits(mut self, limits: PullListLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn filter(&self) -> PullFilter {
        self.filter
    }

    pub fn query(&self) -> PullListQuery {
        self.session.query(self.filter)
    }

    /// The repository identity this model reads. A page host binds one model to one
    /// repository, so two repositories can never share rows or cursors.
    pub fn identity(&self) -> &RepositoryIdentity {
        self.session.identity()
    }

    pub fn rows(&self) -> &[PullSummary] {
        &self.rows
    }

    pub fn next(&self) -> Option<&PullPageCursor> {
        self.next.as_ref()
    }

    pub fn error(&self) -> Option<&PullListError> {
        self.error.as_ref()
    }

    pub fn failed_mode(&self) -> Option<PullRequestMode> {
        self.failed_mode
    }

    pub fn busy(&self) -> Option<PullRequestMode> {
        self.busy
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// Whether the rows on screen are a stored snapshot rather than a verified network
    /// observation. Its continuation is not available and its emptiness is only historical.
    pub fn is_historical(&self) -> bool {
        self.historical
    }

    /// The first-page generation the displayed rows belong to, if they are persisted.
    #[cfg(test)]
    pub fn collection(&self) -> Option<CollectionGeneration> {
        self.collection
    }

    /// A stored first page that confirmed no rows. This is not evidence that the repository
    /// has no pull requests now.
    pub fn is_cached_empty(&self) -> bool {
        self.historical && self.loaded && self.rows.is_empty()
    }

    #[cfg(test)]
    pub fn scroll(&self) -> Option<PullScrollAnchor> {
        self.scroll
    }

    /// A confirmed empty list: the page succeeded completely, has no rows and no pending
    /// failure. An SSO-partial empty page, a cached historical page and a page still loading
    /// are not evidence that the repository has none.
    pub fn is_empty_result(&self) -> bool {
        self.loaded
            && !self.historical
            && self.rows.is_empty()
            && self.error.is_none()
            && self.busy.is_none()
            && self.completeness.as_ref() == Some(&Completeness::Complete)
    }

    /// Some rows or the completeness marker may be missing because the server reported
    /// partial results, a rejected continuation link, or because only a bounded cached first
    /// page is available.
    pub fn is_partial(&self) -> bool {
        self.cached_more || matches!(self.completeness, Some(Completeness::Partial { .. }))
    }

    pub fn context(&self) -> &crate::data::RepositoryReadContext {
        self.session.context()
    }

    pub fn verify(&self) -> bool {
        self.session.verify()
    }

    /// Keep the position the list will restore when the tab is shown again.
    pub fn remember_scroll(&mut self, anchor: PullScrollAnchor) {
        self.scroll = Some(anchor);
    }

    /// Switch filter. The same filter is not a reload; a different one starts a fresh
    /// replacement and drops the position because the rows no longer match.
    pub fn select_filter(&mut self, filter: PullFilter) -> Option<PullListRequest> {
        if filter == self.filter {
            return None;
        }
        self.cancel();
        self.filter = filter;
        self.rows.clear();
        self.next = None;
        self.completeness = None;
        self.error = None;
        self.failed_mode = None;
        self.loaded = false;
        self.historical = false;
        self.cached_more = false;
        self.collection = None;
        self.scroll = None;
        self.begin(PullRequestMode::Replace)
    }

    /// Issue a replacement or continuation.
    ///
    /// A replacement may supersede an in-flight append; a second request of the same mode is
    /// ignored. An append requires a next cursor and no request in flight, and it is only
    /// available for a list that was verified online: appending a live page to a bounded stored
    /// prefix would join two different first-page generations.
    pub fn begin(&mut self, mode: PullRequestMode) -> Option<PullListRequest> {
        if !self.verify() || self.busy == Some(mode) {
            return None;
        }
        if mode == PullRequestMode::Append
            && (self.busy.is_some() || self.next.is_none() || self.historical)
        {
            return None;
        }
        self.cancel();
        let (cancel, source) = CancellationToken::new();
        let cursor = if mode == PullRequestMode::Append {
            self.next.clone()
        } else {
            None
        };
        let page = cursor.as_ref().map_or(1, PullPageCursor::page);
        // A replacement starts a fresh first-page generation; an append keeps the generation
        // the displayed rows were read under, so two list versions cannot be joined.
        let collection = if self.cached.is_some() {
            match mode {
                PullRequestMode::Replace => Some(new_collection_generation()),
                PullRequestMode::Append => self.collection,
            }
        } else {
            None
        };
        let request = PullListRequest {
            mode,
            filter: self.filter,
            cursor,
            page,
            limits: self.limits.clone(),
            generation: self.generation,
            collection,
            session: self.session.clone(),
            cached: self.cached.clone(),
            cancel,
            source,
        };
        // Only a replacement of an empty list can still paint a stored first page. A refresh
        // of rows already on screen keeps them until the network answers.
        self.restore_generation =
            (mode == PullRequestMode::Replace && self.cached.is_some() && !self.loaded)
                .then_some(self.generation);
        self.busy = Some(mode);
        self.error = None;
        self.failed_mode = None;
        self.flight = Some(request.clone());
        Some(request)
    }

    /// Retry the mode that failed: a failed continuation retries its own cursor, a failed
    /// replacement retries the replacement.
    pub fn retry(&mut self) -> Option<PullListRequest> {
        let mode = self.failed_mode?;
        self.begin(mode)
    }

    pub fn cancel(&mut self) {
        if let Some(request) = self.flight.take() {
            request.abort();
        }
        self.restore_generation = None;
        self.generation = self
            .generation
            .checked_add(1)
            .expect("request generation overflow");
        self.busy = None;
    }

    /// Publish stored pages while their replacement is still in flight.
    ///
    /// `pages` are the first page and every continuation that was actually loaded under its
    /// generation, so a pull request first seen on page two survives a restart. The prefix
    /// keeps that generation's next cursor, and the rows stop at the first page that was not
    /// stored. A network result published before this arrives owns the list, and a late replay
    /// after a rejected credential is refused, so a stored snapshot can neither overwrite
    /// fresher rows nor resurrect rows the server just refused.
    pub fn apply_cached(
        &mut self,
        request: &PullListRequest,
        pages: Vec<PullListPage>,
        collection: Option<CollectionGeneration>,
    ) -> bool {
        let Some(first) = pages.first() else {
            return false;
        };
        if request.mode != PullRequestMode::Replace
            || request.cursor.is_some()
            || request.filter != self.filter
            || first.page != 1
            || first.query != self.query()
            || self.restore_generation != Some(request.generation)
            || self.loaded
            || !self.verify()
            || !request.session.verify()
            || request.session.context() != self.session.context()
        {
            return false;
        }
        for (expected, page) in (1..).zip(pages.iter()) {
            if page.page != expected || page.query != self.query() {
                return false;
            }
        }
        // A stored prefix is only as complete as the pages behind it. A partial page stops
        // the prefix there, so its own continuation marker is not offered as readable.
        let mut rows = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut reasons = Vec::new();
        let mut partial = false;
        let mut next = None;
        for page in pages {
            let page_partial = match page.completeness {
                Completeness::Partial { reasons: page } => page,
                Completeness::Complete => Vec::new(),
            };
            for reason in page_partial {
                partial = true;
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
            for pull in page.pulls {
                if seen.insert(pull.id) {
                    rows.push(pull);
                }
            }
            next = page.next;
            if partial {
                next = None;
            }
        }
        self.next = next;
        self.cached_more = self.next.is_some();
        self.rows = rows;
        self.completeness = Some(if partial {
            Completeness::Partial { reasons }
        } else {
            Completeness::Complete
        });
        self.loaded = true;
        self.historical = true;
        self.restore_generation = None;
        self.collection = collection;
        self.scroll = None;
        true
    }

    /// Publish one finished request.
    ///
    /// Success and failure are both checked against the issuing generation and the live
    /// account context before touching rows, exactly like a page host does for its own
    /// services. A rejected append leaves the previous pages and cursor intact.
    pub fn finish(
        &mut self,
        request: &PullListRequest,
        result: Result<PullListPage, PullListError>,
    ) -> PullFinish {
        let current = self
            .flight
            .as_ref()
            .is_some_and(|flight| flight.generation == request.generation);
        if !current {
            return PullFinish::Rejected;
        }
        if !request.session.verify() || request.session.context() != self.session.context() {
            self.flight = None;
            self.busy = None;
            return PullFinish::Rejected;
        }
        self.flight = None;
        self.busy = None;
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                // A rejected credential or lost visibility is not a transient page failure:
                // the rows already on screen were read under an authorization the server just
                // refused, so they must not stay visible or activatable. A network/5xx failure
                // keeps the loaded pages (including a cached prefix) and retries only the
                // failed page.
                let denied = error.access_failure();
                if denied {
                    self.rows.clear();
                    self.next = None;
                    self.completeness = None;
                    self.loaded = false;
                    self.historical = false;
                    self.cached_more = false;
                    self.collection = None;
                    self.restore_generation = None;
                    self.scroll = None;
                }
                self.error = Some(error);
                self.failed_mode = Some(if denied {
                    PullRequestMode::Replace
                } else {
                    request.mode
                });
                return PullFinish::Applied;
            }
        };
        if page.query != self.query() || page.page != request.page {
            return PullFinish::Rejected;
        }
        match request.mode {
            PullRequestMode::Replace => self.rows = page.pulls,
            PullRequestMode::Append => {
                // New pull requests shift the server-side pages while the user reads, so a row
                // may arrive again. Keep one row per id, in the position where it first
                // appeared, and let the newer page refresh its state (for example open →
                // merged while paging).
                let mut index: HashMap<u64, usize> = self
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(index, pull)| (pull.id, index))
                    .collect();
                for pull in page.pulls {
                    if let Some(&index) = index.get(&pull.id) {
                        self.rows[index] = pull;
                    } else {
                        index.insert(pull.id, self.rows.len());
                        self.rows.push(pull);
                    }
                }
            }
        }
        self.next = page.next;
        self.cached_more = false;
        self.historical = false;
        self.restore_generation = None;
        // A verified first page owns the collection its continuations are stored under. An
        // append must persist under exactly this generation, not whatever the model held
        // before; a continuation from the previous first page is never joined to it.
        self.collection = request.collection;
        // A continuation adds rows to the list already on screen. If an earlier page was
        // partial, those rows are not re-fetched, so its warning must survive a later complete
        // page. Replace, by contrast, has just re-read the whole list and resets it.
        let previous = self.completeness.take();
        self.completeness = Some(match (request.mode, previous, page.completeness) {
            (PullRequestMode::Append, Some(Completeness::Partial { mut reasons }), later) => {
                merge_reasons(&mut reasons, later);
                Completeness::Partial { reasons }
            }
            (_, _, current) => current,
        });
        self.error = None;
        self.failed_mode = None;
        self.loaded = true;
        PullFinish::Applied
    }
}
