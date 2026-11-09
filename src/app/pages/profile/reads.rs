//! Account-scoped Profile section reads, carried over from the accepted GPUI page.
//!
//! These are the same page-data/cache paths the accepted Profile page already
//! used: online read, cache-backed read, bounded offline fallback for network
//! failures, typed failures kept distinct from confirmed absence, and the
//! captured authorization token on every historical value. The page host supplies
//! the session, the cache binding and the cancellation token, so no second
//! database or credential path exists here.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::data::page_cache::{
    CachedPageReadSession, PageCacheError, PageOrigin, PageReadMode, PageValue,
};
use crate::data::{PageReadSession, ProfileReadme, RepositoryReadContext};
use crate::domain::blob::BlobLimits;
use crate::domain::cache::CacheMiss;
use crate::domain::error::GitHubError;
use crate::domain::profile::{
    ContributionCalendar, ContributionRange, ProfileBasic, ProfileError, ProfilePinned,
};
use crate::domain::repository::RepositoryError;
use crate::github::RepositoryLimits;
use crate::github::profile::ProfileLimits;
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{CacheAuthorization, CacheError, CacheKey, CacheRead};
use crate::ui::profile::{PresentationError, ProfileProvenance};

#[derive(Clone)]
pub(crate) struct SectionOutcome<T> {
    pub(crate) value: T,
    pub(crate) provenance: Option<ProfileProvenance>,
    pub(crate) authorization: Option<CacheAuthorization>,
}

pub(crate) fn should_fallback_profile(error: &ProfileError) -> bool {
    match error {
        ProfileError::GitHub(
            GitHubError::Network { .. }
            | GitHubError::Timeout { .. }
            | GitHubError::RateLimited { .. },
        ) => true,
        ProfileError::GitHub(GitHubError::UnexpectedStatus { status }) => *status >= 500,
        ProfileError::Graphql { kind, .. } => matches!(
            kind,
            crate::domain::GraphqlErrorKind::RateLimitPrimary
                | crate::domain::GraphqlErrorKind::RateLimitSecondary
        ),
        _ => false,
    }
}

pub(crate) fn should_fallback_readme(error: &RepositoryError) -> bool {
    match error {
        RepositoryError::GitHub(
            GitHubError::Network { .. }
            | GitHubError::Timeout { .. }
            | GitHubError::RateLimited { .. },
        ) => true,
        RepositoryError::GitHub(GitHubError::UnexpectedStatus { status }) => *status >= 500,
        _ => false,
    }
}

pub(crate) fn map_cache_error(error: CacheError) -> PresentationError {
    match error {
        CacheError::AccessDenied => PresentationError::Forbidden,
        CacheError::Stale => PresentationError::Unauthorized,
        CacheError::Corrupt => PresentationError::CacheCorrupt,
        _ => PresentationError::Unavailable,
    }
}

/// Whether a cache failure means "no usable cache binding" or "the cache failed operationally".
///
/// Both must keep the account's online capability: none of them proves anything about the network
/// read, so the page falls back to the plain authenticated session request, which still enforces
/// cancellation, the captured context and authorization. A rejected credential, a stale session or
/// an explicit authorization denial is never routed here.
pub(crate) fn is_uncached_fallback(cache_error: &CacheError) -> bool {
    !matches!(
        cache_error,
        // A stale/deferred access decision is an authorization outcome, not an operational fault:
        // keep it typed instead of retrying as if the cache were merely unavailable.
        CacheError::Stale
    )
}

/// Re-verify the authorization token for a historical value. Lock-free atomic check; no disk I/O.
pub(crate) async fn verified_historical_authorization(
    _repos: &crate::data::RepoService,
    _context: &RepositoryReadContext,
    auth: Option<&CacheAuthorization>,
) -> Result<CacheAuthorization, PresentationError> {
    let Some(auth) = auth.cloned() else {
        return Err(PresentationError::Forbidden);
    };
    if auth.is_current() {
        Ok(auth)
    } else {
        Err(PresentationError::Forbidden)
    }
}

fn unpack_page_value<T>(
    value: PageValue<T>,
    authorization: Option<CacheAuthorization>,
) -> Result<SectionOutcome<T>, PresentationError> {
    match value {
        PageValue::Value {
            value,
            origin,
            metadata,
        } => {
            let historical = origin == PageOrigin::HistoricalSnapshot;
            Ok(SectionOutcome {
                value,
                provenance: Some(ProfileProvenance::from_cache(historical, &metadata)),
                // The token describes the authorization generation the bytes were read under, so
                // it is attached to network results as well: an explicit repository revocation must
                // retire displayed online aggregates, not just the offline badges.
                authorization,
            })
        }
        PageValue::Miss(miss) => Err(PresentationError::from_cache_miss(miss)),
    }
}

