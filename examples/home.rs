//! Standalone, synthetic Home/Feed preview. This is not a product route or a
//! real-data integration; the host adapter and formal Shell mount come later.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/platform/fonts.rs"]
mod fonts;
#[path = "../src/bin_support/home_modules.rs"]
mod home_modules;
#[allow(unused_imports)]
use home_modules as ui;

use std::cell::RefCell;
use std::rc::Rc;

use ely_core::{
    Assets,
    buttons::Button,
    theme::{ActiveTheme, Mode, TextSize, Theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::domain::error::GitHubError;
use crate::domain::home::{
    AuthorResolutions, FeedActor, FeedAuthor, FeedEvent, FeedEventType, FeedOutcome,
    FeedPartialReason, FeedPayload, FeedReadiness, FeedRepository, FollowedAccount, Following,
    FollowingReadiness, HomeError, HomeItem, HomeItemKind, HomePartialReason, HomeReadiness,
    HomeRepositoryRef, HomeSearchOutcome, ResolvedPullRequest, build_feed,
};
use crate::ui::home::{Column, HomeView, RefreshAvailability, RequestBinding};

#[derive(Clone, Copy)]
struct Scenario {
    label: &'static str,
    build: fn() -> (HomeSearchOutcome, FeedOutcome),
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        label: "Mixed",
        build: mixed,
    },
    Scenario {
        label: "Loading / error",
        build: loading_error,
    },
    Scenario {
        label: "Partial",
        build: partial,
    },
    Scenario {
        label: "Filtered out",
        build: filtered_out,
    },
    Scenario {
        label: "Empty / unavailable",
        build: empty_unavailable,
    },
    Scenario {
        label: "Large",
        build: large,
    },
];

struct Preview {
    home: Entity<HomeView>,
    scenario: usize,
    binding: RequestBinding,
    generation: u64,
    action: Rc<RefCell<Option<String>>>,
    cooldown: bool,
    _changed: gpui::Subscription,
}

impl Preview {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let binding = RequestBinding::new("preview-account/home");
        let home = cx.new(|cx| HomeView::new(binding.clone(), 1, window, cx));
        let action = Rc::new(RefCell::new(None));
        let sink = action.clone();
        let weak = cx.weak_entity();
        home.update(cx, |home, cx| {
            // Fixture clock: 2026-10-28 12:00 UTC, independent of wall time.
            home.set_now(
                Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_793_188_800)),
                cx,
            );
            home.on_action(Rc::new(move |action, window, cx| {
                if let crate::ui::home::HomeAction::LoadMoreFeed(_) = action {
                    // Defer like the real host: the emitting HomeView is still on
                    // the update stack, so append after this dispatch completes.
                    let weak = weak.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = weak.update(cx, |preview, cx| preview.load_more(window, cx));
                    });
                    return;
                }
                *sink.borrow_mut() = Some(format!("{action:?}"));
            }));
        });
        let mut preview = Self {
            home: home.clone(),
            // A preview-only switch so a capture/verification run can start on a
            // named scenario without a click. It never affects product code.
            scenario: initial_scenario(),
            binding,
            generation: 1,
            action,
            cooldown: false,
            _changed: cx.observe(&home, |_, _, cx| cx.notify()),
        };
        preview.apply(window, cx);
        preview
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.cooldown = false;
        self.home.update(cx, |home, cx| {
            home.bind(self.binding.clone(), self.generation, window, cx);
        });
        match SCENARIOS[self.scenario].label {
            "Loading / error" => {
                self.home.update(cx, |home, cx| {
                    home.set_loading(Column::Home, &self.binding, self.generation, cx);
                    home.apply_error(
                        Column::Feed,
                        &self.binding,
                        self.generation,
                        HomeError::GitHub(GitHubError::Network {
                            message: "synthetic offline fixture".to_string(),
                        }),
                        cx,
                    );
                });
            }
            "Empty / unavailable" => {
                let (_, feed) = empty_unavailable();
                self.home.update(cx, |home, cx| {
                    home.set_unavailable(
                        Column::Home,
                        &self.binding,
                        self.generation,
                        "Offline and no cached Home snapshot is available; this is not an empty result.",
                        cx,
                    );
                    home.apply_feed(&self.binding, self.generation, feed, cx);
                });
            }
            _ => {
                let (search, feed) = (SCENARIOS[self.scenario].build)();
                self.home.update(cx, |home, cx| {
                    home.apply_home(&self.binding, self.generation, search, cx);
                    home.apply_feed(&self.binding, self.generation, feed, cx);
                });
            }
        }
    }

    /// The preview host's own load-more: a synthetic second source page whose
    /// author/subject is already followed, appended through the real view API.
    /// This exercises the same scroll->append path as the real Shell host.
    fn load_more(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.home.update(cx, |home, cx| {
            home.feed_append_loading(&self.binding, self.generation, cx);
        });
        // A synthetic page-2: one merged contribution plus one star by a
        // followed account, so an append visibly extends the Feed.
        let mut second = build_feed(
            vec![
                merged_event(10, FOLLOWER_LOGIN, "synthetic-owner/older-repo", 43),
                starred_event(11, FOLLOWER_LOGIN, "synthetic-owner/older-repo"),
            ],
            Vec::new(),
            &preview_following(),
            &preview_authors_older(),
        );
        second.next_page = None;
        self.home.update(cx, |home, cx| {
            let existing = match home.feed_state() {
                crate::ui::home::PanelState::Ready {
                    content: crate::ui::home::PanelContent::Ready(outcome),
                    ..
                } => outcome.clone(),
                _ => return,
            };
            let mut outcome = existing;
            outcome.activities.extend(second.activities.clone());
            outcome.next_page = None;
            home.apply_feed_page(&self.binding, self.generation, outcome, cx);
        });
    }

    fn switch(&mut self, scenario: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.scenario = scenario;
        *self.action.borrow_mut() = None;
        self.apply(window, cx);
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .debug_selector(|| "home-preview-toolbar".to_string())
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.colors.border)
            .children(SCENARIOS.iter().enumerate().map(|(index, scenario)| {
                let this = cx.entity();
                Button::new(("home-scenario", index), scenario.label)
                    .variant(if index == self.scenario {
                        ely_core::buttons::ButtonVariant::Primary
                    } else {
                        ely_core::buttons::ButtonVariant::Secondary
                    })
                    .on_click(move |_, window, cx| {
                        this.update(cx, |preview, cx| preview.switch(index, window, cx));
                    })
            }))
            .child(
                Button::new("home-preview-complete", "Finish refresh").on_click({
                    let this = cx.entity();
                    move |_, _, cx| {
                        this.update(cx, |preview, cx| {
                            let (search, feed) = (SCENARIOS[preview.scenario].build)();
                            preview.home.update(cx, |home, cx| {
                                home.apply_home(&preview.binding, preview.generation, search, cx);
                                home.apply_feed(&preview.binding, preview.generation, feed, cx);
                            });
                        });
                    }
                }),
            )
            .child(
                Button::new("home-preview-cooldown", "Toggle Home cooldown").on_click({
                    let home = self.home.clone();
                    let binding = self.binding.clone();
                    let generation = self.generation;
                    {
                        let this = cx.entity();
                        move |_, _, cx| {
                            this.update(cx, |preview, cx| {
                                preview.cooldown = !preview.cooldown;
                                let unavailable = preview.cooldown;
                                home.update(cx, |home, cx| {
                                    home.set_refresh_availability(
                                        Column::Home,
                                        &binding,
                                        generation,
                                        if unavailable {
                                            RefreshAvailability::unavailable("Preview cooldown")
                                        } else {
                                            RefreshAvailability::available()
                                        },
                                        cx,
                                    );
                                });
                                cx.notify();
                            });
                        }
                    }
                }),
            )
    }
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (bg, fg, border, subtle, tiny) = {
            let theme = cx.theme();
            (
                theme.colors.bg,
                theme.colors.fg,
                theme.colors.border,
                theme.colors.fg_subtle,
                theme.text_size(TextSize::Xs),
            )
        };
        let action = self.action.borrow().clone().unwrap_or_else(|| {
            "Synthetic fixture · actions are shown here; nothing is opened.".to_string()
        });
        div()
            .w(window.viewport_size().width)
            .h(window.viewport_size().height)
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .child(self.toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .w_full()
                    .child(self.home.clone()),
            )
            .child(
                div()
                    .debug_selector(|| "home-preview-action".to_string())
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .text_size(tiny)
                    .text_color(subtle)
                    .child(action),
            )
    }
}

