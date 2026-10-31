//! Deterministic tests for the Feed product policy.
//!
//! These describe the **confirmed** rules: only merged pull requests, starred
//! repositories and repository creation; the contribution subject is the pull
//! request author (never the merging actor); and every card's subject must be an
//! account the active account follows. Event shapes mirror the live public
//! `received_events` payloads recorded on 2026-10-30, including the slim
//! `action=merged` object that carries no author.

use super::*;

fn actor(id: u64, login: &str) -> FeedActor {
    FeedActor {
        id,
        login: login.to_string(),
        display_login: None,
        avatar_url: None,
    }
}

fn repository() -> FeedRepository {
    FeedRepository {
        id: 99,
        name: "subsurface/libdc".to_string(),
        url: "https://api.github.com/repos/subsurface/libdc".to_string(),
    }
}

fn event(id: &str, event_type: FeedEventType, payload: FeedPayload, actor: FeedActor) -> FeedEvent {
    FeedEvent {
        id: id.to_string(),
        event_type,
        actor,
        repository: repository(),
        created_at: Some("2026-10-30T12:00:41Z".to_string()),
        public: true,
        payload,
    }
}

/// A live `action=merged` event: slim pull_request (url/id/number/head/base),
/// no `merged` flag, no embedded author.
fn live_merged(id: &str, number: u64, merger: FeedActor) -> FeedEvent {
    event(
        id,
        FeedEventType::PullRequest,
        FeedPayload::PullRequest {
            action: "merged".into(),
            number: Some(number),
            title: None,
            html_url: None,
            state: None,
            merged: None,
            author: None,
        },
        merger,
    )
}

/// A source that embeds the author and the merge flag (still accepted).
fn classic_merged(id: &str, author: FeedAuthor, merger: FeedActor) -> FeedEvent {
    event(
        id,
        FeedEventType::PullRequest,
        FeedPayload::PullRequest {
            action: "closed".into(),
            number: Some(7),
            title: Some("Embedded".into()),
            html_url: None,
            state: Some("closed".into()),
            merged: Some(true),
            author: Some(author),
        },
        merger,
    )
}

fn starred(id: &str, by: FeedActor) -> FeedEvent {
    event(
        id,
        FeedEventType::Watch,
        FeedPayload::Watch {
            action: "started".into(),
        },
        by,
    )
}

fn created_repository(id: &str, by: FeedActor) -> FeedEvent {
    event(
        id,
        FeedEventType::Create,
        FeedPayload::Create {
            ref_type: Some("repository".into()),
            git_ref: None,
        },
        by,
    )
}

fn following(accounts: &[(&str, u64)], readiness: FollowingReadiness) -> Following {
    Following {
        accounts: accounts
            .iter()
            .map(|(login, id)| FollowedAccount {
                id: *id,
                login: (*login).to_string(),
                avatar_url: None,
            })
            .collect(),
        readiness,
    }
}

fn resolved(login: &str, id: u64) -> ResolvedPullRequest {
    ResolvedPullRequest {
        author: FeedAuthor {
            id,
            login: login.to_string(),
            avatar_url: None,
        },
        merged: true,
        title: Some("Resolved title".into()),
        html_url: Some("https://github.com/subsurface/libdc/pull/144".into()),
    }
}

fn complete() -> FollowingReadiness {
    FollowingReadiness::Complete
}

#[test]
fn a_live_merged_event_becomes_a_contribution_once_the_author_is_resolved() {
    let merged = live_merged("1", 144, actor(10, "maintainer"));
    let candidate = merge_candidate(&merged).expect("action=merged is merge evidence");
    assert_eq!(candidate.owner, "subsurface");
    assert_eq!(candidate.repository, "libdc");
    assert_eq!(candidate.pull_request.number, Some(144));

    let mut authors = AuthorResolutions::new();
    authors.insert(candidate.key(), resolved("shulaoda", 31));
    let outcome = build_feed(
        vec![merged],
        Vec::new(),
        &following(&[("shulaoda", 31)], complete()),
        &authors,
    );

    assert_eq!(outcome.activities.len(), 1);
    let activity = &outcome.activities[0];
    assert_eq!(activity.kind, FeedActivityKind::PullRequestMerged);
    assert_eq!(
        activity.subject.login, "shulaoda",
        "the author is credited, not the merger"
    );
    assert_ne!(activity.subject.login, activity.event.actor.login);
    assert_eq!(
        activity
            .pull_request
            .as_ref()
            .and_then(|target| target.number),
        Some(144)
    );
    assert_eq!(
        activity
            .pull_request
            .as_ref()
            .and_then(|target| target.title.as_deref()),
        Some("Resolved title")
    );
    assert_eq!(outcome.readiness, FeedReadiness::Complete);
    assert_eq!(
        outcome.activities[0].event.id, "1",
        "source identity is kept"
    );
}

