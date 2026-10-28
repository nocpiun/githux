//! Deterministic acceptance fixtures for the standalone preview and the tests.
//!
//! These values are preview/test material only. They never claim that the product shell has real
//! repository data wired in, and the product target does not import this module.

use crate::domain::account::{AccountKey, HostId};
use crate::domain::repository::{RepositoryIdentity, RepositoryLocator};
use crate::domain::repository_extras::{
    Contributor, ContributorsOutcome, ContributorsState, ExtrasCompleteness,
    ExtrasIncompleteReason, LanguageStat, LanguagesOutcome, LatestReleaseOutcome, Release,
    ReleasesOutcome, ResponseMetadata,
};
use crate::ui::repository_overview::{OverviewBinding, ReleasesData, SectionState};

pub fn account() -> AccountKey {
    AccountKey::new(1, HostId::github())
}

pub fn repository() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 42,
        locator: RepositoryLocator::new("nocpiun", "githux").expect("valid fixture locator"),
    }
}

pub fn binding(generation: u64) -> OverviewBinding {
    OverviewBinding::new(account(), repository(), generation)
}

const RELEASE_BODY: &str = "Release notes remain data and are not rendered by the compact strip.";

fn release(
    id: u64,
    tag: &str,
    name: Option<&str>,
    draft: bool,
    prerelease: bool,
    body: Option<&str>,
) -> Release {
    Release {
        id,
        node_id: Some(format!("RE_{id}")),
        tag_name: tag.to_string(),
        name: name.map(str::to_string),
        body: body.map(str::to_string),
        html_url: Some(format!(
            "https://example.invalid/nocpiun/githux/releases/{tag}"
        )),
        created_at: Some("2026-10-28T00:00:00Z".to_string()),
        published_at: Some("2026-10-28T00:00:00Z".to_string()),
        draft,
        prerelease,
        raw: serde_json::json!({ "id": id, "tag_name": tag }),
    }
}

pub fn ready_releases() -> ReleasesData {
    let list = ReleasesOutcome {
        repository: repository(),
        releases: vec![
            release(4, "v1.3.0", Some("Next"), false, false, Some(RELEASE_BODY)),
            release(
                3,
                "v1.2.0-rc.1",
                Some("Release candidate"),
                false,
                true,
                None,
            ),
            release(
                2,
                "v1.1.0",
                Some("Draft notes"),
                true,
                false,
                Some("Draft body"),
            ),
            release(1, "v1.0.0", None, false, false, None),
        ],
        completeness: ExtrasCompleteness::Complete,
        page_metadata: vec![ResponseMetadata::default()],
    };
    let latest = LatestReleaseOutcome {
        repository: repository(),
        release: Some(release(
            4,
            "v1.3.0",
            Some("Next"),
            false,
            false,
            Some(RELEASE_BODY),
        )),
        metadata: ResponseMetadata::default(),
    };
    ReleasesData::new(list, Some(latest))
}

/// A list with releases but a 404 latest result. The panel must not call the repository empty.
pub fn latest_none_releases() -> ReleasesData {
    let mut data = ready_releases();
    data.latest = Some(LatestReleaseOutcome {
        repository: repository(),
        release: None,
        metadata: ResponseMetadata::default(),
    });
    data
}

pub fn latest_error_releases() -> ReleasesData {
    let mut data = ready_releases();
    data.latest = None;
    data.latest_error = Some("rate limit exceeded".to_string());
    data
}

pub fn partial_releases() -> ReleasesData {
    let mut data = ready_releases();
    data.list.completeness = ExtrasCompleteness::Partial {
        reasons: vec![ExtrasIncompleteReason::PageLimit { limit: 2 }],
    };
    data
}

pub fn partial_releases_state() -> SectionState<ReleasesData, Vec<ExtrasIncompleteReason>> {
    SectionState::Partial {
        value: partial_releases(),
        reason: vec![ExtrasIncompleteReason::PageLimit { limit: 2 }],
    }
}