/// Parse the optional preview-only scenario selector: a label (case-insensitive)
/// or a 0-based index. Unknown values fall back to the first scenario.
fn initial_scenario() -> usize {
    let Some(argument) = std::env::args().nth(1) else {
        return 0;
    };
    if let Ok(index) = argument.parse::<usize>()
        && index < SCENARIOS.len()
    {
        return index;
    }
    SCENARIOS
        .iter()
        .position(|scenario| scenario.label.eq_ignore_ascii_case(&argument))
        .unwrap_or(0)
}

fn fixture_time(index: usize) -> Option<String> {
    match index % 8 {
        0 => Some("2026-10-28T11:50:00Z".into()),
        1 => Some("2026-10-28T11:00:00Z".into()),
        2 => Some("2026-10-27T12:00:00Z".into()),
        3 => None,
        4 => Some("2026-10-26T12:00:00Z".into()),
        5 => Some("2026-10-28T12:01:00Z".into()),
        6 => Some("invalid timestamp".into()),
        _ => Some("2026-10-28T11:59:20Z".into()),
    }
}

/// Home only presents open entries. The fixtures therefore contain open items
/// only: closed/merged/unknown states are covered by the view/adapter tests,
/// which assert they never become a row.
fn item(index: usize) -> HomeItem {
    HomeItem {
        id: 9_000 + index as u64,
        kind: if index.is_multiple_of(2) {
            HomeItemKind::Issue
        } else {
            HomeItemKind::PullRequest
        },
        number: 100 + index as u64,
        title: if index == 0 {
            "A deliberately long synthetic Issue title used to verify bounded text truncation in a narrow Home panel without horizontal overflow".to_string()
        } else {
            format!("Synthetic work item {index}")
        },
        html_url: String::new(),
        state: "open".to_string(),
        updated_at: fixture_time(index).unwrap_or_default(),
        repository: HomeRepositoryRef {
            owner: "synthetic-owner".to_string(),
            name: format!("repository-{}", index % 3),
        },
    }
}

