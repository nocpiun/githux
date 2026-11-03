//! One account-scoped inbox controller, using the existing authorization path.
use super::{RepoService, RepositoryReadContext, RepositorySession};
use crate::domain::error::GitHubError;
use crate::domain::notification::*;
use crate::domain::repository::RepositoryError;
use crate::github::notifications::{
    self, NotificationCursor, NotificationLimits, NotificationPage, PageFailure,
};
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
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
}
impl InboxSnapshot {
    pub fn is_empty(&self) -> bool {
        self.loaded
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
    cancel: CancellationToken,
}

#[derive(Clone)]
pub struct Notifications {
    session: RepositorySession,
    client: Arc<crate::github::client::GitHubClient>,
    limits: NotificationLimits,
    state: Arc<Mutex<State>>,
}
impl RepoService {
    pub fn notifications(&self) -> Result<Notifications, RepositoryError> {
        Ok(Notifications {
            session: self.session()?,
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
        let state = self.state.lock().expect("inbox state");
        let mut snapshot = state.snapshot.clone();
        snapshot.cooldown_seconds = remaining(state.refresh_after.max(state.request_after));
        snapshot.action_cooldown_seconds = remaining(state.request_after);
        snapshot
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
        if kind == RequestKind::First
            && (state.snapshot.loaded
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
            cancel: state.cancel.clone(),
        }))
    }

    /// Run on the shared BackgroundExecutor, then notify the view through a
    /// channel. Completion checks authorization and generation before changing
    /// data, cooldowns, validators, errors or loading state.
    pub async fn execute(&self, request: InboxRequest) {
        let result = self
            .session
            .read(request.cancel.clone(), async |pat, cancel| {
                // Keep headers alongside an endpoint error through the shared read
                // guard; only authorization failures use the outer error path.
                Ok::<_, GitHubError>(
                    notifications::fetch_page(
                        &self.client,
                        &pat,
                        request.cursor.as_ref(),
                        request.validator.as_deref(),
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
        self.finish(&request, result, Instant::now());
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
        let metadata = match &result {
            Ok(page) => &page.metadata,
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
        match result {
            Err(failure) => {
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
                if request.kind != RequestKind::More {
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
                    state.validator = page.metadata.last_modified;
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
                                Ok(done_meta) => {
                                    Ok(notifications::UnsubscribeOutcome::Completed(done_meta))
                                }
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
