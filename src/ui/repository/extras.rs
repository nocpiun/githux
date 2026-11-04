//! Repository-scoped overview composition. Branch changes never restart these reads.
use super::{PageAction, RepositoryPage, Work, format_sync_age};
use crate::data::page_cache::{
    CachedPageReadSession, PageCacheError, PageOrigin, PageReadMode, PageValue,
};
use crate::data::{PageReadSession, RepositoryReadContext};
use crate::domain::cache::CacheCompleteness;
use crate::domain::error::GitHubError;
use crate::domain::profile::ProfileError;
use crate::domain::repository::RepositoryIdentity;
use crate::domain::repository_extras::{ExtrasCompleteness, RepositoryExtrasError};
use crate::github::RepositoryExtrasLimits;
use crate::ui::repository_overview::{
    OverviewAction, OverviewBinding, OverviewSection, OverviewUpdate, ReleasesData,
    RepositoryOverview, SectionState,
};
use gpui::{AppContext, Context, Entity, Window};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug)]
enum Endpoint {
    Releases,
    Latest,
    Contributors,
    Languages,
}
impl Endpoint {
    fn index(self) -> usize {
        match self {
            Self::Releases => 0,
            Self::Latest => 1,
            Self::Contributors => 2,
            Self::Languages => 3,
        }
    }
}
struct Reply {
    binding: OverviewBinding,
    context: RepositoryReadContext,
    endpoint: Endpoint,
    request: u64,
    update: OverviewUpdate,
}
struct CountsReply {
    binding: OverviewBinding,
    context: RepositoryReadContext,
    request: u64,
    result: Result<
        (crate::domain::repository_counts::RepositoryCounts, bool),
        crate::domain::repository_counts::CountsError,
    >,
}
#[derive(Default)]
pub(super) struct Extras {
    pub view: Option<Entity<RepositoryOverview>>,
    session: Option<PageReadSession>,
    cached: Option<CachedPageReadSession>,
    binding: Option<OverviewBinding>,
    requests: [u64; 4],
    work: [Option<Work>; 4],
    inbox: Arc<Mutex<VecDeque<Reply>>>,
    pub counts: Option<crate::domain::repository_counts::RepositoryCounts>,
    pub counts_historical: bool,
    counts_work: Option<Work>,
    counts_inbox: Arc<Mutex<VecDeque<CountsReply>>>,
    counts_request: u64,
}
impl Extras {
    pub fn stop(&mut self) {
        for work in &mut self.work {
            if let Some(work) = work.take() {
                work.source.cancel();
            }
        }
        if let Some(work) = self.counts_work.take() {
            work.source.cancel();
        }
        self.counts = None;
        self.counts_historical = false;
        self.counts_request += 1;
        self.view = None;
        self.session = None;
        self.cached = None;
        self.binding = None;
        self.inbox.lock().expect("overview inbox").clear();
    }
    fn accepts(&self, page: &RepositoryPage, binding: &OverviewBinding) -> bool {
        page.active
            && self.binding.as_ref() == Some(binding)
            && binding.generation == page.generation
            && page
                .details
                .as_ref()
                .is_some_and(|details| details.identity == binding.repository)
            && self.session.as_ref().is_some_and(|session| {
                Some(session.context()) == page.context.as_ref() && session.verify()
            })
            && page.cache_allowed()
    }
}
impl RepositoryPage {
    pub(super) fn start_overview(
        &mut self,
        repository: RepositoryIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extras.stop();
        let binding = OverviewBinding::new(
            self.context
                .as_ref()
                .expect("repository context")
                .auth()
                .account
                .clone(),
            repository,
            self.generation,
        );
        let view = cx.new(|cx| RepositoryOverview::new(binding.clone(), window, cx));
        let owner = cx.weak_entity();
        view.update(cx, |view, _| {
            view.on_action(std::rc::Rc::new(move |action, window, cx| {
                let owner = owner.clone();
                window.defer(cx, move |window, cx| {
                    let _ = owner.update(cx, |page, cx| page.overview_action(action, window, cx));
                });
            }))
        });
        self.extras.view = Some(view);
        self.extras.binding = Some(binding.clone());
        let session = self.services.accounts.page_session();
        match session {
            Ok(session) if Some(session.context()) == self.context.as_ref() && session.verify() => {
                self.extras.cached = self
                    .services
                    .accounts
                    .repo_service()
                    .cache(session.context())
                    .and_then(|cache| {
                        let (store, scope) = cache.into_parts();
                        CachedPageReadSession::new(session.clone(), store, scope).ok()
                    });
                self.extras.session = Some(session);
            }
            _ => {
                if let Some(view) = &self.extras.view {
                    view.update(cx, |view, cx| {
                        let message =
                            "Authorization changed. Activate this page again.".to_string();
                        view.apply(
                            &binding,
                            OverviewUpdate::Releases(SectionState::Error {
                                message: message.clone(),
                            }),
                            cx,
                        );
                        view.apply(
                            &binding,
                            OverviewUpdate::Contributors(SectionState::Error {
                                message: message.clone(),
                            }),
                            cx,
                        );
                        view.apply(
                            &binding,
                            OverviewUpdate::Languages(SectionState::Error { message }),
                            cx,
                        );
                    });
                }
                self.check_authorization(cx);
                return;
            }
        }
        let offline_only = self
            .details_provenance
            .as_ref()
            .is_some_and(|prov| prov.origin == crate::data::RepositoryCacheOrigin::Snapshot);
        self.read_counts(offline_only);
        for endpoint in [
            Endpoint::Releases,
            Endpoint::Latest,
            Endpoint::Contributors,
            Endpoint::Languages,
        ] {
            self.read_extra(endpoint, offline_only, cx);
        }
    }
    fn read_counts(&mut self, offline_only: bool) {
        let Some(session) = self.extras.session.clone() else {
            return;
        };
        let cached = self.extras.cached.clone();
        let binding = self.extras.binding.clone().unwrap();
        let context = session.context().clone();
        let inbox = self.extras.counts_inbox.clone();
        self.extras.counts_request += 1;
        let request = self.extras.counts_request;
        let work = Work::new();
        let cancel = work.token();
        self.extras.counts_work = Some(work);
        let _ = self.services.executor.handle().spawn(async move {
            let result = match cached {
                Some(cached) => {
                    read_cached_counts(&cached, &binding.repository, offline_only, cancel).await
                }
                None if offline_only => {
                    Err(crate::domain::repository_counts::CountsError::RepositoryIdentity)
                }
                None => session
                    .repository_counts(&binding.repository, cancel)
                    .await
                    .map(|value| (value.value, false)),
            };
            inbox.lock().expect("count inbox").push_back(CountsReply {
                binding,
                context,
                request,
                result,
            });
        });
    }
    fn read_extra(&mut self, endpoint: Endpoint, offline_only: bool, cx: &mut Context<Self>) {
        let Some(binding) = self.extras.binding.clone() else {
            return;
        };
        if !self.extras.accepts(self, &binding) {
            return;
        }
        let index = endpoint.index();
        if let Some(work) = self.extras.work[index].take() {
            work.source.cancel();
        }
        self.extras.requests[index] += 1;
        let request = self.extras.requests[index];
        let work = Work::new();
        let cancel = work.token();
        self.extras.work[index] = Some(work);
        let loading = match endpoint {
            Endpoint::Releases => OverviewUpdate::Releases(SectionState::Loading),
            Endpoint::Latest => OverviewUpdate::LatestRelease(SectionState::Loading),
            Endpoint::Contributors => OverviewUpdate::Contributors(SectionState::Loading),
            Endpoint::Languages => OverviewUpdate::Languages(SectionState::Loading),
        };
        self.extras.view.as_ref().unwrap().update(cx, |view, cx| {
            view.apply(&binding, loading, cx);
        });
        let session = self.extras.session.clone().unwrap();
        let cached = self.extras.cached.clone();
        let context = session.context().clone();
        let inbox = self.extras.inbox.clone();
        let alive = self.alive.clone();
        let repository = binding.repository.clone();
        let submitted = self.services.executor.handle().spawn(async move {
            let limits = RepositoryExtrasLimits::default();
            let update = match cached {
                Some(cached) => {
                    read_cached_extra(
                        &cached,
                        endpoint,
                        &repository,
                        &limits,
                        offline_only,
                        cancel,
                    )
                    .await
                }
                None if offline_only => {
                    let message = "Cached overview is unavailable while offline.".to_string();
                    match endpoint {
                        Endpoint::Releases => {
                            OverviewUpdate::Releases(SectionState::Error { message })
                        }
                        Endpoint::Latest => {
                            OverviewUpdate::LatestRelease(SectionState::Error { message })
                        }
                        Endpoint::Contributors => {
                            OverviewUpdate::Contributors(SectionState::Error { message })
                        }
                        Endpoint::Languages => {
                            OverviewUpdate::Languages(SectionState::Error { message })
                        }
                    }
                }
                None => read_online_extra(&session, endpoint, &repository, &limits, cancel).await,
            };
            if *alive.borrow() {
                inbox.lock().expect("overview inbox").push_back(Reply {
                    binding,
                    context,
                    endpoint,
                    request,
                    update,
                });
            }
        });
        if submitted.is_err() {
            let message = "Background services are closing.".to_string();
            let update = match endpoint {
                Endpoint::Releases => OverviewUpdate::Releases(SectionState::Error { message }),
                Endpoint::Latest => OverviewUpdate::LatestRelease(SectionState::Error { message }),
                Endpoint::Contributors => {
                    OverviewUpdate::Contributors(SectionState::Error { message })
                }
                Endpoint::Languages => OverviewUpdate::Languages(SectionState::Error { message }),
            };
            let binding = self.extras.binding.as_ref().unwrap();
            self.extras.view.as_ref().unwrap().update(cx, |view, cx| {
                view.apply(binding, update, cx);
            });
        }
    }
    pub(super) fn drain_overview(&mut self, cx: &mut Context<Self>) {
        let counts: Vec<_> = self
            .extras
            .counts_inbox
            .lock()
            .expect("count inbox")
            .drain(..)
            .collect();
        for reply in counts {
            if reply.request == self.extras.counts_request
                && self.extras.accepts(self, &reply.binding)
                && self.context.as_ref() == Some(&reply.context)
                && self
                    .services
                    .accounts
                    .repo_service()
                    .is_current(&reply.context)
            {
                match reply
                    .result
                    .ok()
                    .filter(|(value, _)| value.repository == reply.binding.repository)
                {
                    // Historical counts are withheld rather than drawn
                    // identically to freshly verified OPEN counts: the tab badges
                    // have no per-count provenance marker, so a stale number
                    // would read as current.
                    Some((_, true)) => {
                        self.extras.counts = None;
                        self.extras.counts_historical = true;
                    }
                    Some((value, false)) => {
                        self.extras.counts = Some(value);
                        self.extras.counts_historical = false;
                    }
                    None => {
                        self.extras.counts = None;
                        self.extras.counts_historical = false;
                    }
                }
                cx.notify();
            }
        }
        let replies: Vec<_> = self
            .extras
            .inbox
            .lock()
            .expect("overview inbox")
            .drain(..)
            .collect();
        for reply in replies {
            if self.extras.requests[reply.endpoint.index()] != reply.request
                || !self.extras.accepts(self, &reply.binding)
                || self.context.as_ref() != Some(&reply.context)
                || !self
                    .services
                    .accounts
                    .repo_service()
                    .is_current(&reply.context)
            {
                continue;
            }
            self.extras.view.as_ref().unwrap().update(cx, |view, cx| {
                view.apply(&reply.binding, reply.update, cx);
            });
            // The language strip is composed by the page from this same state.
            cx.notify();
        }
    }
    pub(super) fn overview_action(
        &mut self,
        action: OverviewAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let binding = match &action {
            OverviewAction::Refresh { binding, .. }
            | OverviewAction::OpenRelease { binding, .. }
            | OverviewAction::OpenContributor { binding, .. } => binding,
        };
        if !self.extras.accepts(self, binding) {
            return;
        }
        match action {
            OverviewAction::Refresh { section, .. } => match section {
                OverviewSection::Releases => {
                    self.read_extra(Endpoint::Releases, false, cx);
                    self.read_extra(Endpoint::Latest, false, cx);
                }
                OverviewSection::Contributors => self.read_extra(Endpoint::Contributors, false, cx),
                OverviewSection::Languages => self.read_extra(Endpoint::Languages, false, cx),
            },
            action => {
                let Some(view) = self.extras.view.as_ref() else {
                    return;
                };
                let view = view.read(cx);
                let valid = match &action {
                    OverviewAction::OpenRelease {
                        release_id,
                        tag_name,
                        html_url,
                        ..
                    } => view
                        .latest_release()
                        .and_then(SectionState::value)
                        .and_then(|latest| latest.release.as_ref())
                        .is_some_and(|release| {
                            release.id == *release_id
                                && release.tag_name == *tag_name
                                && release.html_url == *html_url
                        }),
                    OverviewAction::OpenContributor {
                        login, html_url, ..
                    } => view.contributors().value().is_some_and(|value| {
                        value.contributors.iter().any(|person| {
                            person.login == *login
                                && person.html_url == *html_url
                                && person.html_url.is_some()
                        })
                    }),
                    _ => false,
                };
                if valid {
                    self.emit(PageAction::Overview(action), window, cx);
                }
            }
        }
    }
    pub(super) fn repository_binding(&self) -> Option<OverviewBinding> {
        self.extras
            .binding
            .clone()
            .filter(|binding| self.extras.accepts(self, binding))
    }
    pub fn overview(&self) -> Option<&Entity<RepositoryOverview>> {
        self.extras.view.as_ref()
    }
}

