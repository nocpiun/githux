//! One account-scoped inbox controller, using the existing authorization path.
mod cache;

use super::{RepoService, RepositoryReadContext, RepositorySession};
use crate::domain::error::GitHubError;
use crate::domain::notification::*;
use crate::domain::repository::RepositoryError;
use crate::github::notifications::{
    self, NotificationCursor, NotificationLimits, NotificationPage, PageFailure,
};
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::storage::cache::{CacheAuthorization, CacheWrite};
use cache::InboxCache;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    First,
    Refresh,
    More,
}

#[derive(Clone, Debug, Default)]
pub struct InboxSnapshot {
    pub items: Vec<NotificationThread>,
    pub loaded: bool,
    pub busy: Option<RequestKind>,
    pub has_more: bool,
    pub partial: Vec<NotificationPartial>,
    pub error: Option<NotificationError>,
    pub failed_request: Option<RequestKind>,
    pub cooldown_seconds: u64,
    /// Rate-limit backoff that also blocks thread actions (poll cooldown does not).
    pub action_cooldown_seconds: u64,
    pub thread_states: HashMap<String, ThreadActionState>,
    pub historical_at_ms: Option<i64>,
    authorization: Option<CacheAuthorization>,
}
impl InboxSnapshot {
    pub fn display_is_current(&self) -> bool {
        self.authorization
            .as_ref()
            .is_none_or(CacheAuthorization::is_current)
    }

    pub fn has_display_authorization(&self) -> bool {
        self.authorization.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.loaded
            && self.historical_at_ms.is_none()
            && self.items.is_empty()
            && !self.has_more
            && self.partial.is_empty()
            && self.error.is_none()
    }
}

struct State {
    snapshot: InboxSnapshot,
    generation: u64,
    next: Option<NotificationCursor>,
    validator: Option<String>,
    cacheable: bool,
    refresh_after: Option<Instant>,
    request_after: Option<Instant>,
    cancel: CancellationToken,
    cancel_source: CancellationTokenSource,
    /// Thread actions must not be aborted by a list read superseding the
    /// page-one representation; only explicit cancel/leave/teardown stops them.
    action_cancel: CancellationToken,
    action_cancel_source: CancellationTokenSource,
    /// thread id -> the updated_at version the user completed/removed.
    /// A newer server version must be allowed back into the inbox.
    tombstones: HashMap<String, String>,
    action_generations: HashMap<String, u64>,
    /// Monotonic per-controller lifecycle. It never resets on cancellation, so
    /// an old reply cannot match a newer action started for the same thread.
    action_lifecycle: u64,
    /// Threads whose ignored=true subscription was confirmed by the server.
    unsubscribed: HashSet<String>,
}
impl Default for State {
    fn default() -> Self {
        let (cancel, cancel_source) = CancellationToken::new();
        let (action_cancel, action_cancel_source) = CancellationToken::new();
        Self {
            snapshot: InboxSnapshot::default(),
            generation: 0,
            next: None,
            validator: None,
            cacheable: true,
            refresh_after: None,
            request_after: None,
            cancel,
            cancel_source,
            action_cancel,
            action_cancel_source,
            tombstones: HashMap::new(),
            action_generations: HashMap::new(),
            action_lifecycle: 0,
            unsubscribed: HashSet::new(),
        }
    }
}

pub struct ThreadActionRequest {
    pub thread_id: String,
    pub action: ThreadAction,
    /// The `updated_at` the user acted on; a later version is a new notification.
    version: String,
    generation: u64,
    pub(crate) cancel: CancellationToken,
}

pub struct InboxRequest {
    generation: u64,
    kind: RequestKind,
    cursor: Option<NotificationCursor>,
    validator: Option<String>,
    authorization: Option<CacheAuthorization>,
    action_lifecycle: u64,
    cancel: CancellationToken,
}

#[derive(Clone)]
pub struct Notifications {
    session: RepositorySession,
    client: Arc<crate::github::client::GitHubClient>,
    limits: NotificationLimits,
    state: Arc<Mutex<State>>,
    cache: Option<InboxCache>,
}
impl RepoService {
    pub fn notifications(&self) -> Result<Notifications, RepositoryError> {
        let session = self.session()?;
        let cache = self
            .cache(session.context())
            .and_then(|account| InboxCache::new(account, self.client.config()));
        Ok(Notifications {
            session,
            cache,
            client: self.client.clone(),
            limits: NotificationLimits::default(),
            state: Arc::new(Mutex::new(State::default())),
        })
    }
}
impl Notifications {
    pub fn context(&self) -> &RepositoryReadContext {
        self.session.context()
    }
    pub fn verify(&self) -> bool {
        self.session.verify()
    }