pub(crate) async fn read_profile_basic(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    authorization: Option<&CacheAuthorization>,
    login: &str,
    cancel: CancellationToken,
) -> Result<SectionOutcome<ProfileBasic>, PresentationError> {
    let Some(cached) = cached else {
        return session
            .profile(login, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                authorization: None,
            })
            .map_err(PresentationError::from);
    };
    match cached
        .profile(PageReadMode::Online, login, cancel.clone())
        .await
    {
        Ok(scoped) => {
            let outcome = unpack_page_value(scoped.value, authorization.cloned())?;
            // The page is keyed by the requested spelling, but the Shell rewrites the route to the
            // server's canonical login. Store the same observation under that canonical spelling too
            // so reopening the canonical route offline can mount the profile. One extra request per
            // alias spelling; the common (already canonical) case issues none.
            if outcome.value.login != login && !cancel.is_cancelled() {
                let _ = cached
                    .profile(PageReadMode::Online, &outcome.value.login, cancel)
                    .await;
            }
            Ok(outcome)
        }
        Err(PageCacheError::Read(error)) => {
            if should_fallback_profile(&error) {
                match cached.profile(PageReadMode::Offline, login, cancel).await {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value,
                            origin,
                            metadata,
                        } => {
                            let historical = origin == PageOrigin::HistoricalSnapshot;
                            let authorization = if historical {
                                Some(
                                    verified_historical_authorization(
                                        repos,
                                        session.context(),
                                        authorization,
                                    )
                                    .await?,
                                )
                            } else {
                                None
                            };
                            Ok(SectionOutcome {
                                value,
                                provenance: Some(ProfileProvenance::from_cache(
                                    historical, &metadata,
                                )),
                                authorization,
                            })
                        }
                        PageValue::Miss(miss) => {
                            if matches!(error, ProfileError::GitHub(GitHubError::Network { .. })) {
                                Err(PresentationError::from_cache_miss(miss))
                            } else {
                                Err(error.into())
                            }
                        }
                    },
                    Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
                        // The offline replay could not even reach the cache; report what the
                        // network read actually said instead of a cache-layer error.
                        Err(error.into())
                    }
                    Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
                    Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
                    Err(PageCacheError::Read(read_err)) => Err(read_err.into()),
                }
            } else {
                Err(error.into())
            }
        }
        Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
            // No usable cache binding, or the cache itself failed. The account's online capability
            // must survive either way: the plain session read keeps cancellation, the captured
            // context and authorization checks.
            session
                .profile(login, cancel)
                .await
                .map(|scoped| SectionOutcome {
                    value: scoped.value,
                    provenance: None,
                    authorization: authorization.cloned(),
                })
                .map_err(PresentationError::from)
        }
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
    }
}

pub(crate) async fn read_profile_pinned(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    authorization: Option<&CacheAuthorization>,
    login: &str,
    cancel: CancellationToken,
) -> Result<SectionOutcome<ProfilePinned>, PresentationError> {
    let limits = ProfileLimits::default();
    let Some(cached) = cached else {
        return session
            .profile_pinned(login, &limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                authorization: None,
            })
            .map_err(PresentationError::from);
    };
    match cached
        .profile_pinned(PageReadMode::Online, login, &limits, cancel.clone())
        .await
    {
        Ok(scoped) => unpack_page_value(scoped.value, authorization.cloned()),
        Err(PageCacheError::Read(error)) => {
            if should_fallback_profile(&error) {
                match cached
                    .profile_pinned(PageReadMode::Offline, login, &limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value,
                            origin,
                            metadata,
                        } => {
                            let historical = origin == PageOrigin::HistoricalSnapshot;
                            let authorization = if historical {
                                Some(
                                    verified_historical_authorization(
                                        repos,
                                        session.context(),
                                        authorization,
                                    )
                                    .await?,
                                )
                            } else {
                                None
                            };
                            Ok(SectionOutcome {
                                value,
                                provenance: Some(ProfileProvenance::from_cache(
                                    historical, &metadata,
                                )),
                                authorization,
                            })
                        }
                        PageValue::Miss(miss) => {
                            if matches!(error, ProfileError::GitHub(GitHubError::Network { .. })) {
                                Err(PresentationError::from_cache_miss(miss))
                            } else {
                                Err(error.into())
                            }
                        }
                    },
                    Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
                        // The offline replay could not even reach the cache; report what the
                        // network read actually said instead of a cache-layer error.
                        Err(error.into())
                    }
                    Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
                    Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
                    Err(PageCacheError::Read(read_err)) => Err(read_err.into()),
                }
            } else {
                Err(error.into())
            }
        }
        Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => session
            .profile_pinned(login, &limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                authorization: authorization.cloned(),
            })
            .map_err(PresentationError::from),
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
    }
}