fn is_transient_github(error: &GitHubError) -> bool {
    match error {
        GitHubError::Network { .. }
        | GitHubError::Timeout { .. }
        | GitHubError::RateLimited { .. } => true,
        GitHubError::UnexpectedStatus { status } => *status >= 500,
        _ => false,
    }
}

fn is_transient_extras(error: &RepositoryExtrasError) -> bool {
    match error {
        RepositoryExtrasError::GitHub(inner) => is_transient_github(inner),
        _ => false,
    }
}

fn is_transient_counts(error: &crate::domain::repository_counts::CountsError) -> bool {
    match error {
        crate::domain::repository_counts::CountsError::Read(ProfileError::GitHub(inner)) => {
            is_transient_github(inner)
        }
        _ => false,
    }
}

async fn read_cached_counts(
    cached: &CachedPageReadSession,
    repository: &RepositoryIdentity,
    offline_only: bool,
    cancel: crate::network::cancel::CancellationToken,
) -> Result<
    (crate::domain::repository_counts::RepositoryCounts, bool),
    crate::domain::repository_counts::CountsError,
> {
    if offline_only {
        return match cached
            .repository_counts(PageReadMode::Offline, repository, cancel)
            .await
        {
            Ok(scoped) => match scoped.value {
                PageValue::Value { value, .. } => Ok((value, true)),
                PageValue::Miss(_) => {
                    Err(crate::domain::repository_counts::CountsError::RepositoryIdentity)
                }
            },
            Err(_) => Err(crate::domain::repository_counts::CountsError::RepositoryIdentity),
        };
    }
    match cached
        .repository_counts(PageReadMode::Online, repository, cancel.clone())
        .await
    {
        Ok(scoped) => match scoped.value {
            PageValue::Value { value, origin, .. } => {
                Ok((value, origin == PageOrigin::HistoricalSnapshot))
            }
            PageValue::Miss(_) => {
                Err(crate::domain::repository_counts::CountsError::RepositoryIdentity)
            }
        },
        Err(PageCacheError::Read(error)) if is_transient_counts(&error) => {
            match cached
                .repository_counts(PageReadMode::Offline, repository, cancel)
                .await
            {
                Ok(scoped) => match scoped.value {
                    PageValue::Value { value, .. } => Ok((value, true)),
                    PageValue::Miss(_) => Err(error),
                },
                Err(_) => Err(error),
            }
        }
        Err(PageCacheError::Read(error)) => Err(error),
        Err(_) => Err(crate::domain::repository_counts::CountsError::RepositoryIdentity),
    }
}

