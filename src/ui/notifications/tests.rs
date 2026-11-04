use super::*;
use crate::auth::{memory::InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::github::client::{GitHubClient, GitHubConfig};
use gpui::{Modifiers, TestAppContext, size};
use std::cell::RefCell;
use std::time::Duration;

#[path = "../../bin_support/notifications_fixture.rs"]
mod fixture;

const FIXED_NOW_SECS: u64 = 1793534400;

fn setup(
    cx: &mut TestAppContext,
) -> (
    crate::data::notifications::Notifications,
    Arc<BackgroundExecutor>,
    Arc<fixture::Fixture>,
) {
    cx.update(ely_core::init_for_tests);
    let executor = Arc::new(BackgroundExecutor::new(2));
    let fixture = Arc::new(fixture::Fixture::new(fixture::Scene::Mixed));
    let accounts = AccountService::new(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(GitHubClient::new(GitHubConfig::default(), fixture.clone())),
        executor.clone(),
    );
    let controller = executor.raw_handle().block_on(async {
        accounts.login(Pat::from_str("synthetic")).await.unwrap();
        let controller = accounts.repo_service().notifications().unwrap();
        controller
            .execute(controller.begin(RequestKind::First).unwrap().unwrap())
            .await;
        controller
            .execute(controller.begin(RequestKind::More).unwrap().unwrap())
            .await;
        controller
    });
    (controller, executor, fixture)
}

#[gpui::test]
fn bounded_narrow_keyboard_and_exact_repository_callback(cx: &mut TestAppContext) {
    let (controller, executor, fixture) = setup(cx);
    let expected = controller.snapshot().items[0].repository.clone();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let sink = captured.clone();
    let on_repository: OnRepository = Rc::new(move |action, _, _| sink.borrow_mut().push(action));
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = NotificationsView::new(
            controller.clone(),
            executor.clone(),
            on_repository,
            window,
            cx,
        );
        view.sync(cx);
        view
    });
    cx.update(|window, _| window.activate_window());
    for dimensions in [size(px(960.0), px(740.0)), size(px(360.0), px(440.0))] {
        cx.simulate_resize(dimensions);
        cx.run_until_parked();
        let bounds = cx.debug_bounds("notifications-rows").unwrap();
        let row = cx.debug_bounds("notification-row-0").unwrap();
        let title = cx.debug_bounds("notification-title-0").unwrap();
        let time = cx.debug_bounds("notification-time-0").unwrap();
        assert!(bounds.size.width <= dimensions.width);
        assert!(bounds.size.height < dimensions.height);
        assert!(row.right() <= bounds.right());
        assert!(title.right() <= row.right());
        assert!(time.right() <= row.right());
    }
    cx.update(|window, cx| view.update(cx, |view, cx| view.focus.focus(window, cx)));
    cx.simulate_keystrokes("down enter");
    assert_eq!(captured.borrow().len(), 1);
    assert_eq!(captured.borrow()[0].repository, expected);
    assert_eq!(captured.borrow()[0].context, *controller.context());
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.selected), Some(13));
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    let link = cx
        .debug_bounds("notification-repository-0")
        .unwrap()
        .center();
    cx.simulate_click(link, Modifiers::none());
    assert_eq!(captured.borrow().len(), 2);
    assert_eq!(captured.borrow()[1].repository, expected);
    // Rendering, keyboard movement and repository activation never fetch a
    // subject, mark a thread read or refresh the inbox.
    assert_eq!(fixture.requests().len(), 3);
}

