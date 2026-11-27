//! Repository-scoped Pull requests state and delivery through the repository page receiver.
//!
//! Every list rule — query identity, paging, retry of the failed page, stale replies, real
//! empty vs. failed, retained filter/rows/position — lives in
//! [`crate::ui::repository_pulls::PullListModel`], which is unit-tested on its own. This
//! module only binds one model to the page's tab, account context, executor, virtual list
//! and scroll gesture.
//!
//! It never issues the personal search query used by the global Issues / Pull Requests
//! pages and it does not touch the Issues tab. A pull request number is only unique inside
//! one repository, so the row action carries the verified identity and account context it
//! was read under.

#[cfg(test)]
mod pull_offline_tests;
#[cfg(test)]
mod tests;
mod view;

use std::sync::mpsc::{self, Receiver, TryRecvError};

use gpui::{Context, FocusHandle, UniformListScrollHandle, Window};

use super::{PageAction, RepositoryPage, RepositoryTab};
use crate::data::page_cache::{CachedPageReadSession, CollectionGeneration};
use crate::domain::error::GitHubError;
use crate::domain::repository::RepositoryError;
use crate::domain::repository_pulls::{PullFilter, PullListPage};
use crate::github::repository_pulls::PullListLimits;
use crate::storage::cache::CacheAuthorization;
use crate::ui::repository_pulls::{
    PullFinish, PullListError, PullListModel, PullListRequest, PullRequestMode, PullScrollAnchor,
};
use crate::ui::scroll;
use crate::ui::work_item_row::WORK_ITEM_ROW_HEIGHT;

/// The stored pages and the first-page generation they were stored under.
type RestoreReply = Option<(Vec<PullListPage>, Option<CollectionGeneration>)>;

/// One read in flight. The model keeps the authoritative request, so a reply is handed back
/// with the request that issued it and the model decides whether it still applies. The optional
/// restore receiver carries the stored first page, which may paint the rows before the network
/// read ends and may settle after it failed.
struct PullsRequest {
    request: PullListRequest,
    results: Option<Receiver<(PullListRequest, Result<PullListPage, PullListError>)>>,
    restore: Option<Receiver<RestoreReply>>,
}

impl Drop for PullsRequest {
    fn drop(&mut self) {
        // The spawned tasks hold clones of the request, so dropping the receivers is not
        // enough to stop a read that is still waiting for a permit or the network.
        self.request.abort();
    }
}

pub(super) struct Pulls {
    /// The tab's selected filter. A view intent: it survives an account change and is
    /// restored when the model is rebuilt under a new session.
    filter: PullFilter,
    model: Option<PullListModel>,
    selected: Option<usize>,
    request: Option<PullsRequest>,
    /// The cache authorization token captured before the last read. A revocation invalidates
    /// it, and no cached row may stay visible after that.
    authorization: Option<CacheAuthorization>,
    /// The list offset the tab was left at, restored when it becomes active again.
    offset: f32,
    focus: FocusHandle,
    list_scroll: UniformListScrollHandle,
    motion: scroll::Motion<gpui::ScrollHandle>,
    bars: scroll::Bars,
    limits: PullListLimits,
}

impl Pulls {
    pub(super) fn new(focus: FocusHandle) -> Self {
        let list_scroll = UniformListScrollHandle::new();
        let motion = scroll::Motion::new(list_scroll.0.borrow().base_handle.clone());
        Self {
            filter: PullFilter::Open,
            model: None,
            selected: None,
            request: None,
            authorization: None,
            offset: 0.0,
            focus,
            list_scroll,
            motion,
            bars: scroll::Bars::new(),
            limits: PullListLimits::default(),
        }
    }

    pub(super) fn filter(&self) -> PullFilter {
        self.filter
    }

    /// Whether everything this tab holds was read under the current authorization.
    pub(super) fn current(&self) -> bool {
        RepositoryPage::authorization_current(&self.authorization)
            && self.model.as_ref().is_none_or(PullListModel::verify)
    }