async fn read_online_extra(
    session: &PageReadSession,
    endpoint: Endpoint,
    repository: &RepositoryIdentity,
    limits: &RepositoryExtrasLimits,
    cancel: crate::network::cancel::CancellationToken,
) -> OverviewUpdate {
    match endpoint {
        Endpoint::Releases => {
            OverviewUpdate::Releases(match session.releases(repository, limits, cancel).await {
                Ok(value) => SectionState::Ready(ReleasesData::new(value.value, None)),
                Err(error) => SectionState::Error {
                    message: error.to_string(),
                },
            })
        }
        Endpoint::Latest => OverviewUpdate::LatestRelease(
            match session.latest_release(repository, limits, cancel).await {
                Ok(value) => SectionState::Ready(value.value),
                Err(error) => SectionState::Error {
                    message: error.to_string(),
                },
            },
        ),
        Endpoint::Contributors => OverviewUpdate::Contributors(
            match session.contributors(repository, limits, cancel).await {
                Ok(value) => SectionState::Ready(value.value),
                Err(error) => SectionState::Error {
                    message: error.to_string(),
                },
            },
        ),
        Endpoint::Languages => {
            OverviewUpdate::Languages(match session.languages(repository, limits, cancel).await {
                Ok(value) => SectionState::Ready(value.value),
                Err(error) => SectionState::Error {
                    message: error.to_string(),
                },
            })
        }
    }
}