/// Cache identity for the last interval a contribution calendar was read at.
fn calendar_range_key(login: &str) -> Result<CacheKey, CacheError> {
    CacheKey::mutable(
        None,
        true,
        "profile-calendar-range",
        login,
        "profile-calendar-range-v1",
        1,
    )
}

fn read_calendar_range(
    store: &crate::storage::cache::CacheStore,
    scope: &crate::storage::cache::CacheScope,
    login: &str,
) -> Option<ContributionRange> {
    let key = calendar_range_key(login).ok()?;
    let CacheRead::Hit(lease) = store.read(scope, &key).ok()? else {
        return None;
    };
    let snapshot = lease.snapshot().ok()??;
    let text = std::str::from_utf8(&snapshot.bytes).ok()?;
    let (from, to) = text.split_once('|')?;
    ContributionRange::new(from, to).ok()
}

fn write_calendar_range(
    store: &crate::storage::cache::CacheStore,
    scope: &crate::storage::cache::CacheScope,
    login: &str,
    range: &ContributionRange,
) -> Result<(), CacheError> {
    let key = calendar_range_key(login)?;
    let ticket = store.begin_write(scope, &key)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let metadata = crate::domain::cache::CacheMetadata {
        representation_version: 1,
        fetched_at_ms: now,
        verified_at_ms: now,
        completeness: crate::domain::cache::CacheCompleteness::Complete,
        etag: None,
        last_modified: None,
        cache_control: None,
    };
    let payload = format!("{}|{}", range.from(), range.to());
    store.put(ticket, payload.as_bytes(), metadata)?;
    Ok(())
}

async fn load_calendar_range(
    repos: &crate::data::RepoService,
    context: &RepositoryReadContext,
    login: &str,
) -> Option<ContributionRange> {
    let cache = repos.cache(context)?;
    let (store, scope) = cache.into_parts();
    let login = login.to_string();
    tokio::task::spawn_blocking(move || read_calendar_range(&store, &scope, &login))
        .await
        .ok()
        .flatten()
}

/// Records the interval of a successful calendar read. Best effort: a cache write failure only
/// means a later offline reopen cannot restore the calendar, never that a wrong range is shown.
async fn store_calendar_range(
    repos: &crate::data::RepoService,
    context: &RepositoryReadContext,
    login: &str,
    range: &ContributionRange,
) {
    let Some(cache) = repos.cache(context) else {
        return;
    };
    let (store, scope) = cache.into_parts();
    let login = login.to_string();
    let range = range.clone();
    let _ =
        tokio::task::spawn_blocking(move || write_calendar_range(&store, &scope, &login, &range))
            .await;
}

pub(crate) async fn read_profile_contributions(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    authorization: Option<&CacheAuthorization>,
    login: &str,
    range: &ContributionRange,
    cancel: CancellationToken,
) -> Result<(SectionOutcome<ContributionCalendar>, ContributionRange), PresentationError> {
    let limits = ProfileLimits::default();
    let Some(cached) = cached else {
        let outcome = session
            .contribution_calendar(login, range, &limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                authorization: None,
            })
            .map_err(PresentationError::from)?;
        store_calendar_range(repos, session.context(), login, range).await;
        return Ok((outcome, range.clone()));
    };
    match cached
        .contribution_calendar(PageReadMode::Online, login, range, &limits, cancel.clone())
        .await
    {
        Ok(scoped) => {
            let outcome = unpack_page_value(scoped.value, authorization.cloned())?;
            store_calendar_range(repos, session.context(), login, range).await;
            Ok((outcome, range.clone()))
        }
        Err(PageCacheError::Read(error)) => {
            if !should_fallback_profile(&error) {
                return Err(error.into());
            }
            // The requested interval is today's rolling year. A snapshot stored on an earlier date
            // lives under that earlier interval, so also try the recorded interval instead of
            // reporting a cache miss that would make offline reading date-dependent.
            let mut offline_ranges = vec![range.clone()];
            if let Some(previous) = load_calendar_range(repos, session.context(), login).await
                && &previous != range
            {
                offline_ranges.push(previous);
            }
            let mut last_miss = None;
            for offline_range in offline_ranges {
                match cached
                    .contribution_calendar(
                        PageReadMode::Offline,
                        login,
                        &offline_range,
                        &limits,
                        cancel.clone(),
                    )
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value,
                            origin,
                            metadata,
                        } => {
                            let historical = origin == PageOrigin::HistoricalSnapshot;
                            let authorization = if historical {
                                Some(
                                    verified_historical_authorization(
                                        repos,
                                        session.context(),
                                        authorization,
                                    )
                                    .await?,
                                )
                            } else {
                                None
                            };
                            let outcome = SectionOutcome {
                                value,
                                provenance: Some(ProfileProvenance::from_cache(
                                    historical, &metadata,
                                )),
                                authorization,
                            };
                            store_calendar_range(repos, session.context(), login, &offline_range)
                                .await;
                            return Ok((outcome, offline_range));
                        }
                        PageValue::Miss(miss) => last_miss = Some(miss),
                    },
                    Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
                        return Err(error.into());
                    }
                    Err(PageCacheError::Cache(cache_err)) => {
                        return Err(map_cache_error(cache_err));
                    }
                    Err(PageCacheError::Context(gh)) => {
                        return Err(ProfileError::GitHub(gh).into());
                    }
                    Err(PageCacheError::Read(read_err)) => return Err(read_err.into()),
                }
            }
            if matches!(error, ProfileError::GitHub(GitHubError::Network { .. })) {
                Err(PresentationError::from_cache_miss(
                    last_miss.unwrap_or(CacheMiss::NotCached),
                ))
            } else {
                Err(error.into())
            }
        }
        Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
            let outcome = session
                .contribution_calendar(login, range, &limits, cancel)
                .await
                .map(|scoped| SectionOutcome {
                    value: scoped.value,
                    provenance: None,
                    authorization: authorization.cloned(),
                })
                .map_err(PresentationError::from)?;
            store_calendar_range(repos, session.context(), login, range).await;
            Ok((outcome, range.clone()))
        }
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
    }
}

