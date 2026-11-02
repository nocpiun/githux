use super::*;
use crate::{
    domain::{
        blob::BlobLimits, home::*, profile::*, repository::RepositoryIdentity,
        repository_counts::RepositoryCounts, repository_extras::*,
    },
    github::{
        RepositoryLimits,
        home::{FeedLimits, FollowingLimits, HomeSearchLimits, PullRequestDetailLimits},
        profile::ProfileLimits,
        repository_extras::RepositoryExtrasLimits,
    },
};

fn home_parameters(
    updated: Option<&UpdatedWindow>,
    limits: &HomeSearchLimits,
) -> serde_json::Value {
    serde_json::json!([format!("{updated:?}"), format!("{limits:?}")])
}

impl CachedPageReadSession {
    /// Complete/Partial keys for the Home aggregate, so tests can hold a lease
    /// or inspect slots without duplicating the request identity.
    #[cfg(test)]
    pub(super) fn home_keys(
        &self,
        updated: Option<&UpdatedWindow>,
        limits: &HomeSearchLimits,
    ) -> Result<(CacheKey, CacheKey), CacheError> {
        let parameters = home_parameters(updated, limits);
        Ok((
            self.key(None, "home-open-involves", parameters.clone(), false)?,
            self.key(None, "home-open-involves", parameters, true)?,
        ))
    }

