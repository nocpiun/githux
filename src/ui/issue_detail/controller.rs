//! Account-bound state machine for a single Issue and its paginated plain comments.
//!
//! Issue detail and comments own independent generations and error slots so a comment page
//! failure never erases the issue body, and a background refresh keeps the currently displayed
//! body and comments visible until the new response settles.

use std::collections::HashSet;

use crate::data::issue_detail::{CachedCommentsSnapshot, CachedIssueSnapshot, IssueDetailSession};
use crate::data::{RepositoryReadContext, Scoped};
use crate::domain::issue_detail::{
    IssueComment, IssueCommentsCursor, IssueCommentsPage, IssueDetail, IssueDetailError,
    IssueLocator,
};
use crate::github::issue_detail::IssueDetailLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::storage::cache::CacheAuthorization;

/// Provenance of the currently displayed issue snapshot.
#[derive(Clone, Debug)]
pub enum IssueOrigin {
    Network,
    Cached {
        verified_at_ms: i64,
        revalidation_failed: bool,
        authorization: CacheAuthorization,
    },
}

impl IssueOrigin {
    pub fn is_authorized(&self) -> bool {
        match self {
            Self::Network => true,
            Self::Cached { authorization, .. } => authorization.is_current(),
        }
    }

    pub fn revalidation_failed(&self) -> bool {
        matches!(
            self,
            Self::Cached {
                revalidation_failed: true,
                ..
            }
        )
    }