async fn read_cached_extra(
    cached: &CachedPageReadSession,
    endpoint: Endpoint,
    repository: &RepositoryIdentity,
    limits: &RepositoryExtrasLimits,
    offline_only: bool,
    cancel: crate::network::cancel::CancellationToken,
) -> OverviewUpdate {
    if offline_only {
        return match endpoint {
            Endpoint::Releases => OverviewUpdate::Releases(
                match cached
                    .releases(PageReadMode::Offline, repository, limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value, metadata, ..
                        } => releases_state(value, PageOrigin::HistoricalSnapshot, &metadata),
                        PageValue::Miss(_) => SectionState::Error {
                            message: "Releases are not available in the local cache.".into(),
                        },
                    },
                    Err(error) => SectionState::Error {
                        message: format!("{error:?}"),
                    },
                },
            ),
            Endpoint::Latest => OverviewUpdate::LatestRelease(
                match cached
                    .latest_release(PageReadMode::Offline, repository, limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value, metadata, ..
                        } => latest_state(value, PageOrigin::HistoricalSnapshot, &metadata),
                        PageValue::Miss(_) => SectionState::Error {
                            message: "Latest release is not available in the local cache.".into(),
                        },
                    },
                    Err(error) => SectionState::Error {
                        message: format!("{error:?}"),
                    },
                },
            ),
            Endpoint::Contributors => OverviewUpdate::Contributors(
                match cached
                    .contributors(PageReadMode::Offline, repository, limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value, metadata, ..
                        } => contributors_state(value, PageOrigin::HistoricalSnapshot, &metadata),
                        PageValue::Miss(_) => SectionState::Error {
                            message: "Contributors are not available in the local cache.".into(),
                        },
                    },
                    Err(error) => SectionState::Error {
                        message: format!("{error:?}"),
                    },
                },
            ),
            Endpoint::Languages => OverviewUpdate::Languages(
                match cached
                    .languages(PageReadMode::Offline, repository, limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value, metadata, ..
                        } => languages_state(value, PageOrigin::HistoricalSnapshot, &metadata),
                        PageValue::Miss(_) => SectionState::Error {
                            message: "Languages are not available in the local cache.".into(),
                        },
                    },
                    Err(error) => SectionState::Error {
                        message: format!("{error:?}"),
                    },
                },
            ),
        };
    }
    match endpoint {
        Endpoint::Releases => {
            let online = cached
                .releases(PageReadMode::Online, repository, limits, cancel.clone())
                .await;
            let state = match online {
                Ok(scoped) => match scoped.value {
                    PageValue::Value {
                        value,
                        origin,
                        metadata,
                    } => releases_state(value, origin, &metadata),
                    PageValue::Miss(miss) => SectionState::Error {
                        message: format!("cached releases unavailable: {miss:?}"),
                    },
                },
                Err(PageCacheError::Read(error)) if is_transient_extras(&error) => match cached
                    .releases(PageReadMode::Offline, repository, limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value, metadata, ..
                        } => releases_state(value, PageOrigin::HistoricalSnapshot, &metadata),
                        PageValue::Miss(_) => SectionState::Error {
                            message: error.to_string(),
                        },
                    },
                    Err(_) => SectionState::Error {
                        message: error.to_string(),
                    },
                },
                Err(PageCacheError::Read(error)) => SectionState::Error {
                    message: error.to_string(),
                },
                Err(PageCacheError::Cache(error)) => SectionState::Error {
                    message: error.to_string(),
                },
                Err(PageCacheError::Context(error)) => SectionState::Error {
                    message: error.to_string(),
                },
            };
            OverviewUpdate::Releases(state)
        }
        Endpoint::Latest => {
            let online = cached
                .latest_release(PageReadMode::Online, repository, limits, cancel.clone())
                .await;
            let state = match online {
                Ok(scoped) => match scoped.value {
                    PageValue::Value {
                        value,
                        origin,
                        metadata,
                    } => latest_state(value, origin, &metadata),
                    PageValue::Miss(miss) => SectionState::Error {
                        message: format!("cached latest release unavailable: {miss:?}"),
                    },
                },
                Err(PageCacheError::Read(error)) if is_transient_extras(&error) => match cached
                    .latest_release(PageReadMode::Offline, repository, limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value, metadata, ..
                        } => latest_state(value, PageOrigin::HistoricalSnapshot, &metadata),
                        PageValue::Miss(_) => SectionState::Error {
                            message: error.to_string(),
                        },
                    },
                    Err(_) => SectionState::Error {
                        message: error.to_string(),
                    },
                },
                Err(PageCacheError::Read(error)) => SectionState::Error {
                    message: error.to_string(),
                },
                Err(PageCacheError::Cache(error)) => SectionState::Error {
                    message: error.to_string(),
                },
                Err(PageCacheError::Context(error)) => SectionState::Error {
                    message: error.to_string(),
                },
            };
            OverviewUpdate::LatestRelease(state)
        }
        Endpoint::Contributors => {
            let online = cached
                .contributors(PageReadMode::Online, repository, limits, cancel.clone())
                .await;
            let state = match online {
                Ok(scoped) => match scoped.value {
                    PageValue::Value {
                        value,
                        origin,
                        metadata,
                    } => contributors_state(value, origin, &metadata),
                    PageValue::Miss(miss) => SectionState::Error {
                        message: format!("cached contributors unavailable: {miss:?}"),
                    },
                },
                Err(PageCacheError::Read(error)) if is_transient_extras(&error) => match cached
                    .contributors(PageReadMode::Offline, repository, limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value, metadata, ..
                        } => contributors_state(value, PageOrigin::HistoricalSnapshot, &metadata),
                        PageValue::Miss(_) => SectionState::Error {
                            message: error.to_string(),
                        },
                    },
                    Err(_) => SectionState::Error {
                        message: error.to_string(),
                    },
                },
                Err(PageCacheError::Read(error)) => SectionState::Error {
                    message: error.to_string(),
                },
                Err(PageCacheError::Cache(error)) => SectionState::Error {
                    message: error.to_string(),
                },
                Err(PageCacheError::Context(error)) => SectionState::Error {
                    message: error.to_string(),
                },
            };
            OverviewUpdate::Contributors(state)
        }
        Endpoint::Languages => {
            let online = cached
                .languages(PageReadMode::Online, repository, limits, cancel.clone())
                .await;
            let state = match online {
                Ok(scoped) => match scoped.value {
                    PageValue::Value {
                        value,
                        origin,
                        metadata,
                    } => languages_state(value, origin, &metadata),
                    PageValue::Miss(miss) => SectionState::Error {
                        message: format!("cached languages unavailable: {miss:?}"),
                    },
                },
                Err(PageCacheError::Read(error)) if is_transient_extras(&error) => match cached
                    .languages(PageReadMode::Offline, repository, limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value, metadata, ..
                        } => languages_state(value, PageOrigin::HistoricalSnapshot, &metadata),
                        PageValue::Miss(_) => SectionState::Error {
                            message: error.to_string(),
                        },
                    },
                    Err(_) => SectionState::Error {
                        message: error.to_string(),
                    },
                },
                Err(PageCacheError::Read(error)) => SectionState::Error {
                    message: error.to_string(),
                },
                Err(PageCacheError::Cache(error)) => SectionState::Error {
                    message: error.to_string(),
                },
                Err(PageCacheError::Context(error)) => SectionState::Error {
                    message: error.to_string(),
                },
            };
            OverviewUpdate::Languages(state)
        }
    }
}

