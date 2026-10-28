//! Standalone developer preview of the reusable Profile Overview.
//!
//! `cargo run --example profile` uses only synthetic data. The normal product
//! binary has no scenario chooser, preview route or debug flag.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/bin_support/profile_modules.rs"]
mod profile_modules;
#[allow(unused_imports)]
use profile_modules as ui;
#[allow(unused_imports)]
use profile_modules::profile;

use ely_core::{
    Assets,
    buttons::{Button, ButtonVariant},
    theme::{ActiveTheme, Mode, TextSize, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use std::rc::Rc;

use crate::{
    content::PreparedBody,
    domain::{
        AccountKey, AuthEpoch, ContributionCalendar, ContributionDay, ContributionRange,
        ContributionWeek, HostId, ProfileBasic, ProfileGraphqlError, ProfilePinned, ProfileRepo,
        Showcase,
    },
    profile::{
        PresentationError, ProfileAction, ProfileBinding, ProfileTarget, ProfileView, SectionState,
    },
    profile_modules::reader::ResourceContext,
};

#[derive(Clone, Copy)]
enum Scenario {
    Complete,
    Partial,
    MixedPins,
    GistsOnly,
    Truncated,
    Unknown,
    Sparse,
    Errors,
    Empty,
}

const SCENARIOS: &[(Scenario, &str)] = &[
    (Scenario::Complete, "Complete"),
    (Scenario::Partial, "Partial + long"),
    (Scenario::MixedPins, "Mixed pins"),
    (Scenario::GistsOnly, "Gists only"),
    (Scenario::Truncated, "Truncated"),
    (Scenario::Unknown, "Unknown levels"),
    (Scenario::Sparse, "Missing fields"),
    (Scenario::Errors, "Errors"),
    (Scenario::Empty, "Real empty"),
];

struct Preview {
    profile: Entity<ProfileView>,
    toolbar_scroll: gpui::ScrollHandle,
    scenario: Scenario,
    action: Option<String>,
}

impl Preview {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let target = ProfileTarget::new(HostId::github(), 42);
        let binding = ProfileBinding::new(AccountKey::new(7, HostId::github()), AuthEpoch(1));
        let profile = cx.new(|cx| ProfileView::new(target, binding, rolling_year(), window, cx));
        let mut preview = Self {
            profile,
            toolbar_scroll: gpui::ScrollHandle::new(),
            scenario: Scenario::Complete,
            action: None,
        };
        let sink = cx.entity().downgrade();
        preview.profile.update(cx, |profile, _| {
            profile.on_action(Rc::new(move |action, _, cx| {
                if let Some(preview) = sink.upgrade() {
                    preview.update(cx, |preview, cx| {
                        preview.action = Some(describe(&action));
                        cx.notify();
                    });
                }
            }));
        });
        preview.load(window, cx);
        preview
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = ProfileTarget::new(HostId::github(), 42);
        let binding = ProfileBinding::new(AccountKey::new(7, HostId::github()), AuthEpoch(1));
        let request = self.profile.update(cx, |profile, cx| {
            profile.bind(target, binding, rolling_year(), cx)
        });
        let scenario = self.scenario;
        self.action = None;
        self.profile.update(cx, |profile, cx| {
            let basic_state = SectionState::Ready(basic(scenario));
            profile.update_basic(&request, basic_state, cx);
            profile.update_repositories(&request, repositories(scenario), cx);
            profile.update_contributions(&request, &rolling_year(), contributions(scenario), cx);
            profile.update_readme_with_context(
                &request,
                readme(scenario),
                ResourceContext::new("octo", "octo", "a".repeat(40), "README.md"),
                window,
                cx,
            );
        });
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("profile-preview-toolbar-scroll")
            .debug_selector(|| "profile-preview-toolbar".to_string())
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .overflow_x_scroll()
            .track_scroll(&self.toolbar_scroll)
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .children(
                SCENARIOS
                    .iter()
                    .enumerate()
                    .map(|(index, (scenario, label))| {
                        let this = cx.entity();
                        let active = std::mem::discriminant(scenario)
                            == std::mem::discriminant(&self.scenario);
                        Button::new(("profile-scenario", index), *label)
                            .variant(if active {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            })
                            .on_click(move |_, window, cx| {
                                this.update(cx, |preview, cx| {
                                    preview.scenario = *scenario;
                                    preview.load(window, cx);
                                });
                            })
                    }),
            )
    }
}

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let action = self.action.clone().unwrap_or_else(|| {
            "Synthetic preview · actions are reported here, never opened automatically.".into()
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors.bg)
            .text_color(cx.theme().colors.fg)
            .child(self.toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.profile.clone()),
            )
            .child(
                div()
                    .debug_selector(|| "profile-preview-action".to_string())
                    .p_2()
                    .border_t_1()
                    .border_color(cx.theme().colors.border)
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .text_color(cx.theme().colors.fg_subtle)
                    .child(action),
            )
    }
}

