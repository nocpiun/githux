//! Repository-scoped Issues state and delivery through the repository page receiver.
#[cfg(test)]
mod tests;
mod view;

use std::sync::mpsc::{self, Receiver, TryRecvError};

use gpui::{Context, FocusHandle, UniformListScrollHandle, Window};

use super::{PageAction, RepositoryPage, RepositoryTab};
use crate::data::page_cache::{CachedPageReadSession, PageCacheError, PageReadMode, PageValue};
use crate::domain::{
    error::GitHubError,
    repository::{RepositoryError, RepositoryIdentity},
};
use crate::github::repository_issues::{IssueCursor, IssueFilter, IssuePage, IssueRow};
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::storage::cache::{CacheAuthorization, CacheError};
use crate::ui::scroll;

type ReadResult = Result<IssuePage, PageCacheError<RepositoryError>>;

struct IssueRequest {
    authorization: Option<CacheAuthorization>,
    cancel: CancellationTokenSource,
    first: bool,
    cache: Option<Receiver<Option<IssuePage>>>,
    network: Receiver<ReadResult>,
}

impl Drop for IssueRequest {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(super) struct Issues {
    repository: Option<RepositoryIdentity>,
    filter: IssueFilter,
    rows: Vec<IssueRow>,
    selected: Option<usize>,
    next: Option<IssueCursor>,
    loaded: bool,
    historical: bool,
    failure: Option<(bool, String)>,
    request: Option<IssueRequest>,
    authorization: Option<CacheAuthorization>,
    focus: FocusHandle,
    list_scroll: UniformListScrollHandle,
    motion: scroll::Motion<gpui::ScrollHandle>,
    bars: scroll::Bars,
}

impl Issues {
    pub(super) fn new(focus: FocusHandle) -> Self {
        let list_scroll = UniformListScrollHandle::new();
        let motion = scroll::Motion::new(list_scroll.0.borrow().base_handle.clone());
        Self {
            repository: None,
            filter: IssueFilter::Open,
            rows: Vec::new(),
            selected: None,
            next: None,
            loaded: false,
            historical: false,
            failure: None,
            request: None,
            authorization: None,
            focus,
            list_scroll,
            motion,
            bars: scroll::Bars::new(),
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.request.is_some()
    }

    pub(super) fn cancel_scroll(&mut self) {
        self.bars.cancel(&self.motion);
    }

    pub(super) fn clear(&mut self) {
        self.cancel_scroll();
        *self = Self::new(self.focus.clone());
    }

    pub(super) fn current(&self) -> bool {
        RepositoryPage::authorization_current(&self.authorization)
    }

    fn apply(
        &mut self,
        page: IssuePage,
        first: bool,
        historical: bool,
        authorization: Option<CacheAuthorization>,
    ) {
        if first {
            self.rows.clear();
            self.selected = None;
            self.motion
                .set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
        }
        let mut seen: std::collections::HashSet<_> =
            self.rows.iter().map(|row| row.number).collect();
        self.rows
            .extend(page.rows.into_iter().filter(|row| seen.insert(row.number)));
        self.next = page.next;
        self.loaded = true;
        self.historical = historical;
        self.authorization = authorization;
    }
}

impl RepositoryPage {
    pub(super) fn sync_issues(&mut self, cx: &mut Context<Self>) {
        let repository = self.details().map(|details| &details.identity);
        if self
            .issues
            .repository
            .as_ref()
            .is_some_and(|old| Some(old) != repository)
        {
            let filter = self.issues.filter;
            self.issues.clear();
            self.issues.filter = filter;
        }
        if self.tab == RepositoryTab::Issues && !self.issues.busy() && self.issues.failure.is_none()
        {
            if !self.issues.loaded {
                self.request_issues(true, cx);
            } else if self.issues.rows.is_empty()
                && self.issues.next.is_some()
                && !self.issues.historical
            {
                self.request_issues(false, cx);
            }
        }
    }

    pub(super) fn request_issues(&mut self, first: bool, cx: &mut Context<Self>) {
        if self.tab != RepositoryTab::Issues || self.issues.busy() || !self.cache_allowed() {
            return;
        }
        let Some(repository) = self.details().map(|details| details.identity.clone()) else {
            return;
        };
        let Ok(session) = self.services.accounts.page_session() else {
            return;
        };
        if Some(session.context()) != self.context.as_ref() || !session.verify() {
            return;
        }
        let cursor = if first {
            None
        } else if let Some(next) = self.issues.next.clone().filter(|_| !self.issues.historical) {
            Some(next)
        } else {
            return;
        };
        let filter = self.issues.filter;
        let authorization = self.current_authorization(repository.id());
        if self.cache_binding.is_some() && authorization.is_none() {
            return;
        }
        let cached = self.cache_binding.clone().and_then(|binding| {
            let (store, scope) = binding.into_parts();
            CachedPageReadSession::new(session.clone(), store, scope).ok()
        });
        let (token, cancel) = CancellationToken::new();
        let cache = cached
            .as_ref()
            .filter(|_| first && !self.issues.loaded)
            .map(|cached| {
                let (send, receive) = mpsc::channel();
                let cached = cached.clone();
                let repository = repository.clone();
                let token = token.clone();
                let _ = self.services.executor.handle().spawn(async move {
                    let value = cached
                        .repository_issues(PageReadMode::Offline, &repository, filter, token)
                        .await;
                    let page = match value.map(|result| result.value) {
                        Ok(PageValue::Value { value, .. }) => Some(value),
                        _ => None,
                    };
                    let _ = send.send(page);
                });
                receive
            });
        let (send, network) = mpsc::channel();
        let worker_repository = repository.clone();
        let _ = self.services.executor.handle().spawn(async move {
            let result = read_online(
                &session,
                cached.as_ref(),
                &worker_repository,
                filter,
                cursor.as_ref(),
                token,
            )
            .await;
            let _ = send.send(result);
        });
        self.issues.repository = Some(repository);
        self.issues.failure = None;
        self.issues.request = Some(IssueRequest {
            authorization,
            cancel,
            first,
            cache,
            network,
        });
        cx.notify();
    }