    pub fn snapshot(&self) -> InboxSnapshot {
        if !self.verify() {
            return InboxSnapshot::default();
        }
        let mut state = self.state.lock().expect("inbox state");
        Self::discard_revoked(&mut state);
        let mut snapshot = state.snapshot.clone();
        snapshot.cooldown_seconds = remaining(state.refresh_after.max(state.request_after));
        snapshot.action_cooldown_seconds = remaining(state.request_after);
        snapshot
    }

    fn discard_revoked(state: &mut State) {
        if !state.snapshot.display_is_current() {
            state.snapshot = InboxSnapshot {
                busy: state.snapshot.busy,
                ..Default::default()
            };
            state.next = None;
            state.validator = None;
            state.cacheable = false;
        }
    }

    /// First entry is idempotent. Refresh may supersede an append; both old
    /// successes and errors then lose publication rights. No render calls this.
    pub fn begin(&self, kind: RequestKind) -> Result<Option<InboxRequest>, NotificationError> {
        self.begin_at(kind, Instant::now())
    }
    fn begin_at(
        &self,
        kind: RequestKind,
        now: Instant,
    ) -> Result<Option<InboxRequest>, NotificationError> {
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        let mut state = self.state.lock().expect("inbox state");
        Self::discard_revoked(&mut state);
        if kind == RequestKind::First
            && ((state.snapshot.loaded && state.snapshot.historical_at_ms.is_none())
                || state.snapshot.busy.is_some()
                || state.snapshot.error.is_some())
        {
            return Ok(None);
        }
        if state.snapshot.busy.is_some()
            && !(kind == RequestKind::Refresh && state.snapshot.busy == Some(RequestKind::More))
        {
            return Err(NotificationError::Busy);
        }
        if kind == RequestKind::More && state.next.is_none() {
            return Ok(None);
        }
        let deadline = if kind == RequestKind::More {
            state.request_after
        } else {
            state.refresh_after.max(state.request_after)
        };
        if let Some(deadline) = deadline
            && deadline > now
        {
            return Err(NotificationError::Cooldown {
                seconds: seconds(deadline.duration_since(now)),
            });
        }
        state.cancel_source.cancel();
        (state.cancel, state.cancel_source) = CancellationToken::new();
        state.generation += 1;
        state.snapshot.busy = Some(kind);
        let validator = if kind != RequestKind::More
            && state.snapshot.loaded
            && state.snapshot.partial.is_empty()
            && state.snapshot.error.is_none()
        {
            state.validator.clone()
        } else {
            None
        };
        Ok(Some(InboxRequest {
            generation: state.generation,
            kind,
            cursor: if kind == RequestKind::More {
                state.next.clone()
            } else {
                None
            },
            validator,
            authorization: self.cache.as_ref().and_then(InboxCache::authorization),
            action_lifecycle: state.action_lifecycle,
            cancel: state.cancel.clone(),
        }))
    }

    /// Run on the shared BackgroundExecutor, then notify the view through a
    /// channel. Completion checks authorization and generation before changing
    /// data, cooldowns, validators, errors or loading state.
    #[cfg(test)]
    pub async fn execute(&self, request: InboxRequest) {
        self.execute_with_history(request, || {}).await;
    }