#[test]
fn a_followed_author_qualifies_even_when_the_merger_is_not_followed() {
    let merged = live_merged("1", 144, actor(10, "drive-by-maintainer"));
    let mut authors = AuthorResolutions::new();
    authors.insert(
        merge_candidate(&merged).expect("candidate").key(),
        resolved("shulaoda", 31),
    );
    let outcome = build_feed(
        vec![merged],
        Vec::new(),
        // Only the author is followed.
        &following(&[("shulaoda", 31)], complete()),
        &authors,
    );
    assert_eq!(outcome.activities.len(), 1);
    assert_eq!(outcome.activities[0].subject.login, "shulaoda");
}

#[test]
fn a_followed_merger_does_not_qualify_when_the_author_is_not_followed() {
    let merged = live_merged("1", 144, actor(10, "followed-merger"));
    let mut authors = AuthorResolutions::new();
    authors.insert(
        merge_candidate(&merged).expect("candidate").key(),
        resolved("stranger", 31),
    );
    let outcome = build_feed(
        vec![merged],
        Vec::new(),
        // The merger is followed, the actual author is not.
        &following(&[("followed-merger", 10)], complete()),
        &authors,
    );
    assert!(
        outcome.activities.is_empty(),
        "the merger must never be substituted for the author"
    );
    assert_eq!(outcome.readiness, FeedReadiness::Complete);
}

#[test]
fn an_unresolved_merged_event_is_withheld_and_reported_not_silently_dropped() {
    let outcome = build_feed(
        vec![live_merged("1", 144, actor(10, "maintainer"))],
        Vec::new(),
        &following(&[("shulaoda", 31)], complete()),
        // No resolution was recorded at all.
        &AuthorResolutions::new(),
    );
    assert!(outcome.activities.is_empty());
    assert!(matches!(
        outcome.readiness,
        FeedReadiness::Partial { ref reasons }
            if reasons.iter().any(|reason| matches!(
                reason,
                FeedPartialReason::AuthorUnresolved { count: 1 }
            ))
    ));
}

#[test]
fn a_detail_response_that_does_not_confirm_the_merge_is_not_merge_evidence() {
    let merged = live_merged("1", 144, actor(10, "maintainer"));
    let mut authors = AuthorResolutions::new();
    authors.insert(
        merge_candidate(&merged).expect("candidate").key(),
        ResolvedPullRequest {
            author: FeedAuthor {
                id: 31,
                login: "shulaoda".into(),
                avatar_url: None,
            },
            merged: false,
            title: None,
            html_url: None,
        },
    );
    let outcome = build_feed(
        vec![merged],
        Vec::new(),
        &following(&[("shulaoda", 31)], complete()),
        &authors,
    );
    assert!(outcome.activities.is_empty());
    assert!(
        !outcome.readiness.is_complete(),
        "contradictory detail evidence is unresolved"
    );
}

#[test]
fn membership_uses_stable_ids_across_renames_and_login_reuse() {
    let following = following(&[("new-login", 31)], complete());
    assert!(following.follows_subject(&actor(31, "old-login")));
    assert!(!following.follows_subject(&actor(99, "new-login")));
}

#[test]
fn closed_events_never_qualify_without_merge_evidence() {
    let closed_without_merge = |merged: Option<bool>| {
        event(
            "1",
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                action: "closed".into(),
                number: Some(7),
                title: Some("Abandoned".into()),
                html_url: None,
                state: Some("closed".into()),
                merged,
                author: Some(FeedAuthor {
                    id: 31,
                    login: "shulaoda".into(),
                    avatar_url: None,
                }),
            },
            actor(10, "maintainer"),
        )
    };
    assert!(merge_candidate(&closed_without_merge(None)).is_none());
    assert!(merge_candidate(&closed_without_merge(Some(false))).is_none());

    let outcome = build_feed(
        vec![
            closed_without_merge(None),
            closed_without_merge(Some(false)),
        ],
        Vec::new(),
        &following(&[("shulaoda", 31)], complete()),
        &AuthorResolutions::new(),
    );
    assert!(outcome.activities.is_empty());
    assert_eq!(
        outcome.readiness,
        FeedReadiness::Complete,
        "an unmerged close is not an unresolved author"
    );
}

#[test]
fn opened_and_reopened_pull_requests_are_never_contributions() {
    for action in ["opened", "reopened"] {
        let candidate = event(
            "1",
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                action: action.into(),
                number: Some(7),
                title: None,
                html_url: None,
                state: Some("open".into()),
                merged: None,
                author: None,
            },
            actor(10, "someone"),
        );
        assert!(merge_candidate(&candidate).is_none(), "{action}");
        assert!(!is_retained_activity(&candidate), "{action}");
    }
}