    /// Whether the rows on screen are a bounded stored prefix rather than a verified read.
    pub(super) fn historical(&self) -> bool {
        self.model
            .as_ref()
            .is_some_and(PullListModel::is_historical)
    }

    pub(super) fn busy(&self) -> bool {
        self.request.is_some()
            || self
                .model
                .as_ref()
                .is_some_and(|model| model.busy().is_some())
    }

    pub(super) fn cancel_scroll(&mut self) {
        self.bars.cancel(&self.motion);
    }

    /// Drop authorization-bound values but keep the filter, which is a view intent.
    pub(super) fn clear(&mut self) {
        self.cancel_scroll();
        self.request = None;
        if let Some(model) = self.model.as_mut() {
            model.cancel();
        }
        self.model = None;
        self.selected = None;
        self.authorization = None;
        self.offset = 0.0;
        self.reset_position();
    }

    /// Put the real virtual list back at the top. The scroll handle and its easing outlive
    /// any one model, so a replaced list must not inherit the previous list's offset.
    fn reset_position(&mut self) {
        self.motion.cancel();
        self.list_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
    }
}

impl RepositoryPage {
    pub(super) fn pulls_model(&self) -> Option<&PullListModel> {
        self.pulls.model.as_ref()
    }

    /// The list's failed read as `(retry the first page, readable reason)`. A continuation
    /// failure retries that page; a first-page failure restarts the list.
    pub(super) fn pulls_failed(&self) -> Option<(bool, String)> {
        let model = self.pulls.model.as_ref()?;
        let error = model.error()?;
        Some((
            model.failed_mode() == Some(PullRequestMode::Replace),
            error.page_error().title,
        ))
    }

    /// Whether the displayed rows are a bounded stored prefix rather than a verified read.
    pub(super) fn pulls_historical(&self) -> bool {
        self.pulls.historical()
    }