    pub async fn execute_with_history(&self, request: InboxRequest, history_ready: impl FnOnce()) {
        if request.kind == RequestKind::First && self.restore_history(&request).await {
            history_ready();
        }
        let ticket = if request
            .authorization
            .as_ref()
            .is_some_and(CacheAuthorization::is_current)
        {
            self.cache_work(InboxCache::begin_write).await.flatten()
        } else {
            None
        };
        let result = self
            .session
            .read(request.cancel.clone(), async |pat, cancel| {
                // Keep headers alongside an endpoint error through the shared read
                // guard; only authorization failures use the outer error path.
                let validator = self.validator_for(&request);
                Ok::<_, GitHubError>(
                    notifications::fetch_page(
                        &self.client,
                        &pat,
                        request.cursor.as_ref(),
                        validator.as_deref(),
                        &self.limits,
                        cancel,
                    )
                    .await,
                )
            })
            .await;
        let result = match result {
            Ok(scoped) => scoped.value,
            Err(error) => Err(PageFailure {
                error: error.into(),
                metadata: NotificationMetadata::default(),
            }),
        };
        if let Err(failure) = &result {
            self.record_rejection(&failure.error).await;
        }
        let forget = match &result {
            Ok(page) => page.metadata.no_store || has_sso(&page.partial),
            Err(failure) => failure.metadata.no_store || is_sso_required(&failure.error),
        };
        let update = result
            .as_ref()
            .is_ok_and(|page| !page.not_modified && page.partial.is_empty());
        self.finish(&request, result, Instant::now());
        if let Some(ticket) = ticket {
            self.persist(&request, ticket, forget, update).await;
        }
    }

    fn validator_for(&self, request: &InboxRequest) -> Option<String> {
        let validator = request.validator.as_ref()?;
        let state = self.state.lock().expect("inbox state");
        (state.generation == request.generation
            && state.action_lifecycle == request.action_lifecycle
            && state.snapshot.display_is_current()
            && state.validator.as_ref() == Some(validator))
        .then(|| validator.clone())
    }

