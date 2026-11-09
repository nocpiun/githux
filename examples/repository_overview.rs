//! Deterministic, headless data preview of the repository overview component.
//!
//! Run with `cargo run --example repository_overview` (no window, no network).
//!
//! The component renders only host-supplied observations. This preview applies the acceptance
//! fixtures through its real state machine and prints the resulting section states, so a wiring or
//! provenance regression is visible without a GUI. The visual preview is the Native Shell page the
//! integrator assembles; this file is development scaffolding only.

#[path = "../src/domain/mod.rs"]
mod domain;

pub mod ui {
    pub mod foundation {
        #[path = "../../src/ui/foundation/mod.rs"]
        mod foundation_impl;
        pub use foundation_impl::*;
    }
    pub mod repository_overview {
        #[path = "../../src/ui/repository_overview/mod.rs"]
        mod repository_overview_impl;
        pub use repository_overview_impl::*;
    }
}

#[path = "../src/ui/repository_overview/fixtures.rs"]
mod fixtures;

use ui::repository_overview::{
    OverviewSection, OverviewUpdate, RepositoryOverview, SectionState,
};

fn describe<T, P>(state: &SectionState<T, P>) -> String {
    match state {
        SectionState::Idle => "idle".into(),
        SectionState::Loading => "loading".into(),
        SectionState::Ready(_) => "ready".into(),
        SectionState::Partial { .. } => "partial".into(),
        SectionState::Historical { sync_label, .. } => format!(
            "historical({})",
            sync_label.as_deref().unwrap_or("unknown sync")
        ),
        SectionState::Error { message } => format!("error({message})"),
    }
}

fn main() -> std::process::ExitCode {
    let binding = fixtures::binding(1);
    let mut overview = RepositoryOverview::new(binding.clone());
    let applied = [
        overview.apply_releases(
            &binding,
            SectionState::Ready(fixtures::ready_releases()),
        ),
        overview.apply_latest_release(&binding, SectionState::Ready(fixtures::ready_latest())),
        overview.apply_contributors(
            &binding,
            SectionState::Ready(fixtures::ready_contributors()),
        ),
        overview.apply_languages(
            &binding,
            SectionState::Ready(fixtures::ready_languages()),
        ),
    ];
    if applied.iter().any(|applied| !applied) {
        eprintln!("a matching binding was rejected by the component");
        return std::process::ExitCode::FAILURE;
    }
    println!("ready:");
    println!("  releases:     {}", describe(overview.releases()));
    println!(
        "  latest:       {}",
        overview
            .latest_release()
            .map(describe)
            .unwrap_or_else(|| "not loaded".into())
    );
    println!("  contributors: {}", describe(overview.contributors()));
    println!("  languages:    {}", describe(overview.languages()));

    // A stale binding and a stale update are rejected as a whole.
    let stale = fixtures::binding(0);
    let stale_releases = fixtures::partial_releases_state();
    if overview.apply_releases(&stale, stale_releases.clone()) {
        eprintln!("a stale generation replaced the displayed releases");
        return std::process::ExitCode::FAILURE;
    }
    println!("stale update rejected: {}", describe(overview.releases()));

    // Historical and partial states keep their provenance instead of being relabelled current.
    overview.apply_releases(&binding, stale_releases);
    println!("partial:      {}", describe(overview.releases()));
    overview.apply_releases(
        &binding,
        SectionState::Historical {
            value: fixtures::ready_releases(),
            sync_label: Some("2 h ago".into()),
            reason: None,
        },
    );
    println!("historical:   {}", describe(overview.releases()));

    // A scoped retry is a typed action for the host; the component never refreshes by itself.
    let action = ui::repository_overview::OverviewAction::Refresh {
        binding,
        section: OverviewSection::Contributors,
    };
    overview.emit(action);
    println!(
        "last action:  {:?}",
        overview.last_action().map(|action| match action {
            ui::repository_overview::OverviewAction::Refresh { section, .. } => {
                format!("refresh:{section:?}")
            }
            ui::repository_overview::OverviewAction::OpenRelease { .. } => "open-release".into(),
            ui::repository_overview::OverviewAction::OpenContributor { .. } => "open-contributor".into(),
        })
    );

    // Every update shape stays constructible; the host applies it with the captured binding.
    let updates = [
        OverviewUpdate::Releases(SectionState::Loading),
        OverviewUpdate::LatestRelease(SectionState::Loading),
        OverviewUpdate::Contributors(SectionState::Loading),
        OverviewUpdate::Languages(SectionState::Loading),
    ];
    for update in updates {
        overview.apply(&binding, update);
    }
    println!("\nThe component renders only supplied observations; no request was made.");
    std::process::ExitCode::SUCCESS
}
