//! Account-bound state machine for a single Issue and its paginated plain comments.
//!
//! Issue detail and comments own independent generations and error slots so a comment page
//! failure never erases the issue body, and a background refresh keeps the currently displayed
//! body and comments visible until the new response settles.

use std::collections::HashSet;

use crate::data::issue_detail::{
    CachedReading, IssueDetailSession, LiveComments, LiveIssue, ReadingScope, Refresh,
};
use crate::data::{RepositoryReadContext, Scoped};
use crate::domain::error::GitHubError;
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
    pub async fn execute_online(&self, limits: &IssueDetailLimits) -> Refresh<LiveIssue> {
        self.session
            .refresh_issue(&self.locator, limits, self.cancel.clone())
            .await
    }

    pub async fn execute_cache(&self, limits: &IssueDetailLimits) -> Option<CachedReading> {
        self.session
            .cached(&self.locator, limits, &self.cancel)
            .await
    }
}

#[derive(Clone)]
pub struct CommentsRequest {
    pub generation: u64,
    pub mode: CommentsRequestMode,
    pub locator: IssueLocator,
    pub cursor: Option<IssueCommentsCursor>,
    scope: Option<ReadingScope>,
    collection: Option<[u8; 16]>,
    persist: bool,
    pub session: IssueDetailSession,
    pub cancel: CancellationToken,
}

