//! Read-only single-Issue detail view with Markdown body and paginated plain comments.
//!
//! Construction starts the initial cache-assisted + online read; rendering never performs I/O.
//! Body and comments reuse [`ReaderView`] in [`MarkdownLayout::Flow`] so the entire issue
//! scrolls smoothly inside one shared [`crate::ui::scroll`] viewport.

pub mod controller;
mod render;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render, ScrollHandle,
    Subscription, Task, Window,
};

use crate::app::avatar::AvatarKey;
use crate::app::pages::PageAvatars;
use crate::content::PreparedBody;
use crate::data::RepoService;
use crate::data::RepositoryReadContext;
use crate::data::issue_detail::{CachedReading, LiveComments, LiveIssue, Refresh};
use crate::domain::error::GitHubError;
use crate::domain::issue_detail::{IssueDetailError, IssueLocator};
use crate::domain::repository::RepositoryLocator;
use crate::github::issue_detail::IssueDetailLimits;
use crate::network::executor::BackgroundExecutor;
use crate::ui::reader::{
    DocumentKind, ImageProvider, LinkTarget, MarkdownLayout, ReaderAction, ReaderView,
    ResourceContext,
};
use crate::ui::scroll::{self, Bars, Motion};

use controller::{CommentsRequest, CommentsRequestMode, Controller, IssueRequest};

/// Typed navigation emitted by [`IssueDetailView`]; the formal host routes these through Shell.
#[derive(Clone, Debug)]
pub enum IssueDetailAction {
    Repository {
        context: RepositoryReadContext,
        repository: RepositoryLocator,
    },
    Author {
        context: RepositoryReadContext,
        login: String,
    },
    External(String),
}

pub type OnIssueDetailAction = Rc<dyn Fn(IssueDetailAction, &mut Window, &mut App)>;

struct CachedReader {
    digest: u64,
    view: Entity<ReaderView>,
}

enum Reply {
    Cached {
        request: IssueRequest,
        snapshot: CachedReading,
    },
    Issue {
        request: IssueRequest,
        refresh: Refresh<LiveIssue>,
    },
    Comments {
        request: CommentsRequest,
        refresh: Refresh<LiveComments>,
    },
}

pub struct IssueDetailView {
    pub controller: Controller,
    repos: RepoService,
    executor: Arc<BackgroundExecutor>,
    limits: IssueDetailLimits,
    avatars: PageAvatars,
    image_provider: Option<Arc<dyn ImageProvider>>,
    on_action: Option<OnIssueDetailAction>,

    inbox: Arc<Mutex<VecDeque<Reply>>>,
    drain_task: Option<Task<()>>,

    body_reader: Option<CachedReader>,
    comment_readers: HashMap<u64, CachedReader>,

    focus: FocusHandle,
    scroll: ScrollHandle,
    motion: Motion<ScrollHandle>,
    bars: Bars,
    clock: Instant,
    active: bool,
    now: SystemTime,
    fixed_now: bool,
    time_task: Option<Task<()>>,

    _activation: Subscription,
    _visibility: Subscription,
}