    pub(super) fn drain_issues(&mut self, cx: &mut Context<Self>) {
        self.sync_issues(cx);
        let Some(mut request) = self.issues.request.take() else {
            return;
        };
        if !Self::authorization_current(&request.authorization) || !self.issues.current() {
            self.issues.clear();
            cx.notify();
            return;
        }
        let waiting_for_cache = self.issues.failure.is_some();
        let result = match request.network.try_recv() {
            _ if waiting_for_cache => None,
            Ok(result) => Some(result),
            Err(TryRecvError::Disconnected) => Some(Err(PageCacheError::Read(
                GitHubError::Network {
                    message: "Issue request worker closed".into(),
                }
                .into(),
            ))),
            Err(TryRecvError::Empty) => None,
        };
        if matches!(&result, Some(Ok(_))) {
            request.cache = None;
        }
        if let Some(cache) = &request.cache {
            match cache.try_recv() {
                Ok(page) => {
                    request.cache = None;
                    if let Some(page) = page {
                        self.issues
                            .apply(page, true, true, request.authorization.clone());
                        cx.notify();
                    }
                }
                Err(TryRecvError::Disconnected) => request.cache = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        match result {
            None if !waiting_for_cache || request.cache.is_some() => {
                self.issues.request = Some(request)
            }
            None => cx.notify(),
            Some(Ok(page)) => {
                let skip = page.rows.is_empty() && page.next.is_some();
                self.issues
                    .apply(page, request.first, false, request.authorization.clone());
                if skip {
                    self.request_issues(false, cx);
                }
                cx.notify();
            }
            Some(Err(error)) => {
                let message = issue_error(&error);
                let denied = access_failure(&error);
                let retry_first = request.first || denied;
                if denied {
                    let filter = self.issues.filter;
                    self.issues.clear();
                    self.issues.filter = filter;
                    self.issues.repository = self.details().map(|details| details.identity.clone());
                } else if request.cache.is_some() {
                    // Let a slower eligible disk read settle after the online failure.
                    self.issues.request = Some(request);
                }
                self.issues.failure = Some((retry_first, message));
                cx.notify();
            }
        }
    }

    fn filter_issues(&mut self, filter: IssueFilter, cx: &mut Context<Self>) {
        if self.tab == RepositoryTab::Issues && self.issues.filter != filter {
            self.issues.clear();
            self.issues.filter = filter;
            self.sync_issues(cx);
            cx.notify();
        }
    }

    fn open_issue(
        &mut self,
        filter: IssueFilter,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.check_authorization(cx);
        if self.tab != RepositoryTab::Issues
            || !self.issues.current()
            || self.issues.filter != filter
        {
            return;
        }
        let Some((repository, index)) = self
            .issues
            .repository
            .as_ref()
            .zip(self.issues.rows.iter().position(|row| row.number == number))
        else {
            return;
        };
        if self
            .details()
            .is_none_or(|details| &details.identity != repository)
        {
            return;
        }
        let Some(context) = self.context.clone() else {
            return;
        };
        let locator = repository.locator().clone();
        self.issues.selected = Some(index);
        self.issues.focus.focus(window, cx);
        self.emit(
            PageAction::OpenIssue {
                context,
                locator,
                number,
            },
            window,
            cx,
        );
        cx.notify();
    }
}

async fn read_online(
    session: &crate::data::PageReadSession,
    cached: Option<&CachedPageReadSession>,
    repository: &RepositoryIdentity,
    filter: IssueFilter,
    cursor: Option<&IssueCursor>,
    token: CancellationToken,
) -> ReadResult {
    if let Some(cached) = cached {
        return cached
            .repository_issues_online(repository, filter, cursor, token)
            .await;
    }
    session
        .repository_issues(repository, filter, cursor, token)
        .await
        .map(|result| result.value)
        .map_err(PageCacheError::Read)
}

fn access_failure(error: &PageCacheError<RepositoryError>) -> bool {
    matches!(
        error,
        PageCacheError::Cache(CacheError::AccessDenied | CacheError::Stale)
            | PageCacheError::Context(GitHubError::Unauthorized | GitHubError::StaleContext)
            | PageCacheError::Read(RepositoryError::GitHub(
                GitHubError::Unauthorized
                    | GitHubError::Forbidden
                    | GitHubError::NotFound
                    | GitHubError::SsoRequired { .. }
            ))
            | PageCacheError::Read(RepositoryError::SsoPartial { .. })
    )
}

fn issue_error(error: &PageCacheError<RepositoryError>) -> String {
    match error {
        PageCacheError::Read(error) => super::PageError::from_repository(error).title.clone(),
        PageCacheError::Context(error) => super::PageError::from_repository(&error.clone().into())
            .title
            .clone(),
        PageCacheError::Cache(CacheError::AccessDenied | CacheError::Stale) => {
            "Issue access could not be verified.".into()
        }
        PageCacheError::Cache(_) => "Could not read the issue cache.".into(),
    }
}
