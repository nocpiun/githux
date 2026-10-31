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
pub mod blob;
pub mod error;
pub mod home;
pub mod profile;
pub mod repo;
pub mod repository;
pub mod repository_counts;
pub mod repository_extras;

// Re-exported surface for the pending UI integration; see docs/auth-and-network.md
// for the minimal call example. Nothing in the current binary consumes it yet.
#[allow(unused_imports)]
pub use account::{AccountIdentity, AccountKey, AccountSummary, AuthEpoch, HostId};
#[allow(unused_imports)]
pub use error::GitHubError;
#[allow(unused_imports)]
pub use home::{
    AuthorResolutions, DateBound, FeedActivity, FeedActivityKind, FeedActivityTarget, FeedActor,
    FeedAuthor, FeedEvent, FeedEventType, FeedEventsRead, FeedFetch, FeedMergeCandidate,
    FeedOutcome, FeedPage, FeedPartialReason, FeedPayload, FeedReadiness, FeedRepository,
    FeedSourceCursor, FeedSourcePage, FollowedAccount, Following, FollowingPartialReason,
    FollowingReadiness, HomeError, HomeItem, HomeItemKind, HomePartialReason, HomeQuery,
    HomeReadiness, HomeRepositoryRef, HomeSearchOutcome, Login, PullRequestDetail,
    PullRequestDetailOutcome, ResolvedPullRequest, UpdatedWindow, build_feed,
    build_feed_activities, build_feed_page, is_retained_activity, local_activity, merge_candidate,
};
#[allow(unused_imports)]
pub use profile::{
    ContributionCalendar, ContributionDay, ContributionMonth, ContributionRange, ContributionWeek,
    GraphqlErrorKind, ProfileBasic, ProfileError, ProfileGraphqlError, ProfilePinned, ProfileRepo,
    Showcase,
};
#[allow(unused_imports)]
pub use repo::{RepoOwner, RepoSummary, RepoVisibility, ReposOutcome, ReposReadiness};