fn rolling_year() -> ContributionRange {
    // A fixed synthetic interval keeps the preview deterministic. Production
    // hosts supply the current rolling-year range; the component has no clock.
    ContributionRange::new("2023-03-01", "2024-02-29").unwrap()
}

fn basic(scenario: Scenario) -> ProfileBasic {
    ProfileBasic {
        id: 42,
        login: "octo".into(),
        name: match scenario {
            Scenario::Sparse => None,
            Scenario::Partial => {
                Some("A Very Long Public Display Name Used To Check Wrapping".into())
            }
            _ => Some("Octo Example".into()),
        },
        bio: match scenario {
            Scenario::Sparse => None,
            Scenario::Partial => Some(
                "A deliberately long public bio with wrapping text, 中文 and emoji 🚀. ".repeat(12),
            ),
            _ => Some("A synthetic public profile for the standalone component preview.".into()),
        },
        avatar_url: Some("https://invalid.example/avatar-is-never-fetched".into()),
        profile_url: Some("https://example.com/octo".into()),
        company: match scenario {
            Scenario::Sparse => None,
            Scenario::Partial => Some("A public organization with a deliberately long name".into()),
            _ => Some("Example public organization".into()),
        },
        location: match scenario {
            Scenario::Sparse => None,
            _ => Some("Public location".into()),
        },
        website_url: match scenario {
            Scenario::Sparse => None,
            _ => Some("https://example.com".into()),
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
        name_with_owner: Some(format!("octo/{name}")),
        description: Some(if name == "long-repository-description" {
            "A long public repository description used to verify that wrapping and clipping cannot cover subsequent Overview sections. ".repeat(12)
        } else {
            format!("Synthetic public repository {name}")
        }),
        url: Some(format!("https://example.com/octo/{name}")),
        is_private: Some(false),
        is_fork: Some(false),
        stargazer_count: Some(id * 10),
        fork_count: Some(id),
        primary_language: Some("Rust".into()),
        owner_login: Some("octo".into()),
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

fn repositories(scenario: Scenario) -> SectionState<ProfilePinned> {
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
        Scenario::Complete | Scenario::Sparse => pinned(Showcase::PopularRepositories(vec![
            repo(2, "popular-two"),
            repo(1, "popular-one"),
        ])),
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

fn contributions(scenario: Scenario) -> SectionState<ContributionCalendar> {
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

fn readme(scenario: Scenario) -> SectionState<PreparedBody> {
    match scenario {
        Scenario::Errors => SectionState::Error(PresentationError::Offline),
        Scenario::Empty => SectionState::Ready(PreparedBody::Empty),
        Scenario::Sparse => SectionState::Loading,
        _ => SectionState::Ready(PreparedBody::Utf8 {
            text: format!(
                "# Synthetic profile README\n\nThis is **prepared content** rendered by the existing ReaderView. [External link](https://example.com) actions are only reported.\n\n## Overview checks\n\n- server order is preserved\n- missing data is not zero\n- avatar URLs are never fetched\n\n{}",
                if matches!(scenario, Scenario::Partial) {
                    "Long README prose with wrapping and `inline_code` to exercise the bounded reader. 中文 emoji 🚀\n\n".repeat(150)
                } else {
                    "Short fixture body.\n".repeat(10)
                }
            ),
            has_bom: false,
        }),
    }
}

fn describe(action: &ProfileAction) -> String {
    format!("target {} · {:?}", action.request.target, action.kind)
}

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_core::init(cx).expect("Ely failed to start");
            fonts::register(cx).expect("the bundled fonts failed to register");
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = fonts::UI_FAMILY.into();
                theme.mono_family = fonts::CODE_FAMILY.into();
            });
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1000.0), px(760.0)),
                        cx,
                    ))),
                    window_min_size: Some(size(px(880.0), px(560.0))),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("Githux · Profile Overview preview".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Preview::new(window, cx)),
            )
            .expect("the Profile preview window failed to open");
            cx.activate(true);
        });
}