    /// A short completeness note when the server could only return part of the list.
    pub(super) fn pulls_partial_label(&self) -> Option<&'static str> {
        let model = self.pulls.model.as_ref()?;
        model
            .is_partial()
            .then_some("Some pull requests may be missing")
    }

    /// Reconcile the tab with the repository this page is displaying.
    pub(super) fn sync_pulls(&mut self, cx: &mut Context<Self>) {
        let Some(identity) = self.details().map(|details| details.identity.clone()) else {
            return;
        };
        if self
            .pulls
            .model
            .as_ref()
            .is_some_and(|model| model.identity().id() != identity.id())
        {
            let filter = self.pulls.filter;
            self.pulls.clear();
            self.pulls.filter = filter;
        }
        if self.tab == RepositoryTab::PullRequests
            && !self.pulls.busy()
            && self
                .pulls
                .model
                .as_ref()
                .is_none_or(|model| !model.loaded() && model.error().is_none())
        {
            self.request_pulls(true, cx);
        }
    }

    pub(super) fn request_pulls(&mut self, first: bool, cx: &mut Context<Self>) {
        if self.tab != RepositoryTab::PullRequests || self.pulls.busy() || !self.cache_allowed() {
            return;
        }
        if first {
            // A replacement may answer with a different list; an index from the old one cannot
            // be trusted until the reply arrives.
            self.pulls.selected = None;
        }
        let Some(identity) = self.details().map(|details| details.identity.clone()) else {
            return;
        };
        let Some(context) = self.context.clone() else {
            return;
        };
        let Ok(session) = self
            .services
            .accounts
            .repo_service()
            .pull_list_session(identity.clone())
        else {
            return;
        };
        if session.context() != &context || !session.verify() {
            return;
        }
        // The persistent first page must belong to the same captured context. A cache
        // binding without a current authorization token displays no cached rows; fail closed
        // exactly like the repository Issues tab.
        let cached = self.cache_binding.clone().and_then(|binding| {
            let page = self.services.accounts.page_session().ok()?;
            if page.context() != &context || !page.verify() {
                return None;
            }
            let (store, scope) = binding.into_parts();
            CachedPageReadSession::new(page, store, scope).ok()
        });
        let authorization = if self.cache_binding.is_some() {
            let authorization = self.current_authorization(identity.id());
            if authorization.is_none() {
                return;
            }
            authorization
        } else {
            None
        };
        let filter = self.pulls.filter;
        // Reuse the model only when it belongs to this same account context and repository;
        // otherwise the rows it holds were read under another authorization.
        let mut model = match self.pulls.model.take() {
            Some(model)
                if model.context() == &context && model.identity().id() == identity.id() =>
            {
                model
            }
            _ => PullListModel::new(session, filter).with_limits(self.pulls.limits.clone()),
        };
        model.set_cache(cached.clone());
        let mode = if first {
            PullRequestMode::Replace
        } else {
            PullRequestMode::Append
        };
        // Restore stored content only into an empty list, before its network result. A refresh
        // of rows already on screen keeps them until the answer arrives.
        let restore = mode == PullRequestMode::Replace && !model.loaded();
        let Some(request) = model.begin(mode) else {
            self.pulls.model = Some(model);
            return;
        };
        self.pulls.authorization = authorization;
        self.pulls.model = Some(model);
        self.spawn_pulls(request, restore, cx);
    }

    /// Retry the read that failed. The model owns the decision: a failed continuation
    /// continues from its own cursor instead of restarting the list.
    pub(super) fn retry_pulls(&mut self, cx: &mut Context<Self>) {
        let request = self.pulls.model.as_mut().and_then(PullListModel::retry);
        match request {
            Some(request) => self.spawn_pulls(request, false, cx),
            None => self.request_pulls(true, cx),
        }
    }

    /// Run one model request on the background executor and keep it until drained.
    ///
    /// A replacement of an empty list also starts the stored first-page read. Both tasks run
    /// beside each other: local storage can paint the rows while the network request is still
    /// pending, and it can settle after that request failed.
    fn spawn_pulls(&mut self, request: PullListRequest, restore: bool, cx: &mut Context<Self>) {
        let task = request.clone();
        let (send, results) = mpsc::channel();
        let _ = self.services.executor.handle().spawn(async move {
            let result = task.execute().await;
            let _ = send.send((task, result));
        });
        let restore = restore.then(|| {
            let (send, receive) = mpsc::channel();
            let task = request.clone();
            let _ = self.services.executor.handle().spawn(async move {
                // The restored rows travel with the generation they were stored under. The
                // replacement's new generation must not relabel them, or a later continuation
                // would be attached to a first page it never belonged to.
                let page = task.restore().await;
                let _ = send.send(page);
            });
            receive
        });
        self.pulls.request = Some(PullsRequest {
            request,
            results: Some(results),
            restore,
        });
        cx.notify();
    }

    pub(super) fn drain_pulls(&mut self, cx: &mut Context<Self>) {
        self.sync_pulls(cx);
        let Some(mut pending) = self.pulls.request.take() else {
            return;
        };
        if !Self::authorization_current(&self.pulls.authorization) || !self.pulls.current() {
            self.pulls.clear();
            cx.notify();
            return;
        }
        // A stored first page may paint the list before the network answers, and may settle
        // after that answer failed. It is applied only while the replacement that issued it
        // still owns the list, so it can never join pages of another first-page generation.
        if let Some(restore) = &pending.restore {
            match restore.try_recv() {
                Ok(page) => {
                    pending.restore = None;
                    if let Some((pages, collection)) = page
                        && let Some(model) = self.pulls.model.as_mut()
                        && model.apply_cached(&pending.request, pages, collection)
                    {
                        cx.notify();
                    }
                }
                Err(TryRecvError::Disconnected) => pending.restore = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let result = match pending.results.as_ref().map(Receiver::try_recv) {
            // The network answer was already consumed; wait only for a slower stored read.
            None => None,
            Some(Ok(message)) => {
                pending.results = None;
                Some(message)
            }
            Some(Err(TryRecvError::Disconnected)) => {
                pending.results = None;
                Some((
                    pending.request.clone(),
                    Err(PullListError::Read(RepositoryError::GitHub(
                        GitHubError::Network {
                            message: "pull request list worker closed".into(),
                        },
                    ))),
                ))
            }
            Some(Err(TryRecvError::Empty)) => None,
        };
        if let Some((request, result)) = result {
            // A reply from another request (another generation, filter or page) is never applied.
            if request.generation() == pending.request.generation()
                && request.filter() == pending.request.filter()
                && request.page() == pending.request.page()
            {
                if result.is_ok() {
                    // A verified network page owns the list; a slower cached paint is stale.
                    pending.restore = None;
                }
                let Some(model) = self.pulls.model.as_mut() else {
                    return;
                };
                let applied = model.finish(&request, result);
                if applied == PullFinish::Rejected && !model.verify() {
                    let filter = model.filter();
                    self.pulls.clear();
                    self.pulls.filter = filter;
                    return;
                }
                cx.notify();
            }
        }
        if pending.results.is_some() || pending.restore.is_some() {
            self.pulls.request = Some(pending);
        }
    }

    pub(super) fn filter_pulls(&mut self, filter: PullFilter, cx: &mut Context<Self>) {
        self.pulls.filter = filter;
        // The old rows are about to be replaced; their selection must not survive.
        self.pulls.selected = None;
        if self.tab != RepositoryTab::PullRequests {
            return;
        }
        if self
            .pulls
            .model
            .as_ref()
            .is_some_and(|model| model.filter() == filter)
        {
            return;
        }
        // A different filter is a new list: the model drops rows, cursor, position and the
        // failed page, then issues its own replacement request.
        let request = self
            .pulls
            .model
            .as_mut()
            .and_then(|model| model.select_filter(filter));
        match request {
            Some(request) => {
                // A filter change always starts an empty replacement list, so a stored first
                // page of the new filter may paint before the network answers.
                let restore = !self.pulls.model.as_ref().is_some_and(PullListModel::loaded);
                self.spawn_pulls(request, restore, cx)
            }
            None => {
                self.pulls.clear();
                self.pulls.filter = filter;
                self.sync_pulls(cx);
            }
        }
        cx.notify();
    }

    pub(super) fn open_pull(
        &mut self,
        filter: PullFilter,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.check_authorization(cx);
        if self.tab != RepositoryTab::PullRequests {
            return;
        }
        let Some(model) = self.pulls.model.as_mut() else {
            return;
        };
        if model.filter() != filter || !model.verify() {
            return;
        }
        let Some(index) = model.rows().iter().position(|row| row.number == number) else {
            return;
        };
        let identity = model.identity().clone();
        if self
            .details()
            .is_none_or(|details| details.identity != identity)
        {
            return;
        }
        let Some(context) = self.context.clone() else {
            return;
        };
        self.pulls.selected = Some(index);
        self.remember_pull_position();
        self.pulls.focus.focus(window, cx);
        self.emit(
            PageAction::OpenPullRequest {
                context,
                locator: identity.locator().clone(),
                number,
            },
            window,
            cx,
        );
        cx.notify();
    }

    /// Record the row and offset the list sits at so leaving the tab keeps the position.
    fn remember_pull_position(&mut self) {
        let Some(model) = self.pulls.model.as_mut() else {
            return;
        };
        let Some(last) = model.rows().len().checked_sub(1) else {
            return;
        };
        let offset: f32 = self
            .pulls
            .list_scroll
            .0
            .borrow()
            .base_handle
            .offset()
            .y
            .into();
        self.pulls.offset = offset;
        let index = ((-offset / WORK_ITEM_ROW_HEIGHT).max(0.0) as usize).min(last);
        let pull_id = model.rows()[index].id;
        model.remember_scroll(PullScrollAnchor { pull_id, offset });
    }

    /// The page stopped being the active tab. The virtual list keeps its own offset and the
    /// model keeps its rows, so returning shows the same filter, pages and position; this only
    /// ends the list's gesture and records the anchor. Nothing is refetched: a refresh is an
    /// explicit action.
    pub(super) fn suspend_pulls(&mut self) {
        self.pulls.cancel_scroll();
        self.remember_pull_position();
    }
}
