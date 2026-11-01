//! One account-scoped inbox controller, using the existing authorization path.
use super::{RepoService, RepositoryReadContext, RepositorySession};
use crate::domain::error::GitHubError;
use crate::domain::notification::*;
use crate::domain::repository::RepositoryError;
use crate::github::notifications::{
    self, NotificationCursor, NotificationLimits, NotificationPage, PageFailure,
};
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use std::collections::HashSet;
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
}
impl Default for State {
    fn default() -> Self {
        let (cancel, cancel_source) = CancellationToken::new();
        Self {
            snapshot: InboxSnapshot::default(),
            generation: 0,
            next: None,
            validator: None,
            refresh_after: None,
            request_after: None,
            cancel,
            cancel_source,
        }
    }
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
        state.refresh_after = now.checked_add(Duration::from_secs(poll));
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
        if let Some(retry) = retry {
            state.request_after = now.checked_add(Duration::from_secs(retry.max(1)));
        }
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

    pub fn cancel(&self) {
        let mut state = self.state.lock().expect("inbox state");
        state.cancel_source.cancel();
        state.generation += 1;
        state.snapshot.busy = None;
    }
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