fn releases_state(
    outcome: crate::domain::repository_extras::ReleasesOutcome,
    origin: PageOrigin,
    metadata: &crate::domain::cache::CacheMetadata,
) -> crate::ui::repository_overview::ReleasesState {
    let partial_reasons = match &outcome.completeness {
        ExtrasCompleteness::Partial { reasons } => Some(reasons.clone()),
        ExtrasCompleteness::Complete => match &metadata.completeness {
            CacheCompleteness::Partial { .. } => Some(Vec::new()),
            CacheCompleteness::Complete => None,
        },
    };
    let historical = origin == PageOrigin::HistoricalSnapshot;
    let sync_label = historical.then(|| format_sync_age(metadata.verified_at_ms));
    let data =
        ReleasesData::new(outcome, None).with_provenance(historical, false, sync_label.clone());
    if historical {
        SectionState::Historical {
            value: data,
            sync_label,
            reason: partial_reasons,
        }
    } else {
        SectionState::Ready(data)
    }
}

fn latest_state(
    outcome: crate::domain::repository_extras::LatestReleaseOutcome,
    origin: PageOrigin,
    metadata: &crate::domain::cache::CacheMetadata,
) -> crate::ui::repository_overview::LatestReleaseState {
    // Notice: CachedPageReadSession marks a 404 latest release (release.is_none())
    // as CacheCompleteness::Partial in storage so a future Complete can win,
    // while online LatestReleaseOutcome is SectionState::Ready(outcome) with release=None
    // (neutral empty dot, not "Partial" text). Only historical snapshots become Historical.
    if origin == PageOrigin::HistoricalSnapshot {
        SectionState::Historical {
            value: outcome,
            sync_label: Some(format_sync_age(metadata.verified_at_ms)),
            reason: None,
        }
    } else {
        SectionState::Ready(outcome)
    }
}