    async fn cache_work<T: Send + 'static>(
        &self,
        work: impl FnOnce(&InboxCache) -> T + Send + 'static,
    ) -> Option<T> {
        let cache = self.cache.clone()?;
        let session = self.session.clone();
        tokio::task::spawn_blocking(move || {
            let _commit = session.service().credential_commit.lock().ok()?;
            session.verify().then(|| work(&cache))
        })
        .await
        .ok()
        .flatten()
    }

    async fn record_rejection(&self, error: &NotificationError) {
        if matches!(error, NotificationError::GitHub(GitHubError::Unauthorized)) {
            let _ = self
                .session
                .service()
                .credential_rejected(self.context())
                .await;
        }
    }

    async fn restore_history(&self, request: &InboxRequest) -> bool {
        let Some(auth) = &request.authorization else {
            return false;
        };
        let result = self
            .session
            .clone()
            .local()
            .read(request.cancel.clone(), async |_, _| {
                Ok::<_, GitHubError>(self.cache_work(InboxCache::read).await.flatten())
            })
            .await;
        let Ok(super::Scoped {
            value: Some((items, stamp)),
            ..
        }) = result
        else {
            return false;
        };
        let mut state = self.state.lock().expect("inbox state");
        if !self.verify()
            || state.generation != request.generation
            || !auth.is_current()
            || state.snapshot.loaded
        {
            return false;
        }
        state.snapshot.items = items;
        state.snapshot.loaded = true;
        state.snapshot.historical_at_ms = Some(stamp);
        state.snapshot.authorization = Some(auth.clone());
        true
    }

    async fn persist(
        &self,
        request: &InboxRequest,
        ticket: CacheWrite,
        forget: bool,
        update: bool,
    ) {
        if request
            .authorization
            .as_ref()
            .is_none_or(|auth| !auth.is_current())
        {
            return;
        }
        let items = {
            let state = self.state.lock().expect("inbox state");
            if forget {
                None
            } else if state.generation == request.generation
                && update
                && state.cacheable
                && state.snapshot.display_is_current()
                && state.snapshot.historical_at_ms.is_none()
                && state.snapshot.partial.is_empty()
                && state.action_lifecycle == request.action_lifecycle
                && !actions_pending(&state)
            {
                Some(
                    state
                        .snapshot
                        .items
                        .iter()
                        .filter(|item| !item.unread)
                        .take(cache::MAX_ROWS)
                        .cloned()
                        .collect(),
                )
            } else {
                return;
            }
        };
        self.cache_work(move |cache| cache.write(ticket, items))
            .await;
    }

    fn finish(
        &self,
        request: &InboxRequest,
        result: Result<NotificationPage, PageFailure>,
        now: Instant,
    ) {
        if !self.verify() {
            return;
        }
        let mut state = self.state.lock().expect("inbox state");
        if request.generation != state.generation {
            return;
        }
        state.snapshot.busy = None;
        Self::discard_revoked(&mut state);
        if request
            .authorization
            .as_ref()
            .is_some_and(|auth| !auth.is_current())
        {
            if let Err(PageFailure {
                error: error @ NotificationError::GitHub(GitHubError::Unauthorized),
                ..
            }) = result
            {
                state.snapshot.error = Some(error);
            }
            return;
        }
        let metadata = match &result {
            Ok(page) => &page.metadata,
            Err(failure) => &failure.metadata,
        };
        // A disconnected cache-first read received no server poll deadline.
        let disconnected = state.snapshot.historical_at_ms.is_some()
            && matches!(
                &result,
                Err(PageFailure {
                    error: NotificationError::GitHub(
                        GitHubError::Network { .. } | GitHubError::Timeout { .. }
                    ),
                    ..
                })
            );
        let poll = metadata
            .poll_interval_seconds
            .or((!disconnected).then_some(60));
        if let Some(deadline) =
            poll.and_then(|poll| now.checked_add(Duration::from_secs(poll.max(1))))
        {
            state.refresh_after = Some(match state.refresh_after {
                Some(existing) if existing > deadline => existing,
                _ => deadline,
            });
        }
        let rate = &metadata.rate_limit;
        let reset = if rate.remaining == Some(0) {
            rate.reset.map(|reset| {
                reset.saturating_sub(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                )
            })
        } else {
            None
        };
        let rate_failure = matches!(
            &result,
            Err(PageFailure {
                error: NotificationError::GitHub(GitHubError::RateLimited { .. }),
                ..
            })
        );
        let retry = rate
            .retry_after
            .map(u64::from)
            .into_iter()
            .chain(reset)
            .max()
            .or(if rate_failure { Some(60) } else { None });
        extend_request_after(&mut state, retry, now);
        if metadata.no_store {
            state.cacheable = false;
            state.validator = None;
        }
        match result {
            Err(failure) => {
                if is_sso_required(&failure.error) {
                    state.cacheable = false;
                }
                if state.snapshot.historical_at_ms.is_some()
                    && (failure.metadata.no_store || !failure.error.is_transient())
                {
                    state.snapshot = InboxSnapshot::default();
                }
                state.snapshot.error = Some(failure.error);
                state.snapshot.failed_request = Some(request.kind);
                state.validator = None;
            }
            Ok(page) if page.not_modified => {
                // The matching saved representation includes all loaded pages.
                // 304 never discards rows or continuation state.
                state.snapshot.error = None;
                state.snapshot.failed_request = None;
            }
            Ok(page) => {
                if state.snapshot.historical_at_ms.is_some()
                    && !page.partial.is_empty()
                    && !page.metadata.no_store
                    && !has_sso(&page.partial)
                {
                    state.snapshot.partial = page.partial;
                    state.snapshot.error = None;
                    state.snapshot.failed_request = None;
                    return;
                }
                if request.kind != RequestKind::More {
                    state.cacheable = !page.metadata.no_store && !actions_pending(&state);
                    state.snapshot.historical_at_ms = None;
                    state.snapshot.authorization = request.authorization.clone();
                    state.snapshot.items.clear();
                    state.snapshot.partial.clear();
                }
                let mut ids: HashSet<String> = state
                    .snapshot
                    .items
                    .iter()
                    .map(|item| item.id.clone())
                    .collect();
                let mut truncated = false;
                for item in page.items {
                    // Only the exact version the user completed is suppressed;
                    // newer activity with the same thread id is a new inbox row.
                    if state.tombstones.get(&item.id) == Some(&item.updated_at) {
                        continue;
                    }
                    state.tombstones.remove(&item.id);
                    if ids.insert(item.id.clone()) {
                        if state.snapshot.items.len() >= self.limits.max_items {
                            truncated = true;
                            break;
                        }
                        state.snapshot.items.push(item);
                    }
                }
                state.snapshot.partial.extend(page.partial);
                state.next = page.next;
                if truncated
                    || (state.snapshot.items.len() >= self.limits.max_items && state.next.is_some())
                {
                    state.snapshot.partial.push(NotificationPartial::ItemLimit);
                    state.next = None;
                }
                state.snapshot.loaded = true;
                state.snapshot.error = None;
                state.snapshot.failed_request = None;
                state.snapshot.has_more = state.next.is_some();
                if request.kind != RequestKind::More {
                    state.validator = if page.metadata.no_store
                        || state.action_lifecycle != request.action_lifecycle
                    {
                        None
                    } else {
                        page.metadata.last_modified
                    };
                }
                if !state.snapshot.partial.is_empty() {
                    state.validator = None;
                }
            }
        }
    }

    /// Start an action on an individual thread (Done or Unsubscribe).
    /// Guarded against concurrent actions on the same thread, account staleness
    /// and rate limit cooldown.
    pub fn begin_thread_action(
        &self,
        thread_id: &str,
        action: ThreadAction,
    ) -> Result<Option<ThreadActionRequest>, NotificationError> {
        self.begin_thread_action_at(thread_id, action, Instant::now())
    }

    fn begin_thread_action_at(
        &self,
        thread_id: &str,
        action: ThreadAction,
        now: Instant,
    ) -> Result<Option<ThreadActionRequest>, NotificationError> {
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        let mut state = self.state.lock().expect("inbox state");
        if !state.snapshot.display_is_current() {
            Self::discard_revoked(&mut state);
            return Ok(None);
        }
        if state.snapshot.historical_at_ms.is_some() {
            return Ok(None);
        }
        let acted_version = state
            .snapshot
            .items
            .iter()
            .find(|item| item.id == thread_id)
            .map(|item| item.updated_at.clone())
            .unwrap_or_default();
        if let Some(ThreadActionState::Busy(_)) = state.snapshot.thread_states.get(thread_id) {
            return Err(NotificationError::Busy);
        }
        if action == ThreadAction::Unsubscribe && state.unsubscribed.contains(thread_id) {
            // The ignore is already confirmed; only the remaining Done step may run.
            return Ok(None);
        }
        if let Some(deadline) = state.request_after
            && deadline > now
        {
            return Err(NotificationError::Cooldown {
                seconds: seconds(deadline.duration_since(now)),
            });
        }
        // A mutation changes the local representation even when the HTTP
        // validator for an earlier list response is still available.
        state.validator = None;
        state.cacheable = false;
        state.action_lifecycle += 1;
        let action_generation = state.action_lifecycle;
        state
            .action_generations
            .insert(thread_id.to_string(), action_generation);
        state
            .snapshot
            .thread_states
            .insert(thread_id.to_string(), ThreadActionState::Busy(action));
        Ok(Some(ThreadActionRequest {
            thread_id: thread_id.to_string(),
            action,
            version: acted_version,
            generation: action_generation,
            cancel: state.action_cancel.clone(),
        }))
    }

    /// Execute the thread action in background and apply result.
    pub async fn execute_thread_action(&self, request: ThreadActionRequest) {
        let thread_id = request.thread_id.clone();
        let action = request.action;
        let client = self.client.clone();
        // Invalidate older list writers before sending the mutation. Cancellation
        // can hide a successful response from the controller.
        self.cache_work(InboxCache::invalidate).await;
        let result = match action {
            ThreadAction::Done => {
                let tid = thread_id.clone();
                let res = self
                    .session
                    .read(request.cancel.clone(), async move |pat, cancel| {
                        Ok::<_, GitHubError>(
                            notifications::mark_done(&client, &pat, &tid, cancel).await,
                        )
                    })
                    .await;
                match res {
                    Ok(scoped) => match scoped.value {
                        Ok(meta) => Ok(notifications::UnsubscribeOutcome::Completed(meta)),
                        Err(fail) => Err(fail),
                    },
                    Err(err) => Err(PageFailure {
                        error: err.into(),
                        metadata: NotificationMetadata::default(),
                    }),
                }
            }
            ThreadAction::Unsubscribe => {
                // Two independent authorization checkpoints: an account switch
                // or PAT replacement between the PUT and the DELETE must stop
                // the second write from using the old credential.
                let first = {
                    let tid = thread_id.clone();
                    let step_client = client.clone();
                    let res = self
                        .session
                        .read(request.cancel.clone(), async move |pat, cancel| {
                            Ok::<_, GitHubError>(
                                notifications::ignore_subscription(
                                    &step_client,
                                    &pat,
                                    &tid,
                                    cancel,
                                )
                                .await,
                            )
                        })
                        .await;
                    match res {
                        Ok(scoped) => scoped.value,
                        Err(err) => Err(PageFailure {
                            error: err.into(),
                            metadata: NotificationMetadata::default(),
                        }),
                    }
                };
                match first {
                    Ok(meta) => {
                        let tid = thread_id.clone();
                        let step_client = client.clone();
                        let res = self
                            .session
                            .read(request.cancel.clone(), async move |pat, cancel| {
                                Ok::<_, GitHubError>(
                                    notifications::mark_done(&step_client, &pat, &tid, cancel)
                                        .await,
                                )
                            })
                            .await;
                        match res {
                            Ok(scoped) => match scoped.value {
                                Ok(done_meta) => Ok(notifications::UnsubscribeOutcome::Completed(
                                    done_meta.merge_restrictions(&meta),
                                )),
                                Err(failure) => {
                                    Ok(notifications::UnsubscribeOutcome::IgnoredDoneFailed {
                                        done_error: failure.error,
                                        metadata: failure.metadata.merge_restrictions(&meta),
                                    })
                                }
                            },
                            Err(err) => Ok(notifications::UnsubscribeOutcome::IgnoredDoneFailed {
                                done_error: err.into(),
                                metadata: NotificationMetadata::default().merge_restrictions(&meta),
                            }),
                        }
                    }
                    Err(failure) => Err(failure),
                }
            }
        };
        match &result {
            Ok(notifications::UnsubscribeOutcome::Completed(_)) => {
                self.cache_work(InboxCache::invalidate).await;
            }
            Ok(notifications::UnsubscribeOutcome::IgnoredDoneFailed { done_error, .. }) => {
                self.record_rejection(done_error).await;
            }
            Err(failure) => self.record_rejection(&failure.error).await,
        }
        self.finish_thread_action(&request, result, Instant::now());
    }

    fn finish_thread_action(
        &self,
        request: &ThreadActionRequest,
        result: Result<notifications::UnsubscribeOutcome, PageFailure>,
        now: Instant,
    ) {
        if !self.verify() {
            return;
        }
        let mut state = self.state.lock().expect("inbox state");
        if state.action_generations.get(&request.thread_id) != Some(&request.generation) {
            return;
        }
        let metadata = match &result {
            Ok(notifications::UnsubscribeOutcome::Completed(meta)) => meta,
            Ok(notifications::UnsubscribeOutcome::IgnoredDoneFailed { metadata, .. }) => metadata,
            Err(failure) => &failure.metadata,
        };
        let poll = metadata.poll_interval_seconds.unwrap_or(60).max(1);
        if let Some(deadline) = now.checked_add(Duration::from_secs(poll)) {
            state.refresh_after = Some(match state.refresh_after {
                Some(existing) if existing > deadline => existing,
                _ => deadline,
            });
        }
        let rate = &metadata.rate_limit;
        let reset = if rate.remaining == Some(0) {
            rate.reset.map(|reset| {
                reset.saturating_sub(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                )
            })
        } else {
            None
        };
        // A partially completed Unsubscribe is Ok(IgnoredDoneFailed), so a
        // nested Done rate-limit error needs the same fallback as outer errors.
        let rate_failure = match &result {
            Ok(notifications::UnsubscribeOutcome::IgnoredDoneFailed { done_error, .. }) => {
                matches!(
                    done_error,
                    NotificationError::GitHub(GitHubError::RateLimited { .. })
                )
            }
            Err(PageFailure { error, .. }) => matches!(
                error,
                NotificationError::GitHub(GitHubError::RateLimited { .. })
            ),
            _ => false,
        };
        let retry = rate
            .retry_after
            .map(u64::from)
            .into_iter()
            .chain(reset)
            .max()
            .or(if rate_failure { Some(60) } else { None });
        extend_request_after(&mut state, retry, now);

        match result {
            Ok(notifications::UnsubscribeOutcome::Completed(_)) => {
                // Done succeeded: remove from items, add to tombstones, remove thread state
                if !request.version.is_empty() {
                    state
                        .tombstones
                        .insert(request.thread_id.clone(), request.version.clone());
                }
                state.unsubscribed.remove(&request.thread_id);
                // Remove only the version the user completed. A concurrent
                // refresh may already hold newer unread activity (version B)
                // for the same thread id; that row must stay visible.
                state.snapshot.items.retain(|item| {
                    item.id != request.thread_id
                        || (!request.version.is_empty() && item.updated_at != request.version)
                });
                state.snapshot.thread_states.remove(&request.thread_id);
            }
            Ok(notifications::UnsubscribeOutcome::IgnoredDoneFailed { done_error, .. }) => {
                // Subscription was ignored, but Done failed: keep that fact.
                state.unsubscribed.insert(request.thread_id.clone());
                state.snapshot.thread_states.insert(
                    request.thread_id.clone(),
                    ThreadActionState::UnsubscribedPendingDone { error: done_error },
                );
            }
            Err(failure) => {
                // A confirmed ignore must survive a failed Done retry; the row
                // keeps saying "unsubscribed, finish Done" instead of offering
                // the already-completed Unsubscribe again.
                let state_value = if state.unsubscribed.contains(&request.thread_id) {
                    ThreadActionState::UnsubscribedPendingDone {
                        error: failure.error,
                    }
                } else {
                    ThreadActionState::Failed {
                        action: request.action,
                        error: failure.error,
                    }
                };
                state
                    .snapshot
                    .thread_states
                    .insert(request.thread_id.clone(), state_value);
            }
        }
    }

    pub fn cancel(&self) {
        let mut state = self.state.lock().expect("inbox state");
        state.cancel_source.cancel();
        state.action_cancel_source.cancel();
        (state.cancel, state.cancel_source) = CancellationToken::new();
        (state.action_cancel, state.action_cancel_source) = CancellationToken::new();
        state.generation += 1;
        // Keep monotonic per-thread generations, but also invalidate every
        // currently in-flight action: after cancellation, no old reply may
        // publish a failure state or remove a row into the retained view.
        state.action_lifecycle += 1;
        let lifecycle = state.action_lifecycle;
        for thread_id in state
            .snapshot
            .thread_states
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            state.action_generations.insert(thread_id, lifecycle);
        }
        state.snapshot.busy = None;
        // Busy/failed progress is transient, but a confirmed unsubscription is
        // a server fact: keep its pending-Done presentation so a retained view
        // returning later still retries only Done instead of re-offering
        // Unsubscribe (which the controller would refuse anyway).
        state
            .snapshot
            .thread_states
            .retain(|_, value| matches!(value, ThreadActionState::UnsubscribedPendingDone { .. }));
    }
}
fn has_sso(partial: &[NotificationPartial]) -> bool {
    partial
        .iter()
        .any(|reason| matches!(reason, NotificationPartial::Sso { .. }))
}