pub(crate) async fn read_profile_readme(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    authorization: Option<&CacheAuthorization>,
    basic: &ProfileBasic,
    cancel: CancellationToken,
) -> Result<SectionOutcome<ProfileReadme>, PresentationError> {
    let repo_limits = RepositoryLimits::default();
    let blob_limits = BlobLimits::default();
    let Some(cached) = cached else {
        return session
            .profile_readme(basic, &repo_limits, &blob_limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                authorization: None,
            })
            .map_err(repository_error);
    };
    match cached
        .profile_readme(
            PageReadMode::Online,
            basic,
            &repo_limits,
            &blob_limits,
            cancel.clone(),
        )
        .await
    {
        Ok(scoped) => unpack_page_value(scoped.value, authorization.cloned()),
        Err(PageCacheError::Read(error)) => {
            if should_fallback_readme(&error) {
                match cached
                    .profile_readme(
                        PageReadMode::Offline,
                        basic,
                        &repo_limits,
                        &blob_limits,
                        cancel,
                    )
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value,
                            origin,
                            metadata,
                        } => {
                            let historical = origin == PageOrigin::HistoricalSnapshot;
                            let authorization = if historical {
                                Some(
                                    verified_historical_authorization(
                                        repos,
                                        session.context(),
                                        authorization,
                                    )
                                    .await?,
                                )
                            } else {
                                None
                            };
                            Ok(SectionOutcome {
                                value,
                                provenance: Some(ProfileProvenance::from_cache(
                                    historical, &metadata,
                                )),
                                authorization,
                            })
                        }
                        PageValue::Miss(miss) => {
                            if matches!(error, RepositoryError::GitHub(GitHubError::Network { .. }))
                            {
                                Err(PresentationError::from_cache_miss(miss))
                            } else {
                                Err(repository_error(error))
                            }
                        }
                    },
                    Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
                        Err(repository_error(error))
                    }
                    Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
                    Err(PageCacheError::Context(gh)) => {
                        Err(PresentationError::from(ProfileError::GitHub(gh)))
                    }
                    Err(PageCacheError::Read(read_err)) => Err(repository_error(read_err)),
                }
            } else {
                Err(repository_error(error))
            }
        }
        Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => session
            .profile_readme(basic, &repo_limits, &blob_limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                authorization: authorization.cloned(),
            })
            .map_err(repository_error),
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(PresentationError::from(ProfileError::GitHub(gh))),
    }
}

pub(crate) fn repository_error(error: RepositoryError) -> PresentationError {
    match error {
        RepositoryError::GitHub(GitHubError::NotFound) => PresentationError::NotFound,
        RepositoryError::GitHub(error) => PresentationError::from(ProfileError::GitHub(error)),
        RepositoryError::SsoPartial { .. } => PresentationError::SsoRequired,
        RepositoryError::InvalidInput { .. }
        | RepositoryError::InvalidResponse { .. }
        | RepositoryError::EmptyOrUnavailable => PresentationError::InvalidData,
        RepositoryError::LimitExceeded { .. } => PresentationError::Unavailable,
    }
}
