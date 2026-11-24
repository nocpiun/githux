//! One validated workspace destination for a Home Issue/PR row.
//!
//! Home already carries the stable work-item identity (global id, kind, repository-local number
//! and repository). This module turns that identity into the coordinates a workspace tab needs,
//! once, for both kinds, so the host cannot derive a destination from a display title and cannot
//! drop the Issue/PR distinction: an Issue and a Pull request in the same repository with the same
//! number stay two different targets, and the same number in another repository stays another
//! target. Invalid identities (zero number, unusable owner/name) produce no target at all instead
//! of a guessed route.
//!
//! This module only describes a destination. Registering routes, reusing tabs and rendering
//! details stay with the workspace and the owning detail pages.

use crate::domain::home::HomeItemKind;
use crate::ui::home::WorkItemIdentity;

/// One validated Issue/PR destination: repository (namespace, name) plus repository-local number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkItemTarget {
    Issue {
        namespace: String,
        name: String,
        number: u64,
    },
    PullRequest {
        namespace: String,
        name: String,
        number: u64,
    },
}

impl WorkItemTarget {
    pub fn kind(&self) -> HomeItemKind {
        match self {
            Self::Issue { .. } => HomeItemKind::Issue,
            Self::PullRequest { .. } => HomeItemKind::PullRequest,
        }
    }

    /// Repository coordinates, exactly as validated from the identity.
    pub fn repository(&self) -> (&str, &str) {
        match self {
            Self::Issue {
                namespace, name, ..
            }
            | Self::PullRequest {
                namespace, name, ..
            } => (namespace, name),
        }
    }

    pub fn number(&self) -> u64 {
        match self {
            Self::Issue { number, .. } | Self::PullRequest { number, .. } => *number,
        }
    }
}

/// The workspace destination for one Home work item, or `None` when the identity is not usable.
///
/// The title is never consulted: the kind comes from the typed `HomeItemKind`, and the repository,
/// number and kind all come from the same identity the caller displayed.
pub fn work_item_target(identity: &WorkItemIdentity) -> Option<WorkItemTarget> {
    let number = identity.number;
    if number == 0 {
        return None;
    }
    let locator = crate::domain::repository::RepositoryLocator::new(
        identity.repository.owner.clone(),
        identity.repository.name.clone(),
    )
    .ok()?;
    let namespace = locator.owner().to_string();
    let name = locator.name().to_string();
    Some(match identity.kind {
        HomeItemKind::Issue => WorkItemTarget::Issue {
            namespace,
            name,
            number,
        },
        HomeItemKind::PullRequest => WorkItemTarget::PullRequest {
            namespace,
            name,
            number,
        },
    })
}
