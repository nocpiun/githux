//! Production single-Issue detail host. It joins the account-scoped issue/comments reader to
//! [`IssueDetailView`] and routes repository, author profile and external links through Shell.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui::{AnyElement, AppContext, Context, Entity, IntoElement, Render, Subscription, Window};

use crate::app::accounts::AccountServices;
use crate::content::images::{
    ImageFailure, ImageRenderOptions, ImageRequest, ImageScope, ImageSink, ImageTarget,
};
use crate::data::reader_images::RepositoryImageService;
use crate::data::{AccountCache, RepoService, RepositorySession};
use crate::domain::issue_detail::IssueLocator;
use crate::domain::repo::RepoVisibility;
use crate::domain::repository::{RepositoryIdentity, RepositoryLocator, RevisionSpec};
use crate::github::RepositoryLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::network::executor::BackgroundExecutor;
use crate::network::reader_images::{ExternalImageBytes, ReqwestExternalImages};
use crate::ui::issue_detail::{IssueDetailAction, IssueDetailView};
use crate::ui::reader::{ImageLimits, ImageProvider};
use crate::workspace::Route;

use super::{NavigationSink, PageAvatars, PageNavigation};

pub struct IssueDetailPage {
    view: Option<Entity<IssueDetailView>>,
    _changed: Option<Subscription>,
}

impl IssueDetailPage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        namespace: &str,
        name: &str,
        number: u64,
        services: Option<AccountServices>,
        navigation: NavigationSink,
        avatars: PageAvatars,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            view: None,
            _changed: None,
        };
        let Some(services) = services else {
            return this;
        };
        let Ok(locator) = IssueLocator::new(namespace, name, number) else {
            return this;
        };
        let repos = services.repositories();
        if repos.issue_detail_session().is_err() {
            return this;
        }
        let active_account = services.accounts().active_summary();
        let image_provider: Option<Arc<dyn ImageProvider>> =
            ReqwestExternalImages::new().ok().and_then(|external| {
                let session = repos.session().ok()?;
                Some(Arc::new(IssueImageProvider::new(
                    repos.clone(),
                    session,
                    locator.repository().clone(),
                    services.executor().clone(),
                    Arc::new(external) as Arc<dyn ExternalImageBytes>,
                )) as Arc<dyn ImageProvider>)
            });
        let view = cx.new(|cx| {
            let mut view = IssueDetailView::new(
                repos.clone(),
                services.executor().clone(),
                locator,
                avatars,
                window,
                cx,
            )
            .expect("the account session was verified before construction");
            view.set_image_provider(image_provider, cx);
            view
        });
        view.update(cx, |view, _| {
            let on_action: crate::ui::issue_detail::OnIssueDetailAction =
                Rc::new(move |action, window, cx| match action {
                    IssueDetailAction::Repository {
                        context,
                        repository,
                    } => {
                        if !repos.is_current(&context) {
                            return;
                        }
                        navigation(
                            PageNavigation::Open(Route::Repository {
                                namespace: repository.owner().to_string(),
                                name: repository.name().to_string(),
                            }),
                            window,
                            cx,
                        );
                    }
                    IssueDetailAction::Author { context, login } => {
                        if !repos.is_current(&context) {
                            return;
                        }
                        navigation(
                            PageNavigation::Open(Route::profile(login, active_account.as_ref())),
                            window,
                            cx,
                        );
                    }
                    IssueDetailAction::External(url) => {
                        navigation(PageNavigation::External(url), window, cx);
                    }
                });
            view.on_action(on_action);
        });
        this._changed = Some(cx.observe(&view, |_, _, cx| cx.notify()));
        this.view = Some(view);
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.set_active(active, cx));
        }
    }

    pub(crate) fn refresh_state(&self, cx: &gpui::App) -> crate::ui::shell_refresh::RefreshState {
        let Some(view) = &self.view else {
            return crate::ui::shell_refresh::RefreshState::disabled("Sign in to refresh Issue");
        };
        let view = view.read(cx);
        if !view.controller_is_current() {
            crate::ui::shell_refresh::RefreshState::disabled("Sign in to refresh Issue")
        } else if view.is_refreshing() {
            crate::ui::shell_refresh::RefreshState::refreshing("Refreshing Issue")
        } else {
            crate::ui::shell_refresh::RefreshState::ready("Refresh Issue")
        }
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(view) = &self.view else {
            return false;
        };
        view.update(cx, |view, cx| view.refresh(cx))
    }
}

impl Render for IssueDetailPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(view) = self.view.clone() {
            return view.into_any_element();
        }
        placeholder(
            "Sign in to read this issue. No saved issue content is available.",
            cx,
        )
    }
}

fn placeholder<E: 'static>(message: &str, cx: &mut Context<E>) -> AnyElement {
    use ely_core::theme::{ActiveTheme, TextSize};
    use gpui::{ParentElement, Styled, div, prelude::*};
    div()
        .debug_selector(|| "issue-detail-page-unavailable".to_string())
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_6()
        .text_align(gpui::TextAlign::Center)
        .text_size(cx.theme().text_size(TextSize::Sm))
        .text_color(cx.theme().colors.fg_subtle)
        .child(message.to_string())
        .into_any_element()
}

