//! Behavioral coverage for the overview state machine and its compact summary helpers.
//!
//! The component only stores host-supplied observations: these tests prove stale bindings and stale
//! updates are rejected, partial/historical states keep their provenance, and the pure summary
//! helpers never invent a share or a name. No window, service or network is involved.

use super::summary::{contributor_display, format_share, language_segments};
use super::*;
use crate::domain::account::{AccountKey, HostId};
use crate::domain::repository::{RepositoryIdentity, RepositoryLocator};
use crate::domain::repository_extras::{
    Contributor, ExtrasCompleteness, LanguageStat, LanguagesOutcome, LatestReleaseOutcome, Release,
    ReleasesOutcome, ResponseMetadata,
};

fn repository() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 42,
        locator: RepositoryLocator::new("octo", "ledger").unwrap(),
    }
}

fn binding(generation: u64) -> OverviewBinding {
    OverviewBinding::new(
        AccountKey::new(1, HostId::github()),
        repository(),
        generation,
    )
}

fn metadata() -> ResponseMetadata {
    ResponseMetadata {
        etag: None,
        last_modified: None,
        cache_control: None,
        poll_interval_seconds: None,
    }
}

fn release(id: u64, tag: &str) -> Release {
    Release {
        id,
        node_id: None,
        tag_name: tag.into(),
        name: Some(tag.into()),
        body: None,
        html_url: Some(format!("https://github.com/octo/ledger/releases/tag/{tag}")),
        created_at: None,
        published_at: Some("2026-10-20T10:00:00Z".into()),
        draft: false,
        prerelease: false,
        raw: serde_json::Value::Null,
    }
}

fn releases(releases: Vec<Release>) -> ReleasesOutcome {
    ReleasesOutcome {
        repository: repository(),
        releases,
        completeness: ExtrasCompleteness::Complete,
        page_metadata: Vec::new(),
    }
}

fn latest(release: Option<Release>) -> LatestReleaseOutcome {
    LatestReleaseOutcome {
        repository: repository(),
        release,
        metadata: metadata(),
    }
}

fn languages(items: &[(&str, u64)], total: u64) -> LanguagesOutcome {
    LanguagesOutcome {
        repository: repository(),
        languages: items
            .iter()
            .map(|(name, bytes)| LanguageStat {
                name: (*name).into(),
                bytes: *bytes,
                share: None,
            })
            .collect(),
        total_bytes: total,
        metadata: metadata(),
    }
}

#[test]
fn a_stale_binding_never_overwrites_the_displayed_panel() {
    let current = binding(7);
    let mut overview = RepositoryOverview::new(current.clone());
    assert!(overview.apply_releases(
        &current,
        SectionState::Ready(ReleasesData::new(
            releases(vec![release(1, "v1.0.0")]),
            None
        ))
    ));
    // Another generation/account/repository is rejected as a whole.
    let stale = binding(6);
    assert!(!overview.apply_releases(
        &stale,
        SectionState::Ready(ReleasesData::new(
            releases(vec![release(2, "v2.0.0")]),
            None
        ))
    ));
    // A value for a different repository inside a matching binding is rejected too.
    let other = RepositoryIdentity {
        id: 99,
        locator: RepositoryLocator::new("octo", "other").unwrap(),
    };
    let mut wrong = releases(vec![release(3, "v3.0.0")]);
    wrong.repository = other;
    assert!(!overview.apply_releases(
        &current,
        SectionState::Ready(ReleasesData::new(wrong, None))
    ));
    let shown = overview.releases().value().expect("the first value stays");
    assert_eq!(shown.list.releases[0].id, 1);
    assert_eq!(overview.last_action(), None);
}