#[gpui::test]
fn empty_failure_and_partial_render_distinctly(cx: &mut TestAppContext) {
    cx.update(ely_core::init_for_tests);
    let executor = Arc::new(BackgroundExecutor::new(2));
    let fixture = Arc::new(fixture::Fixture::new(fixture::Scene::Empty));
    let accounts = AccountService::new(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(GitHubClient::new(GitHubConfig::default(), fixture.clone())),
        executor.clone(),
    );
    let controller = executor.raw_handle().block_on(async {
        accounts.login(Pat::from_str("synthetic")).await.unwrap();
        let controller = accounts.repo_service().notifications().unwrap();
        controller
            .execute(controller.begin(RequestKind::First).unwrap().unwrap())
            .await;
        controller
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        NotificationsView::new(controller, executor, Rc::new(|_, _, _| {}), window, cx)
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.snapshot.is_empty()));
    view.update(cx, |view, cx| {
        view.snapshot.error = Some(crate::domain::error::GitHubError::Unauthorized.into());
        cx.notify();
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |view, _| view.snapshot.is_empty()));
    view.update(cx, |view, cx| {
        view.snapshot.error = None;
        view.snapshot.partial = vec![crate::domain::notification::NotificationPartial::Sso {
            organizations: vec!["123".into()],
        }];
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("notifications-partial").is_some());
    assert!(!view.read_with(cx, |view, _| view.snapshot.is_empty()));
}

#[gpui::test]
fn row_geometry_read_state_and_relative_time(cx: &mut TestAppContext) {
    let (controller, executor, _) = setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = NotificationsView::new(
            controller.clone(),
            executor.clone(),
            Rc::new(|_, _, _| {}),
            window,
            cx,
        );
        view.set_now(
            Some(std::time::UNIX_EPOCH + Duration::from_secs(FIXED_NOW_SECS)),
            cx,
        );
        view
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    // Icon centre sits on the row's vertical centre, not top-compensated.
    let row0 = cx.debug_bounds("notification-row-0").unwrap();
    let icon0 = cx.debug_bounds("notification-icon-0").unwrap();
    let center = |b: gpui::Bounds<gpui::Pixels>| b.origin.y + b.size.height / 2.0;
    assert!((center(row0) - center(icon0)).abs() < gpui::px(1.0));

    // Owner/repo line and title share a left origin (the same gutter).
    let repo = cx.debug_bounds("notification-repository-0").unwrap();
    let title = cx.debug_bounds("notification-title-0").unwrap();
    assert!((repo.origin.x - title.origin.x).abs() < gpui::px(1.0));

    // The time sits on the repository metadata line, right of the repo link,
    // not on the title line.
    let time0 = cx.debug_bounds("notification-time-0").unwrap();
    assert!(time0.origin.y < title.origin.y);
    assert!(repo.right() <= time0.origin.x);

    // Read vs unread differ by background + dot, not by any "Read"/"Unread"
    // label. Find one unread and one read row deterministically.
    let snapshot = view.read_with(cx, |view, _| view.snapshot.clone());
    let unread_idx = snapshot.items.iter().position(|item| item.unread).unwrap();
    let read_idx = snapshot.items.iter().position(|item| !item.unread).unwrap();
    assert_ne!(unread_idx, read_idx);
    let unread_row = cx.debug_bounds("notification-row-0").unwrap();
    let read_row = cx.debug_bounds("notification-row-1").unwrap();
    assert_ne!(unread_row, read_row);
    // The unread dot is a 6px circle on unread rows.
    let dot = cx.debug_bounds("notification-dot-0").unwrap();
    assert!(dot.size.width > gpui::px(0.0));

    // A minute/hour/day spread against the fixed clock.
    let minute = cx.debug_bounds("notification-time-7").unwrap();
    let hour = cx.debug_bounds("notification-time-5").unwrap();
    let day = cx.debug_bounds("notification-time-2").unwrap();
    assert!(!minute.is_empty() && !hour.is_empty() && !day.is_empty());
}

#[gpui::test]
fn cooldown_wake_is_rearmed_after_deactivation(cx: &mut TestAppContext) {
    let (controller, executor, _) = setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        NotificationsView::new(controller, executor, Rc::new(|_, _, _| {}), window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    // A server cooldown is in force; the wake-up task exists.
    assert!(view.read_with(cx, |view, _| view.cooldown_wake.is_some()));
    // Deactivation cancels activity, which also drops that one-shot task.
    view.update(cx, |view, cx| view.set_active(false, cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.cooldown_wake.is_none()));
    // Reactivation must re-arm the remaining cooldown even for an empty inbox,
    // otherwise Refresh would stay disabled with nothing to wake it.
    view.update(cx, |view, cx| view.set_active(true, cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.cooldown_wake.is_some()));
    // A fixed clock starts no elapsed-time task.
    view.update(cx, |view, cx| {
        view.set_now(
            Some(std::time::UNIX_EPOCH + Duration::from_secs(FIXED_NOW_SECS)),
            cx,
        )
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.time_task.is_none()));
}

/// Leaving the page before its first response cancels the request without setting `loaded`, so
/// returning to the retained tab must retry that read; a loaded, in-flight or failed snapshot must
/// not be retried. The rule is a pure function of the published snapshot, so it is asserted
/// directly: a live retry here would leave a network read in flight when the GUI test scheduler
/// tears the window down.
#[gpui::test]
fn reactivating_an_interrupted_cold_inbox_resumes_the_first_read(cx: &mut TestAppContext) {
    let (controller, executor, _) = setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view =
            NotificationsView::new(controller, executor, Rc::new(|_, _, _| {}), window, cx);
        // Reproduce the interrupted first read: the request is cancelled, nothing is loaded.
        view.snapshot = InboxSnapshot::default();
        view
    });
    cx.run_until_parked();

    view.update(cx, |view, _| {
        assert!(
            view.resume_needed(),
            "an idle, never-loaded inbox must resume its initial read"
        );
        assert_eq!(view.resume_kind(), RequestKind::First);
    });
    view.update(cx, |view, _| {
        view.snapshot.loaded = true;
        assert!(!view.resume_needed(), "a loaded inbox is not re-read");
        assert_eq!(view.resume_kind(), RequestKind::Refresh);
        view.snapshot.loaded = false;
    });
    view.update(cx, |view, _| {
        view.snapshot.busy = Some(RequestKind::More);
        assert!(
            !view.resume_needed(),
            "an in-flight request is not duplicated"
        );
        view.snapshot.busy = None;
    });
    view.update(cx, |view, _| {
        view.snapshot.error = Some(crate::domain::error::GitHubError::Unauthorized.into());
        assert!(
            !view.resume_needed(),
            "a real failure keeps its retry control"
        );
        view.snapshot.error = None;
    });
}

