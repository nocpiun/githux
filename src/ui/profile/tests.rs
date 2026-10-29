use gpui::{Entity, TestAppContext, VisualTestContext, point, px};

use crate::{
    content::{PreparedBody, PreparedContent},
    domain::{
        AccountKey, AuthEpoch, ContributionCalendar, ContributionDay, ContributionRange,
        ContributionWeek, HostId, ProfileBasic, ProfileGraphqlError, ProfilePinned, ProfileRepo,
        Showcase,
        repository::{
            ObjectSha, ReadmeDescriptor, RepositoryIdentity, RepositoryLocator, RepositoryPath,
        },
    },
    ui::reader::{LinkTarget, ReaderAction},
};

use super::{
    CellLevel, PresentationError, ProfileActionKind, ProfileBinding, ProfileTarget, ProfileView,
    SectionState, render::initials, repository_presentation,
};

fn target(id: u64) -> ProfileTarget {
    ProfileTarget::new(HostId::github(), id)
}

fn binding(id: u64, epoch: u64) -> ProfileBinding {
    ProfileBinding::new(AccountKey::new(id, HostId::github()), AuthEpoch(epoch))
}

fn range() -> ContributionRange {
    ContributionRange::new("2024-02-27", "2024-03-04").unwrap()
}

fn basic(login: &str) -> ProfileBasic {
    ProfileBasic {
        id: 42,
        login: login.into(),
        name: Some("Long Example Name".into()),
        bio: Some("Public bio".into()),
        avatar_url: Some("https://avatars.invalid/not-fetched.png".into()),
        profile_url: Some("https://example.invalid/profile".into()),
        company: Some("Public company".into()),
        location: Some("Public location".into()),
        website_url: Some("https://example.invalid".into()),
        followers: Some(10),
        following: Some(20),
        public_repos: Some(30),
        public_gists: Some(40),
        created_at: Some("2024-01-01T00:00:00Z".into()),
    }
}

fn repo(id: u64, name: &str) -> ProfileRepo {
    ProfileRepo {
        id: Some(id),
        name: name.into(),
        name_with_owner: Some(format!("owner/{name}")),
        description: Some(format!("description {name}")),
        url: Some(format!("https://example.invalid/{name}")),
        is_private: Some(false),
        is_fork: Some(false),
        stargazer_count: Some(id),
        fork_count: Some(0),
        primary_language: Some("Rust".into()),
        owner_login: Some("owner".into()),
    }
}

fn pinned(showcase: Showcase) -> ProfilePinned {
    ProfilePinned {
        repos: vec![],
        total_repos: 0,
        pinned_truncated: false,
        has_pinned_items: !matches!(showcase, Showcase::PopularRepositories(_)),
        showcase_truncated: false,
        showcase,
        errors: vec![],
    }
}

fn day(date: &str, count: u64, level: &str) -> ContributionDay {
    ContributionDay {
        date: date.into(),
        count,
        level: level.into(),
        color: "not-a-css-value".into(),
    }
}

fn calendar(days: Vec<ContributionDay>) -> ContributionCalendar {
    ContributionCalendar {
        weeks: vec![ContributionWeek { days }],
        months: vec![],
        total_contributions: 42,
        errors: vec![],
    }
}

fn long_range_days() -> Vec<ContributionDay> {
    let mut result = Vec::new();
    let (mut year, mut month, mut date) = (2023, 3, 1);
    for index in 0..366 {
        result.push(day(
            &format!("{year:04}-{month:02}-{date:02}"),
            index,
            "FIRST_QUARTILE",
        ));
        date += 1;
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if year == 2024 => 29,
            2 => 28,
            _ => unreachable!(),
        };
        if date > days {
            date = 1;
            month += 1;
            if month > 12 {
                month = 1;
                year += 1;
            }
        }
    }
    result
}

