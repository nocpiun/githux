//! Framework-independent business types shared across auth, network, github and data layers.
//!
//! These types carry no secrets and no I/O: they describe stable identities,
//! authorization epochs, repository summaries and typed errors so that later
//! UI integration only depends on [`AccountSummary`], [`AuthEpoch`], [`RepoSummary`]
//! and [`GitHubError`], never on token internals.
//!
//! Status: first reusable slice of R2 (PAT verification + credential storage +
//! account epochs + cancellable GitHub reads). Not yet wired into any UI.

// The GPUI shell does not yet consume this service layer (that is the separate
// UI-integration task). This module-scoped allowance is limited to these
// not-yet-called modules so real warnings stay visible elsewhere; the crate
// root deliberately has no `#![allow]` blanket.
#![allow(dead_code)]

pub mod account;
pub mod error;
pub mod repo;

// Re-exported surface for the pending UI integration; see docs/auth-and-network.md
// for the minimal call example. Nothing in the current binary consumes it yet.
#[allow(unused_imports)]
pub use account::{AccountIdentity, AccountKey, AccountSummary, AuthEpoch, HostId};
#[allow(unused_imports)]
pub use error::GitHubError;
#[allow(unused_imports)]
pub use repo::{RepoOwner, RepoSummary, RepoVisibility, ReposOutcome, ReposReadiness};
