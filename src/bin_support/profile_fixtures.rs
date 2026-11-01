//! Developer-only profile scenarios shared by example and geometry tests.
use crate::{
    content::PreparedBody,
    domain::{
        ContributionCalendar, ContributionDay, ContributionRange, ContributionWeek, ProfileBasic,
        ProfileGraphqlError, ProfilePinned, ProfileRepo, Showcase,
    },
    profile::{PresentationError, SectionState},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Complete,
    Partial,
    MixedPins,
    GistsOnly,
    Truncated,
    Unknown,
    Sparse,
    Errors,
    Empty,
    NoReadme,
}

pub const SCENARIOS: &[(Scenario, &str)] = &[
    (Scenario::Complete, "Normal / Design"),
    (Scenario::Partial, "Partial + long"),
    (Scenario::MixedPins, "Mixed pins"),
    (Scenario::GistsOnly, "Gists only"),
    (Scenario::Truncated, "Truncated"),
    (Scenario::Unknown, "Unknown levels"),
    (Scenario::Sparse, "Missing fields"),
    (Scenario::Errors, "Errors"),
    (Scenario::Empty, "Real empty"),
    (Scenario::NoReadme, "No README (absent)"),
];

pub fn rolling_year() -> ContributionRange {
    // A fixed synthetic interval keeps the preview deterministic. Production
    // hosts supply the current rolling-year range; the component has no clock.
    ContributionRange::new("2023-03-01", "2024-02-29").unwrap()
}

pub fn basic(scenario: Scenario) -> ProfileBasic {
    ProfileBasic {
        id: 42,
        login: "lin-example".into(),
        name: match scenario {
            Scenario::Sparse => None,
            Scenario::Partial => {
                Some("A Very Long Public Display Name Used To Check Wrapping".into())
            }
            _ => Some("Lin Example".into()),
        },
        bio: match scenario {
            Scenario::Sparse => None,
            Scenario::Partial => Some(
                "A deliberately long public bio with wrapping text, 中文 and emoji 🚀. ".repeat(12),
            ),
            _ => Some("Building thoughtful tools for developers.".into()),
        },
        avatar_url: Some("https://invalid.example/avatar-is-never-fetched".into()),
        profile_url: Some("https://example.com/octo".into()),
        company: match scenario {
            Scenario::Sparse => None,
            Scenario::Partial => Some("A public organization with a deliberately long name".into()),
            _ => None,
        },
        location: match scenario {
            Scenario::Sparse => None,
            _ => Some("Shanghai".into()),
        },
        website_url: match scenario {
            Scenario::Sparse => None,
            _ => Some("https://lin.example.com".into()),
        },
        followers: match scenario {
            Scenario::Sparse => None,
            _ => Some(128),
        },
        following: match scenario {
            Scenario::Sparse => None,
            _ => Some(32),
        },
        public_repos: match scenario {
            Scenario::Sparse => None,
            _ => Some(12),
        },
        public_gists: match scenario {
            Scenario::Sparse => None,
            _ => Some(3),
        },
        created_at: match scenario {
            Scenario::Sparse => None,
            _ => Some("2020-01-01T00:00:00Z".into()),
        },
    }
}

fn repo(id: u64, name: &str) -> ProfileRepo {
    ProfileRepo {
        id: Some(id),
        name: name.into(),
        name_with_owner: Some(format!("lin-example/{name}")),
        description: Some(if name == "long-repository-description" {
            "A long public repository description used to verify that wrapping and clipping cannot cover subsequent Overview sections. ".repeat(12)
        } else {
            match name {
                "githux" => "A native GitHub client for Windows.",
                "notes" => "A simple, focused note-taking app.",
                "atlas" => "A modern, minimal workspace for ideas.",
                _ => "My development environment configuration.",
            }
            .to_string()
        }),
        url: Some(format!("https://example.com/octo/{name}")),
        is_private: Some(false),
        is_fork: Some(false),
        stargazer_count: Some([3400, 1200, 2100, 842][id.saturating_sub(1) as usize % 4]),
        fork_count: Some(0),
        primary_language: Some("Rust".into()),
        owner_login: Some("lin-example".into()),
    }
}

fn pinned(showcase: Showcase) -> SectionState<ProfilePinned> {
    SectionState::Ready(ProfilePinned {
        repos: vec![],
        total_repos: 0,
        pinned_truncated: false,
        has_pinned_items: !matches!(showcase, Showcase::PopularRepositories(_)),
        showcase_truncated: false,
        showcase,
        errors: vec![],
    })
}

