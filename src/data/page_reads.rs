//! Account-scoped online reads for Home/Feed, Profile and repository extras.
//!
//! Every adapter uses the one RepositorySession authorization path. This module
//! owns no PAT, credential loading, semaphore, cache, polling or UI state.
use super::{AccountService, RepoService, RepositoryReadContext, RepositorySession, Scoped};
use crate::content::PreparedContent;
use crate::domain::blob::{BlobIdentity, BlobLimits};
use crate::domain::error::GitHubError;
use crate::domain::home::{
    AuthorResolutions, FeedEventsRead, FeedFetch, FeedOutcome, Following, HomeError, HomeQuery,
    HomeSearchOutcome, Login, UpdatedWindow, build_feed_page, is_retained_activity,
    merge_candidate,
};
use crate::domain::profile::{
    ContributionCalendar, ContributionRange, ProfileBasic, ProfileError, ProfilePinned,
};
use crate::domain::repo::RepoVisibility;
use crate::domain::repository::{
    Completeness, ObjectSha, RepositoryError, RepositoryIdentity, RepositoryLocator, RevisionSpec,
    SnapshotDirectory,
};
use crate::domain::repository_extras::{
    ContributorsOutcome, LanguagesOutcome, LatestReleaseOutcome, ReleasesOutcome,
    RepositoryExtrasError,
};
use crate::github::home::{FeedLimits, FollowingLimits, HomeSearchLimits, PullRequestDetailLimits};
use crate::github::profile::ProfileLimits;
use crate::github::repository_extras::RepositoryExtrasLimits;
use crate::github::{self, RepositoryLimits};
use crate::network::cancel::CancellationToken;

/// One captured active-account context shared by every page-domain read.
/// Clone it for independent concurrent calls; never recapture it between the
/// steps of a profile README read. It contains no credential.
#[derive(Clone)]
pub struct PageReadSession {
    repository: RepositorySession,
    client: std::sync::Arc<crate::github::client::GitHubClient>,
    login: Login,
}

impl std::fmt::Debug for PageReadSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageReadSession")
            .field("context", self.context())
            .finish_non_exhaustive()
    }
}

impl RepoService {
    /// Capture the authoritative active account exactly once. The Home/Feed
    /// login comes from that account, never from a page's target login.
    pub fn page_session(&self) -> Result<PageReadSession, RepositoryError> {
        let repository = self.session()?;
        let login = Login::new(repository.context().active_login())
            .map_err(|_| GitHubError::StaleContext)?;
        Ok(PageReadSession {
            repository,
            client: self.client.clone(),
            login,
        })
    }
}

impl AccountService {
    pub fn page_session(&self) -> Result<PageReadSession, RepositoryError> {
        self.repo_service().page_session()
    }
}

impl PageReadSession {
    /// Independent OPEN connections at the current captured account context.
    pub async fn repository_counts(
        &self,
        repository: &RepositoryIdentity,
        cancel: CancellationToken,
    ) -> Result<
        Scoped<crate::domain::repository_counts::RepositoryCounts>,
        crate::domain::repository_counts::CountsError,
    > {
        self.repository
            .read(cancel, async move |pat, cancel| {
                crate::github::repository_counts::fetch(&self.client, &pat, repository, cancel)
                    .await
            })
            .await
    }

    pub fn context(&self) -> &RepositoryReadContext {
        self.repository.context()
    }

    /// Publication check for the matching RepoService and current account.
    pub fn verify(&self) -> bool {
        self.repository.verify()
    }

    pub fn login(&self) -> &Login {
        &self.login
    }