impl IssueDetailView {
    pub fn new(
        repos: RepoService,
        executor: Arc<BackgroundExecutor>,
        locator: IssueLocator,
        avatars: PageAvatars,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, IssueDetailError> {
        let session = repos.issue_detail_session()?;
        let scroll = ScrollHandle::new();
        let activation = cx.observe_window_activation(window, |view, _, cx| {
            scroll::cancel_region(&mut view.bars, &view.motion);
            view.cancel_reader_interactions(cx);
            cx.notify();
        });
        let visibility = cx.observe_window_visibility(window, |view, visibility, _, cx| {
            if !visibility.is_visible() {
                view.cancel_activity(cx);
            }
            cx.notify();
        });
        let mut view = Self {
            controller: Controller::new(session, locator),
            repos,
            executor,
            limits: IssueDetailLimits::default(),
            avatars,
            image_provider: None,
            on_action: None,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            drain_task: None,
            body_reader: None,
            comment_readers: HashMap::new(),
            focus: cx.focus_handle().tab_stop(true),
            motion: Motion::new(scroll.clone()),
            scroll,
            bars: Bars::new(),
            clock: Instant::now(),
            active: true,
            now: SystemTime::now(),
            fixed_now: false,
            time_task: None,
            _activation: activation,
            _visibility: visibility,
        };
        view.request_issue(true, cx);
        Ok(view)
    }

    pub fn on_action(&mut self, callback: OnIssueDetailAction) {
        self.on_action = Some(callback);
    }

    /// Attach an account-scoped [`ImageProvider`] to the issue body and comment readers.
    pub fn set_image_provider(
        &mut self,
        provider: Option<Arc<dyn ImageProvider>>,
        cx: &mut Context<Self>,
    ) {
        self.image_provider = provider.clone();
        if let Some(reader) = &self.body_reader {
            reader.view.update(cx, |reader, cx| {
                reader.set_image_provider(provider.clone(), cx)
            });
        }
        for reader in self.comment_readers.values() {
            reader.view.update(cx, |reader, cx| {
                reader.set_image_provider(provider.clone(), cx)
            });
        }
    }

    #[allow(dead_code)]
    pub fn set_limits(&mut self, limits: IssueDetailLimits) {
        self.limits = limits;
    }

    #[allow(dead_code)]
    pub fn set_now(&mut self, now: Option<SystemTime>, cx: &mut Context<Self>) {
        self.fixed_now = now.is_some();
        self.now = now.unwrap_or_else(SystemTime::now);
        self.time_task = None;
        cx.notify();
    }

    pub fn controller_is_current(&self) -> bool {
        self.controller.current()
    }

    pub fn is_refreshing(&self) -> bool {
        self.controller.is_busy()
    }

    /// Revalidate issue detail and page-1 comments while preserving currently displayed content.
    pub fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.active || !self.controller.current() || self.controller.is_busy() {
            return false;
        }
        self.request_issue(false, cx)
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if !active {
            self.shutdown_activity(cx);
        } else {
            if !self.controller.issue_loaded && !self.controller.issue_busy {
                self.request_issue(self.controller.issue.is_none(), cx);
            } else if self.controller.comments_busy.is_none()
                && (!self.controller.comments_loaded
                    || (self.controller.cached_comments_partial.is_some()
                        && self.controller.comments_error.is_none()))
            {
                self.request_comments(CommentsRequestMode::Replace, cx);
            }
        }
        cx.notify();
    }

    fn shutdown_activity(&mut self, cx: &mut Context<Self>) {
        self.controller.cancel_all();
        self.drain_task = None;
        self.inbox.lock().expect("issue detail inbox").clear();
        self.cancel_activity(cx);
    }

    fn cancel_activity(&mut self, cx: &mut Context<Self>) {
        self.time_task = None;
        scroll::cancel_region(&mut self.bars, &self.motion);
        self.cancel_reader_interactions(cx);
    }

    fn cancel_reader_interactions(&self, cx: &mut Context<Self>) {
        if let Some(reader) = &self.body_reader {
            reader
                .view
                .update(cx, |reader, cx| reader.cancel_interactions(cx));
        }
        for reader in self.comment_readers.values() {
            reader
                .view
                .update(cx, |reader, cx| reader.cancel_interactions(cx));
        }
    }

