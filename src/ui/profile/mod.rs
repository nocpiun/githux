//! Reusable Profile Overview presentation.
//!
//! The host supplies already-read public profile data, prepared same-name
//! repository README content and an explicit validated contribution interval.
//! This module performs no I/O, authorization inference, avatar fetching,
//! README discovery or date defaulting. Fixture scenarios live only in the
//! standalone example/test assembly.

#![allow(dead_code)] // The future Shell adapter owns mounting this component.

mod header;
mod heatmap;
mod model;
mod render;
mod view;

#[cfg(test)]
mod tests;

pub use heatmap::{CellLevel, DayPresentation, HeatmapPresentation, heatmap_presentation};
pub use model::{
    PresentationError, ProfileBinding, ProfileRequest, ProfileTarget, RepositoryPresentation,
    RepositoryTarget, SectionState, repository_presentation,
};
pub use view::ProfileView;

use crate::ui::reader::ReaderAction;

/// User intent only. The host decides whether and how to navigate/open a URL.
/// The request key identifies the binding that produced the action, allowing a
/// host to reject an event from a stale page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileAction {
    pub request: ProfileRequest,
    pub kind: ProfileActionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileActionKind {
    Profile,
    Repository(RepositoryTarget),
    ExternalLink { url: String },
    Readme(ReaderAction),
}
