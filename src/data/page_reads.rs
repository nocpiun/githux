//! Account-scoped online reads for Home/Feed, Profile and repository extras.
//!
//! Every adapter uses the one RepositorySession authorization path. This module
//! owns no PAT, credential loading, semaphore, cache, polling or UI state.
use super::{AccountService, RepoService, RepositoryReadContext, RepositorySession, Scoped};
use crate::content::PreparedContent;
use crate::domain::blob::{BlobIdentity, BlobLimits};
use crate::domain::error::GitHubError;
use crate::domain::home::{
    FeedFetch, HomeError, HomeQuery, HomeSearchOutcome, Login, UpdatedWindow,
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
use crate::github::home::{FeedLimits, HomeSearchLimits};
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

    /// Explicit online Feed read. No validator is accepted or sent; an
    /// unexpected 304 is a typed HomeError, never NotModified/empty events.
    pub async fn feed(
        &self,
        limits: &FeedLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<FeedFetch>, HomeError> {
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
                        message: "unexpected 304 without a conditional validator".into(),
                    }),
                    events => Ok(events),
                },
            )
            .await
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
