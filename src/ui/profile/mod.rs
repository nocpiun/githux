//! Reusable Profile Overview presentation on Native Shell.
//!
//! The host supplies already-read public profile data, a prepared same-name
//! repository README document and an explicit validated contribution interval.
//! This module performs no I/O, authorization inference, avatar fetching, README
//! discovery or date defaulting. Fixture scenarios live only in the standalone
//! example/test assembly.

mod heatmap;
mod model;
mod render;
mod view;

#[cfg(test)]
mod tests;

pub use heatmap::{CellLevel, DayPresentation, HeatmapPresentation, heatmap_presentation};
pub use model::{
    PresentationError, ProfileBinding, ProfileProvenance, ProfileRequest, ProfileTarget,
    RepositoryPresentation, RepositoryTarget, SectionState, repository_presentation,
};
pub use view::ProfileView;

pub(crate) use crate::domain::ProfileRepo;

use crate::ui::reader::ReaderAction;

/// User intent only. The host decides whether and how to navigate/open a URL.
/// The request key identifies the binding that produced the action, allowing a
/// host to reject an event from a stale page.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileAction {
    pub request: ProfileRequest,
    pub kind: ProfileActionKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProfileActionKind {
    Profile,
    Repository(RepositoryTarget),
    ExternalLink { url: String },
    Readme(ReaderAction),
    RetryBasic,
    RetryRepositories,
    RetryContributions,
    RetryReadme,
}

impl From<ReaderAction> for ProfileActionKind {
    fn from(action: ReaderAction) -> Self {
        Self::Readme(action)
    }
}
