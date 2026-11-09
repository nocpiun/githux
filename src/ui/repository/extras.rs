//! Repository-scoped overview composition. Branch changes never restart these reads.
//!
//! The reads use the page's captured `PageContext`: the same account/page services as the rest of
//! the page, the same cache binding and the same cancel/authorization discipline. Nothing here
//! opens a database, credential or HTTP client of its own.
use super::{CapturedAuthorization, RepositoryController, Work, format_sync_age};
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
use crate::storage::cache::CacheAuthorization;
use crate::ui::repository_overview::{
    OverviewAction, OverviewBinding, OverviewUpdate, ReleasesData, RepositoryOverview, SectionState,
};
use crate::workspace::Sessions;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, atomic::Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Endpoint {
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

    fn loading(self) -> OverviewUpdate {
        match self {
            Self::Releases => OverviewUpdate::Releases(SectionState::Loading),
            Self::Latest => OverviewUpdate::LatestRelease(SectionState::Loading),
            Self::Contributors => OverviewUpdate::Contributors(SectionState::Loading),
            Self::Languages => OverviewUpdate::Languages(SectionState::Loading),
        }
    }

    fn message(self, message: String) -> OverviewUpdate {
        match self {
            Self::Releases => OverviewUpdate::Releases(SectionState::Error { message }),
            Self::Latest => OverviewUpdate::LatestRelease(SectionState::Error { message }),
            Self::Contributors => OverviewUpdate::Contributors(SectionState::Error { message }),
            Self::Languages => OverviewUpdate::Languages(SectionState::Error { message }),
        }
    }
}

struct Reply {
    binding: OverviewBinding,
    context: RepositoryReadContext,
    /// In-process cache invalidation token captured before this read.
    authorization: CapturedAuthorization,
    endpoint: Endpoint,
    request: u64,
    update: OverviewUpdate,
}

struct CountsReply {
    binding: OverviewBinding,
    context: RepositoryReadContext,
    authorization: CapturedAuthorization,
    request: u64,
    result: Result<
        (crate::domain::repository_counts::RepositoryCounts, bool),
        crate::domain::repository_counts::CountsError,
    >,
}

#[derive(Default)]
pub(super) struct Extras {
    view: Option<RepositoryOverview>,
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
    /// Token captured before the overview reads that are currently displayed, so
    /// invalidation is detected without any storage access.
    displayed_authorization: CapturedAuthorization,
}

impl Extras {
    pub(super) fn overview_mut(&mut self) -> Option<&mut RepositoryOverview> {
        self.view.as_mut()
    }

    pub(super) fn overview(&self) -> Option<&RepositoryOverview> {
        self.view.as_ref()
    }

    pub(super) fn displayed_authorization(&self) -> CapturedAuthorization {
        self.displayed_authorization.clone()
    }
}

pub(super) fn current_binding(page: &RepositoryController) -> Option<OverviewBinding> {
    page.extras.binding.clone()
}

pub(super) fn stop(page: &mut RepositoryController) {
    for work in &mut page.extras.work {
        if let Some(work) = work.take() {
            work.source.cancel();
        }
    }
    if let Some(work) = page.extras.counts_work.take() {
        work.source.cancel();
    }
    page.extras.counts = None;
    page.extras.counts_historical = false;
    page.extras.counts_request += 1;
    page.extras.displayed_authorization = None;
    page.extras.view = None;
    page.extras.session = None;
    page.extras.cached = None;
    page.extras.binding = None;
    page.extras.inbox.lock().expect("overview inbox").clear();
    page.extras
        .counts_inbox
        .lock()
        .expect("count inbox")
        .clear();
}

/// Whether the displayed overview's own cache authorization is still current.
/// Lock-free atomic loads only.
pub(super) fn authorization_current(page: &RepositoryController) -> bool {
    page.extras
        .displayed_authorization
        .as_ref()
        .is_none_or(CacheAuthorization::is_current)
}

pub(super) fn accepts(page: &RepositoryController, binding: &OverviewBinding) -> bool {
    page.active
        && page.extras.binding.as_ref() == Some(binding)
        && binding.generation == page.generation
        && page
            .details
            .as_ref()
            .is_some_and(|details| details.identity == binding.repository)
        && page.extras.session.as_ref().is_some_and(|session| {
            Some(session.context()) == page.context_data.as_ref() && session.verify()
        })
        && page.cache_allowed()
        && authorization_current(page)
}

