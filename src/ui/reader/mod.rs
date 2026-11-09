//! Read-only repository content reader for the Native Shell.
//!
//! The reader turns `content::PreparedContent` (or a raw UTF-8 body) plus a
//! fixed repository/commit/path context into one of the reader's
//! presentations: Markdown, source text, or an honest status for an empty,
//! unsupported or over-budget file.
//!
//! It is pure Rust on top of NSH (`nsh-core` / `nsh-base`): no GPUI, no Ely,
//! no JS runtime. The host owns services, authorization and scheduling:
//!
//! * [`ReaderDocument`] is prepared on a background executor from the bytes the
//!   existing content service already accepted (2 MiB budget, binary and NUL
//!   rejection, BOM/line-ending preservation).
//! * [`ImageBridge`] adapts the existing `content::images::ImageProvider`; the
//!   reader never opens a URL itself and never feeds a README address to NSH's
//!   default image loader.
//! * [`ReaderHandle`] lives on the NSH UI thread: the host polls it from its own
//!   async drain, suspends it when hidden, replaces the document, and retires it
//!   when the captured account capability is revoked.
//!
//! Link presses never reach the system opener here. Each one is resolved
//! against the context (same repository, same commit, same directory) and
//! reported as a [`ReaderAction`]; dangerous schemes are refused. The
//! integration layer owns opening an external URL or switching files.
//!
//! See `docs/nsh-reader.md` for the mount/poll/replace contract, the image
//! invalidation rules and the remaining native verification boundaries.

// The reader is one slice the sibling page tasks are still wiring. Its public
// API is a contract for those pages, so the not-yet-consumed handles and the
// kept-for-compatibility parser surface are intentionally allowed dead here
// (the same scoped allowance the GPUI reader carried), not left to warn.
#![allow(dead_code)]

pub mod fixtures;
pub mod images;
pub mod links;
mod markdown;
mod model;
mod view;

#[allow(unused_imports)]
pub use crate::content::images::{
    DecodedImage, ImageCompletion, ImageFailure, ImageFormat, ImageKey, ImageLimits, ImageProvider,
    ImageRefusal, ImageRequest, ImageScope, ImageSink, ImageTarget,
};
#[allow(unused_imports)]
pub use images::{ImageBridge, Picture};
pub use links::LinkTarget;
#[allow(unused_imports)]
pub use model::{DocumentKind, MAX_TEXT_BYTES, ReaderBody, ReaderDocument};
#[allow(unused_imports)]
pub use view::{ReaderAction, ReaderHandle, ReaderLayout, dark_tokens, reader};

use crate::domain::blob::BlobIdentity;

/// The fixed repository version a document's relative links resolve against.
///
/// Built from a [`BlobIdentity`], so the commit is the one the bytes were read
/// at. Nothing here is a branch or a "latest" pointer.
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

    pub fn from_prepared(content: &crate::content::PreparedContent) -> Self {
        Self::from_identity(&content.identity)
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn commit(&self) -> &str {
        &self.commit
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}
