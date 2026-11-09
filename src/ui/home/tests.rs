//! Deterministic Native Shell tests for the reusable Home/Feed view.
//!
//! They cover the two independent panels, the open-only product rule, Partial
//! honesty, historical snapshot marking, the Feed append footer, relative time and
//! one-shot typed callbacks. Real Windows input, IME and real GitHub data remain
//! manual checks recorded in the task devlog.

use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

use super::presentation::{feed_summary, home_summary};
use super::rows::{event_description, feed_repository};
use super::*;
use crate::domain::error::GitHubError;
use crate::domain::home::{
    BodyPreview, FeedActivity, FeedActivityKind, FeedActivityTarget, FeedActor, FeedEvent,
    FeedEventType, FeedOutcome, FeedPartialReason, FeedPayload, FeedReadiness, FeedRepository,
    HomeItem, HomeItemKind, HomePartialReason, HomeReadiness, HomeRepositoryRef, HomeSearchOutcome,
};

static ACTIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOCK: Mutex<()> = Mutex::new(());

fn binding() -> RequestBinding {
    RequestBinding::new("account-a/home")
}

fn fixed_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_793_188_800)
}

fn home_item(index: usize, kind: HomeItemKind, state: &str) -> HomeItem {
    HomeItem {
        id: 10_000 + index as u64,
        kind,
        number: 100 + index as u64,
        title: if index == 0 {
            "A very long synthetic title that must be truncated rather than widen the column"
                .to_string()
        } else {
            format!("Synthetic work item {index}")
        },
        html_url: String::new(),
        state: state.to_string(),
        updated_at: format!("2026-10-{:02}T00:00:00Z", (index % 28) + 1),
        repository: HomeRepositoryRef {
            owner: "owner".to_string(),
            name: format!("repo-{}", index % 4),
        },
    }
}

fn home_outcome(items: Vec<HomeItem>, readiness: HomeReadiness) -> HomeSearchOutcome {
    let total_count = items.len() as u64;
    HomeSearchOutcome {
        items,
        total_count,
        incomplete_results: !readiness.is_complete(),
        readiness,
    }
}

fn feed_event(index: usize, event_type: FeedEventType, payload: FeedPayload) -> FeedEvent {
    FeedEvent {
        id: format!("event-{index}"),
        event_type,
        actor: FeedActor {
            id: 500 + index as u64,
            login: format!("actor-{index}"),
            display_login: None,
            avatar_url: None,
        },
        repository: FeedRepository {
            id: 700 + index as u64,
            name: "owner/repo".to_string(),
            url: String::new(),
        },
        created_at: Some("2026-10-28T00:00:00Z".to_string()),
        public: true,
        payload,
    }
}

fn activity(
    kind: FeedActivityKind,
    subject: FeedActor,
    event: FeedEvent,
    pull_request: Option<FeedActivityTarget>,
) -> FeedActivity {
    FeedActivity {
        kind,
        subject,
        pull_request,
        event,
        repository_description: None,
    }
}

fn starred_activity(index: usize) -> FeedActivity {
    let event = feed_event(
        index,
        FeedEventType::Watch,
        FeedPayload::Watch {
            action: "started".into(),
        },
    );
    let subject = event.actor.clone();
    activity(FeedActivityKind::RepositoryStarred, subject, event, None)
}

/// A merged contribution whose credited subject is the pull-request author.
fn merged_activity(index: usize) -> FeedActivity {
    let event = feed_event(
        index,
        FeedEventType::PullRequest,
        FeedPayload::PullRequest {
            action: "merged".into(),
            number: Some(40 + index as u64),
            title: None,
            html_url: None,
            state: None,
            merged: None,
            author: None,
            body: None,
        },
    );
    let subject = FeedActor {
        id: 900,
        login: "contributor".into(),
        display_login: None,
        avatar_url: None,
    };
    activity(
        FeedActivityKind::PullRequestMerged,
        subject,
        event,
        Some(FeedActivityTarget {
            number: Some(40 + index as u64),
            title: Some(format!("Contributor change {index}")),
            html_url: None,
            body_preview: Some(BodyPreview {
                excerpt: "A detailed pull request description that exceeds the preview.".into(),
                is_truncated: true,
            }),
        }),
    )
}