/// Account-scoped image provider for Issue bodies and comments.
///
/// External `https://` images go directly to [`RepositoryImageService`] without extra GitHub API
/// calls. Repository-relative images (which carry an empty commit in issue Markdown) lazily
/// resolve the repository's default branch HEAD commit and real `RepositoryIdentity` once, then
/// delegate to a properly scoped [`RepositoryImageService`] with that pinned SHA.
pub(super) struct IssueImageProvider {
    repos: RepoService,
    session: RepositorySession,
    locator: RepositoryLocator,
    executor: Arc<BackgroundExecutor>,
    external: Arc<dyn ExternalImageBytes>,
    external_service: RepositoryImageService,
    resolved_repo: Arc<tokio::sync::OnceCell<(String, RepositoryImageService)>>,
    pending: Mutex<HashMap<ImageScope, Vec<CancellationTokenSource>>>,
}

impl IssueImageProvider {
    pub(super) fn new(
        repos: RepoService,
        session: RepositorySession,
        locator: RepositoryLocator,
        executor: Arc<BackgroundExecutor>,
        external: Arc<dyn ExternalImageBytes>,
    ) -> Self {
        let context = session.context().clone();
        let cache = repos.cache(&context).map(|account_cache: AccountCache| {
            let (store, scope) = account_cache.into_parts();
            (store, scope, false)
        });
        let external_service = RepositoryImageService::with_cache(
            session.clone(),
            RepositoryIdentity {
                id: 0,
                locator: locator.clone(),
            },
            executor.clone(),
            external.clone(),
            ImageLimits::default(),
            cache,
        );
        Self {
            repos,
            session,
            locator,
            executor,
            external,
            external_service,
            resolved_repo: Arc::new(tokio::sync::OnceCell::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }
}

impl ImageProvider for IssueImageProvider {
    fn start(&self, request: ImageRequest, sink: ImageSink) {
        self.start_with_options(request, sink, ImageRenderOptions::default());
    }

    fn start_with_options(
        &self,
        mut request: ImageRequest,
        sink: ImageSink,
        options: ImageRenderOptions,
    ) {
        let needs_revision = matches!(
            &request.target,
            ImageTarget::Repository { commit, .. } if commit.is_empty()
        );
        if !needs_revision {
            self.external_service
                .start_with_options(request, sink, options);
            return;
        }
        if !self.session.verify() {
            sink.publish(
                request.scope,
                request.key,
                0,
                true,
                None,
                Err(ImageFailure::StaleContext),
            );
            return;
        }
        let (cancel, source) = CancellationToken::new();
        self.pending
            .lock()
            .expect("issue image pending lock")
            .entry(request.scope)
            .or_default()
            .push(source);

        let repos = self.repos.clone();
        let session = self.session.clone();
        let locator = self.locator.clone();
        let executor = self.executor.clone();
        let external = self.external.clone();
        let resolved_repo = self.resolved_repo.clone();

        let _ = self.executor.handle().spawn(async move {
            let resolved = resolved_repo
                .get_or_try_init(|| async {
                    let limits = RepositoryLimits::default();
                    let details = session
                        .repository(locator.clone(), &limits, cancel.clone())
                        .await
                        .map_err(|_| ImageFailure::Network)?
                        .value;
                    let branch = details.default_branch.ok_or(ImageFailure::NotFound)?;
                    let snapshot = session
                        .revision(
                            &details.identity,
                            RevisionSpec::Ref(branch),
                            &limits,
                            cancel.clone(),
                        )
                        .await
                        .map_err(|_| ImageFailure::Network)?
                        .value;
                    let sha = snapshot.commit().as_str().to_string();
                    let is_private = details.visibility != RepoVisibility::Public;
                    let cache =
                        repos
                            .cache(session.context())
                            .map(|account_cache: AccountCache| {
                                let (store, scope) = account_cache.into_parts();
                                (store, scope, is_private)
                            });
                    let service = RepositoryImageService::with_cache(
                        session.clone(),
                        details.identity,
                        executor.clone(),
                        external.clone(),
                        ImageLimits::default(),
                        cache,
                    );
                    Ok((sha, service))
                })
                .await;

            if cancel.is_cancelled() {
                return;
            }
            if !session.verify() {
                sink.publish(
                    request.scope,
                    request.key,
                    0,
                    true,
                    None,
                    Err(ImageFailure::StaleContext),
                );
                return;
            }
            let Ok((sha, service)) = resolved else {
                sink.publish(
                    request.scope,
                    request.key,
                    0,
                    true,
                    None,
                    Err(resolved.err().unwrap_or(ImageFailure::NotFound)),
                );
                return;
            };
            if let ImageTarget::Repository { commit, .. } = &mut request.target {
                *commit = sha.clone();
            }
            service.start_with_options(request, sink, options);
        });
    }

    fn cancel(&self, scope: ImageScope) {
        if let Some(sources) = self
            .pending
            .lock()
            .expect("issue image pending lock")
            .remove(&scope)
        {
            for source in sources {
                source.cancel();
            }
        }
        self.external_service.cancel(scope);
        if let Some((_, service)) = self.resolved_repo.get() {
            service.cancel(scope);
        }
    }
}