fn readme(login: &str, text: &str) -> PreparedContent {
    let locator = RepositoryLocator::new(login, login).unwrap();
    let repository = RepositoryIdentity { id: 42, locator };
    let sha = ObjectSha::new("a".repeat(40)).unwrap();
    PreparedContent {
        identity: crate::domain::blob::BlobIdentity::from_readme(&ReadmeDescriptor {
            repository,
            commit: sha.clone(),
            path: RepositoryPath::new("README.md").unwrap(),
            blob: sha,
            size: text.len() as u64,
        }),
        body: PreparedBody::Utf8 {
            text: text.into(),
            has_bom: false,
        },
    }
}

#[test]
fn repository_pin_policy_preserves_order_and_uncertainty() {
    let ordered = pinned(Showcase::PinnedRepositories(vec![
        repo(3, "third"),
        repo(1, "first"),
    ]));
    let ordered = ProfilePinned {
        repos: vec![repo(8, "z-first"), repo(2, "a-second")],
        total_repos: 2,
        ..ordered
    };
    let view = repository_presentation(&ordered);
    assert_eq!(view.title, "Pinned repositories");
    assert_eq!(
        view.repositories
            .iter()
            .map(|repo| repo.id)
            .collect::<Vec<_>>(),
        vec![Some(8), Some(2)]
    );

    let popular = repository_presentation(&pinned(Showcase::PopularRepositories(vec![
        repo(2, "popular-b"),
        repo(99, "popular-a"),
    ])));
    assert_eq!(popular.title, "Popular repositories");
    assert_eq!(popular.repositories[0].name, "popular-b");

    for showcase in [
        Showcase::PinnedGistsOnly { gists: 3 },
        Showcase::Truncated {
            repositories: vec![repo(1, "prefix")],
            gists: 2,
        },
        Showcase::Unclassified,
    ] {
        let view = repository_presentation(&pinned(showcase));
        assert_eq!(view.title, "Pinned repositories");
        assert!(view.note.is_some());
    }
}

#[test]
fn mixed_and_gist_only_show_repository_pins_and_an_explicit_hint() {
    let mixed = ProfilePinned {
        repos: vec![repo(1, "one")],
        total_repos: 1,
        ..pinned(Showcase::PinnedWithGists {
            repositories: vec![repo(2, "showcase-order")],
            gists: 2,
        })
    };
    let view = repository_presentation(&mixed);
    assert_eq!(view.repositories[0].name, "one");
    assert!(view.note.unwrap().contains("2 gist pins"));

    let gist_only = repository_presentation(&pinned(Showcase::PinnedGistsOnly { gists: 1 }));
    assert!(gist_only.repositories.is_empty());
    let note = gist_only.note.unwrap();
    assert!(note.contains("gist pin"));
    assert!(gist_only.empty.unwrap().contains("not proof"));
}

#[test]
fn real_empty_partial_denied_and_truncated_are_not_complete_empty() {
    let empty = repository_presentation(&pinned(Showcase::PopularRepositories(vec![])));
    assert_eq!(empty.title, "Popular repositories");
    assert!(empty.repositories.is_empty());
    assert!(empty.note.is_none());

    let partial = ProfilePinned {
        errors: vec![ProfileGraphqlError {
            kind: crate::domain::GraphqlErrorKind::Forbidden,
            path: vec!["user".into()],
        }],
        ..pinned(Showcase::PopularRepositories(vec![]))
    };
    assert!(
        repository_presentation(&partial)
            .note
            .unwrap()
            .contains("Partial data")
    );

    let truncated = ProfilePinned {
        repos: vec![repo(1, "first")],
        total_repos: 7,
        pinned_truncated: true,
        showcase_truncated: true,
        ..pinned(Showcase::Truncated {
            repositories: vec![repo(1, "first")],
            gists: 0,
        })
    };
    assert!(
        repository_presentation(&truncated)
            .note
            .unwrap()
            .contains("prefix")
    );
    assert!(
        PresentationError::Forbidden
            .message()
            .contains("authorization")
    );
}

