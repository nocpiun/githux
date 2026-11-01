//! Presentation-only identity, section states and repository policy.

use std::fmt;

use crate::domain::{
    AccountKey, AuthEpoch, GitHubError, GraphqlErrorKind, HostId, ProfileError, ProfilePinned,
    ProfileRepo, Showcase,
};

/// The profile being displayed. This is a resource identity, never evidence
/// about which account may read it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileTarget {
    pub host: HostId,
    pub id: u64,
}

impl ProfileTarget {
    pub fn new(host: HostId, id: u64) -> Self {
        Self { host, id }
    }
}

/// Local binding captured by the host. A new account, authorization epoch or
/// service selection must produce a new binding/generation before results are
/// published. The view never turns a profile login into this identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileBinding {
    pub account: AccountKey,
    pub epoch: AuthEpoch,
}

impl ProfileBinding {
    pub fn new(account: AccountKey, epoch: AuthEpoch) -> Self {
        Self { account, epoch }
    }
}

/// Key attached to every host update and every emitted user action.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileRequest {
    pub target: ProfileTarget,
    pub binding: ProfileBinding,
    pub generation: u64,
}

/// Host-observed loading, failure or usable data. No failed or missing read is
/// converted into a successful empty section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SectionState<T> {
    #[default]
    Loading,
    Error(PresentationError),
    /// A confirmed non-error absence, used by the Profile README section. This is not a failed
    /// read and must not be rendered as an authentication/network problem.
    Unavailable(String),
    Ready(T),
}

/// Safe, typed presentation failure. Raw service errors are not needed by the
/// view, so integration maps them without adding credentials or server text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresentationError {
    Unauthorized,
    Forbidden,
    NotFound,
    RateLimited,
    SsoRequired,
    Offline,
    Timeout,
    Cancelled,
    InvalidData,
    Unavailable,
}

impl PresentationError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Unauthorized => "Authentication is required or expired.",
            Self::Forbidden | Self::SsoRequired => {
                "This section is not available with the current authorization."
            }
            Self::NotFound => "This section was not found or is not visible.",
            Self::RateLimited => "GitHub rate limited this read. Try again later.",
            Self::Offline => "This section could not be read while offline.",
            Self::Timeout => "This section timed out.",
            Self::Cancelled => "This read was cancelled.",
            Self::InvalidData => "GitHub returned data this view cannot understand.",
            Self::Unavailable => "This section is temporarily unavailable.",
        }
    }
}

impl From<GraphqlErrorKind> for PresentationError {
    fn from(kind: GraphqlErrorKind) -> Self {
        match kind {
            GraphqlErrorKind::NotFound => Self::NotFound,
            GraphqlErrorKind::Unauthorized => Self::Unauthorized,
            GraphqlErrorKind::Forbidden => Self::Forbidden,
            GraphqlErrorKind::RateLimitPrimary | GraphqlErrorKind::RateLimitSecondary => {
                Self::RateLimited
            }
            GraphqlErrorKind::SsoRequired { .. } => Self::SsoRequired,
            GraphqlErrorKind::Unknown => Self::Unavailable,
        }
    }
}

impl From<ProfileError> for PresentationError {
    fn from(error: ProfileError) -> Self {
        match error {
            ProfileError::GitHub(error) => match error {
                GitHubError::Unauthorized | GitHubError::StaleContext => Self::Unauthorized,
                GitHubError::RateLimited { .. } => Self::RateLimited,
                GitHubError::SsoRequired { .. } => Self::SsoRequired,
                GitHubError::Forbidden => Self::Forbidden,
                GitHubError::NotFound => Self::NotFound,
                GitHubError::Network { .. } => Self::Offline,
                GitHubError::Timeout { .. } => Self::Timeout,
                GitHubError::Cancelled => Self::Cancelled,
                GitHubError::Parse { .. } => Self::InvalidData,
                GitHubError::UnexpectedStatus { .. }
                | GitHubError::ResponseTooLarge { .. }
                | GitHubError::CredentialStore { .. } => Self::Unavailable,
            },
            ProfileError::Graphql { kind, .. } => kind.into(),
            ProfileError::InvalidInput { .. } | ProfileError::InvalidResponse { .. } => {
                Self::InvalidData
            }
            ProfileError::RedirectNotFollowed { .. } | ProfileError::LimitExceeded { .. } => {
                Self::Unavailable
            }
        }
    }
}

/// Stable repository resource carried by an action. `id` remains optional
/// because the existing API model permits older/unknown `databaseId` values;
/// the view never invents one from a login or URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryTarget {
    pub id: Option<u64>,
    pub name_with_owner: String,
    pub url: Option<String>,
}

