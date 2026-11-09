//! Native Rust reader for NSH v0.4.0. Host owns services, authorization and scheduling.
//! See docs/nsh-reader.md for dependencies, mount/poll/replace and native evidence boundaries.
pub mod images;
pub mod links;
mod model;
mod view;
mod parser {
    // The copied parser keeps the GPUI reader's full API surface; the NSH
    // consumer uses a subset, so a few kept-for-compatibility variants and
    // helpers are intentionally not exercised here (L006: innermost allow,
    // remove once the NSH consumer covers them).
    #![allow(dead_code)]
    pub(super) mod ast;
    pub(super) mod html;
}

use crate::domain::blob::BlobIdentity;
pub use links::{LinkTarget, RepositoryLink};
pub use model::{DocumentKind, MAX_TEXT_BYTES, ReaderBody, ReaderDocument};
pub use view::{ReaderAction, ReaderHandle, ReaderLayout, dark_tokens, reader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceContext {
    pub owner: String,
    pub name: String,
    pub commit: String,
    pub path: String,
}
impl ResourceContext {
    pub fn new(
        owner: impl Into<String>,
        name: impl Into<String>,
        commit: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
            commit: commit.into(),
            path: path.into(),
        }
    }
    pub fn from_identity(identity: &BlobIdentity) -> Self {
        Self::new(
            identity.repository().locator().owner(),
            identity.repository().locator().name(),
            identity.commit().as_str(),
            identity.path().as_str(),
        )
    }
}