fn contributors_state(
    outcome: crate::domain::repository_extras::ContributorsOutcome,
    origin: PageOrigin,
    metadata: &crate::domain::cache::CacheMetadata,
) -> crate::ui::repository_overview::ContributorsState {
    let partial_reasons = match &outcome.completeness {
        ExtrasCompleteness::Partial { reasons } => Some(reasons.clone()),
        ExtrasCompleteness::Complete => match &metadata.completeness {
            CacheCompleteness::Partial { .. } => Some(Vec::new()),
            CacheCompleteness::Complete => None,
        },
    };
    if origin == PageOrigin::HistoricalSnapshot {
        SectionState::Historical {
            value: outcome,
            sync_label: Some(format_sync_age(metadata.verified_at_ms)),
            reason: partial_reasons,
        }
    } else {
        SectionState::Ready(outcome)
    }
}

fn languages_state(
    outcome: crate::domain::repository_extras::LanguagesOutcome,
    origin: PageOrigin,
    metadata: &crate::domain::cache::CacheMetadata,
) -> crate::ui::repository_overview::LanguagesState {
    let partial_reason = match &metadata.completeness {
        CacheCompleteness::Partial { reason } => Some(reason.clone()),
        CacheCompleteness::Complete => None,
    };
    if origin == PageOrigin::HistoricalSnapshot {
        SectionState::Historical {
            value: outcome,
            sync_label: Some(format_sync_age(metadata.verified_at_ms)),
            reason: partial_reason,
        }
    } else {
        SectionState::Ready(outcome)
    }
}

