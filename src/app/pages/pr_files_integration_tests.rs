use super::*;

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    wait_for_bounds(cx, selector);
    let position = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(position, gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn files_changed_shell_tabs_load_lazily_and_keep_diffs_scoped_to_the_pr(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (shell, accounts, cx) = open_shell(cx, transport.clone());
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(760.)));
    login(&accounts, cx);
    wait_until(&accounts, cx, |accounts| {
        matches!(
            &accounts.snapshot().repositories.preview,
            crate::ui::accounts::RepositoryPreview::Ready(_)
        )
    });
    let open_pr = |cx: &mut VisualTestContext, number| {
        cx.update_window_entity(&shell, |shell, window, cx| {
            shell.navigate(
                Route::PullRequest {
                    namespace: "alice".into(),
                    name: "demo".into(),
                    number,
                },
                window,
                cx,
            );
        });
    };
    open_pr(cx, 5);
    wait_for_bounds(cx, "pr-conversation-body");
    assert_eq!(transport.seen("/pulls/5/files?"), 0);
    click(cx, "pr-tab-files-changed");
    wait_for_bounds(cx, "pr-diff-row-2");
    assert_eq!(transport.seen("/pulls/5/files?"), 1);
    assert!(cx.debug_bounds("pr-conversation-body").is_none());

    // The PNG fixture omits patch, as does the rename without text edits.
    for file in ["pr-file-3", "pr-file-5"] {
        click(cx, file);
        wait_for_bounds(cx, "pr-text-diff-unavailable");
        assert!(cx.debug_bounds("pr-diff-row-2").is_none());
    }
    open_pr(cx, 21);
    wait_for_bounds(cx, "pr-conversation-body");
    assert_eq!(transport.seen("/pulls/21/files?"), 0);
    assert!(cx.debug_bounds("pr-file-3").is_none());
    click(cx, "pr-tab-files-changed");
    wait_for_bounds(cx, "pr-files-empty");
    assert_eq!(transport.seen("/pulls/21/files?"), 1);
    assert!(cx.debug_bounds("pr-text-diff-unavailable").is_none());
    assert!(cx.debug_bounds("pr-diff-row-2").is_none());

    open_pr(cx, 5);
    wait_for_bounds(cx, "pr-text-diff-unavailable");
    click(cx, "pr-tab-conversation");
    wait_for_bounds(cx, "pr-conversation-body");
    assert_eq!(transport.seen("/pulls/5/files?"), 1);
    assert_eq!(transport.seen("/pulls/21/files?"), 1);
}