impl CommentsRequest {
    pub async fn execute_online(&self, limits: &IssueDetailLimits) -> Refresh<LiveComments> {
        self.session
            .refresh_comments(
                &self.locator,
                self.scope.as_ref(),
                self.collection,
                self.cursor.as_ref(),
                limits,
                self.cancel.clone(),
                self.persist,
            )
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
    pub cached_comments_partial: Option<bool>,
    repository: Option<u64>,
    scope: Option<ReadingScope>,
    authorization: Option<CacheAuthorization>,
    collection: Option<[u8; 16]>,
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
            cached_comments_partial: None,
            repository: None,
            scope: None,
            authorization: None,
            collection: None,
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

    /// Revoked stamps can never authorize a reply or a retained reader again.
    pub fn enforce_cache_authorization(&mut self) -> bool {
        if !self.reading_is_authorized() {
            self.clear_reading(GitHubError::StaleContext.into());
            return true;
        }
        false
    }

    pub(super) fn reading_is_authorized(&self) -> bool {
        self.authorization
            .as_ref()
            .is_none_or(CacheAuthorization::is_current)
            && self.issue_origin.is_authorized()
            && self.comments_origin.is_authorized()
    }

    fn clear_reading(&mut self, error: IssueDetailError) {
        self.cancel_all();
        self.issue = None;
        self.issue_loaded = false;
        self.issue_origin = IssueOrigin::Network;
        self.issue_error = Some(error);
        self.repository = None;
        self.scope = None;
        self.authorization = None;
        self.clear_comments();
    }

    fn clear_comments(&mut self) {
        self.comments.clear();
        self.comments_loaded = false;
        self.comments_next = None;
        self.comments_error = None;
        self.comments_failed_mode = None;
        self.comments_origin = IssueOrigin::Network;
        self.cached_comments_partial = None;
        self.collection = None;
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
        if self.comments_busy.is_some() {
            self.cancel_all();
        }
        self.issue_generation = self.issue_generation.wrapping_add(1);
        if let Some(previous) = self.issue_cancel.take() {
            previous.cancel();
        }
        let (token, source) = CancellationToken::new();
        self.issue_cancel = Some(source);
        self.issue_busy = true;
        self.issue_error = None;
        self.comments_error = None;
        if let IssueOrigin::Cached {
            revalidation_failed,
            ..
        } = &mut self.comments_origin
        {
            *revalidation_failed = false;
        }
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
            scope: self.scope.clone(),
            collection: self.collection,
            persist: self.issue_loaded && self.issue_error.is_none(),
            session: self.session.clone(),
            cancel: token,
        })
    }

    pub fn apply_cached_reading(
        &mut self,
        request: &IssueRequest,
        snapshot: CachedReading,
    ) -> bool {
        if !self.matches_issue(request)
            || self.issue.is_some()
            || !snapshot.authorization.is_current()
        {
            return false;
        }
        let origin = IssueOrigin::Cached {
            verified_at_ms: snapshot.verified_at_ms,
            revalidation_failed: false,
            authorization: snapshot.authorization,
        };
        self.repository = Some(snapshot.scope.repository);
        self.scope = Some(snapshot.scope);
        self.issue = Some(snapshot.issue);
        self.issue_origin = origin.clone();
        if let Some(comments) = snapshot.comments {
            self.comments = comments.comments;
            self.comments_loaded = true;
            self.cached_comments_partial = Some(comments.partial);
            self.comments_origin = origin;
        }
        true
    }

    pub fn finish_issue(&mut self, request: &IssueRequest, refresh: Refresh<LiveIssue>) -> bool {
        if !self.matches_issue(request) {
            return false;
        }
        self.issue_busy = false;
        self.issue_cancel = None;
        match refresh.result {
            Ok(live) => {
                if live
                    .authorization
                    .as_ref()
                    .is_some_and(|auth| !auth.is_current())
                {
                    self.clear_reading(GitHubError::StaleContext.into());
                    return true;
                }
                if self.repository != live.repository || refresh.no_store {
                    self.clear_comments();
                }
                self.repository = live.repository;
                self.scope = live.scope;
                self.authorization = live.authorization;
                self.issue = Some(live.issue);
                self.issue_origin = IssueOrigin::Network;
                self.issue_loaded = true;
            }
            Err(error) if error.is_cancelled() => {}
            Err(error)
                if error.invalidates_reading()
                    || (matches!(self.issue_origin, IssueOrigin::Cached { .. })
                        && (refresh.no_store || !error.is_transient())) =>
            {
                self.clear_reading(error)
            }
            Err(error) => {
                if let IssueOrigin::Cached {
                    revalidation_failed,
                    ..
                } = &mut self.issue_origin
                {
                    *revalidation_failed = error.is_transient();
                }
                self.issue_error = Some(error);
            }
        }
        true
    }

    fn matches_issue(&self, request: &IssueRequest) -> bool {
        request.generation == self.issue_generation
            && self.current()
            && request.locator.same_resource(&self.locator)
    }

    pub fn finish_issue_comments(
        &mut self,
        request: &CommentsRequest,
        refresh: Refresh<LiveComments>,
    ) -> bool {
        if !self.matches_comments(request) {
            return false;
        }
        match refresh.result {
            Err(error) if error.invalidates_reading() => self.clear_reading(error),
            Err(error) => {
                if request.mode == CommentsRequestMode::Replace && !error.is_cancelled() {
                    self.comments_next = None;
                }
                if let IssueOrigin::Cached {
                    revalidation_failed,
                    ..
                } = &mut self.comments_origin
                {
                    if refresh.no_store || (!error.is_transient() && !error.is_cancelled()) {
                        self.clear_comments();
                    } else {
                        *revalidation_failed = error.is_transient();
                    }
                }
                self.finish_comments(request, Err(error));
            }
            Ok(mut live) => {
                if request.mode == CommentsRequestMode::Replace {
                    let mut seen = HashSet::new();
                    live.page
                        .value
                        .comments
                        .retain(|comment| seen.insert(comment.id));
                }
                self.collection = live.collection;
                self.cached_comments_partial = None;
                self.finish_comments(request, Ok(live.page));
            }
        }
        true
    }

    pub(super) fn prepare_comment_display(&mut self) {
        self.comments_error = None;
        if let IssueOrigin::Cached {
            revalidation_failed,
            ..
        } = &mut self.comments_origin
        {
            *revalidation_failed = false;
        }
    }

    fn matches_comments(&self, request: &CommentsRequest) -> bool {
        request.generation == self.comments_generation
            && self.current()
            && request.locator.same_resource(&self.locator)
    }

    pub fn finish_comments(
        &mut self,
        request: &CommentsRequest,
        result: Result<Scoped<IssueCommentsPage>, IssueDetailError>,
    ) -> bool {
        if !self.matches_comments(request) {
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