pub(super) fn start(page: &mut RepositoryController, repository: RepositoryIdentity) {
    stop(page);
    let Some(context) = page.context_data.clone() else {
        return;
    };
    let binding = OverviewBinding::new(
        context.auth().account.clone(),
        repository.clone(),
        page.generation,
    );
    page.extras.view = Some(RepositoryOverview::new(binding.clone()));
    page.extras.binding = Some(binding.clone());
    let session = match page.context.page_session() {
        Ok(session)
            if Some(session.context()) == page.context_data.as_ref() && session.verify() =>
        {
            session
        }
        _ => {
            if let Some(view) = page.extras.view.as_mut() {
                let message = "Authorization changed. Reopen the repository page.".to_string();
                for update in [
                    Endpoint::Releases.message(message.clone()),
                    Endpoint::Contributors.message(message.clone()),
                    Endpoint::Languages.message(message),
                ] {
                    view.apply(&binding, update);
                }
            }
            page.check_authorization(&Sessions::new(), true);
            return;
        }
    };
    page.extras.cached = page.cache_binding.as_ref().and_then(|cache| {
        let (store, scope) = cache.clone().into_parts();
        CachedPageReadSession::new(session.clone(), store, scope).ok()
    });
    page.extras.session = Some(session);
    let offline_only = page
        .details_provenance
        .as_ref()
        .is_some_and(|prov| prov.origin == crate::data::RepositoryCacheOrigin::Snapshot);
    let authorization = page.current_authorization();
    page.extras.displayed_authorization = authorization.clone();
    read_counts(page, offline_only, authorization.clone());
    for endpoint in [
        Endpoint::Releases,
        Endpoint::Latest,
        Endpoint::Contributors,
        Endpoint::Languages,
    ] {
        read_section(page, endpoint, offline_only, authorization.clone());
    }
}

pub(super) fn read_section(
    page: &mut RepositoryController,
    endpoint: Endpoint,
    offline_only: bool,
    authorization: CapturedAuthorization,
) {
    let Some(binding) = page.extras.binding.clone() else {
        return;
    };
    if !accepts(page, &binding) {
        return;
    }
    let index = endpoint.index();
    if let Some(work) = page.extras.work[index].take() {
        work.source.cancel();
    }
    page.extras.requests[index] += 1;
    let request = page.extras.requests[index];
    let work = Work::new();
    let cancel = work.token();
    page.extras.work[index] = Some(work);
    if let Some(view) = page.extras.view.as_mut() {
        view.apply(&binding, endpoint.loading());
    }
    let Some(session) = page.extras.session.clone() else {
        return;
    };
    let cached = page.extras.cached.clone();
    let context = session.context().clone();
    let inbox = page.extras.inbox.clone();
    let alive = page.alive.clone();
    let repository = binding.repository.clone();
    let spawned = page.context.services().spawn(async move {
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
                endpoint.message("Cached overview is unavailable while offline.".to_string())
            }
            None => read_online_extra(&session, endpoint, &repository, &limits, cancel).await,
        };
        if alive.load(Ordering::Acquire) {
            inbox.lock().expect("overview inbox").push_back(Reply {
                binding,
                context,
                authorization,
                endpoint,
                request,
                update,
            });
        }
    });
    if spawned.is_err() {
        let binding = page.extras.binding.clone();
        if let (Some(binding), Some(view)) = (binding, page.extras.view.as_mut()) {
            view.apply(
                &binding,
                endpoint.message("Background services are closing.".to_string()),
            );
        }
    }
}

fn read_counts(
    page: &mut RepositoryController,
    offline_only: bool,
    authorization: CapturedAuthorization,
) {
    let Some(session) = page.extras.session.clone() else {
        return;
    };
    let cached = page.extras.cached.clone();
    let Some(binding) = page.extras.binding.clone() else {
        return;
    };
    let context = session.context().clone();
    let inbox = page.extras.counts_inbox.clone();
    page.extras.counts_request += 1;
    let request = page.extras.counts_request;
    let work = Work::new();
    let cancel = work.token();
    page.extras.counts_work = Some(work);
    let spawned = page.context.services().spawn(async move {
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
            authorization,
            request,
            result,
        });
    });
    let _ = spawned;
}