/// A loaded snapshot is reused across tab switches: `set_active` never re-reads it.
#[gpui::test]
fn reactivating_a_loaded_inbox_does_not_re_read(cx: &mut TestAppContext) {
    let (controller, executor, fixture) = setup(cx);
    let reads = || {
        fixture
            .requests()
            .iter()
            .filter(|request| request.method == crate::network::transport::HttpMethod::Get)
            .count()
    };
    let before = reads();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view =
            NotificationsView::new(controller, executor, Rc::new(|_, _, _| {}), window, cx);
        view.sync(cx);
        view
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.snapshot.loaded));
    view.update(cx, |view, cx| {
        view.set_active(false, cx);
        view.set_active(true, cx);
    });
    cx.run_until_parked();
    assert_eq!(reads(), before, "a loaded inbox is not re-read");
}

#[gpui::test]
fn three_actions_stay_inside_the_row_and_do_not_navigate(cx: &mut TestAppContext) {
    let (controller, executor, fixture) = setup(cx);
    let captured = Rc::new(RefCell::new(0usize));
    let sink = captured.clone();
    let on_repository: OnRepository = Rc::new(move |_, _, _| *sink.borrow_mut() += 1);
    let (view, cx) = cx.add_window_view(|window, cx| {
        NotificationsView::new(
            controller.clone(),
            executor.clone(),
            on_repository,
            window,
            cx,
        )
    });
    cx.update(|window, _| window.activate_window());
    for dimensions in [size(px(960.0), px(740.0)), size(px(420.0), px(620.0))] {
        cx.simulate_resize(dimensions);
        cx.run_until_parked();
        let row = cx.debug_bounds("notification-row-0").unwrap();
        let actions = cx.debug_bounds("notification-actions-0").unwrap();
        let title = cx.debug_bounds("notification-title-0").unwrap();
        let done = cx.debug_bounds("notification-action-done-0").unwrap();
        let unsub = cx
            .debug_bounds("notification-action-unsubscribe-0")
            .unwrap();
        let save = cx.debug_bounds("notification-action-save-0").unwrap();
        assert!(actions.right() <= row.right());
        assert!(
            title.right() <= actions.left(),
            "title must shrink before the actions"
        );
        assert!(done.left() < unsub.left() && unsub.left() < save.left());
        assert!(save.right() <= row.right());
    }
    let before = fixture
        .requests()
        .into_iter()
        .filter(|request| request.method != crate::network::transport::HttpMethod::Get)
        .count();
    // The unavailable Save button must never send a request or fake a local save.
    let save = cx
        .debug_bounds("notification-action-save-0")
        .unwrap()
        .center();
    cx.simulate_click(save, Modifiers::none());
    cx.run_until_parked();
    let after = fixture
        .requests()
        .into_iter()
        .filter(|request| request.method != crate::network::transport::HttpMethod::Get)
        .count();
    assert_eq!(before, after, "Save for later stays a no-op");
    // Done removes exactly one row and never triggers repository navigation.
    let done = cx
        .debug_bounds("notification-action-done-0")
        .unwrap()
        .center();
    cx.simulate_click(done, Modifiers::none());
    // The write runs on the shared Tokio executor; drain both that task and the
    // GPUI oneshot callback before asserting.
    for _ in 0..100 {
        cx.run_until_parked();
        if view.read_with(cx, |view, _| view.snapshot.items.len()) == 13 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // setup() loads both pages (14 rows); Done removes exactly this one.
    assert_eq!(view.read_with(cx, |view, _| view.snapshot.items.len()), 13);
    assert_eq!(*captured.borrow(), 0);
}

#[gpui::test]
fn action_buttons_unlock_when_the_server_cooldown_expires(cx: &mut TestAppContext) {
    let (controller, executor, _) = setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        NotificationsView::new(controller, executor, Rc::new(|_, _, _| {}), window, cx)
    });
    cx.run_until_parked();
    // No cooldown: actions are available.
    assert!(view.read_with(cx, |view, _| {
        view.action_available("9000", ThreadAction::Done)
    }));
    // A rate-limit deadline in the near future disables them...
    view.update(cx, |view, cx| {
        view.action_cooldown_until = Some(Instant::now() + Duration::from_millis(120));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |view, _| {
        view.action_available("9000", ThreadAction::Done)
    }));
    // ...and once it elapses the same recomputation re-enables them without a
    // request, matching the wake-up repaint.
    std::thread::sleep(Duration::from_millis(200));
    assert!(view.read_with(cx, |view, _| {
        view.action_available("9000", ThreadAction::Done)
    }));
}