#[test]
fn missing_basic_fields_have_initials_and_no_private_inference() {
    let sparse = ProfileBasic {
        name: None,
        bio: None,
        avatar_url: None,
        company: None,
        location: None,
        website_url: None,
        followers: None,
        following: None,
        public_repos: None,
        public_gists: None,
        created_at: None,
        ..basic("single-login")
    };
    assert_eq!(initials(&sparse), "S");
    assert_eq!(initials(&basic("irrelevant")), "LE");
    let unicode = ProfileBasic {
        name: Some("李 小明".into()),
        ..basic("x")
    };
    assert_eq!(initials(&unicode), "李小");
}

fn setup(cx: &mut TestAppContext) {
    cx.update(ely_core::init_for_tests);
    cx.update(|cx| {
        ely_core::theme::Theme::update(cx, |theme| {
            theme.font_family = "Noto Sans".into();
            theme.mono_family = "Google Sans Code".into();
        })
    });
}

fn open(cx: &mut TestAppContext) -> (Entity<ProfileView>, &mut VisualTestContext) {
    setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        ProfileView::new(target(42), binding(7, 1), range(), window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (view, cx)
}

fn publish_ready(view: &Entity<ProfileView>, cx: &mut VisualTestContext) {
    let request = view.read_with(cx, |view, _| view.request());
    view.update_in(cx, |view, window, cx| {
        assert!(view.update_basic(&request, SectionState::Ready(basic("person")), cx));
        assert!(view.update_repositories(
            &request,
            SectionState::Ready(ProfilePinned {
                repos: vec![repo(1, "one"), repo(2, "two")],
                total_repos: 2,
                ..pinned(Showcase::PinnedRepositories(vec![]))
            }),
            cx
        ));
        assert!(view.update_contributions(
            &request,
            &range(),
            SectionState::Ready(calendar(vec![
                day("2024-02-27", 0, "NONE"),
                day("2024-02-28", 1, "FIRST_QUARTILE"),
                day("2024-02-29", 2, "SECOND_QUARTILE"),
                day("2024-03-04", 3, "FUTURE_LEVEL"),
            ])),
            cx
        ));
        assert!(view.update_readme(
            &request,
            SectionState::Ready(readme(
                "person",
                "# Public README\n\n[external](https://example.com)"
            )),
            window,
            cx
        ));
    });
    cx.run_until_parked();
}

#[gpui::test]
fn binding_changes_invalidate_all_sections_and_stale_updates(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let old = view.read_with(cx, |view, _| view.request());
    publish_ready(&view, cx);
    view.update_in(cx, |view, window, cx| {
        view.emit(ProfileActionKind::Profile, window, cx)
    });
    let new = view.update_in(cx, |view, _, cx| {
        view.bind(target(99), binding(8, 2), range(), cx)
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| {
        matches!(view.basic(), SectionState::Loading)
            && matches!(view.repositories(), SectionState::Loading)
            && matches!(view.contributions(), SectionState::Loading)
            && matches!(view.readme_state(), SectionState::Loading)
            && view.reader().is_none()
            && view.last_action().is_none()
    }));
    let rejected = view.update_in(cx, |view, window, cx| {
        !view.update_basic(&old, SectionState::Ready(basic("old")), cx)
            && !view.update_repositories(
                &old,
                SectionState::Ready(pinned(Showcase::PopularRepositories(vec![]))),
                cx,
            )
            && !view.update_contributions(&old, &range(), SectionState::Ready(calendar(vec![])), cx)
            && !view.update_readme(
                &old,
                SectionState::Error(PresentationError::Offline),
                window,
                cx,
            )
    });
    assert!(rejected);
    assert!(view.read_with(cx, |view, _| view.accepts(&new)));
    assert!(cx.debug_bounds("profile-basic").is_none());
}

#[gpui::test]
fn partial_failures_do_not_blank_successful_sections(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    let request = view.read_with(cx, |view, _| view.request());
    view.update(cx, |view, cx| {
        assert!(view.update_repositories(
            &request,
            SectionState::Error(PresentationError::Forbidden),
            cx
        ));
        assert!(!view.update_contributions(
            &request,
            &ContributionRange::new("2023-01-01", "2023-12-31").unwrap(),
            SectionState::Error(PresentationError::Unavailable),
            cx
        ));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("profile-basic").is_some());
    assert!(cx.debug_bounds("profile-readme-reader").is_some());
    assert!(cx.debug_bounds("profile-section-error").is_some());
    assert!(cx.debug_bounds("profile-contribution-day-0").is_some());
}

#[gpui::test]
fn readme_uses_reader_and_actions_keep_the_current_target(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    assert!(cx.debug_bounds("reader-markdown").is_some());
    let reader_bounds = cx.debug_bounds("profile-readme-reader").unwrap();
    // No profile-owned fixed viewport: the wrapper is sized by its parent flow and the reader
    // itself owns no vertical scrollbar here (the shared Reader Flow API is still external).
    assert!(reader_bounds.size.height > px(0.0));
    assert!(
        cx.debug_bounds("profile-readme-scrollbar-y").is_none(),
        "Profile must not add a second vertical scroll owner"
    );
    let reader = view.read_with(cx, |view, _| view.reader().unwrap().clone());
    reader.update_in(cx, |reader, window, cx| {
        reader.handle_link("https://example.com/docs", window, cx)
    });
    let action = view.read_with(cx, |view, _| view.last_action().cloned().unwrap());
    assert_eq!(action.request.target, target(42));
    assert!(matches!(
        action.kind,
        ProfileActionKind::Readme(ReaderAction::Link(LinkTarget::External { .. }))
    ));
}

#[gpui::test]
fn replaced_readme_cannot_emit_and_rebind_drops_focused_actions(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    let old_reader = view.read_with(cx, |view, _| view.reader().unwrap().clone());
    view.update_in(cx, |view, window, cx| {
        let request = view.request();
        view.update_readme(
            &request,
            SectionState::Ready(readme("person", "new body")),
            window,
            cx,
        );
        view.repository_focus_for_test().focus(window, cx);
    });
    old_reader.update_in(cx, |reader, window, cx| {
        reader.handle_link("https://example.invalid/old", window, cx);
    });
    assert!(view.read_with(cx, |view, _| view.last_action().is_none()));
    view.update_in(cx, |view, _, cx| {
        view.bind(target(42), binding(9, 5), range(), cx);
    });
    publish_ready(&view, cx);
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |view, _| view.last_action().is_none()));
}