/// Applies the queued overview and count replies on the UI drain.
pub(super) fn drain(page: &mut RepositoryController, sessions: &Sessions) {
    let counts: Vec<_> = page
        .extras
        .counts_inbox
        .lock()
        .expect("count inbox")
        .drain(..)
        .collect();
    for reply in counts {
        if reply.request != page.extras.counts_request
            || !reply
                .authorization
                .as_ref()
                .is_none_or(CacheAuthorization::is_current)
            || !accepts(page, &reply.binding)
            || page.context_data.as_ref() != Some(&reply.context)
            || !page
                .context
                .services()
                .repositories()
                .is_current(&reply.context)
            || !page.context.is_current(sessions)
        {
            continue;
        }
        match reply
            .result
            .ok()
            .filter(|(value, _)| value.repository == reply.binding.repository)
        {
            // Historical counts are withheld rather than drawn identically to
            // freshly verified OPEN counts: the tab badges have no per-count
            // provenance marker, so a stale number would read as current.
            Some((_, true)) => {
                page.extras.counts = None;
                page.extras.counts_historical = true;
            }
            Some((value, false)) => {
                page.extras.counts = Some(value);
                page.extras.counts_historical = false;
            }
            None => {
                page.extras.counts = None;
                page.extras.counts_historical = false;
            }
        }
    }
    let replies: Vec<_> = page
        .extras
        .inbox
        .lock()
        .expect("overview inbox")
        .drain(..)
        .collect();
    for reply in replies {
        if page.extras.requests[reply.endpoint.index()] != reply.request
            || !reply
                .authorization
                .as_ref()
                .is_none_or(CacheAuthorization::is_current)
            || !accepts(page, &reply.binding)
            || page.context_data.as_ref() != Some(&reply.context)
            || !page
                .context
                .services()
                .repositories()
                .is_current(&reply.context)
            || !page.context.is_current(sessions)
        {
            continue;
        }
        if let Some(view) = page.extras.view.as_mut() {
            view.apply(&reply.binding, reply.update);
        }
    }
}

/// Whether a typed release/contributor action still describes what is displayed.
pub(super) fn valid_action(page: &RepositoryController, action: &OverviewAction) -> bool {
    let Some(view) = page.extras.view.as_ref() else {
        return false;
    };
    match action {
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
                person.login == *login && person.html_url == *html_url && person.html_url.is_some()
            })
        }),
        OverviewAction::Refresh { .. } => false,
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
                    message: format!("{error:?}"),
                },
            })
        }
        Endpoint::Latest => OverviewUpdate::LatestRelease(
            match session.latest_release(repository, limits, cancel).await {
                Ok(value) => SectionState::Ready(value.value),
                Err(error) => SectionState::Error {
                    message: format!("{error:?}"),
                },
            },
        ),
        Endpoint::Contributors => OverviewUpdate::Contributors(
            match session.contributors(repository, limits, cancel).await {
                Ok(value) => SectionState::Ready(value.value),
                Err(error) => SectionState::Error {
                    message: format!("{error:?}"),
                },
            },
        ),
        Endpoint::Languages => {
            OverviewUpdate::Languages(match session.languages(repository, limits, cancel).await {
                Ok(value) => SectionState::Ready(value.value),
                Err(error) => SectionState::Error {
                    message: format!("{error:?}"),
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
        return read_offline_extra(cached, endpoint, repository, limits, cancel).await;
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
                Err(error) => SectionState::Error {
                    message: format!("{error:?}"),
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
                Err(error) => SectionState::Error {
                    message: format!("{error:?}"),
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
                Err(error) => SectionState::Error {
                    message: format!("{error:?}"),
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
                Err(error) => SectionState::Error {
                    message: format!("{error:?}"),
                },
            };
            OverviewUpdate::Languages(state)
        }
    }
}

async fn read_offline_extra(
    cached: &CachedPageReadSession,
    endpoint: Endpoint,
    repository: &RepositoryIdentity,
    limits: &RepositoryExtrasLimits,
    cancel: crate::network::cancel::CancellationToken,
) -> OverviewUpdate {
    match endpoint {
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
