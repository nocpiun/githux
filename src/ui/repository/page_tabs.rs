//! The repository page's own horizontal content tabs.
//!
//! These are the *in-page* tabs from the confirmed product structure — summary,
//! then horizontal content tabs, then the Code area — and they are deliberately
//! separate from the application-level workspace tab strip in `ui::tabs`.
//!
//! This round implements only the Code tab. The other entries reuse the
//! confirmed repository naming and are drawn as normal tabs whose content area
//! stays empty, exactly as the product convention requires: no placeholder
//! cards, no fake settings, no copied GitHub feature menus. Releases,
//! Contributors and Languages are owned by the separate overview component and
//! are not wired here.

/// One in-page content tab of a repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RepositoryTab {
    Code,
    Issues,
    PullRequests,
    Releases,
    Settings,
}

impl RepositoryTab {
    /// Every tab this page draws, in order.
    pub const ALL: [RepositoryTab; 5] = [
        RepositoryTab::Code,
        RepositoryTab::Issues,
        RepositoryTab::PullRequests,
        RepositoryTab::Releases,
        RepositoryTab::Settings,
    ];

    pub fn icon(self) -> ely_core::primitives::IconName {
        use ely_core::primitives::IconName;
        match self {
            Self::Code => IconName::Code,
            Self::Issues => IconName::CircleDot,
            Self::PullRequests => IconName::GitPullRequest,
            Self::Releases => IconName::Tag,
            Self::Settings => IconName::Settings,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Issues => "Issues",
            Self::PullRequests => "Pull requests",
            Self::Releases => "Releases",
            Self::Settings => "Settings",
        }
    }

    /// The stable identifier used for element ids and debug selectors.
    pub fn id(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Issues => "issues",
            Self::PullRequests => "pulls",
            Self::Releases => "releases",
            Self::Settings => "settings",
        }
    }

    /// Whether this round has content for the tab. Only Code does.
    pub fn has_content(self) -> bool {
        matches!(self, Self::Code)
    }
}