#[gpui::test]
fn repository_actions_use_typed_stable_resource_and_keyboard(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    cx.simulate_resize(gpui::size(px(880.0), px(560.0)));
    frames(cx);
    view.update_in(cx, |view, window, cx| {
        view.repository_focus_for_test().focus(window, cx);
    });
    cx.simulate_keystrokes("right enter");
    let action = view.read_with(cx, |view, _| view.last_action().cloned().unwrap());
    let ProfileActionKind::Repository(repository) = action.kind else {
        panic!("expected repository action");
    };
    assert_eq!(repository.id, Some(2));
    assert_eq!(repository.name_with_owner, "owner/two");
    assert_eq!(action.request.binding, binding(7, 1));
    view.update(cx, |view, cx| {
        let request = view.request();
        view.update_repositories(
            &request,
            SectionState::Ready(ProfilePinned {
                repos: vec![
                    repo(1, "one"),
                    repo(2, "two"),
                    repo(3, "three"),
                    repo(4, "four"),
                ],
                total_repos: 4,
                ..pinned(Showcase::PinnedRepositories(vec![]))
            }),
            cx,
        );
    });
    cx.simulate_keystrokes("home down enter");
    let action = view.read_with(cx, |view, _| view.last_action().cloned().unwrap());
    assert!(
        matches!(action.kind, ProfileActionKind::Repository(repository) if repository.id == Some(3))
    );
    cx.simulate_keystrokes("right up enter");
    let action = view.read_with(cx, |view, _| view.last_action().cloned().unwrap());
    assert!(
        matches!(action.kind, ProfileActionKind::Repository(repository) if repository.id == Some(2))
    );
    cx.simulate_keystrokes("right enter");
    let action = view.read_with(cx, |view, _| view.last_action().cloned().unwrap());
    assert!(
        matches!(action.kind, ProfileActionKind::Repository(repository) if repository.id == Some(2)),
        "Right at a row edge stays in that row"
    );
}