impl Drop for Extras {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::repository::tests::{frames, normal};
    use gpui::TestAppContext;
    #[gpui::test]
    fn stale_count_error_does_not_replace_newer_value_or_new_authorization(
        cx: &mut TestAppContext,
    ) {
        let (fixture, page, cx) = normal(cx);
        page.update(cx, |page, cx| {
            let binding = page.extras.binding.clone().unwrap();
            let context = page.context.clone().unwrap();
            let value = crate::domain::repository_counts::RepositoryCounts {
                repository: binding.repository.clone(),
                issues: Ok(3),
                pull_requests: Ok(1),
            };
            let current = page.extras.counts_request;
            page.extras.counts = Some(value.clone());
            page.extras
                .counts_inbox
                .lock()
                .unwrap()
                .push_back(CountsReply {
                    binding,
                    context,
                    request: current - 1,
                    result: Err(crate::domain::repository_counts::CountsError::RepositoryIdentity),
                });
            page.drain_overview(cx);
            assert_eq!(page.extras.counts, Some(value));
        });
        fixture.login_as(8, "new");
        page.update_in(cx, |page, w, cx| page.drain(w, cx));
        assert!(page.read_with(cx, |page, _| page.extras.counts.is_none()));
    }
    #[gpui::test]
    fn stale_endpoint_result_and_error_cannot_overwrite_retry_or_new_binding(
        cx: &mut TestAppContext,
    ) {
        let (_fixture, page, cx) = normal(cx);
        page.update(cx, |page, cx| {
            let binding = page.extras.binding.clone().unwrap();
            let context = page.context.clone().unwrap();
            // Isolate the publication boundary from the real worker's speed:
            // a fresh worker may already finish on Linux before this assertion.
            if let Some(work) = page.extras.work[3].take() {
                work.source.cancel();
            }
            page.extras.requests[3] += 1;
            let request = page.extras.requests[3];
            page.extras.inbox.lock().unwrap().clear();
            page.extras.view.as_ref().unwrap().update(cx, |view, cx| {
                view.apply_languages(&binding, SectionState::Loading, cx);
            });
            page.extras.inbox.lock().unwrap().push_back(Reply {
                binding: binding.clone(),
                context: context.clone(),
                endpoint: Endpoint::Languages,
                request: request - 1,
                update: OverviewUpdate::Languages(SectionState::Error {
                    message: "old error".into(),
                }),
            });
            page.drain_overview(cx);
            assert!(
                page.extras
                    .view
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .languages()
                    .is_loading()
            );
            page.deactivate(cx);
            page.extras.inbox.lock().unwrap().push_back(Reply {
                binding,
                context,
                endpoint: Endpoint::Languages,
                request,
                update: OverviewUpdate::Languages(SectionState::Error {
                    message: "after deactivate".into(),
                }),
            });
            page.drain_overview(cx);
            assert!(page.extras.view.is_none());
        });
        frames(cx);
    }
}
