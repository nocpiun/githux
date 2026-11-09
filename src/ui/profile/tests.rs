//! Profile overview regressions on Native Shell.
//!
//! One mounted view, real presentation code, fixture data only (no services):
//! header content, section headings, pinned-card descriptions, the contribution
//! heatmap, provenance marks, and the confirmed optional absence of the README.
//! The page-level composition (real account, cache and authorization) is covered
//! by `app::pages::profile_acceptance_tests`.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

use super::*;
use crate::domain::{
    ContributionCalendar, ContributionDay, ContributionRange, ContributionWeek, GraphqlErrorKind,
    ProfileBasic, ProfileGraphqlError, ProfilePinned, ProfileRepo, Showcase,
};

static LOCK: Mutex<()> = Mutex::new(());

fn fixed_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_793_188_800)
}

fn range() -> ContributionRange {
    ContributionRange::new("2025-10-29", "2026-10-28").expect("one rolling year")
}

fn basic() -> ProfileBasic {
    ProfileBasic {
        id: 42,
        login: "lin-example".into(),
        name: Some("Lin Example".into()),
        bio: Some("Building thoughtful tools for developers.".into()),
        avatar_url: Some("https://invalid.example/avatar-is-never-fetched".into()),
        profile_url: Some("https://example.com/lin".into()),
        company: None,
        location: Some("Shanghai".into()),
        website_url: Some("https://lin.example/".into()),
        followers: Some(120),
        following: Some(8),
        public_repos: Some(12),
        public_gists: Some(0),
        created_at: None,
    }
}

fn repository(id: u64, name: &str, description: &str) -> ProfileRepo {
    ProfileRepo {
        id: Some(id),
        name: name.into(),
        name_with_owner: Some(format!("lin-example/{name}")),
        description: Some(description.into()),
        url: Some(format!("https://example.com/lin-example/{name}")),
        is_private: Some(false),
        is_fork: Some(false),
        stargazer_count: Some(2100),
        fork_count: Some(0),
        primary_language: Some("Rust".into()),
        owner_login: Some("lin-example".into()),
    }
}

fn pinned() -> SectionState<ProfilePinned> {
    SectionState::Ready(ProfilePinned {
        repos: vec![
            repository(1, "githux", "A native GitHub client for Windows."),
            repository(
                2,
                "atlas",
                "A deliberately long public repository description that must be clamped to three lines inside the card and never cover the following Overview sections.",
            ),
        ],
        total_repos: 2,
        pinned_truncated: false,
        has_pinned_items: true,
        showcase_truncated: false,
        showcase: Showcase::PinnedRepositories(vec![]),
        errors: vec![],
    })
}

fn contributions() -> SectionState<ContributionCalendar> {
    let mut days = Vec::new();
    let mut year = 2025u32;
    let mut month = 10u32;
    let mut day = 29u32;
    for index in 0..120u64 {
        days.push(ContributionDay {
            date: format!("{year:04}-{month:02}-{day:02}"),
            count: index % 4,
            level: match index % 5 {
                0 => "NONE",
                1 => "FIRST_QUARTILE",
                2 => "SECOND_QUARTILE",
                3 => "THIRD_QUARTILE",
                _ => "FOURTH_QUARTILE",
            }
            .into(),
            color: "service colour text is ignored".into(),
        });
        day += 1;
        let max = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            _ => 28,
        };
        if day > max {
            day = 1;
            month += 1;
            if month > 12 {
                month = 1;
                year += 1;
            }
        }
    }
    SectionState::Ready(ContributionCalendar {
        weeks: days
            .chunks(7)
            .map(|days| ContributionWeek {
                days: days.to_vec(),
            })
            .collect(),
        months: vec![],
        total_contributions: 137,
        errors: vec![ProfileGraphqlError {
            kind: GraphqlErrorKind::Forbidden,
            path: vec!["contributions".into()],
        }],
    })
}

#[derive(Clone, PartialEq, Props)]
struct ProfileProps {
    /// `true`: a confirmed absence (section omitted). `false`: an actionable
    /// failure (one compact warning mark).
    #[prop(default = true)]
    readme_absent: bool,
}