#[gpui::test]
fn normal_cards_reflow_and_keyboard_follow_the_actual_columns(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    view.update(cx, |view, cx| {
        let request = view.request();
        view.update_repositories(
            &request,
            SectionState::Ready(ProfilePinned {
                repos: vec![
                    repo(1, "one"),
                    repo(2, "two"),
                    repo(3, "three"),
                    repo(4, "four"),
                ],
                total_repos: 4,
                ..pinned(Showcase::PinnedRepositories(vec![]))
            }),
            cx,
        );
    });
    cx.simulate_resize(gpui::size(px(1100.0), px(800.0)));
    frames(cx);
    let first = cx.debug_bounds("profile-repository-0").unwrap();
    let second = cx.debug_bounds("profile-repository-1").unwrap();
    let third = cx.debug_bounds("profile-repository-2").unwrap();
    assert_eq!(first.top(), second.top());
    assert!(third.top() >= first.bottom());
    assert!(
        second.right() <= px(1100.0),
        "first={first:?} second={second:?}"
    );
    cx.simulate_resize(gpui::size(px(600.0), px(560.0)));
    frames(cx);
    let first = cx.debug_bounds("profile-repository-0").unwrap();
    let second = cx.debug_bounds("profile-repository-1").unwrap();
    assert!(second.top() >= first.bottom());
    assert!(second.right() <= px(600.0));
    view.update_in(cx, |view, window, cx| {
        view.repository_focus_for_test().focus(window, cx)
    });
    cx.simulate_keystrokes("home down enter");
    assert!(
        matches!(view.read_with(cx, |view, _| view.last_action().cloned()),
        Some(action) if matches!(&action.kind, ProfileActionKind::Repository(repo) if repo.id == Some(2)))
    );
}

#[gpui::test]
fn public_metadata_stays_inside_the_minimum_window(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    cx.simulate_resize(gpui::size(px(880.0), px(560.0)));
    frames(cx);
    let header = cx.debug_bounds("profile-basic").unwrap();
    let metadata = cx.debug_bounds("profile-public-metadata").unwrap();
    assert!(
        metadata.right() <= header.right() + px(1.0),
        "{metadata:?} vs {header:?}"
    );
    let name = cx.debug_bounds("profile-name").unwrap();
    let login = cx.debug_bounds("profile-login").unwrap();
    assert!(login.top() >= name.bottom(), "login belongs below the name");
    assert!(metadata.top() >= login.bottom());
    assert!(
        header.size.height < px(180.0),
        "short identity does not become a hero card"
    );
}