fn feed_outcome(activities: Vec<FeedActivity>, readiness: FeedReadiness) -> FeedOutcome {
    FeedOutcome {
        activities,
        etag: None,
        poll_interval_seconds: Some(60),
        next_page: None,
        readiness,
    }
}

#[component]
fn HomeViewHarness() -> impl IntoElement {
    let view = HomeView::new();
    let mut view = view;
    view.set_now(fixed_now());
    view.bind(binding(), 1);
    let sink: OnAction = std::rc::Rc::new(|action: HomeAction| {
        let label = match action {
            HomeAction::Refresh(request) => format!("refresh {:?}", request.column),
            HomeAction::Repository(repository) => format!("repository {}", repository.full_name()),
            HomeAction::Profile(profile) => format!("profile {}", profile.login),
            HomeAction::ActivateWorkItem(item) => format!("activate #{}", item.number),
            HomeAction::ActivateFeed(activity) => format!("feed {}", activity.id),
            HomeAction::LoadMoreFeed(_) => "load more".to_string(),
            HomeAction::OpenPullRequest { repository, number } => {
                format!("pull request {} #{number}", repository.full_name())
            }
        };
        ACTIONS.lock().expect("actions").push(label);
    });
    let items = vec![
        home_item(0, HomeItemKind::Issue, "open"),
        home_item(1, HomeItemKind::PullRequest, "open"),
        // A non-conforming closed row must never become visible.
        home_item(2, HomeItemKind::Issue, "closed"),
    ];
    let home = home_outcome(
        items,
        HomeReadiness::Partial {
            reasons: vec![HomePartialReason::PageFailed {
                message: "page 2 failed".into(),
            }],
        },
    );
    assert!(view.apply_home_snapshot(
        &binding(),
        1,
        SnapshotPresentation::historical_at(home, 1_760_000_000_000),
    ));
    let mut feed = feed_outcome(
        vec![starred_activity(0), merged_activity(1)],
        FeedReadiness::Partial {
            reasons: vec![FeedPartialReason::PageFailed {
                message: "page 2 failed".into(),
            }],
        },
    );
    feed.activities[0].repository_description = Some("Synthetic star description".into());
    assert!(view.apply_feed(&binding(), 1, feed));
    assert!(view.feed_append_failed(&binding(), 1, 2));
    let element = view.element(&sink, None);
    view_harness_shell(element)
}

fn view_harness_shell(element: AnyElement) -> AnyElement {
    view()
        .size_full()
        .bg(crate::ui::foundation::theme::theme().background)
        .child(element)
        .into_element()
}

fn mount() -> Headless {
    ACTIONS.lock().unwrap().clear();
    let root = Root::of::<HomeViewHarness>().tokens(crate::ui::foundation::theme::tokens());
    let mut app = Headless::start(EngineConfig::new(root)).expect("headless app starts");
    app.set_viewport(nsh_core::geometry::Size::new(
        nsh_core::geometry::px(1200.0),
        nsh_core::geometry::px(900.0),
    ))
    .unwrap();
    app
}

#[test]
fn open_only_rows_render_with_their_actions() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut app = mount();
    app.wait_for("id:home-item-row-0")
        .expect("the first open row renders");
    app.wait_for("id:home-item-row-1")
        .expect("the second open row renders");
    assert!(
        !app.screen().shows("id:home-item-row-2"),
        "a closed row must not be rendered, indexed or focusable"
    );
    // Activating the row title emits the typed work-item action; the row's own
    // links stay internal, so no external URL is ever produced.
    app.press("text:Synthetic work item 1")
        .expect("the row title is pressable");
    let actions = ACTIONS.lock().unwrap().clone();
    assert_eq!(actions, vec!["activate #101".to_string()]);
    assert!(
        !actions.iter().any(|action| action.contains("external")),
        "row activation must not open a browser"
    );
}