    pub async fn home(
        &self,
        updated: Option<UpdatedWindow>,
        limits: &HomeSearchLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<HomeSearchOutcome>, HomeError> {
        let mut query = HomeQuery::new(self.login.clone());
        if let Some(updated) = updated {
            query = query.with_updated_window(updated);
        }
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::home::search_involving(&self.client, &pat, &query, limits, cancel).await
            })
            .await
    }

    /// Read the accounts the captured login follows.
    ///
    /// Membership is **evidence**, not a boolean default: an `Err` or a
    /// `Partial` readiness means the relationship set is unknown for accounts
    /// outside the returned list, and callers must not present those subjects as
    /// followed or not followed.
    pub async fn following(
        &self,
        limits: &FollowingLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<Following>, HomeError> {
        let login = self.login.clone();
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::home::fetch_following(&self.client, &pat, &login, limits, cancel).await
            })
            .await
    }

    /// Raw received-events read (source list + scheduler metadata). The product
    /// policy is applied by [`Self::feed`].
    ///
    /// No validator is sent: without an account-bound source snapshot a 304
    /// cannot be safely replayed. An unexpected 304 is a typed HomeError.
    pub async fn feed_events(
        &self,
        limits: &FeedLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<FeedEventsRead>, HomeError> {
        let login = self.login.clone();
        self.repository
            .read(
                cancel,
                async move |pat, cancel| match github::home::fetch_received_events(
                    &self.client,
                    &pat,
                    &login,
                    None,
                    limits,
                    cancel,
                )
                .await?
                {
                    FeedFetch::NotModified { .. } => Err(HomeError::InvalidResponse {
                        message: "unexpected 304 without a validated local snapshot".into(),
                    }),
                    FeedFetch::Events(events) => Ok(events),
                },
            )
            .await
    }

    /// One merged pull request's detail, used to resolve the contribution author.
    pub async fn pull_request_detail(
        &self,
        owner: &str,
        repository: &str,
        number: u64,
        limits: &PullRequestDetailLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<crate::domain::home::PullRequestDetail>, HomeError> {
        let owner = owner.to_string();
        let repository = repository.to_string();
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::home::fetch_pull_request_detail(
                    &self.client,
                    &pat,
                    &owner,
                    &repository,
                    number,
                    limits,
                    cancel,
                )
                .await
            })
            .await
    }

    /// One source page of received events from a validated cursor.
    ///
    /// Unlike [`Self::feed_events`], this does not re-read the whole timeline: it
    /// fetches exactly the requested page and returns its next-page cursor. The
    /// caller owns continuation state so account switches and refresh cannot mix
    /// generations.
    pub async fn feed_source_page(
        &self,
        cursor: crate::domain::home::FeedSourceCursor,
        limits: &FeedLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<crate::domain::home::FeedSourcePage>, HomeError> {
        let login = self.login.clone();
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::home::fetch_received_events_page(
                    &self.client,
                    &pat,
                    &login,
                    cursor,
                    limits,
                    cancel,
                )
                .await
            })
            .await
    }

    /// One older Feed page applied to an existing visible list.
    ///
    /// Fetches the source page at `cursor`, re-reads the followed-account set and
    /// resolves any missing merged authors, then applies the product policy and
    /// de-duplicates against `seen_ids`. The returned [`FeedPage`] carries its own
    /// next cursor and reasons; the host folds it into the retained outcome.
    /// A failed page is a hard error so the host retries the same cursor.
    #[allow(clippy::too_many_arguments)]
    pub async fn feed_page(
        &self,
        cursor: crate::domain::home::FeedSourceCursor,
        limits: &FeedLimits,
        following_limits: &FollowingLimits,
        author_limits: &PullRequestDetailLimits,
        max_author_lookups: usize,
        seen_ids: &mut std::collections::HashSet<String>,
        cancel: CancellationToken,
    ) -> Result<Scoped<crate::domain::home::FeedPage>, HomeError> {
        let page = self
            .feed_source_page(cursor, limits, cancel.clone())
            .await?
            .value;
        let following = self
            .following(following_limits, cancel.clone())
            .await?
            .value;
        let authors = self
            .resolve_authors(
                &page.events,
                author_limits,
                max_author_lookups,
                cancel.clone(),
            )
            .await?;
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let built = build_feed_page(&page, &following, &authors, seen_ids);
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        Ok(Scoped {
            context: self.context().clone(),
            value: built,
        })
    }

    /// Feed: read the first source page, resolve the merged-pull-request authors
    /// that need it, and apply the followed-account constraint.
    ///
    /// The returned outcome exposes the validated next source page, so the host
    /// fetches older pages incrementally on end-of-scroll instead of eagerly
    /// reading the whole (bounded) timeline upfront. A failed or partial
    /// auxiliary read is never reported as a complete empty Feed.
    pub async fn feed(
        &self,
        limits: &FeedLimits,
        following_limits: &FollowingLimits,
        author_limits: &PullRequestDetailLimits,
        max_author_lookups: usize,
        cancel: CancellationToken,
    ) -> Result<Scoped<FeedOutcome>, HomeError> {
        let page = self
            .feed_source_page(
                crate::domain::home::FeedSourceCursor { page: 1 },
                limits,
                cancel.clone(),
            )
            .await?
            .value;
        let following = match self.following(following_limits, cancel.clone()).await {
            Ok(following) => following.value,
            Err(error) if matches!(error, HomeError::GitHub(GitHubError::Cancelled)) => {
                return Err(error);
            }
            // Membership is unknown, so no card can be certified. Report the read
            // failure instead of publishing an empty, complete-looking Feed.
            Err(error) => return Err(error),
        };
        let authors = self
            .resolve_authors(
                &page.events,
                author_limits,
                max_author_lookups,
                cancel.clone(),
            )
            .await?;
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let mut seen = std::collections::HashSet::new();
        let built = build_feed_page(&page, &following, &authors, &mut seen);
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        Ok(Scoped {
            context: self.context().clone(),
            value: built.into_outcome(),
        })
    }

    /// Resolve authors for the merged pull requests in `events` that do not carry
    /// an embedded author.
    ///
    /// Each distinct target is read at most once per call. When the lookup budget
    /// is exhausted the remaining targets are recorded as unresolved (not as "no
    /// author"), so the outcome reports withheld contributions rather than
    /// silently dropping them.
    async fn resolve_authors(
        &self,
        events: &[crate::domain::home::FeedEvent],
        limits: &PullRequestDetailLimits,
        max_lookups: usize,
        cancel: CancellationToken,
    ) -> Result<AuthorResolutions, HomeError> {
        let mut authors = AuthorResolutions::new();
        let mut pending = Vec::new();
        for event in events.iter().filter(|event| is_retained_activity(event)) {
            let Some(candidate) = merge_candidate(event) else {
                continue;
            };
            let key = candidate.key();
            if authors.get(&key).is_some() {
                continue;
            }
            if candidate.embedded_author.is_some() {
                // No detail read needed; build_feed uses the embedded author.
                continue;
            }
            let Some(number) = candidate.pull_request.number else {
                authors.mark_unresolved(key);
                continue;
            };
            // Reserve the key before scheduling, including budget-exhausted
            // targets, so duplicate events never consume another lookup.
            authors.mark_unresolved(key.clone());
            if pending.len() >= max_lookups {
                continue;
            }
            pending.push((key, candidate.owner, candidate.repository, number));
        }

        // Four workers share the bounded queue. These futures belong to this
        // read (no detached tasks); cancellation drops all in-flight workers.
        // Every request still acquires the account service's shared permit and
        // validates the captured account. Results are keyed, so completion order
        // cannot change the source order of the eventual Feed cards.
        let next = std::sync::atomic::AtomicUsize::new(0);
        let worker = || async {
            let mut resolved = Vec::new();
            while let Some((key, owner, repository, number)) =
                pending.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
            {
                match self
                    .pull_request_detail(owner, repository, *number, limits, cancel.clone())
                    .await
                {
                    Ok(detail) => {
                        if let crate::domain::home::PullRequestDetailOutcome::Attributed(author) =
                            detail.value.outcome
                        {
                            resolved.push((key.clone(), author));
                        }
                    }
                    Err(error) if matches!(error, HomeError::GitHub(GitHubError::Cancelled)) => {
                        return Err(error);
                    }
                    // Failed/unattributable reads remain unresolved; never
                    // guess the author from the merge actor.
                    Err(_) => {}
                }
            }
            Ok::<_, HomeError>(resolved)
        };
        let (a, b, c, d) = tokio::try_join!(biased; worker(), worker(), worker(), worker())?;
        for (key, author) in a.into_iter().chain(b).chain(c).chain(d) {
            authors.insert(key, author);
        }
        Ok(authors)
    }

    pub async fn profile(
        &self,
        target_login: &str,
        cancel: CancellationToken,
    ) -> Result<Scoped<ProfileBasic>, ProfileError> {
        let target_login = target_login.to_owned();
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::profile::fetch_profile(&self.client, &pat, &target_login, cancel).await
            })
            .await
    }

    pub async fn profile_pinned(
        &self,
        target_login: &str,
        limits: &ProfileLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<ProfilePinned>, ProfileError> {
        let target_login = target_login.to_owned();
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::profile::fetch_profile_pinned(
                    &self.client,
                    &pat,
                    &target_login,
                    limits,
                    cancel,
                )
                .await
            })
            .await
    }

    pub async fn contribution_calendar(
        &self,
        target_login: &str,
        range: &ContributionRange,
        limits: &ProfileLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<ContributionCalendar>, ProfileError> {
        let target_login = target_login.to_owned();
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::profile::fetch_contribution_calendar(
                    &self.client,
                    &pat,
                    &target_login,
                    range,
                    limits,
                    cancel,
                )
                .await
            })
            .await
    }

    pub async fn releases(
        &self,
        repository: &RepositoryIdentity,
        limits: &RepositoryExtrasLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<ReleasesOutcome>, RepositoryExtrasError> {
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::repository_extras::fetch_releases(
                    &self.client,
                    &pat,
                    repository,
                    limits,
                    cancel,
                )
                .await
            })
            .await
    }

    /// Explicit online latest-release read without conditional validators.
    pub async fn latest_release(
        &self,
        repository: &RepositoryIdentity,
        limits: &RepositoryExtrasLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<LatestReleaseOutcome>, RepositoryExtrasError> {
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::repository_extras::fetch_latest_release(
                    &self.client,
                    &pat,
                    repository,
                    limits,
                    None,
                    cancel,
                )
                .await
            })
            .await
    }

    pub async fn contributors(
        &self,
        repository: &RepositoryIdentity,
        limits: &RepositoryExtrasLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<ContributorsOutcome>, RepositoryExtrasError> {
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::repository_extras::fetch_contributors(
                    &self.client,
                    &pat,
                    repository,
                    limits,
                    cancel,
                )
                .await
            })
            .await
    }

    /// Explicit online language-byte read without conditional validators.
    pub async fn languages(
        &self,
        repository: &RepositoryIdentity,
        limits: &RepositoryExtrasLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<LanguagesOutcome>, RepositoryExtrasError> {
        self.repository
            .read(cancel, async move |pat, cancel| {
                github::repository_extras::fetch_languages(
                    &self.client,
                    &pat,
                    repository,
                    limits,
                    None,
                    cancel,
                )
                .await
            })
            .await
    }

    /// Discover a target profile's canonical same-name PUBLIC repository, fix
    /// its default-branch commit, then discover and prepare the root README.
    /// All steps use this one captured authorization context.
    pub async fn profile_readme(
        &self,
        profile: &ProfileBasic,
        repository_limits: &RepositoryLimits,
        blob_limits: &BlobLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<ProfileReadme>, RepositoryError> {
        let locator = RepositoryLocator::new(&profile.login, &profile.login)?;
        let details = self
            .repository
            .repository(locator, repository_limits, cancel.clone())
            .await?;
        let identity = details.value.identity.clone();
        let canonical_same_name = details
            .value
            .owner
            .login
            .eq_ignore_ascii_case(&profile.login)
            && identity
                .locator()
                .name()
                .eq_ignore_ascii_case(&profile.login);
        let default_branch = match (
            details.value.visibility,
            canonical_same_name,
            details.value.default_branch.clone(),
        ) {
            (RepoVisibility::Public, true, Some(branch)) => branch,
            (RepoVisibility::Public, true, None) => {
                return self.readme_outcome(ProfileReadme::NotEligible {
                    repository: identity,
                    reason: ProfileReadmeIneligible::NoDefaultBranch,
                });
            }
            _ => {
                return self.readme_outcome(ProfileReadme::NotEligible {
                    repository: identity,
                    reason: ProfileReadmeIneligible::NotPublicSameNameRepository,
                });
            }
        };
        let snapshot = self
            .repository
            .revision(
                &identity,
                RevisionSpec::Ref(default_branch),
                repository_limits,
                cancel.clone(),
            )
            .await?;
        let identity = snapshot.value.repository().clone();
        let commit = snapshot.value.commit().clone();
        // The repository README endpoint prefers .github/ and docs/ over the
        // root. Profile README eligibility requires the root README.md itself.
        let root = self
            .repository
            .directory(
                &SnapshotDirectory::root(&snapshot.value),
                repository_limits,
                cancel.clone(),
            )
            .await?;
        let readme_index = root.value.entries.iter().position(|entry| {
            entry.path.as_str().eq_ignore_ascii_case("README.md")
                && matches!(
                    entry.kind,
                    crate::domain::repository::TreeEntryKind::File { .. }
                )
        });
        if let Some(index) = readme_index {
            let content = self
                .repository
                .content(
                    &BlobIdentity::from_file(&root.value, index)?,
                    blob_limits,
                    cancel,
                )
                .await?;
            self.readme_outcome(ProfileReadme::Prepared {
                repository: identity,
                commit,
                content: content.value,
            })
        } else if root.value.completeness == Completeness::Complete
            && root.value.truncated == Some(false)
        {
            self.readme_outcome(ProfileReadme::Missing {
                repository: identity,
                commit,
            })
        } else {
            self.readme_outcome(ProfileReadme::Unavailable {
                repository: identity,
                commit,
            })
        }
    }

    fn readme_outcome(
        &self,
        value: ProfileReadme,
    ) -> Result<Scoped<ProfileReadme>, RepositoryError> {
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        Ok(Scoped {
            context: self.context().clone(),
            value,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileReadmeIneligible {
    NotPublicSameNameRepository,
    NoDefaultBranch,
}

/// A failed discovery/read is `Err(RepositoryError)`, never one of these data
/// states. Incomplete root discovery remains `Unavailable`, never `Missing`.
#[derive(Debug)]
pub enum ProfileReadme {
    Prepared {
        repository: RepositoryIdentity,
        commit: ObjectSha,
        content: PreparedContent,
    },
    Missing {
        repository: RepositoryIdentity,
        commit: ObjectSha,
    },
    Unavailable {
        repository: RepositoryIdentity,
        commit: ObjectSha,
    },
    NotEligible {
        repository: RepositoryIdentity,
        reason: ProfileReadmeIneligible,
    },
}