    pub fn verified_at_ms(&self) -> Option<i64> {
        match self {
            Self::Network => None,
            Self::Cached { verified_at_ms, .. } => Some(*verified_at_ms),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentsRequestMode {
    Replace,
    Append,
}

#[derive(Clone)]
pub struct IssueRequest {
    pub generation: u64,
    pub locator: IssueLocator,
    pub session: IssueDetailSession,
    pub cancel: CancellationToken,
}

impl IssueRequest {
    pub async fn execute_online(
        &self,
        limits: &IssueDetailLimits,
    ) -> Result<Scoped<IssueDetail>, IssueDetailError> {
        self.session
            .fetch_issue(&self.locator, limits, self.cancel.clone())
            .await
    }

    pub async fn execute_cache(
        &self,
    ) -> Result<Option<Scoped<CachedIssueSnapshot>>, IssueDetailError> {
        self.session
            .read_cached_issue(&self.locator, self.cancel.clone())
            .await
    }
}

#[derive(Clone)]
pub struct CommentsRequest {
    pub generation: u64,
    pub mode: CommentsRequestMode,
    pub locator: IssueLocator,
    pub cursor: Option<IssueCommentsCursor>,
    pub session: IssueDetailSession,
    pub cancel: CancellationToken,
}

impl CommentsRequest {
    pub async fn execute_online(
        &self,
        limits: &IssueDetailLimits,
    ) -> Result<Scoped<IssueCommentsPage>, IssueDetailError> {
        self.session
            .fetch_comments_page(
                &self.locator,
                self.cursor.as_ref(),
                limits,
                self.cancel.clone(),
            )
            .await
    }

    pub async fn execute_cache(
        &self,
        limits: &IssueDetailLimits,
    ) -> Result<Option<Scoped<CachedCommentsSnapshot>>, IssueDetailError> {
        self.session
            .read_cached_comments_first_page(&self.locator, limits, self.cancel.clone())
            .await
    }
}

pub struct Controller {
    session: IssueDetailSession,
    pub locator: IssueLocator,

    pub issue: Option<IssueDetail>,
    pub issue_origin: IssueOrigin,
    pub issue_loaded: bool,
    pub issue_busy: bool,
    pub issue_error: Option<IssueDetailError>,
    issue_generation: u64,
    issue_cancel: Option<CancellationTokenSource>,

    pub comments: Vec<IssueComment>,
    pub comments_origin: IssueOrigin,
    pub comments_loaded: bool,
    pub comments_next: Option<IssueCommentsCursor>,
    pub comments_busy: Option<CommentsRequestMode>,
    pub comments_error: Option<IssueDetailError>,
    pub comments_failed_mode: Option<CommentsRequestMode>,
    comments_generation: u64,
    comments_cancel: Option<CancellationTokenSource>,
}

impl Controller {
    pub fn new(session: IssueDetailSession, locator: IssueLocator) -> Self {
        Self {
            session,
            locator,
            issue: None,
            issue_origin: IssueOrigin::Network,
            issue_loaded: false,
            issue_busy: false,
            issue_error: None,
            issue_generation: 0,
            issue_cancel: None,
            comments: Vec::new(),
            comments_origin: IssueOrigin::Network,
            comments_loaded: false,
            comments_next: None,
            comments_busy: None,
            comments_error: None,
            comments_failed_mode: None,
            comments_generation: 0,
            comments_cancel: None,
        }
    }

    pub fn context(&self) -> &RepositoryReadContext {
        self.session.context()
    }

    pub fn current(&self) -> bool {
        self.session.verify()
    }

    pub fn is_busy(&self) -> bool {
        self.current() && (self.issue_busy || self.comments_busy.is_some())
    }

    /// Enforce L022: if a cached snapshot's authorization token was revoked, drop it immediately.
    pub fn enforce_cache_authorization(&mut self) -> bool {
        let mut changed = false;
        if self.issue.is_some() && !self.issue_origin.is_authorized() {
            self.issue = None;
            self.issue_origin = IssueOrigin::Network;
            self.issue_loaded = false;
            changed = true;
        }
        if !self.comments.is_empty() && !self.comments_origin.is_authorized() {
            self.comments.clear();
            self.comments_origin = IssueOrigin::Network;
            self.comments_loaded = false;
            self.comments_next = None;
            changed = true;
        }
        changed
    }

    pub fn cancel_all(&mut self) {
        self.issue_generation = self.issue_generation.wrapping_add(1);
        if let Some(cancel) = self.issue_cancel.take() {
            cancel.cancel();
        }
        self.issue_busy = false;

        self.comments_generation = self.comments_generation.wrapping_add(1);
        if let Some(cancel) = self.comments_cancel.take() {
            cancel.cancel();
        }
        self.comments_busy = None;
    }

    pub fn begin_issue(&mut self) -> Option<IssueRequest> {
        if !self.current() || self.issue_busy {
            return None;
        }
        self.issue_generation = self.issue_generation.wrapping_add(1);
        if let Some(previous) = self.issue_cancel.take() {
            previous.cancel();
        }
        let (token, source) = CancellationToken::new();
        self.issue_cancel = Some(source);
        self.issue_busy = true;
        // Hide stale retry badge while an explicit refresh/retry is in flight.
        if let IssueOrigin::Cached {
            revalidation_failed,
            ..
        } = &mut self.issue_origin
        {
            *revalidation_failed = false;
        }
        Some(IssueRequest {
            generation: self.issue_generation,
            locator: self.locator.clone(),
            session: self.session.clone(),
            cancel: token,
        })
    }

    pub fn begin_comments(&mut self, mode: CommentsRequestMode) -> Option<CommentsRequest> {
        if !self.current() {
            return None;
        }
        if self.comments_busy == Some(mode) {
            return None;
        }
        if mode == CommentsRequestMode::Append
            && (self.comments_busy.is_some()
                || matches!(self.comments_origin, IssueOrigin::Cached { .. }))
        {
            return None;
        }
        let cursor = match mode {
            CommentsRequestMode::Replace => None,
            CommentsRequestMode::Append => Some(self.comments_next.clone()?),
        };
        self.comments_generation = self.comments_generation.wrapping_add(1);
        if let Some(previous) = self.comments_cancel.take() {
            previous.cancel();
        }
        let (token, source) = CancellationToken::new();
        self.comments_cancel = Some(source);
        self.comments_busy = Some(mode);
        Some(CommentsRequest {
            generation: self.comments_generation,
            mode,
            locator: self.locator.clone(),
            cursor,
            session: self.session.clone(),
            cancel: token,
        })
    }

    /// Apply an authorized cached issue snapshot only if no live issue is already shown and
    /// the request generation is still current. If the same generation's online read already
    /// failed with a confirmed missing/auth error, reject the late snapshot; if it failed with a
    /// transient error, mark `revalidation_failed = true`.
    pub fn apply_cached_issue(
        &mut self,
        request: &IssueRequest,
        snapshot: Option<Scoped<CachedIssueSnapshot>>,
    ) -> bool {
        if request.generation != self.issue_generation
            || !self.current()
            || self.issue.is_some()
            || !request.locator.same_resource(&self.locator)
        {
            return false;
        }
        if let Some(error) = &self.issue_error
            && (error.is_missing()
                || error.is_auth_or_permission()
                || matches!(error, IssueDetailError::NotAnIssue))
        {
            return false;
        }
        let Some(scoped) = snapshot else {
            return false;
        };
        if &scoped.context != self.context() || !scoped.value.authorization.is_current() {
            return false;
        }
        let revalidation_failed = !self.issue_busy
            && self
                .issue_error
                .as_ref()
                .is_some_and(IssueDetailError::is_transient);
        self.issue = Some(scoped.value.issue);
        self.issue_origin = IssueOrigin::Cached {
            verified_at_ms: scoped.value.verified_at_ms,
            revalidation_failed,
            authorization: scoped.value.authorization,
        };
        true
    }

    /// Apply an authorized cached page-1 comments snapshot only if no comments are loaded yet
    /// and the same generation has not already failed with a missing/auth error.
    pub fn apply_cached_comments(
        &mut self,
        request: &CommentsRequest,
        snapshot: Option<Scoped<CachedCommentsSnapshot>>,
    ) -> bool {
        if request.generation != self.comments_generation
            || request.mode != CommentsRequestMode::Replace
            || !self.current()
            || self.comments_loaded
            || !request.locator.same_resource(&self.locator)
        {
            return false;
        }
        if let Some(error) = &self.comments_error
            && (error.is_missing() || error.is_auth_or_permission())
        {
            return false;
        }
        let Some(scoped) = snapshot else {
            return false;
        };
        if &scoped.context != self.context() || !scoped.value.authorization.is_current() {
            return false;
        }
        let revalidation_failed = self.comments_busy.is_none()
            && self
                .comments_error
                .as_ref()
                .is_some_and(IssueDetailError::is_transient);
        self.comments = scoped.value.page.comments;
        // Never allow live page-2 append against an un-revalidated cached page 1:
        // deleted/shifted comments between snapshots would silently skip entries.
        self.comments_next = None;
        self.comments_loaded = true;
        self.comments_origin = IssueOrigin::Cached {
            verified_at_ms: scoped.value.verified_at_ms,
            revalidation_failed,
            authorization: scoped.value.authorization,
        };
        true
    }

    pub fn finish_issue(
        &mut self,
        request: &IssueRequest,
        result: Result<Scoped<IssueDetail>, IssueDetailError>,
    ) -> bool {
        if request.generation != self.issue_generation
            || !self.current()
            || !request.locator.same_resource(&self.locator)
        {
            return false;
        }
        self.issue_busy = false;
        self.issue_cancel = None;
        match result {
            Ok(scoped) => {
                if &scoped.context != self.context() {
                    return false;
                }
                self.issue = Some(scoped.value);
                self.issue_origin = IssueOrigin::Network;
                self.issue_loaded = true;
                self.issue_error = None;
            }
            Err(error) => {
                if error.is_cancelled() {
                    return true;
                }
                if error.is_missing()
                    || error.is_auth_or_permission()
                    || matches!(error, IssueDetailError::NotAnIssue)
                {
                    // Confirmed absence or permission loss must not keep stale cached content.
                    self.issue = None;
                    self.issue_origin = IssueOrigin::Network;
                    self.issue_loaded = false;
                    self.comments.clear();
                    self.comments_next = None;
                    self.comments_loaded = false;
                    self.comments_error = None;
                } else if self.issue.is_some()
                    && let IssueOrigin::Cached {
                        revalidation_failed,
                        ..
                    } = &mut self.issue_origin
                {
                    *revalidation_failed = true;
                }
                self.issue_error = Some(error);
            }
        }
        true
    }

    pub fn finish_comments(
        &mut self,
        request: &CommentsRequest,
        result: Result<Scoped<IssueCommentsPage>, IssueDetailError>,
    ) -> bool {
        if request.generation != self.comments_generation
            || !self.current()
            || !request.locator.same_resource(&self.locator)
        {
            return false;
        }
        self.comments_busy = None;
        self.comments_cancel = None;
        match result {
            Ok(scoped) => {
                if &scoped.context != self.context() {
                    return false;
                }
                let page = scoped.value;
                match request.mode {
                    CommentsRequestMode::Replace => {
                        self.comments = page.comments;
                    }
                    CommentsRequestMode::Append => {
                        let mut seen: HashSet<u64> =
                            self.comments.iter().map(|comment| comment.id).collect();
                        for comment in page.comments {
                            if seen.insert(comment.id) {
                                self.comments.push(comment);
                            }
                        }
                    }
                }
                self.comments_next = page.next;
                self.comments_loaded = true;
                self.comments_origin = IssueOrigin::Network;
                self.comments_error = None;
                self.comments_failed_mode = None;
            }
            Err(error) => {
                if error.is_cancelled() {
                    return true;
                }
                let cleared = error.is_missing() || error.is_auth_or_permission();
                if cleared {
                    self.comments.clear();
                    self.comments_next = None;
                    self.comments_loaded = false;
                    self.comments_origin = IssueOrigin::Network;
                }
                self.comments_error = Some(error);
                self.comments_failed_mode = Some(if cleared {
                    CommentsRequestMode::Replace
                } else {
                    request.mode
                });
            }
        }
        true
    }

    /// True only when comments have genuinely loaded with zero items and no error.
    pub fn comments_confirmed_empty(&self) -> bool {
        self.comments_loaded
            && self.comments.is_empty()
            && self.comments_error.is_none()
            && self.comments_busy.is_none()
    }
}
