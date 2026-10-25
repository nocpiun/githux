//! GitHub REST API adaptation for Classic PAT reads.
//!
//! Responsibilities:
//! - Fixed `User-Agent`, `Accept`, `X-GitHub-Api-Version` headers.
//! - Authorization injected **only** for the trusted HTTPS `api.github.com`
//!   host (tests use an explicit localhost mock; redirects to other hosts or
//!   HTTP downgrade are rejected and the header is never forwarded).
//! - `GET /user` for stable account identity verification.
//! - `GET /user/repos` with full `Link` pagination and affiliation
//!   `owner,collaborator,organization_member`.
//! - Typed error classification and rate-limit/Retry-After parsing.
//!
//! Status: first reusable R2 slice. GET reads only.

// The GPUI shell does not yet consume this service layer (that is the separate
// UI-integration task). This module-scoped allowance is limited to these
// not-yet-called modules so real warnings stay visible elsewhere; the crate
// root deliberately has no `#![allow]` blanket.
#![allow(dead_code)]

pub mod blob;
pub mod branches;
pub mod client;
pub mod error;
pub mod headers;
pub mod permission;
pub mod readme;
pub mod repos;
pub mod repository;
pub mod repository_extras;
pub mod revision;
pub mod tree;
pub mod user;

#[allow(unused_imports)]
pub use branches::fetch_branches;
#[allow(unused_imports)]
pub use client::{GitHubClient, GitHubConfig};
#[allow(unused_imports)]
pub use headers::{parse_link_next, parse_sso_authorization_url, parse_sso_header};
#[allow(unused_imports)]
pub use permission::{WriteAction, WritePermission, is_write_confirmed_allowed, write_permission};
#[allow(unused_imports)]
pub use readme::discover_readme;
#[allow(unused_imports)]
pub use repos::fetch_user_repos;
#[allow(unused_imports)]
pub use repository::{RepositoryLimits, fetch_repository};
#[allow(unused_imports)]
pub use repository_extras::{
    RepositoryExtrasLimits, fetch_contributors, fetch_languages, fetch_latest_release,
    fetch_releases,
};
#[allow(unused_imports)]
pub use revision::resolve_revision;
#[allow(unused_imports)]
pub use tree::fetch_directory;
#[allow(unused_imports)]
pub use user::{AuthenticatedUser, fetch_authenticated_user};