/// The merged pull request is authored by a contributor and produced by a
/// different (maintainer) actor, so the credited identity is visible in preview.
const CONTRIBUTOR_LOGIN: &str = "synthetic-contributor";
/// The followed account that stars and creates repositories in the preview.
const FOLLOWER_LOGIN: &str = "synthetic-user";
/// An account nobody follows in the preview: its activities must never appear.
const STRANGER_LOGIN: &str = "synthetic-stranger";

fn event(index: usize, actor_login: &str, repository: &str) -> FeedEvent {
    FeedEvent {
        id: format!("synthetic-event-{index}"),
        event_type: FeedEventType::Watch,
        actor: FeedActor {
            id: if actor_login == FOLLOWER_LOGIN {
                50
            } else {
                60
            },
            login: actor_login.to_string(),
            display_login: None,
            avatar_url: None,
        },
        repository: FeedRepository {
            id: 70 + index as u64,
            name: repository.into(),
            url: String::new(),
        },
        created_at: fixture_time(index),
        public: true,
        payload: FeedPayload::Watch {
            action: "started".into(),
        },
    }
}

/// A live `action=merged` event: the payload's `pull_request` carries only
/// url/id/number/head/base, so the preview also demonstrates author enrichment
/// from the detail endpoint via `AuthorResolutions`.
fn merged_event(index: usize, merger: &str, repository: &str, number: u64) -> FeedEvent {
    let mut event = event(index, merger, repository);
    event.event_type = FeedEventType::PullRequest;
    event.payload = FeedPayload::PullRequest {
        action: "merged".into(),
        number: Some(number),
        title: None,
        html_url: None,
        state: None,
        merged: None,
        author: None,
    };
    event
}

fn created_repository_event(index: usize, actor_login: &str, repository: &str) -> FeedEvent {
    let mut event = event(index, actor_login, repository);
    event.event_type = FeedEventType::Create;
    event.payload = FeedPayload::Create {
        ref_type: Some("repository".into()),
        git_ref: None,
    };
    event
}

fn starred_event(index: usize, actor_login: &str, repository: &str) -> FeedEvent {
    event(index, actor_login, repository)
}

