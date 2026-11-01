use super::*;
use crate::auth::{memory::InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::github::client::{GitHubClient, GitHubConfig};
use gpui::{Modifiers, TestAppContext, size};
use std::cell::RefCell;
#[path = "../../bin_support/notifications_fixture.rs"]
mod fixture;

#[gpui::test]
fn bounded_narrow_keyboard_and_exact_repository_callback(cx: &mut TestAppContext) {
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
        assert!(bounds.size.width <= dimensions.width);
        assert!(bounds.size.height < dimensions.height);
        assert!(row.right() <= bounds.right());
        assert!(title.right() <= row.right());
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