#[gpui::test]
fn header_links_and_account_rebind_do_not_infer_authorization(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    cx.simulate_resize(gpui::size(px(880.0), px(560.0)));
    frames(cx);
    let website = cx.debug_bounds("profile-website").unwrap();
    cx.simulate_click(website.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(matches!(
        view.read_with(cx, |view, _| view.last_action().cloned()),
        Some(action) if matches!(action.kind, ProfileActionKind::ExternalLink { .. })
    ));
    let rebound = view.update_in(cx, |view, _, cx| {
        view.bind(target(42), binding(8, 4), range(), cx)
    });
    assert_eq!(rebound.target, target(42));
    assert_eq!(rebound.binding, binding(8, 4));
    assert!(rebound.generation > 0);
    assert!(view.read_with(cx, |view, _| view.last_action().is_none()));
}

#[gpui::test]
fn heatmap_hover_focus_and_keyboard_expose_real_day_data(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    view.update_in(cx, |view, window, cx| {
        view.heatmap_focus_for_test().focus(window, cx);
    });
    cx.simulate_keystrokes("down down");
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_day_detail()),
        Some("2024-02-29: 2 contributions".into())
    );
    cx.simulate_keystrokes("home right");
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_day_detail()),
        Some("2024-03-04: 3 contributions; level unavailable".into())
    );
    cx.simulate_keystrokes("home down down");
    assert!(cx.debug_bounds("profile-day-detail").is_some());
    let leap = cx.debug_bounds("profile-contribution-day-2").unwrap();
    cx.simulate_mouse_move(leap.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_day_detail()),
        Some("2024-02-29: 2 contributions".into())
    );
    cx.simulate_keystrokes("end");
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_day_detail()),
        Some("2024-03-04: 3 contributions; level unavailable".into())
    );
    let model = view.read_with(cx, |view, _| view.heatmap_model().unwrap());
    assert_eq!(model.missing_days, 3);
    assert_eq!(model.observed_days, 4);
    assert!(matches!(
        model.weeks[0].days[6].level(),
        Some(CellLevel::Unknown)
    ));
}

#[gpui::test]
fn page_wheel_reaches_sections_below_the_viewport(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    cx.simulate_resize(gpui::size(px(880.0), px(560.0)));
    frames(cx);
    let before = cx.debug_bounds("profile-repositories-section").unwrap();
    let header = cx.debug_bounds("profile-basic").unwrap();
    cx.simulate_mouse_move(header.center(), None, gpui::Modifiers::none());
    cx.simulate_event(gpui::ScrollEvent {
        position: header.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-300.0))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.page_scroll.offset().y < px(0.0)));
    let after = cx.debug_bounds("profile-repositories-section").unwrap();
    assert!(after.top() < before.top());

    view.update(cx, |view, _| view.show_page_scrollbar_for_test());
    cx.run_until_parked();
    let thumb = cx.debug_bounds("profile-page-scrollbar-thumb").unwrap();
    let start = thumb.center();
    cx.simulate_mouse_move(start, None, gpui::Modifiers::none());
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.run_until_parked();
    for step in 1..=3 {
        cx.simulate_mouse_move(
            start + point(px(0.0), px(5.0 * step as f32)),
            gpui::MouseButton::Left,
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
    }
    cx.simulate_mouse_up(
        start + point(px(0.0), px(15.0)),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.page_dragging_for_test()),
        None
    );
    let before_wheel = view.read_with(cx, |view, _| view.page_scroll.offset().y);
    // Drive the page's real scroll target the same way its wheel overlay does. With the README
    // still owning a Reader viewport, pointer input over the inner document belongs to the reader;
    // the page's own target is asserted here without claiming the shared Reader Flow is present.
    view.update(cx, |view, _| {
        view.page_motion
            .set_offset(point(px(0.0), before_wheel - px(30.0)));
    });
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |view, _| view.page_scroll.offset().y < before_wheel),
        "the Profile page still owns its own scroll target"
    );
}

#[gpui::test]
fn repository_card_stats_align_with_the_card_content_edge(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    publish_ready(&view, cx);
    let card = cx.debug_bounds("profile-repository-0").unwrap();
    let stats = cx.debug_bounds("profile-repository-stats-0").unwrap();
    let description = cx.debug_bounds("profile-repository-description-0").unwrap();
    assert!(
        (stats.left() - card.left() - px(12.0)).abs() <= px(1.0),
        "stats start at {stats:?}, card content edge is {:?}",
        card.left() + px(12.0)
    );
    assert!(
        (description.left() - stats.left()).abs() <= px(1.0),
        "description keeps the same content edge: {description:?} vs {stats:?}"
    );
    assert!(
        stats.left() >= card.left(),
        "the footer never escapes the card's content box"
    );
}