#[test]
fn an_embedded_author_is_used_without_a_detail_read() {
    let embedded = classic_merged(
        "1",
        FeedAuthor {
            id: 31,
            login: "shulaoda".into(),
            avatar_url: None,
        },
        actor(10, "maintainer"),
    );
    let outcome = build_feed(
        vec![embedded],
        Vec::new(),
        &following(&[("shulaoda", 31)], complete()),
        // No resolution recorded: the embedded author is enough.
        &AuthorResolutions::new(),
    );
    assert_eq!(outcome.activities.len(), 1);
    assert_eq!(outcome.activities[0].subject.login, "shulaoda");
    assert_eq!(outcome.readiness, FeedReadiness::Complete);
}

#[test]
fn starred_and_created_repositories_require_a_followed_actor() {
    let outcome = build_feed(
        vec![
            starred("1", actor(1, "followed")),
            starred("2", actor(2, "not-followed")),
            created_repository("3", actor(1, "followed")),
            created_repository("4", actor(2, "not-followed")),
        ],
        Vec::new(),
        &following(&[("followed", 1)], complete()),
        &AuthorResolutions::new(),
    );
    let ids: Vec<&str> = outcome
        .activities
        .iter()
        .map(|activity| activity.event.id.as_str())
        .collect();
    assert_eq!(ids, vec!["1", "3"], "only followed actors qualify");
    assert_eq!(outcome.readiness, FeedReadiness::Complete);
    assert_eq!(
        outcome.activities[0].kind,
        FeedActivityKind::RepositoryStarred
    );
    assert_eq!(
        outcome.activities[1].kind,
        FeedActivityKind::RepositoryCreated
    );
}

#[test]
fn watching_and_branch_or_tag_creation_never_produce_a_card() {
    let events = vec![
        event(
            "1",
            FeedEventType::Watch,
            FeedPayload::Watch {
                action: "stopped".into(),
            },
            actor(1, "followed"),
        ),
        event(
            "2",
            FeedEventType::Create,
            FeedPayload::Create {
                ref_type: Some("branch".into()),
                git_ref: Some("feature".into()),
            },
            actor(1, "followed"),
        ),
        event(
            "3",
            FeedEventType::Create,
            FeedPayload::Create {
                ref_type: Some("tag".into()),
                git_ref: Some("v1".into()),
            },
            actor(1, "followed"),
        ),
        event(
            "4",
            FeedEventType::Delete,
            FeedPayload::Delete {
                ref_type: Some("repository".into()),
                git_ref: None,
            },
            actor(1, "followed"),
        ),
    ];
    let outcome = build_feed(
        events,
        Vec::new(),
        &following(&[("followed", 1)], complete()),
        &AuthorResolutions::new(),
    );
    assert!(outcome.activities.is_empty());
    assert_eq!(outcome.readiness, FeedReadiness::Complete);
}

#[test]
fn an_incomplete_following_list_withholds_unread_subjects_and_reports_partial() {
    let outcome = build_feed(
        vec![
            starred("1", actor(1, "known-followed")),
            starred("2", actor(2, "maybe-followed")),
        ],
        Vec::new(),
        &following(
            &[("known-followed", 1)],
            FollowingReadiness::Partial {
                reasons: vec![FollowingPartialReason::PageLimit],
            },
        ),
        &AuthorResolutions::new(),
    );
    let ids: Vec<&str> = outcome
        .activities
        .iter()
        .map(|activity| activity.event.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["1"],
        "a subject absent from a partial list is unknown, not 'not followed'"
    );
    assert!(
        matches!(
            outcome.readiness,
            FeedReadiness::Partial { ref reasons }
                if reasons.iter().any(|reason| matches!(
                    reason,
                    FeedPartialReason::FollowingIncomplete { .. }
                ))
        ),
        "unknown membership must never look like a complete empty Feed"
    );
}

#[test]
fn a_complete_following_list_that_lacks_every_subject_is_a_normal_empty_feed() {
    let outcome = build_feed(
        vec![starred("1", actor(2, "not-followed"))],
        Vec::new(),
        &following(&[("someone-else", 1)], complete()),
        &AuthorResolutions::new(),
    );
    assert!(outcome.activities.is_empty());
    assert_eq!(
        outcome.readiness,
        FeedReadiness::Complete,
        "policy filtering is not truncation"
    );
}

#[test]
fn source_partial_reasons_survive_the_policy() {
    let outcome = build_feed(
        vec![starred("1", actor(1, "followed"))],
        vec![FeedPartialReason::PageFailed {
            message: "synthetic".into(),
        }],
        &following(&[("followed", 1)], complete()),
        &AuthorResolutions::new(),
    );
    assert_eq!(outcome.activities.len(), 1);
    assert!(matches!(
        outcome.readiness,
        FeedReadiness::Partial { ref reasons }
            if reasons.iter().any(|reason| matches!(reason, FeedPartialReason::PageFailed { .. }))
    ));
}

