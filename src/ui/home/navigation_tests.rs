//! Unit coverage for the Home Issue/PR destination seam. These cases are pure: no window, route
//! table or host is involved, so they prove only that the identity mapping keeps kind, repository
//! and number distinct. Opening and reusing the real tab is covered by the host tests.

use super::*;
use crate::domain::home::HomeItemKind;
use crate::ui::home::{RepositoryIdentity, WorkItemIdentity};

fn identity(kind: HomeItemKind, owner: &str, name: &str, number: u64) -> WorkItemIdentity {
    WorkItemIdentity {
        id: 1_000 + number,
        kind,
        number,
        repository: RepositoryIdentity {
            id: None,
            owner: owner.to_string(),
            name: name.to_string(),
        },
    }
}

#[test]
fn issue_and_pull_request_stay_distinct_at_the_same_number() {
    let issue = work_item_target(&identity(HomeItemKind::Issue, "octo", "repo", 7))
        .expect("a valid issue identity");
    let pull_request = work_item_target(&identity(HomeItemKind::PullRequest, "octo", "repo", 7))
        .expect("a valid pull request identity");
    assert_ne!(issue, pull_request);
    assert_eq!(issue.kind(), HomeItemKind::Issue);
    assert_eq!(pull_request.kind(), HomeItemKind::PullRequest);
    assert_eq!(issue.number(), 7);
    assert_eq!(pull_request.number(), 7);
    assert_eq!(issue.repository(), ("octo", "repo"));
}

#[test]
fn the_same_number_in_another_repository_is_another_target() {
    let first = work_item_target(&identity(HomeItemKind::Issue, "octo", "repo", 7))
        .expect("a valid issue identity");
    let second = work_item_target(&identity(HomeItemKind::Issue, "other", "repo", 7))
        .expect("a valid issue identity");
    assert_ne!(first, second);
    assert_eq!(first.repository(), ("octo", "repo"));
    assert_eq!(second.repository(), ("other", "repo"));
}

#[test]
fn the_target_keeps_the_repository_spelling_the_row_displayed() {
    // Case is preserved in the destination; the workspace, not this seam, decides alias reuse.
    let target = work_item_target(&identity(HomeItemKind::PullRequest, "Octo", "Repo", 44))
        .expect("a valid pull request identity");
    assert_eq!(target.repository(), ("Octo", "Repo"));
    assert_eq!(target.number(), 44);
}

#[test]
fn unusable_identities_produce_no_destination_instead_of_a_guessed_route() {
    // Zero is not a repository-local number GitHub ever returns.
    assert_eq!(
        work_item_target(&identity(HomeItemKind::Issue, "octo", "repo", 0)),
        None
    );
    // Owner/name come from the API; a value that cannot address a repository is refused rather
    // than turned into a route segment.
    assert_eq!(
        work_item_target(&identity(HomeItemKind::Issue, "", "repo", 7)),
        None
    );
    assert_eq!(
        work_item_target(&identity(HomeItemKind::Issue, "octo", "re/po", 7)),
        None
    );
    assert_eq!(
        work_item_target(&identity(HomeItemKind::PullRequest, "oc\nto", "repo", 7)),
        None
    );
}