#[gpui::test]
fn an_expired_wake_task_arms_the_remaining_deadline(cx: &mut TestAppContext) {
    let (controller, executor, _) = setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        NotificationsView::new(controller, executor, Rc::new(|_, _, _| {}), window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    // An action deadline expires first; the later polling deadline still needs
    // a wake-up after the first one-shot task has completed.
    view.update(cx, |view, cx| {
        view.action_cooldown_until = Some(Instant::now() + Duration::from_millis(50));
        view.cooldown_until = Some(Instant::now() + Duration::from_secs(120));
        view.cooldown_wake = None;
        view.sync_cooldown_wake(cx);
    });
    assert!(view.read_with(cx, |view, _| view.cooldown_wake.is_some()));
    std::thread::sleep(Duration::from_millis(180));
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |view, _| view.cooldown_wake.is_some()),
        "the wake task must clear its slot and re-arm the remaining deadline"
    );
}

#[gpui::test]
fn releasing_the_view_cancels_its_thread_action(cx: &mut TestAppContext) {
    let (controller, executor, _) = setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let view = NotificationsView::new(
            controller.clone(),
            executor,
            Rc::new(|_, _, _| {}),
            window,
            cx,
        );
        let _ = controller
            .begin_thread_action("9000", ThreadAction::Done)
            .unwrap();
        view
    });
    let action = controller
        .begin_thread_action("9001", ThreadAction::Done)
        .unwrap()
        .unwrap();
    // Remove the rendered view before dropping the last UI entity. on_release
    // must cancel even if the weak visible-lifetime callback cannot be upgraded.
    let weak = view.downgrade();
    cx.replace_root_view(|_, _| gpui::EmptyView);
    drop(view);
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "the view must be released");
    assert!(action.cancel.is_cancelled());
}
