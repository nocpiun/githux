use super::*;
use crate::auth::token::Pat;
use gpui::TestAppContext;
#[path = "pr_files_fixture.rs"]
pub(crate) mod fixture;
use fixture::{EMPTY, FIRST, Fixture};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn files_changed_decodes_paths_hunks_missing_patch_and_both_forms_of_truncation() {
    runtime().block_on(async {
        let fixture = Fixture::new().await;
        let page = fixture.session(FIRST).page(1, CancellationToken::default()).await.unwrap().value;
        assert_eq!(page.end, PageEnd::More);
        assert_eq!(page.files[0].filename, "docs/变更说明.md");
        assert_eq!(page.files[0].status, Status::Added);
        let Patch::Text { rows, partial: false } = &page.files[0].patch else { panic!("expected added text") };
        assert_eq!((rows[2].old, rows[2].new, rows[2].text.as_str()), (None, Some(2), "新增说明"));
        assert_eq!(page.files[1].status, Status::Removed);
        let Patch::Text { rows, partial: false } = &page.files[1].patch else { panic!("expected removed text") };
        assert_eq!((rows[1].old, rows[1].new), (Some(1), None));
        assert_eq!(rows.last().unwrap().kind, RowKind::Note);
        let renamed = &page.files[2];
        assert_eq!(renamed.status, Status::Renamed);
        assert_eq!(renamed.previous_filename.as_deref(), Some("src/old name.rs"));
        let Patch::Text { rows, partial: false } = &renamed.patch else { panic!("expected renamed text") };
        assert_eq!((rows[3].old, rows[3].new), (Some(3), Some(3)));
        assert_eq!((rows[6].old, rows[6].new), (None, Some(90)));
        assert_eq!(page.files[3].patch, Patch::Unavailable);
        assert_eq!(page.files[5].patch, Patch::Unavailable);
        for index in [4, 6] {
            assert!(matches!(&page.files[index].patch, Patch::Text { rows, partial: true } if rows.len() > 2));
        }
    });
}

#[test]
fn files_changed_caps_pagination_and_rejects_a_link_for_a_different_pr() {
    runtime().block_on(async {
        let fixture = Fixture::new().await;
        let session = fixture.session(3);
        let last = session
            .page(100, CancellationToken::default())
            .await
            .unwrap()
            .value;
        assert_eq!(last.end, PageEnd::FileLimit);
        assert_eq!(last.offset() + last.files.len() as u32, FILE_LIMIT);
        assert!(matches!(
            session.page(101, CancellationToken::default()).await,
            Err(RepositoryError::InvalidInput { .. })
        ));
        assert!(matches!(
            fixture
                .session(4)
                .page(1, CancellationToken::default())
                .await,
            Err(RepositoryError::InvalidResponse { .. })
        ));
        assert_eq!(fixture.transport.paths.lock().unwrap().len(), 3);
    });
}

#[gpui::test]
fn files_changed_keeps_page_on_failure_and_rejects_old_pr_and_account_replies(
    cx: &mut TestAppContext,
) {
    let runtime = runtime();
    let fixture = runtime.block_on(Fixture::new());
    let first = fixture.session(FIRST);
    let second = fixture.session(EMPTY);
    cx.update(ely_core::init_for_tests);
    let (view, cx) =
        cx.add_window_view(|_, _| FilesView::new(first.clone(), fixture.executor.clone()));
    cx.run_until_parked();
    assert_eq!(fixture.transport.paths.lock().unwrap().len(), 1);

    // Keep the UI scheduler deterministic by delivering real reader results at
    // controlled points. Simulate an already queued old success and old error.
    let read = |session: &FilesSession, page| {
        runtime.block_on(session.page(page, CancellationToken::default()))
    };
    let token = CancellationToken::default();
    let page_one = read(&first, 1);
    let failed = read(&first, 2);
    let retry = read(&first, 2);
    view.update(cx, |view, cx| {
        view.active = true;
        view.finish(&first, &token, 1, page_one, cx);
        view.finish(&first, &token, 2, failed, cx);
        assert_eq!(view.page.as_ref().unwrap().number, 1);
        assert_eq!(view.failure.as_ref().unwrap().0, 2);
        view.failure = None;
        view.finish(&first, &token, 2, retry, cx);
        assert_eq!(view.file().unwrap().filename, "docs/page-two.md");
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.widest_row,
            Some(2),
            "escaped bidi controls shape wider than the longer ASCII input"
        );
        let handle = view.diff_scroll.0.borrow();
        let reach = handle.base_handle.max_offset().x;
        assert!(reach > px(0.));
        assert!(reach + handle.base_handle.bounds().size.width >= view.widest_text);
    });
    let requests = fixture.transport.paths.lock().unwrap().clone();
    assert_eq!(requests[2], requests[3]);
    assert!(requests[2].ends_with("/pulls/1/files?per_page=30&page=2"));
    let old_page = read(&first, 1);
    let empty = read(&second, 1);
    view.update(cx, |view, cx| {
        view.set_active(false, cx);
        view.rebind(second.clone(), cx);
        view.active = true;
        view.finish(&second, &token, 1, empty, cx);
        view.finish(&first, &token, 1, old_page, cx);
        view.finish(&first, &token, 2, Err(worker_closed()), cx);
        assert!(view.page.as_ref().unwrap().files.is_empty());
        assert!(view.failure.is_none());
    });

    let old_account_page = read(&first, 1);
    runtime
        .block_on(fixture.accounts.login(Pat::from_str("fixture-account-b")))
        .unwrap();
    assert!(!first.verify());
    let new_account = fixture.session(FIRST);
    let before = fixture.transport.paths.lock().unwrap().len();
    assert!(matches!(
        read(&first, 1),
        Err(RepositoryError::GitHub(GitHubError::StaleContext))
    ));
    assert_eq!(fixture.transport.paths.lock().unwrap().len(), before);
    view.update(cx, |view, cx| {
        view.set_active(false, cx);
        view.rebind(new_account, cx);
        view.active = true;
        view.finish(&first, &token, 1, old_account_page, cx);
        assert!(view.page.is_none());
    });
}
