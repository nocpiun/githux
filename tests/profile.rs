//! Integration assembly for the reusable Profile Overview tests.

#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/bin_support/profile_modules.rs"]
mod profile_modules;
#[allow(unused_imports)]
use profile_modules as ui;
#[allow(unused_imports)]
use profile_modules::profile;

#[path = "../src/bin_support/profile_fixtures.rs"]
mod profile_fixtures;

#[gpui::test]
fn design_fixture_renders_real_profile_content(cx: &mut gpui::TestAppContext) {
    use profile_fixtures::{SCENARIOS, Scenario};
    cx.update(ely_core::init_for_tests);
    for (scenario, _) in SCENARIOS {
        let _ = profile_fixtures::basic(*scenario);
        let _ = profile_fixtures::repositories(*scenario);
        let _ = profile_fixtures::contributions(*scenario);
        let _ = profile_fixtures::readme(*scenario);
    }
    let (view, cx) = cx.add_window_view(|window, cx| {
        profile::ProfileView::new(
            profile::ProfileTarget::new(domain::HostId::github(), 42),
            profile::ProfileBinding::new(
                domain::AccountKey::new(7, domain::HostId::github()),
                domain::AuthEpoch(1),
            ),
            profile_fixtures::rolling_year(),
            window,
            cx,
        )
    });
    view.update_in(cx, |view, window, cx| {
        let request = view.request();
        view.update_basic(
            &request,
            profile::SectionState::Ready(profile_fixtures::basic(Scenario::Complete)),
            cx,
        );
        view.update_repositories(
            &request,
            profile_fixtures::repositories(Scenario::Complete),
            cx,
        );
        view.update_contributions(
            &request,
            &profile_fixtures::rolling_year(),
            profile_fixtures::contributions(Scenario::Complete),
            cx,
        );
        view.update_readme_with_context(
            &request,
            profile_fixtures::readme(Scenario::Complete),
            ui::reader::ResourceContext::new(
                "lin-example",
                "lin-example",
                "a".repeat(40),
                "README.md",
            ),
            window,
            cx,
        );
    });
    cx.simulate_resize(gpui::size(gpui::px(1100.0), gpui::px(800.0)));
    for _ in 0..3 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
    let root = cx.debug_bounds("profile-overview").unwrap();
    let header = cx.debug_bounds("profile-basic").unwrap();
    assert!(header.left() > root.left());
    assert!(header.right() <= root.right());
    assert!(cx.debug_bounds("profile-repository-3").unwrap().right() <= root.right());
    assert!(cx.debug_bounds("profile-contributions-section").is_some());
}