#[test]
fn feed_cards_keep_repository_description_and_read_more() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut app = mount();
    app.wait_for("Synthetic star description")
        .expect("the starred card shows its repository description");
    app.wait_for("id:feed-pr-read-more-1")
        .expect("a truncated body offers Read more");
    app.press("id:feed-0-repository")
        .expect("the Feed repository link is pressable");
    let actions = ACTIONS.lock().unwrap().clone();
    assert_eq!(actions, vec!["repository owner/repo".to_string()]);
    ACTIONS.lock().unwrap().clear();
    app.press("id:feed-pr-read-more-1")
        .expect("Read more is pressable");
    let actions = ACTIONS.lock().unwrap().clone();
    assert_eq!(
        actions,
        vec!["pull request owner/repo #41".to_string()],
        "Read more carries the internal pull-request target"
    );
    assert!(
        !app.screen().shows("external"),
        "Read more must not become a browser link"
    );
}

#[test]
fn partial_notices_saved_marks_and_append_footer_are_visible() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut app = mount();
    app.wait_for("id:home-partial")
        .expect("the Home partial notice renders");
    app.wait_for("id:feed-partial")
        .expect("the Feed partial notice renders");
    app.wait_for("id:home-historical")
        .expect("the saved snapshot is marked");
    assert!(
        app.screen().shows("Saved"),
        "a historical snapshot is not presented as a live result"
    );
    app.wait_for("id:feed-append-failed")
        .expect("a failed append keeps the list and offers a retry");
    assert!(
        !app.screen().shows("Load more"),
        "appending uses a compact warning, not a prose button"
    );
}

#[test]
fn summaries_and_errors_keep_their_accepted_wording() {
    let complete = home_outcome(
        vec![home_item(0, HomeItemKind::Issue, "open")],
        HomeReadiness::Complete,
    );
    assert!(home_summary(&complete).notice.is_none());
    let partial = home_outcome(
        vec![home_item(0, HomeItemKind::Issue, "open")],
        HomeReadiness::Partial {
            reasons: vec![HomePartialReason::PageFailed {
                message: "page 2 failed".into(),
            }],
        },
    );
    let notice = home_summary(&partial).notice.expect("partial notice");
    assert!(notice.starts_with("Partial result ·"));
    assert!(notice.contains("not a complete history or unread list"));
    assert_eq!(
        super::presentation::error_message(&HomeError::GitHub(GitHubError::Unauthorized)),
        "Authentication failed. The host should verify or replace this account's authorization."
    );
    let feed = feed_outcome(
        Vec::new(),
        FeedReadiness::Partial {
            reasons: vec![FeedPartialReason::AuthorUnresolved { count: 2 }],
        },
    );
    let notice = feed_summary(&feed).notice.expect("feed partial notice");
    assert!(notice.contains("The author of 2 merged pull request(s) could not be confirmed"));
}

#[test]
fn relative_time_and_credited_subject_match_the_accepted_copy() {
    // The single relative-time formatter is shared with Notifications.
    let now = UNIX_EPOCH + Duration::from_secs(1_793_188_800);
    assert_eq!(
        super::rows::relative_label(Some("2026-10-28T00:00:00Z"), now),
        crate::ui::relative_time::relative(Some("2026-10-28T00:00:00Z"), now)
    );
    assert_eq!(
        super::rows::relative_label(Some("not-a-time"), now),
        "Time unavailable"
    );
    let merged = merged_activity(0);
    let description = event_description(&merged);
    assert!(description.starts_with("contributor contributed to owner/repo"));
    assert!(
        !description.contains("actor-0"),
        "the merge credits the author, never the merging actor"
    );
    assert_eq!(
        feed_repository(&merged.event).full_name(),
        "owner/repo".to_string()
    );
}