pub fn repositories(scenario: Scenario) -> SectionState<ProfilePinned> {
    match scenario {
        Scenario::Errors => SectionState::Error(PresentationError::Forbidden),
        Scenario::Empty => pinned(Showcase::PopularRepositories(vec![])),
        Scenario::GistsOnly => pinned(Showcase::PinnedGistsOnly { gists: 3 }),
        Scenario::Unknown => pinned(Showcase::Unclassified),
        Scenario::Truncated => SectionState::Ready(ProfilePinned {
            repos: vec![repo(1, "first-pin")],
            total_repos: 8,
            pinned_truncated: true,
            showcase_truncated: true,
            ..match pinned(Showcase::Truncated {
                repositories: vec![repo(1, "first-pin")],
                gists: 1,
            }) {
                SectionState::Ready(value) => value,
                _ => unreachable!(),
            }
        }),
        Scenario::MixedPins => SectionState::Ready(ProfilePinned {
            repos: vec![repo(1, "first-pin"), repo(2, "second-pin")],
            total_repos: 2,
            ..match pinned(Showcase::PinnedWithGists {
                repositories: vec![],
                gists: 2,
            }) {
                SectionState::Ready(value) => value,
                _ => unreachable!(),
            }
        }),
        Scenario::Partial => SectionState::Ready(ProfilePinned {
            repos: vec![
                repo(1, "long-repository-description"),
                repo(2, "second-pin"),
            ],
            total_repos: 2,
            errors: vec![ProfileGraphqlError {
                kind: crate::domain::GraphqlErrorKind::Unknown,
                path: vec!["user".into()],
            }],
            ..match pinned(Showcase::PinnedRepositories(vec![])) {
                SectionState::Ready(value) => value,
                _ => unreachable!(),
            }
        }),
        Scenario::Complete | Scenario::Sparse | Scenario::NoReadme => {
            let repos = vec![
                repo(1, "githux"),
                repo(2, "notes"),
                repo(3, "atlas"),
                repo(4, "dotfiles"),
            ];
            SectionState::Ready(ProfilePinned {
                total_repos: 4,
                repos: repos.clone(),
                pinned_truncated: false,
                has_pinned_items: true,
                showcase_truncated: false,
                showcase: Showcase::PinnedRepositories(repos),
                errors: vec![],
            })
        }
    }
}

fn day(date: &str, count: u64, level: &str) -> ContributionDay {
    ContributionDay {
        date: date.into(),
        count,
        level: level.into(),
        color: "service colour text is ignored".into(),
    }
}

pub fn contributions(scenario: Scenario) -> SectionState<ContributionCalendar> {
    if matches!(scenario, Scenario::Errors) {
        return SectionState::Error(PresentationError::NotFound);
    }
    if matches!(scenario, Scenario::Empty) {
        return SectionState::Ready(ContributionCalendar {
            weeks: vec![],
            months: vec![],
            total_contributions: 0,
            errors: vec![],
        });
    }
    let mut dates = Vec::new();
    let mut year = 2023;
    let mut month = 3;
    let mut day_number = 1;
    for index in 0..366 {
        dates.push(format!("{year:04}-{month:02}-{day_number:02}"));
        day_number += 1;
        let max = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if year == 2024 => 29,
            2 => 28,
            _ => unreachable!(),
        };
        if day_number > max {
            day_number = 1;
            month += 1;
            if month > 12 {
                month = 1;
                year += 1;
            }
        }
        if index == 365 {
            break;
        }
    }
    let level = if matches!(scenario, Scenario::Unknown) {
        "FUTURE_LEVEL"
    } else {
        "SECOND_QUARTILE"
    };
    let mut days = dates
        .iter()
        .enumerate()
        .map(|(index, date)| {
            day(
                date,
                if index % 5 == 0 { 0 } else { index as u64 % 7 },
                if index % 5 == 0 { "NONE" } else { level },
            )
        })
        .collect::<Vec<_>>();
    if matches!(scenario, Scenario::Partial) {
        days.truncate(200);
        days.push(day("2024-02-29", 2, "FIRST_QUARTILE"));
    }
    SectionState::Ready(ContributionCalendar {
        weeks: days
            .chunks(7)
            .map(|days| ContributionWeek {
                days: days.to_vec(),
            })
            .collect(),
        months: vec![],
        total_contributions: days.iter().map(|day| day.count).sum(),
        errors: if matches!(scenario, Scenario::Partial) {
            vec![ProfileGraphqlError {
                kind: crate::domain::GraphqlErrorKind::Forbidden,
                path: vec!["contributions".into()],
            }]
        } else {
            vec![]
        },
    })
}

pub fn readme(scenario: Scenario) -> SectionState<PreparedBody> {
    match scenario {
        Scenario::Errors => SectionState::Error(PresentationError::Offline),
        Scenario::Empty => SectionState::Ready(PreparedBody::Empty),
        Scenario::Sparse => SectionState::Loading,
        // A confirmed optional absence: the whole README section must disappear, not show a
        // "not found" card. The real Shell reaches this from Missing/NotEligible/a 404; the
        // example exposes it as a selectable scenario so the omission is natively previewable.
        Scenario::NoReadme => SectionState::Unavailable(
            "No visible public same-name profile repository or root README.".into(),
        ),
        _ => SectionState::Ready(PreparedBody::Utf8 {
            text: if matches!(scenario, Scenario::Partial) {
                format!(
                    "## Hi, I'm Lin

I build desktop tools and contribute to open source.

{}",
                    "A longer story with 中文 and `code`, and [notes](notes.md).

"
                    .repeat(200)
                )
            } else {
                "## Hi, I'm Lin

I build desktop tools and contribute to open source.

[Projects](https://example.com/projects) · [Writing](https://example.com/writing) · [Contact](https://example.com/contact)".into()
            },
            has_bom: false,
        }),
    }
}