#[test]
fn partial_and_historical_states_keep_their_provenance() {
    let current = binding(1);
    let mut overview = RepositoryOverview::new(current.clone());
    overview.apply_releases(
        &current,
        SectionState::Historical {
            value: ReleasesData::new(releases(vec![release(1, "v1.0.0")]), None).with_provenance(
                true,
                false,
                Some("2 h ago".into()),
            ),
            sync_label: Some("2 h ago".into()),
            reason: Some(Vec::new()),
        },
    );
    assert!(overview.releases().is_historical());
    assert!(overview.releases().is_partial());
    assert_eq!(overview.releases().sync_label(), Some("2 h ago"));
    overview.apply_latest_release(&current, SectionState::Ready(latest(None)));
    assert!(
        overview
            .latest_release()
            .is_some_and(|state| !state.is_historical())
    );
    assert!(
        overview
            .latest_release()
            .and_then(SectionState::value)
            .is_some()
    );
    // A draft/pre-release fact is kept as is; Latest is a separate verified observation.
    assert!(
        !overview
            .latest_release()
            .unwrap()
            .value()
            .unwrap()
            .release
            .is_some()
    );
}

#[test]
fn releases_and_latest_are_independent_and_a_latest_404_is_not_a_list_result() {
    let current = binding(1);
    let mut overview = RepositoryOverview::new(current.clone());
    overview.apply_releases(
        &current,
        SectionState::Ready(ReleasesData::new(
            releases(vec![release(1, "v1.0.0"), release(2, "v0.9.0")]),
            None,
        )),
    );
    overview.apply_latest_release(&current, SectionState::Ready(latest(None)));
    // The list is still complete and its count is drawn; the latest is a neutral empty value.
    assert_eq!(overview.releases().value().unwrap().list.releases.len(), 2);
    assert!(
        overview
            .latest_release()
            .unwrap()
            .value()
            .unwrap()
            .release
            .is_none()
    );
}

#[test]
fn language_segments_require_checked_byte_totals() {
    let ok =
        language_segments(&languages(&[("Rust", 30), ("Markdown", 70)], 100)).expect("segments");
    assert_eq!(ok.len(), 2);
    assert_eq!(ok[0].name, "Markdown");
    assert!((ok[0].share - 0.7).abs() < f64::EPSILON);
    // A total that does not match the parts is not trusted, so no strip is drawn.
    assert!(language_segments(&languages(&[("Rust", 30)], 100)).is_none());
    // Zero bytes is an honest empty strip, not an error.
    assert_eq!(
        language_segments(&languages(&[], 0)).expect("empty").len(),
        0
    );
    assert_eq!(format_share(0.004), "<1%");
    assert_eq!(format_share(0.42), "42%");
}

#[test]
fn contributor_display_prefers_a_real_name_and_handles_anonymous() {
    let named = Contributor {
        id: Some(1),
        login: Some("octocat".into()),
        name: Some("The Octocat".into()),
        email: None,
        avatar_url: None,
        html_url: Some("https://github.com/octocat".into()),
        contributions: 10,
        raw: serde_json::Value::Null,
    };
    assert_eq!(contributor_display(&named), "The Octocat");
    let anonymous = Contributor {
        id: None,
        login: None,
        name: None,
        email: Some("anon@example.invalid".into()),
        avatar_url: None,
        html_url: None,
        contributions: 1,
        raw: serde_json::Value::Null,
    };
    assert_eq!(contributor_display(&anonymous), "Anonymous contributor");
}

#[test]
fn a_decoded_contributor_picture_is_attached_once_per_binding() {
    let current = binding(1);
    let mut overview = RepositoryOverview::new(current.clone());
    let image = AvatarImage {
        source: "data:image/png;base64,AAAA".into(),
        width: 4,
        height: 4,
    };
    assert!(overview.set_avatar_image(&current, 7, image.clone()));
    assert!(overview.has_avatar_image(&current, 7));
    // A new binding clears pictures resolved for the old one.
    let next = binding(2);
    overview.set_binding(next.clone());
    assert!(!overview.has_avatar_image(&next, 7));
    assert!(!overview.set_avatar_image(&current, 7, image));
}