#[test]
fn following_matching_is_case_insensitive_and_ignores_blank_logins() {
    let following = following(&[("MixedCase", 1)], complete());
    assert!(following.follows("mixedcase"));
    assert!(following.follows("MIXEDCASE"));
    assert!(!following.follows(""));
    assert!(!following.follows("   "));
}

#[test]
fn author_resolutions_are_keyed_and_replaceable() {
    let mut authors = AuthorResolutions::new();
    authors.insert("subsurface/libdc#144", resolved("first", 1));
    assert_eq!(
        authors
            .get("subsurface/libdc#144")
            .and_then(|value| value.as_ref())
            .map(|value| value.author.login.as_str()),
        Some("first")
    );
    // A refresh must not keep a stale author for the same target.
    authors.insert("subsurface/libdc#144", resolved("second", 2));
    assert_eq!(
        authors
            .get("subsurface/libdc#144")
            .and_then(|value| value.as_ref())
            .map(|value| value.author.login.as_str()),
        Some("second")
    );
    authors.mark_unresolved("subsurface/libdc#144");
    assert!(
        authors
            .get("subsurface/libdc#144")
            .is_some_and(Option::is_none)
    );
    assert!(authors.get("other#1").is_none());
}

#[test]
fn a_merge_candidate_without_a_usable_repository_identity_is_not_retained() {
    let mut broken = live_merged("1", 144, actor(10, "maintainer"));
    broken.repository.name = "no-separator".to_string();
    assert!(merge_candidate(&broken).is_none());
    let mut empty_owner = live_merged("1", 144, actor(10, "maintainer"));
    empty_owner.repository.name = "/libdc".to_string();
    assert!(merge_candidate(&empty_owner).is_none());
}

#[test]
fn feed_page_appends_without_duplicating_overlapping_events() {
    let following = following(&[("shulaoda", 31), ("friend", 1)], complete());
    let mut authors = AuthorResolutions::new();
    authors.insert(
        "subsurface/libdc#144",
        ResolvedPullRequest {
            author: FeedAuthor {
                id: 31,
                login: "shulaoda".into(),
                avatar_url: None,
            },
            merged: true,
            title: None,
            html_url: None,
        },
    );

    // First page: one star and a merged contribution.
    let first_page = FeedSourcePage {
        events: vec![
            starred("s1", actor(1, "friend")),
            live_merged("m1", 144, actor(10, "maintainer")),
        ],
        next: Some(FeedSourceCursor { page: 2 }),
        reasons: vec![],
        etag: Some("\"feed\"".into()),
        poll_interval_seconds: Some(60),
    };
    let mut seen = HashSet::new();
    let first = build_feed_page(&first_page, &following, &authors, &mut seen);
    let mut outcome = first.into_outcome();
    assert_eq!(outcome.activities.len(), 2);
    assert_eq!(outcome.next_page, Some(FeedSourceCursor { page: 2 }));

    // Second page overlaps one event and adds one new star; the overlapped merged
    // event must not add a duplicate card.
    let second_page = FeedSourcePage {
        events: vec![
            live_merged("m1", 144, actor(10, "maintainer")), // duplicate
            starred("s2", actor(1, "friend")),               // new
        ],
        next: None,
        reasons: vec![],
        etag: None,
        poll_interval_seconds: None,
    };
    let page = build_feed_page(&second_page, &following, &authors, &mut seen);
    outcome.extend(page);
    let ids: Vec<&str> = outcome
        .activities
        .iter()
        .map(|activity| activity.event.id.as_str())
        .collect();
    assert_eq!(ids, vec!["s1", "m1", "s2"], "overlap must not duplicate");
    assert_eq!(outcome.next_page, None, "second page ends the timeline");
}

#[test]
fn a_filtered_empty_page_keeps_the_cursor_for_the_next_page() {
    // A page that parses to zero visible activities is not end-of-history when a
    // next page exists: the cursor must survive so the host can continue.
    let following = following(&[("friend", 1)], complete());
    let page = FeedSourcePage {
        events: vec![
            // All technical noise / unfollowed subjects.
            event(
                "push1",
                FeedEventType::Push,
                FeedPayload::Push {
                    git_ref: None,
                    before: None,
                    head: None,
                    size: None,
                },
                actor(9, "noise"),
            ),
        ],
        next: Some(FeedSourceCursor { page: 3 }),
        reasons: vec![],
        etag: None,
        poll_interval_seconds: None,
    };
    let mut seen = HashSet::new();
    let built = build_feed_page(&page, &following, &AuthorResolutions::new(), &mut seen);
    assert!(built.activities.is_empty());
    assert_eq!(built.next_page, Some(FeedSourceCursor { page: 3 }));
}