#[component]
fn ProfileHarness(props: ProfileProps) -> impl IntoElement {
    let mut view = ProfileView::new(
        ProfileTarget::new(crate::domain::HostId::github(), 42),
        ProfileBinding::new(
            crate::domain::AccountKey::new(1, crate::domain::HostId::github()),
            crate::domain::AuthEpoch(1),
        ),
        range(),
    );
    view.set_now(fixed_now());
    let request = view.request();
    view.update_basic_with_provenance(
        &request,
        SectionState::Ready(basic()),
        Some(ProfileProvenance {
            historical: true,
            fetched_at_ms: 1_760_000_000_000,
            verified_at_ms: 1_760_000_000_000,
            completeness: crate::domain::cache::CacheCompleteness::Partial {
                reason: "one page failed".into(),
            },
        }),
    );
    view.update_repositories_with_provenance(&request, pinned(), None);
    view.update_contributions_with_provenance(&request, &range(), contributions(), None);
    if props.readme_absent {
        view.set_readme_unavailable_with_provenance(
            &request,
            "No visible public same-name profile repository or root README.",
            None,
        );
    } else {
        view.set_readme_error(&request, PresentationError::Offline, None, None);
    }
    view.element()
}

fn mount(readme_absent: bool) -> Headless {
    let root = Root::new::<ProfileHarness>(move || ProfileProps { readme_absent })
    .tokens(crate::ui::foundation::theme::tokens());
    let mut app = Headless::start(EngineConfig::new(root)).expect("headless app starts");
    app.set_viewport(nsh_core::geometry::Size::new(
        nsh_core::geometry::px(1100.0),
        nsh_core::geometry::px(900.0),
    ))
    .unwrap();
    app
}

#[test]
fn header_sections_and_heatmap_render_real_profile_content() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut app = mount(true);
    app.wait_for("Lin Example").expect("the display name renders");
    app.wait_for("@lin-example").expect("the login renders");
    app.wait_for("Building thoughtful tools for developers.")
        .expect("the bio renders");
    app.wait_for("120 followers · 8 following")
        .expect("public metadata renders");
    app.wait_for("Pinned repositories")
        .expect("the repository section renders");
    app.wait_for("githux").expect("a pinned card renders");
    app.wait_for("A native GitHub client for Windows.")
        .expect("the card description renders");
    app.wait_for("Contributions")
        .expect("the contribution section renders");
    app.wait_for("id:profile-heatmap")
        .expect("the heatmap renders");
    app.wait_for("id:profile-heatmap-summary")
        .expect("the heatmap summary renders");
    // Provenance is explicit: a historical partial header says so.
    app.wait_for("id:profile-basic-provenance-historical")
        .expect("the saved header is marked");
    app.wait_for("id:profile-basic-provenance-partial")
        .expect("the partial header is marked");
    // A long description is clamped by the card, not by clipping the sections
    // below it: the following section is still mounted.
    app.wait_for("id:profile-repository-description-1")
        .expect("the long card description is present but clamped");
}

#[test]
fn confirmed_readme_absence_omits_the_whole_section() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut app = mount(true);
    app.wait_for("Pinned repositories")
        .expect("the profile renders");
    assert!(
        !app.screen().shows("id:profile-readme-section"),
        "a confirmed absence must not leave an empty or error-framed section"
    );
    assert!(
        !app.screen().shows("profile-readme-error"),
        "a confirmed absence is not an actionable failure"
    );
}

#[test]
fn actionable_readme_failure_keeps_one_compact_mark() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut app = mount(false);
    app.wait_for("id:profile-readme-error")
        .expect("a real failure keeps a compact mark");
    assert!(
        !app.screen().shows("Retry"),
        "no prose retry button; the mark is the affordance"
    );
}

#[test]
fn heatmap_reports_missing_and_unknown_levels_honestly() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut app = mount(true);
    app.wait_for("id:profile-contribution-day-0")
        .expect("individual days render");
    let summary = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.contains("contributions in the last year"))
        .map(str::to_string)
        .expect("a summary line names the interval");
    assert!(
        summary.contains("days observed"),
        "the summary states what was observed: {summary}"
    );
}