/// Events the Feed product policy must exclude. They run through the real
/// `build_feed` so the preview proves no card or placeholder appears for
/// push/issue/comment/release/fork/membership/branch/tag/unknown, non-merged pull
/// requests, and activities by an account that is not followed.
fn excluded_events() -> Vec<FeedEvent> {
    let mut noise = Vec::new();

    let mut push = event(100, FOLLOWER_LOGIN, "synthetic-owner/repository");
    push.event_type = FeedEventType::Push;
    push.payload = FeedPayload::Push {
        git_ref: Some("refs/heads/main".into()),
        before: None,
        head: None,
        size: Some(3),
    };
    noise.push(push);

    let mut issue = event(101, FOLLOWER_LOGIN, "synthetic-owner/repository");
    issue.event_type = FeedEventType::Issues;
    issue.payload = FeedPayload::Issues {
        action: "closed".into(),
        number: Some(7),
        title: Some("Synthetic issue".into()),
        html_url: None,
        state: Some("closed".into()),
    };
    noise.push(issue);

    let mut branch = event(102, FOLLOWER_LOGIN, "synthetic-owner/repository");
    branch.event_type = FeedEventType::Create;
    branch.payload = FeedPayload::Create {
        ref_type: Some("branch".into()),
        git_ref: Some("preview".into()),
    };
    noise.push(branch);

    let mut opened = event(103, FOLLOWER_LOGIN, "synthetic-owner/repository");
    opened.event_type = FeedEventType::PullRequest;
    opened.payload = FeedPayload::PullRequest {
        action: "opened".into(),
        number: Some(43),
        title: Some("Synthetic open pull request".into()),
        html_url: None,
        state: Some("open".into()),
        merged: None,
        author: None,
    };
    noise.push(opened);

    let mut closed_unmerged = event(104, FOLLOWER_LOGIN, "synthetic-owner/repository");
    closed_unmerged.event_type = FeedEventType::PullRequest;
    closed_unmerged.payload = FeedPayload::PullRequest {
        action: "closed".into(),
        number: Some(44),
        title: Some("Synthetic unmerged pull request".into()),
        html_url: None,
        state: Some("closed".into()),
        merged: Some(false),
        author: None,
    };
    noise.push(closed_unmerged);

    let mut unknown = event(105, FOLLOWER_LOGIN, "synthetic-owner/repository");
    unknown.event_type = FeedEventType::Unknown("FutureSyntheticEvent".into());
    unknown.payload = FeedPayload::Unknown;
    noise.push(unknown);

    // Retained activity **types**, but by an account the preview does not follow.
    noise.push(created_repository_event(
        106,
        STRANGER_LOGIN,
        "synthetic-stranger/private-repo",
    ));
    noise.push(starred_event(
        107,
        STRANGER_LOGIN,
        "synthetic-stranger/another-repo",
    ));
    // A merged contribution whose author is not followed.
    let unfollowed_merge = merged_event(108, FOLLOWER_LOGIN, "synthetic-owner/other", 501);
    noise.push(unfollowed_merge);

    noise
}

/// The followed-account set used by every preview scenario.
fn preview_following() -> Following {
    Following {
        accounts: vec![
            FollowedAccount {
                id: 50,
                login: FOLLOWER_LOGIN.into(),
                avatar_url: None,
            },
            FollowedAccount {
                id: 90_001,
                login: CONTRIBUTOR_LOGIN.into(),
                avatar_url: None,
            },
        ],
        readiness: FollowingReadiness::Complete,
    }
}

/// The author resolutions a real host would obtain from the pull-request detail
/// endpoint for the preview's merged events.
fn preview_authors() -> AuthorResolutions {
    let mut authors = AuthorResolutions::new();
    authors.insert(
        "synthetic-owner/repository#42",
        ResolvedPullRequest {
            author: FeedAuthor {
                id: 90_001,
                login: CONTRIBUTOR_LOGIN.into(),
                avatar_url: None,
            },
            merged: true,
            title: Some("Synthetic merged pull request".into()),
            html_url: None,
        },
    );
    authors.insert(
        "synthetic-owner/other#501",
        ResolvedPullRequest {
            author: FeedAuthor {
                id: 60,
                login: STRANGER_LOGIN.into(),
                avatar_url: None,
            },
            merged: true,
            title: Some("Unfollowed author's contribution".into()),
            html_url: None,
        },
    );
    authors
}