    fn sync_time_task(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.active || !window.is_window_active() || !window.is_visible() {
            self.time_task = None;
            return;
        }
        if self.fixed_now
            || (self.controller.issue.is_none() && self.controller.comments.is_empty())
        {
            self.time_task = None;
            return;
        }
        if self.time_task.is_none() {
            self.now = SystemTime::now();
            self.time_task = Some(cx.spawn(async |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(30))
                        .await;
                    if this
                        .update(cx, |view, cx| {
                            view.now = SystemTime::now();
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
    }

    pub(super) fn request_issue(&mut self, read_cache: bool, cx: &mut Context<Self>) -> bool {
        if !self.active {
            return false;
        }
        let Some(request) = self.controller.begin_issue() else {
            return false;
        };
        self.dispatch_issue(request, read_cache, cx);
        true
    }

    pub(super) fn request_comments(
        &mut self,
        mode: CommentsRequestMode,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.active || self.controller.issue_busy || self.controller.issue.is_none() {
            return false;
        }
        let Some(request) = self.controller.begin_comments(mode) else {
            return false;
        };
        self.controller.prepare_comment_display();
        self.dispatch_comments(request, cx);
        true
    }

    fn ensure_drain_task(&mut self, cx: &mut Context<Self>) {
        if self.drain_task.is_some() {
            return;
        }
        self.drain_task = Some(cx.spawn(async |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(15))
                    .await;
                let keep_running = this
                    .update(cx, |view, cx| {
                        view.drain_inbox(cx);
                        let has_pending = view.controller.is_busy()
                            || !view.inbox.lock().expect("issue detail inbox").is_empty();
                        if !has_pending {
                            view.drain_task = None;
                        }
                        has_pending
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        }));
    }

    fn drain_inbox(&mut self, cx: &mut Context<Self>) {
        if self.apply_replies(cx) {
            cx.notify();
        }
    }

    fn apply_replies(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = self.controller.enforce_cache_authorization();
        let replies: Vec<_> = self
            .inbox
            .lock()
            .expect("issue detail inbox")
            .drain(..)
            .collect();
        for reply in replies {
            changed |= match reply {
                Reply::Cached { request, snapshot } => {
                    self.controller.apply_cached_reading(&request, snapshot)
                }
                Reply::Issue { request, refresh } => {
                    let changed = self.controller.finish_issue(&request, refresh);
                    if changed && self.controller.issue.is_some() {
                        let weak = cx.weak_entity();
                        cx.defer(move |cx| {
                            let _ = weak.update(cx, |view, cx| {
                                view.request_comments(CommentsRequestMode::Replace, cx);
                            });
                        });
                    }
                    changed
                }
                Reply::Comments { request, refresh } => {
                    self.controller.finish_issue_comments(&request, refresh)
                }
            };
        }
        changed | self.controller.enforce_cache_authorization()
    }

    fn dispatch_issue(&mut self, request: IssueRequest, read_cache: bool, cx: &mut Context<Self>) {
        let online = request.clone();
        let limits = self.limits.clone();
        let inbox = self.inbox.clone();
        let spawned = self.executor.handle().spawn(async move {
            // Publish both sections from the same cached read before starting revalidation.
            if read_cache && let Some(snapshot) = online.execute_cache(&limits).await {
                inbox
                    .lock()
                    .expect("issue detail inbox")
                    .push_back(Reply::Cached {
                        request: online.clone(),
                        snapshot,
                    });
            }
            let refresh = online.execute_online(&limits).await;
            inbox
                .lock()
                .expect("issue detail inbox")
                .push_back(Reply::Issue {
                    request: online,
                    refresh,
                });
        });
        if spawned.is_err() {
            self.controller
                .finish_issue(&request, executor_unavailable());
        } else {
            self.ensure_drain_task(cx);
        }
        cx.notify();
    }

    fn dispatch_comments(&mut self, request: CommentsRequest, cx: &mut Context<Self>) {
        let online = request.clone();
        let limits = self.limits.clone();
        let inbox = self.inbox.clone();
        let spawned = self.executor.handle().spawn(async move {
            let refresh = online.execute_online(&limits).await;
            inbox
                .lock()
                .expect("issue detail inbox")
                .push_back(Reply::Comments {
                    request: online,
                    refresh,
                });
        });
        if spawned.is_err() {
            self.controller
                .finish_issue_comments(&request, executor_unavailable());
        } else {
            self.ensure_drain_task(cx);
        }
        cx.notify();
    }

    pub(super) fn emit(&self, action: IssueDetailAction, window: &mut Window, cx: &mut App) {
        if !self.active || !self.controller.current() || !self.controller.reading_is_authorized() {
            return;
        }
        let context_ok = match &action {
            IssueDetailAction::Repository { context, .. }
            | IssueDetailAction::Author { context, .. } => self.repos.is_current(context),
            IssueDetailAction::External(_) => true,
        };
        if context_ok && let Some(callback) = &self.on_action {
            callback(action, window, cx);
        }
    }

    /// Synchronize Flow Markdown [`ReaderView`] entities with the current issue body and comments.
    /// Existing readers with unchanged content digests are kept intact so background refreshes and
    /// comment pagination never flicker or lose text selection.
    fn sync_readers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.controller.locator.owner().to_string();
        let name = self.controller.locator.name().to_string();

        match self
            .controller
            .issue
            .as_ref()
            .and_then(|issue| issue.body.as_deref())
        {
            Some(text) if !text.trim().is_empty() => {
                let digest = body_digest(text);
                let needs_rebuild = self
                    .body_reader
                    .as_ref()
                    .is_none_or(|cached| cached.digest != digest);
                if needs_rebuild {
                    let view =
                        self.build_markdown_reader(&owner, &name, "issue.md", text, window, cx);
                    self.body_reader = Some(CachedReader { digest, view });
                }
            }
            _ => {
                self.body_reader = None;
            }
        }

        let live_ids: std::collections::HashSet<u64> =
            self.controller.comments.iter().map(|c| c.id).collect();
        self.comment_readers.retain(|id, _| live_ids.contains(id));

        for comment in &self.controller.comments {
            let Some(text) = comment.body.as_deref().filter(|t| !t.trim().is_empty()) else {
                self.comment_readers.remove(&comment.id);
                continue;
            };
            let digest = body_digest(text);
            let needs_rebuild = self
                .comment_readers
                .get(&comment.id)
                .is_none_or(|cached| cached.digest != digest);
            if needs_rebuild {
                let path = format!("comment-{}.md", comment.id);
                let view = self.build_markdown_reader(&owner, &name, &path, text, window, cx);
                self.comment_readers
                    .insert(comment.id, CachedReader { digest, view });
            }
        }
    }

    fn build_markdown_reader(
        &self,
        owner: &str,
        name: &str,
        path: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ReaderView> {
        let has_bom = text.starts_with('\u{feff}');
        let body = PreparedBody::Utf8 {
            text: text.to_string(),
            has_bom,
        };
        let context = ResourceContext::new(owner, name, "", path);
        let owner_entity = cx.entity().downgrade();
        let link_owner = cx.entity().downgrade();
        let image_provider = self.image_provider.clone();
        cx.new(|cx| {
            let mut reader = ReaderView::new(body, DocumentKind::Markdown, context, window, cx);
            reader.set_markdown_layout(MarkdownLayout::Flow, cx);
            reader.set_image_provider(image_provider, cx);
            let reader_weak = cx.entity().downgrade();
            reader.on_layout_changed(Rc::new(move |_, cx| {
                if let Some(view) = reader_weak.upgrade() {
                    view.update(cx, |_, cx| cx.notify());
                }
                if let Some(owner) = owner_entity.upgrade() {
                    owner.update(cx, |_, cx| cx.notify());
                }
            }));
            reader.on_action(Rc::new(move |action, window, cx| {
                let target_url = match action {
                    ReaderAction::Link(LinkTarget::External { url }) => Some(url),
                    ReaderAction::Link(LinkTarget::Repository(link)) => {
                        resolve_relative_issue_link(
                            &link.owner,
                            &link.name,
                            &link.path,
                            link.anchor.as_deref(),
                        )
                    }
                    ReaderAction::Link(_) => None,
                };
                if let Some(url) = target_url
                    && let Some(owner) = link_owner.upgrade()
                {
                    owner.update(cx, |view, cx| {
                        view.emit(IssueDetailAction::External(url), window, cx);
                    });
                }
            }));
            reader
        })
    }

    pub(super) fn avatar_key(
        author: &crate::domain::issue_detail::IssueAuthor,
    ) -> Option<AvatarKey> {
        let id = author.id?;
        let url = author.avatar_url.clone()?;
        Some(AvatarKey::new("github.com", id, url))
    }
}

fn executor_unavailable<T>() -> Refresh<T> {
    Refresh {
        result: Err(GitHubError::Network {
            message: "request executor unavailable".into(),
        }
        .into()),
        no_store: false,
    }
}

fn body_digest(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Resolve a repository-relative link in work-item Markdown to GitHub's `HEAD` view.
pub(super) fn resolve_relative_issue_link(
    owner: &str,
    name: &str,
    path: &str,
    anchor: Option<&str>,
) -> Option<String> {
    let mut url = reqwest::Url::parse("https://github.com").ok()?;
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.push(owner);
        segments.push(name);
        segments.push("blob");
        segments.push("HEAD");
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            segments.push(segment);
        }
    }
    if let Some(anchor) = anchor.filter(|a| !a.is_empty()) {
        url.set_fragment(Some(anchor));
    }
    Some(url.to_string())
}

impl Focusable for IssueDetailView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for IssueDetailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_replies(cx);
        self.sync_time_task(window, cx);
        let current = self.controller.current();
        if !current {
            scroll::cancel_region(&mut self.bars, &self.motion);
            self.image_provider = None;
            self.body_reader = None;
            self.comment_readers.clear();
        } else {
            self.sync_readers(window, cx);
        }
        if self.motion.tick(Instant::now(), cx.reduce_motion()) {
            window.request_animation_frame();
        }
        let now_ms = self.clock.elapsed().as_millis() as u64;
        if self.bars.dragging().is_some() && !cx.has_active_drag() {
            self.bars.settle(&self.motion, now_ms, cx.reduce_motion());
        }
        let opacity = self.bars.opacity(now_ms);
        if self.bars.is_animating() {
            window.request_animation_frame();
        }
        self.render_page(current, opacity, cx)
    }
}
