use super::super::tests::{frames, normal, settle};
use super::*;
use gpui::{Modifiers, TestAppContext};
use std::{cell::RefCell, rc::Rc};

#[gpui::test]
fn repository_issue_open_preserves_filter_pages_and_scroll_on_workspace_suspend(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);
    let actions = Rc::new(RefCell::new(Vec::new()));
    page.update(cx, |page, cx| {
        let actions = actions.clone();
        page.on_action(Rc::new(move |action, _, _| {
            actions.borrow_mut().push(action)
        }));
        page.select_tab(RepositoryTab::Issues, cx);
    });
    settle(&page, cx, |page| {
        page.issues.rows.len() == 30 && !page.issues.busy()
    });
    page.update(cx, |page, cx| page.request_issues(false, cx));
    settle(&page, cx, |page| {
        page.issues.rows.len() == 31 && !page.issues.busy()
    });
    assert_eq!(fixture.transport.count("/issues?"), 3);
    assert!(cx.debug_bounds("repository-issue-70").is_none());
    page.update_in(cx, |page, window, cx| page.issues.focus.focus(window, cx));
    cx.simulate_keystrokes("end");
    frames(cx);
    let offset = page.read_with(cx, |page, _| {
        page.issues.list_scroll.0.borrow().base_handle.offset()
    });
    assert!(offset.y < gpui::px(0.0));
    let last = cx
        .debug_bounds("repository-issue-title-69")
        .expect("last issue is reachable");
    cx.simulate_click(last.center(), Modifiers::none());
    assert!(
        matches!(&actions.borrow()[..], [PageAction::OpenIssue { locator, number: 69, .. }] if locator.name() == "ledger")
    );
    page.update(cx, |page, cx| page.suspend(cx));
    frames(cx);
    page.read_with(cx, |page, _| {
        assert_eq!(page.issues.filter, IssueFilter::Open);
        assert_eq!(page.issues.rows.len(), 31);
        assert_eq!(
            page.issues.list_scroll.0.borrow().base_handle.offset(),
            offset
        );
    });
    cx.simulate_keystrokes("enter");
    assert_eq!(actions.borrow().len(), 2);
}

#[gpui::test]
fn repository_issue_cache_paints_while_refresh_is_in_flight_and_survives_failure(
    cx: &mut TestAppContext,
) {
    let (_fixture, page, cx) = normal(cx);
    let (disk, online) = page.update(cx, |page, cx| {
        page.select_tab(RepositoryTab::Issues, cx);
        let request = page.issues.request.as_mut().unwrap();
        let (disk, cache) = mpsc::channel();
        let (online, network) = mpsc::channel();
        request.cache = Some(cache);
        request.network = network;
        (disk, online)
    });
    disk.send(Some(IssuePage {
        rows: vec![IssueRow {
            number: 42,
            title: "Cached issue".into(),
            updated_at: "2026-11-01T12:00:00Z".into(),
        }],
        next: None,
    }))
    .unwrap();
    page.update(cx, |page, cx| page.drain_issues(cx));
    frames(cx);
    assert!(cx.debug_bounds("repository-issue-title-42").is_some());
    assert!(page.read_with(cx, |page, _| page.issues.busy() && page.issues.historical));
    online
        .send(Err(PageCacheError::Read(
            GitHubError::Timeout { seconds: 1 }.into(),
        )))
        .unwrap();
    page.update(cx, |page, cx| page.drain_issues(cx));
    frames(cx);
    assert!(cx.debug_bounds("repository-issue-title-42").is_some());
    assert!(page.read_with(cx, |page, _| !page.issues.busy()
        && page.issues.failure.is_some()));
    page.update(cx, |page, cx| page.request_issues(true, cx));
    settle(&page, cx, |page| {
        page.issues.rows.len() == 30 && !page.issues.busy()
    });
    assert!(page.read_with(cx, |page, _| !page.issues.historical
        && page.issues.failure.is_none()));
}

#[gpui::test]
fn repository_issue_filter_and_account_changes_retire_previous_delivery(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    let open = page.update(cx, |page, cx| {
        page.select_tab(RepositoryTab::Issues, cx);
        let (send, network) = mpsc::channel();
        page.issues.request.as_mut().unwrap().network = network;
        page.filter_issues(IssueFilter::Closed, cx);
        send
    });
    assert!(
        open.send(Ok(IssuePage {
            rows: vec![],
            next: None
        }))
        .is_err()
    );
    settle(&page, cx, |page| {
        page.issues.rows.first().is_some_and(|row| row.number == 42)
    });
    let closed = page.update(cx, |page, cx| {
        page.request_issues(true, cx);
        let (send, network) = mpsc::channel();
        page.issues.request.as_mut().unwrap().network = network;
        send
    });
    fixture.login_as(2, "other-user");
    page.update_in(cx, |page, window, cx| page.drain(window, cx));
    assert!(
        closed
            .send(Ok(IssuePage {
                rows: vec![],
                next: None
            }))
            .is_err()
    );
    page.read_with(cx, |page, _| assert!(page.issues.rows.is_empty()));
}