fn is_sso_required(error: &NotificationError) -> bool {
    matches!(
        error,
        NotificationError::GitHub(GitHubError::SsoRequired { .. })
    )
}

fn actions_pending(state: &State) -> bool {
    state
        .snapshot
        .thread_states
        .values()
        .any(|action| matches!(action, ThreadActionState::Busy(_)))
}

/// Concurrent completions may each report a rate-limit deadline. A later,
/// shorter hint must never shorten an already outstanding longer restriction.
fn extend_request_after(state: &mut State, retry: Option<u64>, now: Instant) {
    let Some(retry) = retry else {
        return;
    };
    let Some(candidate) = now.checked_add(Duration::from_secs(retry.max(1))) else {
        return;
    };
    state.request_after = Some(match state.request_after {
        Some(existing) if existing > candidate => existing,
        _ => candidate,
    });
}

fn seconds(duration: Duration) -> u64 {
    duration.as_secs() + u64::from(duration.subsec_nanos() > 0)
}
fn remaining(deadline: Option<Instant>) -> u64 {
    deadline.map_or(0, |deadline| {
        seconds(deadline.saturating_duration_since(Instant::now()))
    })
}

#[cfg(test)]
mod tests;

impl Drop for Notifications {
    fn drop(&mut self) {
        // The last owner must not leave an independent action token alive: UI
        // release and account/session teardown then stop both read and write
        // work without cancelling other clones that are still using it.
        if Arc::strong_count(&self.state) == 1 {
            let state = self.state.lock().expect("inbox state");
            state.cancel_source.cancel();
            state.action_cancel_source.cancel();
        }
    }
}