    pub async fn home(
        &self,
        mode: PageReadMode,
        updated: Option<UpdatedWindow>,
        limits: &HomeSearchLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<HomeSearchOutcome>>, PageCacheError<HomeError>> {
        let parameters = home_parameters(updated.as_ref(), limits);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            None,
            "home-open-involves",
            parameters,
            cancel,
            |session| async move { session.home(updated, limits, request_cancel).await },
            |value| partial(value.readiness.is_complete(), "Home query incomplete"),
        )
        .await
    }
    pub async fn profile(
        &self,
        mode: PageReadMode,
        login: &str,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<ProfileBasic>>, PageCacheError<ProfileError>> {
        let request_cancel = cancel.clone();
        self.read(
            mode,
            None,
            "profile-basic",
            serde_json::json!(login),
            cancel,
            |session| async move { session.profile(login, request_cancel).await },
            |_| CacheCompleteness::Complete,
        )
        .await
    }
    pub async fn profile_pinned(
        &self,
        mode: PageReadMode,
        login: &str,
        limits: &ProfileLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<ProfilePinned>>, PageCacheError<ProfileError>> {
        let parameters = serde_json::json!([login, format!("{limits:?}")]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            None,
            "profile-showcase-query-v1",
            parameters,
            cancel,
            |session| async move { session.profile_pinned(login, limits, request_cancel).await },
            |v| {
                partial(
                    v.errors.is_empty()
                        && !v.pinned_truncated
                        && !v.showcase_truncated
                        && !matches!(
                            v.showcase,
                            Showcase::Unclassified | Showcase::Truncated { .. }
                        ),
                    "Profile showcase incomplete",
                )
            },
        )
        .await
    }
    pub async fn contribution_calendar(
        &self,
        mode: PageReadMode,
        login: &str,
        range: &ContributionRange,
        limits: &ProfileLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<ContributionCalendar>>, PageCacheError<ProfileError>> {
        let parameters = serde_json::json!([
            login,
            range.from().to_string(),
            range.to().to_string(),
            format!("{limits:?}")
        ]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            None,
            "profile-calendar-query-v1",
            parameters,
            cancel,
            |session| async move {
                session
                    .contribution_calendar(login, range, limits, request_cancel)
                    .await
            },
            |v| partial(v.errors.is_empty(), "Contribution calendar incomplete"),
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn feed(
        &self,
        mode: PageReadMode,
        limits: &FeedLimits,
        following: &FollowingLimits,
        authors: &PullRequestDetailLimits,
        max_authors: usize,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<FeedOutcome>>, PageCacheError<HomeError>> {
        let parameters = serde_json::json!([
            format!("{limits:?}"),
            format!("{following:?}"),
            format!("{authors:?}"),
            max_authors.to_string()
        ]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            None,
            "feed-first-policy-v1",
            parameters,
            cancel,
            |session| async move {
                session
                    .feed(limits, following, authors, max_authors, request_cancel)
                    .await
            },
            |v| partial(v.readiness.is_complete(), "Feed dependencies incomplete"),
        )
        .await
    }
    /// One source cursor per snapshot. Seen IDs are committed only after the
    /// cached read and publication checks succeed.
    #[allow(clippy::too_many_arguments)]
    pub async fn feed_page(
        &self,
        mode: PageReadMode,
        cursor: FeedSourceCursor,
        limits: &FeedLimits,
        following: &FollowingLimits,
        authors: &PullRequestDetailLimits,
        max_authors: usize,
        seen: &mut std::collections::HashSet<String>,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<FeedPage>>, PageCacheError<HomeError>> {
        let parameters = serde_json::json!([cursor.page, limits.per_page]);
        // The source representation is independent of seen IDs. Replay runs the
        // same policy against the caller's current set; no mixed page DTO is saved.
        let parameters = serde_json::json!([
            parameters,
            format!("{limits:?}"),
            format!("{following:?}"),
            format!("{authors:?}"),
            max_authors
        ]);
        let local = std::sync::Arc::new(std::sync::Mutex::new(seen.clone()));
        let published = local.clone();
        let request_cancel = cancel.clone();
        let value = self
            .read(
                mode,
                None,
                "feed-older-policy-v1",
                parameters,
                cancel,
                |session| async move {
                    let mut seen_ids = local.lock().unwrap().clone();
                    let value = session
                        .feed_page(
                            cursor,
                            limits,
                            following,
                            authors,
                            max_authors,
                            &mut seen_ids,
                            request_cancel,
                        )
                        .await?;
                    *local.lock().unwrap() = seen_ids;
                    Ok::<_, HomeError>(value)
                },
                |v| partial(v.reasons.is_empty(), "Feed page incomplete"),
            )
            .await?;
        if matches!(value.value, PageValue::Value { .. }) {
            *seen = published.lock().unwrap().clone();
        }
        Ok(value)
    }
    /// Optional star-card descriptions keep the existing bounded lookup path.
    /// A failed lookup remains None with Partial provenance, never proof that a
    /// repository has no description.
    pub async fn repository_descriptions(
        &self,
        mode: PageReadMode,
        repositories: Vec<(String, String)>,
        max_lookups: usize,
        cancel: CancellationToken,
    ) -> Result<
        Scoped<PageValue<std::collections::HashMap<String, Option<String>>>>,
        PageCacheError<HomeError>,
    > {
        let parameters = serde_json::json!([repositories, max_lookups]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            None,
            "feed-repository-descriptions",
            parameters,
            cancel,
            |session| async move {
                session
                    .repository_descriptions(repositories, max_lookups, request_cancel)
                    .await
            },
            |v| {
                partial(
                    v.values().all(Option::is_some),
                    "Optional repository descriptions unavailable",
                )
            },
        )
        .await
    }
    pub async fn releases(
        &self,
        mode: PageReadMode,
        repository: &RepositoryIdentity,
        limits: &RepositoryExtrasLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<ReleasesOutcome>>, PageCacheError<RepositoryExtrasError>> {
        let parameters = serde_json::json!([format!("{repository:?}"), format!("{limits:?}")]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            Some(repository.id()),
            "repository-releases",
            parameters,
            cancel,
            |session| async move { session.releases(repository, limits, request_cancel).await },
            |v| {
                partial(
                    v.completeness == ExtrasCompleteness::Complete,
                    "Release pages incomplete",
                )
            },
        )
        .await
    }
    pub async fn contributors(
        &self,
        mode: PageReadMode,
        repository: &RepositoryIdentity,
        limits: &RepositoryExtrasLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<ContributorsOutcome>>, PageCacheError<RepositoryExtrasError>> {
        let parameters = serde_json::json!([format!("{repository:?}"), format!("{limits:?}")]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            Some(repository.id()),
            "repository-contributors",
            parameters,
            cancel,
            |session| async move {
                session
                    .contributors(repository, limits, request_cancel)
                    .await
            },
            |v| {
                partial(
                    v.completeness == ExtrasCompleteness::Complete,
                    "Contributor pages incomplete",
                )
            },
        )
        .await
    }
    pub async fn latest_release(
        &self,
        mode: PageReadMode,
        repository: &RepositoryIdentity,
        limits: &RepositoryExtrasLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<LatestReleaseOutcome>>, PageCacheError<RepositoryExtrasError>>
    {
        let parameters = serde_json::json!([format!("{repository:?}"), format!("{limits:?}")]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            Some(repository.id()),
            "repository-latest-release",
            parameters,
            cancel,
            |session| async move {
                session
                    .latest_release(repository, limits, request_cancel)
                    .await
            },
            |v| {
                partial(
                    v.release.is_some(),
                    "Latest release not found or inaccessible",
                )
            },
        )
        .await
    }
    pub async fn languages(
        &self,
        mode: PageReadMode,
        repository: &RepositoryIdentity,
        limits: &RepositoryExtrasLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<LanguagesOutcome>>, PageCacheError<RepositoryExtrasError>> {
        let parameters = serde_json::json!([format!("{repository:?}"), format!("{limits:?}")]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            Some(repository.id()),
            "repository-languages",
            parameters,
            cancel,
            |session| async move { session.languages(repository, limits, request_cancel).await },
            |_| CacheCompleteness::Complete,
        )
        .await
    }
    pub async fn repository_counts(
        &self,
        mode: PageReadMode,
        repository: &RepositoryIdentity,
        cancel: CancellationToken,
    ) -> Result<
        Scoped<PageValue<RepositoryCounts>>,
        PageCacheError<crate::domain::repository_counts::CountsError>,
    > {
        let parameters = serde_json::json!(format!("{repository:?}"));
        let request_cancel = cancel.clone();
        self.read(
            mode,
            Some(repository.id()),
            "repository-counts-query-v1",
            parameters,
            cancel,
            |session| async move { session.repository_counts(repository, request_cancel).await },
            |v| {
                partial(
                    v.issues.is_ok() && v.pull_requests.is_ok(),
                    "Repository counts incomplete",
                )
            },
        )
        .await
    }
    pub async fn profile_readme(
        &self,
        mode: PageReadMode,
        profile: &ProfileBasic,
        repository_limits: &RepositoryLimits,
        blob_limits: &BlobLimits,
        cancel: CancellationToken,
    ) -> Result<
        Scoped<PageValue<ProfileReadme>>,
        PageCacheError<crate::domain::repository::RepositoryError>,
    > {
        // The repository ID is not known until discovery. Keep this whole
        // composite under the account aggregate gate, including its public bytes.
        let parameters = serde_json::json!([
            profile.id.to_string(),
            profile.login,
            format!("{repository_limits:?}"),
            format!("{blob_limits:?}")
        ]);
        let request_cancel = cancel.clone();
        self.read(
            mode,
            None,
            "profile-root-readme-v1",
            parameters,
            cancel,
            |session| async move {
                session
                    .profile_readme(profile, repository_limits, blob_limits, request_cancel)
                    .await
            },
            |v| {
                partial(
                    !matches!(v, ProfileReadme::Unavailable { .. }),
                    "Profile root tree incomplete",
                )
            },
        )
        .await
    }
}