impl RepositoryTarget {
    pub fn from_repo(repo: &ProfileRepo) -> Self {
        let name_with_owner = repo.name_with_owner.clone().unwrap_or_else(|| {
            repo.owner_login
                .as_ref()
                .map(|owner| format!("{owner}/{}", repo.name))
                .unwrap_or_else(|| repo.name.clone())
        });
        Self {
            id: repo.id,
            name_with_owner,
            url: repo.url.clone(),
        }
    }
}

/// Resolved display policy for the repository section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryPresentation {
    pub title: &'static str,
    pub repositories: Vec<ProfileRepo>,
    pub note: Option<String>,
    pub empty: Option<&'static str>,
}

fn observed_repositories(pinned: &ProfilePinned, showcase: &[ProfileRepo]) -> Vec<ProfileRepo> {
    if pinned.repos.is_empty() && !showcase.is_empty() {
        // Partial GraphQL data can leave one connection unavailable while the
        // other still contains typed repository nodes. Keep those observations.
        showcase.to_vec()
    } else {
        // The dedicated repository-pin connection defines repository pin order.
        pinned.repos.clone()
    }
}

fn pin_notes(pinned: &ProfilePinned, gists: usize, truncated: bool) -> Option<String> {
    let mut notes = Vec::new();
    if gists > 0 {
        notes.push(format!(
            "{gists} gist pin{} observed; gist browsing is not included.",
            if gists == 1 { "" } else { "s" }
        ));
    }
    if truncated || pinned.pinned_truncated {
        let total = usize::try_from(pinned.total_repos).unwrap_or(usize::MAX);
        notes.push(format!(
            "Only a prefix is shown ({} of {total} repository pins observed).",
            pinned.repos.len()
        ));
    }
    if pinned.showcase_truncated && !truncated {
        notes.push("The pin showcase was truncated; its full classification is unknown.".into());
    }
    (!notes.is_empty()).then(|| notes.join(" "))
}

/// Apply the approved policy without changing server order or manufacturing a
/// fallback. GitHub Popular is used only when `has_pinned_items` is false *and*
/// the service classified the result as Popular.
pub fn repository_presentation(pinned: &ProfilePinned) -> RepositoryPresentation {
    if !pinned.has_pinned_items {
        if let Showcase::PopularRepositories(repositories) = &pinned.showcase {
            return RepositoryPresentation {
                title: "Popular repositories",
                repositories: repositories.clone(),
                note: partial_note(&pinned.errors),
                empty: Some("GitHub returned no Popular repositories."),
            };
        }
        return RepositoryPresentation {
            title: "Pinned repositories",
            repositories: pinned.repos.clone(),
            note: Some(
                "The showcase was not confirmed as GitHub Popular; no fallback was invented."
                    .into(),
            ),
            empty: Some("No typed repository pins were returned."),
        };
    }

    let (repositories, gists, truncated, extra) = match &pinned.showcase {
        Showcase::PinnedRepositories(repositories) => {
            (observed_repositories(pinned, repositories), 0, false, None)
        }
        Showcase::PinnedWithGists {
            repositories,
            gists,
        } => (observed_repositories(pinned, repositories), *gists, false, None),
        Showcase::PinnedGistsOnly { gists } => {
            (pinned.repos.clone(), *gists, false, Some("No repository pins were returned; this does not mean the profile has no repositories.".to_string()))
        }
        Showcase::Truncated {
            repositories,
            gists,
        } => (observed_repositories(pinned, repositories), *gists, true, None),
        Showcase::Unclassified => (
            pinned.repos.clone(),
            0,
            false,
            Some("Some pins could not be classified; no Popular fallback is shown.".into()),
        ),
        Showcase::PopularRepositories(_) => (
            pinned.repos.clone(),
            0,
            false,
            Some("Pins exist, but the showcase classification is inconsistent; no Popular fallback is shown.".into()),
        ),
    };
    let mut notes = Vec::new();
    notes.extend(extra);
    notes.extend(pin_notes(pinned, gists, truncated));
    notes.extend(partial_note(&pinned.errors));
    RepositoryPresentation {
        title: "Pinned repositories",
        repositories,
        note: (!notes.is_empty()).then(|| notes.join(" ")),
        empty: Some(
            "No repository pins were returned; this is not proof that no repositories exist.",
        ),
    }
}

fn partial_note(errors: &[crate::domain::ProfileGraphqlError]) -> Option<String> {
    (!errors.is_empty()).then(|| {
        format!(
            "Partial data: {} field error{} accompanied the usable results.",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" }
        )
    })
}

impl fmt::Display for ProfileTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.host, self.id)
    }
}