/// Author resolutions for the preview's second source page.
fn preview_authors_older() -> AuthorResolutions {
    let mut authors = AuthorResolutions::new();
    authors.insert(
        "synthetic-owner/older-repo#43",
        ResolvedPullRequest {
            author: FeedAuthor {
                id: 90_001,
                login: CONTRIBUTOR_LOGIN.into(),
                avatar_url: None,
            },
            merged: true,
            title: Some("Older synthetic merged pull request".into()),
            html_url: None,
        },
    );
    authors
}

/// Three live-shaped source events: the merged contribution (author resolved from
/// the detail endpoint), a star and a repository creation, all by followed
/// accounts.
fn retained_events() -> Vec<FeedEvent> {
    vec![
        merged_event(0, FOLLOWER_LOGIN, "synthetic-owner/repository", 42),
        starred_event(1, FOLLOWER_LOGIN, "synthetic-owner/repository"),
        created_repository_event(2, FOLLOWER_LOGIN, "synthetic-owner/repository"),
    ]
}

fn search(items: Vec<HomeItem>, readiness: HomeReadiness) -> HomeSearchOutcome {
    let total_count = items.len() as u64;
    HomeSearchOutcome {
        items,
        total_count,
        incomplete_results: !matches!(readiness, HomeReadiness::Complete),
        readiness,
    }
}

fn feed() -> FeedOutcome {
    // Every preview scenario goes through the real product policy, exactly as the
    // app's data layer does. The Mixed scenario exposes a next page so the
    // preview can actually scroll and append (not just render a first frame).
    let mut outcome = build_feed(
        retained_events(),
        Vec::new(),
        &preview_following(),
        &preview_authors(),
    );
    outcome.next_page = Some(crate::domain::home::FeedSourceCursor { page: 2 });
    outcome
}

fn feed_with(readiness: FeedReadiness) -> FeedOutcome {
    FeedOutcome {
        poll_interval_seconds: Some(60),
        readiness,
        ..feed()
    }
}

fn mixed() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search((0..6).map(item).collect(), HomeReadiness::Complete),
        {
            let mut source = retained_events();
            source.extend(excluded_events());
            let mut outcome =
                build_feed(source, Vec::new(), &preview_following(), &preview_authors());
            // Expose a second source page so the preview can actually scroll and
            // append, not just render a first frame.
            outcome.next_page = Some(crate::domain::home::FeedSourceCursor { page: 2 });
            outcome
        },
    )
}

fn loading_error() -> (HomeSearchOutcome, FeedOutcome) {
    mixed()
}

fn partial() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search(
            (0..4).map(item).collect(),
            HomeReadiness::Partial {
                reasons: vec![
                    HomePartialReason::ResultLimit,
                    HomePartialReason::IncompleteResults,
                ],
            },
        ),
        feed_with(FeedReadiness::Partial {
            reasons: vec![FeedPartialReason::FollowingIncomplete {
                details: vec!["a later followed-accounts page failed".into()],
            }],
        }),
    )
}

/// Every source event is technical noise, an unfollowed activity, or an unmerged
/// pull request, so the Feed column must be the normal minimal empty state while
/// Home stays populated.
fn filtered_out() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search((0..3).map(item).collect(), HomeReadiness::Complete),
        build_feed(
            excluded_events(),
            Vec::new(),
            &preview_following(),
            &preview_authors(),
        ),
    )
}

fn empty_unavailable() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search(Vec::new(), HomeReadiness::Complete),
        build_feed(
            Vec::new(),
            Vec::new(),
            &preview_following(),
            &preview_authors(),
        ),
    )
}

fn large() -> (HomeSearchOutcome, FeedOutcome) {
    (
        search(
            (0..1000).map(item).collect(),
            HomeReadiness::Partial {
                reasons: vec![HomePartialReason::ResultLimit],
            },
        ),
        feed_with(FeedReadiness::Complete),
    )
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
            let bounds = Bounds::centered(None, size(px(1180.0), px(800.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(620.0), px(520.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Githux · Home view preview".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Preview::new(window, cx)),
            )
            .expect("the Home preview window failed to open");
            cx.activate(true);
        });
}