#[gpui::test]
fn long_content_and_long_ranges_stay_bounded(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let request = view.read_with(cx, |view, _| view.request());
    view.update_in(cx, |view, window, cx| {
        let long_range = ContributionRange::new("2023-03-01", "2024-02-29").unwrap();
        let new_request = view.bind(target(42), binding(7, 1), long_range.clone(), cx);
        assert!(view.update_basic(
            &new_request,
            SectionState::Ready(ProfileBasic {
                bio: Some("long public bio ".repeat(100)),
                company: Some("long-public-company-".repeat(30)),
                ..basic("person")
            }),
            cx
        ));
        assert!(view.update_repositories(
            &new_request,
            SectionState::Ready(ProfilePinned {
                repos: vec![ProfileRepo {
                    description: Some("long repository description ".repeat(100)),
                    ..repo(1, "long")
                }],
                total_repos: 1,
                ..pinned(Showcase::PinnedRepositories(vec![]))
            }),
            cx
        ));
        assert!(view.update_contributions(
            &new_request,
            &long_range,
            SectionState::Ready(calendar(long_range_days())),
            cx
        ));
        assert!(view.update_readme(
            &new_request,
            SectionState::Ready(readme(
                "person",
                &("# Long\n\n".to_string() + &"word ".repeat(5000))
            )),
            window,
            cx
        ));
        let _ = request;
    });
    cx.simulate_resize(gpui::size(px(880.0), px(560.0)));
    frames(cx);
    assert!(cx.debug_bounds("profile-basic").is_some());
    assert!(cx.debug_bounds("profile-repositories-section").is_some());
    assert!(cx.debug_bounds("profile-readme-reader").is_some());
    let graph = cx.debug_bounds("profile-heatmap-grid").unwrap();
    let wide_reach = view.read_with(cx, |view, _| view.heatmap_scroll.max_offset().x);
    cx.simulate_resize(gpui::size(px(600.0), px(560.0)));
    frames(cx);
    assert!(
        view.read_with(cx, |view, _| view.heatmap_scroll.max_offset().x
            > wide_reach)
    );
    let max = view.read_with(cx, |view, _| view.heatmap_scroll.max_offset().x);
    view.update(cx, |view, _| {
        view.heatmap_motion
            .set_axis_offset(gpui::Axis::Horizontal, -max)
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.heatmap_scroll.offset().x < px(0.0)));
    assert!(view.read_with(cx, |view, _| view.heatmap_scroll.offset().x
        >= -view.heatmap_scroll.max_offset().x));
    view.update_in(cx, |view, window, cx| {
        view.heatmap_focus_for_test().focus(window, cx);
    });
    cx.simulate_keystrokes("home end");
    let max = view.read_with(cx, |view, _| view.heatmap_scroll.max_offset().x);
    assert!(view.read_with(cx, |view, _| {
        (view.heatmap_scroll.offset().x + max).abs() <= px(1.0)
    }));
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_day_detail()),
        Some("2024-02-29: 365 contributions".into())
    );
    cx.simulate_keystrokes("home");
    assert_eq!(
        view.read_with(cx, |view, _| view.heatmap_scroll.offset().x),
        px(0.0)
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_day_detail()),
        Some("2023-03-01: 0 contributions".into())
    );
    view.update_in(cx, |view, _, _| {
        let started = std::time::Instant::now() - std::time::Duration::from_millis(60);
        assert!(
            view.page_motion
                .wheel(started, -50.0, gpui::Axis::Vertical, false, false)
        );
        assert!(
            view.heatmap_motion
                .wheel(started, -50.0, gpui::Axis::Horizontal, false, false)
        );
        view.advance_scroll(started + std::time::Duration::from_millis(60), false);
        assert!(view.page_motion.is_animating());
        assert!(view.page_scroll.offset().y < px(0.0));
        assert!(
            view.heatmap_scroll.offset().x < px(0.0),
            "both regions advance in the same frame"
        );
    });
    let day = cx.debug_bounds("profile-contribution-day-0").unwrap();
    assert!(
        day.top() > graph.top(),
        "months have their own row above days"
    );
    assert!(day.size.height < graph.size.height / 7.0);
}

fn frames(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
}