pub fn ready_contributors() -> ContributorsOutcome {
    ContributorsOutcome {
        repository: repository(),
        contributors: vec![
            Contributor {
                login: Some("nocpiun".to_string()),
                id: Some(7),
                name: Some("Nocpiun".to_string()),
                email: Some("never-show@example.invalid".to_string()),
                avatar_url: Some("https://example.invalid/avatar.png".to_string()),
                html_url: Some("https://example.invalid/nocpiun".to_string()),
                contributions: 120,
                raw: serde_json::json!({ "login": "nocpiun" }),
            },
            Contributor {
                login: None,
                id: None,
                name: Some("Anonymous Builder".to_string()),
                email: Some("anon@example.invalid".to_string()),
                avatar_url: None,
                html_url: None,
                contributions: 3,
                raw: serde_json::json!({ "email": "anon@example.invalid" }),
            },
        ],
        state: ContributorsState::Content,
        completeness: ExtrasCompleteness::Complete,
        page_metadata: vec![ResponseMetadata::default()],
    }
}

pub fn large_contributors() -> ContributorsOutcome {
    let mut outcome = ready_contributors();
    outcome.contributors = (0..300)
        .map(|index| Contributor {
            login: Some(format!("user{index}")),
            id: Some(1000 + index as u64),
            name: Some(format!("User {index}")),
            email: Some(format!("user{index}@example.invalid")),
            avatar_url: None,
            html_url: Some(format!("https://example.invalid/user{index}")),
            contributions: index as u64,
            raw: serde_json::json!({ "login": format!("user{index}") }),
        })
        .collect();
    outcome
}

pub fn empty_repository_contributors() -> ContributorsOutcome {
    ContributorsOutcome {
        repository: repository(),
        contributors: Vec::new(),
        state: ContributorsState::EmptyRepository,
        completeness: ExtrasCompleteness::Complete,
        page_metadata: vec![ResponseMetadata::default()],
    }
}

pub fn empty_content_contributors() -> ContributorsOutcome {
    ContributorsOutcome {
        repository: repository(),
        contributors: Vec::new(),
        state: ContributorsState::Content,
        completeness: ExtrasCompleteness::Complete,
        page_metadata: vec![ResponseMetadata::default()],
    }
}

pub fn partial_contributors() -> ContributorsOutcome {
    let mut outcome = ready_contributors();
    outcome.completeness = ExtrasCompleteness::Partial {
        reasons: vec![ExtrasIncompleteReason::Failed(
            crate::domain::repository_extras::RepositoryExtrasError::InvalidLimits("fixture"),
        )],
    };
    outcome
}

pub fn ready_languages() -> LanguagesOutcome {
    LanguagesOutcome {
        repository: repository(),
        languages: vec![
            LanguageStat {
                name: "Rust".to_string(),
                bytes: 620,
                share: Some(0.62),
            },
            LanguageStat {
                name: "TypeScript".to_string(),
                bytes: 250,
                share: Some(0.25),
            },
            LanguageStat {
                name: "Markdown".to_string(),
                bytes: 130,
                share: Some(0.13),
            },
        ],
        total_bytes: 1000,
        metadata: ResponseMetadata::default(),
    }
}

pub fn zero_languages() -> LanguagesOutcome {
    LanguagesOutcome {
        repository: repository(),
        languages: Vec::new(),
        total_bytes: 0,
        metadata: ResponseMetadata::default(),
    }
}

pub fn many_languages() -> LanguagesOutcome {
    let mut outcome = ready_languages();
    outcome.languages = (0..20)
        .map(|index| LanguageStat {
            name: format!("Language {index}"),
            bytes: 100 - index,
            share: Some((100 - index) as f64 / 1000.0),
        })
        .collect();
    outcome.total_bytes = outcome
        .languages
        .iter()
        .map(|language| language.bytes)
        .sum();
    for language in &mut outcome.languages {
        language.share = Some(language.bytes as f64 / outcome.total_bytes as f64);
    }
    outcome
}

pub fn partial_languages_state() -> SectionState<LanguagesOutcome, String> {
    SectionState::Partial {
        value: ready_languages(),
        reason: "a language count was rejected".to_string(),
    }
}
