use super::*;
use crate::ui::fixture::{FixtureRuntime, Scene};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, size};
use std::cell::RefCell;

fn complete(controller: &mut Controller, request: &Request, runtime: &tokio::runtime::Runtime) {
    let result = runtime.block_on(request.execute(&WorkItemLimits::default()));
    assert!(controller.finish(request, result));
}
#[test]
fn paging_dedup_retry_refresh_and_filter_generation_share_real_adapters() {
    let fixture = FixtureRuntime::new(Scene::AppendFailure);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut controller = Controller::new(
        fixture.accounts.repo_service().work_item_session().unwrap(),
        WorkItemKind::Issue,
    );
    let initial = controller.begin(RequestMode::Replace).unwrap();
    assert!(controller.begin(RequestMode::Replace).is_none());
    complete(&mut controller, &initial, &runtime);
    assert_eq!(controller.rows.len(), 50);
    let append = controller.begin(RequestMode::Append).unwrap();
    complete(&mut controller, &append, &runtime);
    assert_eq!(controller.rows.len(), 50);
    assert!(controller.error.is_some());
    let retry = controller.begin(RequestMode::Append).unwrap();
    complete(&mut controller, &retry, &runtime);
    assert_eq!(controller.rows.len(), 99);
    let old = controller.begin(RequestMode::Append).unwrap();
    let refresh = controller.begin(RequestMode::Replace).unwrap();
    assert!(!controller.finish(
        &old,
        Err(crate::domain::error::GitHubError::Unauthorized.into())
    ));
    complete(&mut controller, &refresh, &runtime);
    assert_eq!(controller.rows.len(), 50);
    let old = controller.begin(RequestMode::Append).unwrap();
    let old_success = runtime.block_on(old.execute(&WorkItemLimits::default()));
    let changed = controller
        .filter(
            WorkItemKind::PullRequest,
            Relation::Created,
            StateFilter::Closed,
        )
        .unwrap();
    assert!(controller.rows.is_empty());
    assert!(!controller.finish(&old, old_success));
    assert!(!controller.finish(
        &old,
        Err(crate::domain::error::GitHubError::Unauthorized.into())
    ));
    complete(&mut controller, &changed, &runtime);
    assert!(
        controller
            .rows
            .iter()
            .all(|item| item.kind == WorkItemKind::PullRequest)
    );
    assert!(
        controller
            .rows
            .iter()
            .any(|item| item.state == ItemState::Merged)
    );
    assert!(
        controller
            .rows
            .iter()
            .any(|item| item.state == ItemState::Closed)
    );
}
#[test]
fn initial_failure_empty_partial_and_repeatable_fixture() {
    let fixture = FixtureRuntime::new(Scene::InitialFailure);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut controller = Controller::new(
        fixture.accounts.repo_service().work_item_session().unwrap(),
        WorkItemKind::Issue,
    );
    let initial = controller.begin(RequestMode::Replace).unwrap();
    complete(&mut controller, &initial, &runtime);
    assert!(!controller.loaded);
    assert!(!controller.is_empty());
    assert!(controller.error.is_some());
    let retry = controller.begin(RequestMode::Replace).unwrap();
    complete(&mut controller, &retry, &runtime);
    assert_eq!(controller.rows.len(), 50);
    for scene in [Scene::Empty, Scene::Partial, Scene::Mixed, Scene::Mixed] {
        fixture.transport.set_scene(scene);
        let request = controller.begin(RequestMode::Replace).unwrap();
        complete(&mut controller, &request, &runtime);
        assert_eq!(controller.is_empty(), scene == Scene::Empty);
        assert_eq!(!controller.partial.is_empty(), scene == Scene::Partial);
    }
}
#[test]
fn thousand_result_limit_requires_explicit_page_requests_and_never_exceeds_bound() {
    let fixture = FixtureRuntime::new(Scene::Long);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut controller = Controller::new(
        fixture.accounts.repo_service().work_item_session().unwrap(),
        WorkItemKind::Issue,
    );
    let initial = controller.begin(RequestMode::Replace).unwrap();
    complete(&mut controller, &initial, &runtime);
    assert_eq!(
        fixture.transport.requests().len(),
        2,
        "login plus exactly one page"
    );
    let mut pages = 1;
    while controller.next.is_some() {
        let request = controller.begin(RequestMode::Append).unwrap();
        complete(&mut controller, &request, &runtime);
        pages += 1;
    }
    assert_eq!(pages, 20);
    assert!(controller.rows.len() <= 1000);
    assert!(controller.partial.contains(&PartialReason::ResultLimit));
    assert!(controller.begin(RequestMode::Append).is_none());
}
#[test]
fn refresh_failure_retains_rows_cursor_and_retries_replace() {
    let fixture = FixtureRuntime::new(Scene::Mixed);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut controller = Controller::new(
        fixture.accounts.repo_service().work_item_session().unwrap(),
        WorkItemKind::Issue,
    );
    let request = controller.begin(RequestMode::Replace).unwrap();
    complete(&mut controller, &request, &runtime);
    let original = controller.rows.clone();
    let cursor = controller.next.clone();
    fixture.transport.set_scene(Scene::InitialFailure);
    let request = controller.begin(RequestMode::Replace).unwrap();
    complete(&mut controller, &request, &runtime);
    assert_eq!(controller.rows, original);
    assert_eq!(controller.next, cursor);
    assert_eq!(controller.failed_mode, Some(RequestMode::Replace));
    let retry = controller.begin(controller.failed_mode.unwrap()).unwrap();
    complete(&mut controller, &retry, &runtime);
    assert_eq!(controller.rows, original);
    assert!(controller.error.is_none());
}
#[test]
fn account_replacement_rejects_old_success_and_old_error_and_rebind_clears_rows() {
    let fixture = FixtureRuntime::new(Scene::Mixed);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut controller = Controller::new(
        fixture.accounts.repo_service().work_item_session().unwrap(),
        WorkItemKind::Issue,
    );
    let old = controller.begin(RequestMode::Replace).unwrap();
    let result = runtime.block_on(old.execute(&WorkItemLimits::default()));
    runtime
        .block_on(
            fixture
                .accounts
                .login(crate::auth::token::Pat::from_str("synthetic-new-token")),
        )
        .unwrap();
    assert!(!controller.finish(&old, result));
    assert!(!controller.finish(
        &old,
        Err(crate::domain::error::GitHubError::Unauthorized.into())
    ));
    controller.rebind(fixture.accounts.repo_service().work_item_session().unwrap());
    assert!(controller.rows.is_empty());
    let request = controller.begin(RequestMode::Replace).unwrap();
    complete(&mut controller, &request, &runtime);
    assert_eq!(controller.rows.len(), 50);
}
#[test]
fn delayed_old_filter_response_cannot_replace_created_rows() {
    let fixture = FixtureRuntime::new(Scene::Delayed);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut controller = Controller::new(
        fixture.accounts.repo_service().work_item_session().unwrap(),
        WorkItemKind::Issue,
    );
    runtime.block_on(async {
        let old = controller.begin(RequestMode::Replace).unwrap();
        let limits = WorkItemLimits::default();
        let old_future = old.execute(&limits);
        tokio::pin!(old_future);
        tokio::select! {
            _ = &mut old_future => panic!("expected delayed fixture"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
        let created = controller
            .filter(WorkItemKind::Issue, Relation::Created, StateFilter::Open)
            .unwrap();
        let result = created.execute(&WorkItemLimits::default()).await;
        assert!(controller.finish(&created, result));
        assert!(!controller.rows.is_empty());
        assert!(controller.rows.iter().all(|row| row.id >= 15000));
        assert!(!controller.finish(&old, old_future.await));
        assert!(controller.rows.iter().all(|row| row.id >= 15000));
    });
}
fn open(
    cx: &mut TestAppContext,
    scene: Scene,
) -> (
    FixtureRuntime,
    Entity<WorkItemsView>,
    &mut VisualTestContext,
) {
    let fixture = FixtureRuntime::new(scene);
    cx.update(ely_core::init_for_tests);
    let repos = fixture.accounts.repo_service();
    let executor = fixture.executor.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut controller = Controller::new(repos.work_item_session().unwrap(), WorkItemKind::Issue);
    let request = controller.begin(RequestMode::Replace).unwrap();
    let result = runtime.block_on(request.execute(&WorkItemLimits::default()));
    assert!(controller.finish(&request, result));
    let accounts = fixture.accounts.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        WorkItemsView::with_controller(controller, accounts.repo_service(), executor, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    (fixture, view, cx)
}
fn load(view: &Entity<WorkItemsView>, cx: &mut VisualTestContext, mode: RequestMode) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let request = view
        .update(cx, |view, _| view.controller.begin(mode))
        .expect("requestable");
    let result = runtime.block_on(request.execute(&WorkItemLimits::default()));
    view.update(cx, |view, cx| {
        assert!(view.apply_result(&request, result, cx))
    });
    cx.run_until_parked();
}
#[gpui::test]
fn real_rows_repository_callback_and_unconnected_title(cx: &mut TestAppContext) {
    let (_fixture, view, cx) = open(cx, Scene::Mixed);
    cx.simulate_resize(size(px(900.0), px(650.0)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("work-item-10000").is_some());
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = actions.clone();
    view.update(cx, |view, _| {
        view.on_repository(Rc::new(move |navigation, _, _| {
            sink.borrow_mut().push(navigation.repository)
        }))
    });
    let title = cx.debug_bounds("work-title-10000").unwrap().center();
    cx.simulate_click(title, Modifiers::none());
    assert!(actions.borrow().is_empty());
    let repo = cx.debug_bounds("work-repo-10000").unwrap().center();
    cx.simulate_click(repo, Modifiers::none());
    assert_eq!(actions.borrow().len(), 1);
    assert_eq!(actions.borrow()[0].owner, "acme");
    assert_eq!(
        actions.borrow()[0].name,
        "a-very-long-repository-name-for-narrow-layouts"
    );
    // Keyboard activation is the same explicit repository target.
    view.update(cx, |view, cx| {
        view.selected = Some(10000);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(
        actions.borrow().len(),
        1,
        "selection alone does not navigate or open details"
    );
}
#[gpui::test]
fn append_then_keyboard_last_row_and_filter_render_real_identity(cx: &mut TestAppContext) {
    let (_fixture, view, cx) = open(cx, Scene::Mixed);
    load(&view, cx, RequestMode::Append);
    assert_eq!(view.read_with(cx, |view, _| view.controller.rows.len()), 99);
    cx.update(|window, cx| view.update(cx, |view, cx| view.focus.focus(window, cx)));
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    assert!(cx.debug_bounds("work-item-10099").is_some());
    view.update(cx, |view, cx| {
        let request = view
            .controller
            .filter(
                WorkItemKind::PullRequest,
                Relation::Created,
                StateFilter::Closed,
            )
            .unwrap();
        view.reset_list();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(request.execute(&WorkItemLimits::default()));
        assert!(view.apply_result(&request, result, cx));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("work-item-25000").is_some());
    assert!(cx.debug_bounds("work-item-10000").is_none());
}
#[gpui::test]
fn bounded_virtual_list_keyboard_end_and_no_render_requests(cx: &mut TestAppContext) {
    let (fixture, view, cx) = open(cx, Scene::Mixed);
    cx.simulate_resize(size(px(420.0), px(450.0)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("work-item-10000").is_some());
    assert!(cx.debug_bounds("work-item-10049").is_none());
    let count = fixture.transport.requests().len();
    for _ in 0..4 {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    assert_eq!(fixture.transport.requests().len(), count);
    cx.update(|window, cx| view.update(cx, |view, cx| view.focus.focus(window, cx)));
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    assert!(cx.debug_bounds("work-item-10049").is_some());
    let bounds = cx.debug_bounds("work-rows").unwrap();
    assert!(bounds.size.width <= px(420.0));
    assert!(bounds.size.height < px(450.0));
    let title = cx.debug_bounds("work-title-10049").unwrap();
    let row = cx.debug_bounds("work-item-10049").unwrap();
    assert!(
        title.right() <= bounds.right(),
        "title {title:?} outside rows {bounds:?}"
    );
    assert!(
        row.right() <= bounds.right(),
        "row {row:?} outside rows {bounds:?}"
    );
}
