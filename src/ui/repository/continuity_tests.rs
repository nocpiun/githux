use super::*;

#[gpui::test]
fn continuity_same_commit_and_manual_refresh_keep_reader_scroll_selection_and_overview(
    cx: &mut TestAppContext,
) {
    let mut fixture = CachedFixture::new(Scene::ReadmeParagraphEnd);
    fixture.login(1);
    warm_up(cx, &fixture);
    fixture.restart();
    fixture.transport.hold(true);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    let (reader, overview) = page.read_with(cx, |page, _| {
        (
            page.reader().unwrap().clone(),
            page.overview().unwrap().entity_id(),
        )
    });
    let doc = reader.read_with(cx, |reader, _| reader.markdown().unwrap().clone());
    doc.update(cx, |doc, cx| {
        doc.select_all(cx);
        doc.list_state().scroll_to(gpui::ListOffset {
            item_ix: 5,
            offset_in_item: gpui::px(4.),
        });
        cx.notify();
    });
    frames(cx);
    let (scroll, selection) = doc.read_with(cx, |doc, _| {
        (doc.list_state().logical_scroll_top(), doc.selection())
    });
    assert!(scroll.item_ix > 0);
    assert!(!selection.is_empty());
    fixture.transport.hold(false);
    fixture.transport.release_all();
    settle(&page, cx, |page| {
        assert_eq!(page.reader().unwrap().entity_id(), reader.entity_id());
        assert_eq!(page.reader_status(), &ReaderStatus::Showing);
        !page.is_showing_cached_snapshot()
    });
    fixture.transport.hold(true);
    page.update(cx, |page, cx| page.refresh_repository(cx));
    frames(cx);
    assert!(cx.debug_bounds("reader-markdown").is_some());
    page.read_with(cx, |page, _| {
        assert_eq!(page.reader().unwrap().entity_id(), reader.entity_id());
        assert_eq!(page.overview().unwrap().entity_id(), overview);
        assert!(page.tree("").is_some());
    });
    fixture.transport.hold(false);
    fixture.transport.release_all();
    settle(&page, cx, |page| {
        assert_eq!(page.reader().unwrap().entity_id(), reader.entity_id());
        !page.snapshot_preliminary
            && page
                .revision_provenance()
                .is_some_and(|p| p.origin == RepositoryCacheOrigin::Network)
    });
    doc.read_with(cx, |doc, _| {
        let now = doc.list_state().logical_scroll_top();
        assert_eq!(now.item_ix, scroll.item_ix);
        assert_eq!(now.offset_in_item, scroll.offset_in_item);
        assert_eq!(doc.selection(), selection);
    });
    let actions = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    page.update(cx, |page, _| {
        let actions = actions.clone();
        page.on_action(std::rc::Rc::new(move |action, _, _| {
            actions.borrow_mut().push(action)
        }));
    });
    reader.update_in(cx, |reader, window, cx| {
        reader.handle_link("https://example.invalid/retained", window, cx)
    });
    frames(cx);
    assert_eq!(
        actions.borrow().len(),
        1,
        "retained reader callbacks must use the new refresh identity"
    );
    page.update(cx, |page, cx| {
        page.select_tab(crate::ui::repository::RepositoryTab::Issues, cx)
    });
    fixture.transport.hold(true);
    page.update(cx, |page, cx| page.refresh_repository(cx));
    frames(cx);
    assert_eq!(
        page.read_with(cx, |page, _| page.tab()),
        crate::ui::repository::RepositoryTab::Issues
    );
    fixture.transport.hold(false);
    fixture.transport.release_all();
}

fn pending_changed_commit<'a>(
    cx: &'a mut TestAppContext,
    fixture: &mut CachedFixture,
    path: Option<&str>,
    gate: &str,
) -> (
    Entity<OfflineHost>,
    Entity<RepositoryPage>,
    &'a mut VisualTestContext,
) {
    fixture.login(1);
    warm_up(cx, fixture);
    fixture.restart();
    fixture.transport.hold(true);
    let opened = open_cached(
        cx,
        fixture,
        RepositoryNavigation {
            path: path.map(str::to_string),
            ..RepositoryNavigation::default()
        },
    );
    let (host, page, cx) = opened;
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    assert!(cx.debug_bounds("reader").is_some());
    assert!(page.read_with(cx, |page, _| page.is_showing_cached_snapshot()));
    fixture
        .transport
        .moved_main
        .store(true, std::sync::atomic::Ordering::SeqCst);
    *fixture.transport.held_path.lock().unwrap() = Some(gate.into());
    fixture.transport.release_all();
    settle(&page, cx, |_| {
        fixture
            .transport
            .held_requests
            .load(std::sync::atomic::Ordering::SeqCst)
            >= 1
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.reader_status(), &ReaderStatus::Showing);
        assert_eq!(page.fixed_commit().unwrap().as_str(), MAIN_COMMIT);
        assert!(page.replacement.is_some());
    });
    (host, page, cx)
}

#[gpui::test]
fn continuity_replacement_failure_retains_the_old_commit_and_explains_the_failure(
    cx: &mut TestAppContext,
) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    let gate = "/git/blobs/b00000000000000000000000000000000000000a";
    let (_host, page, cx) = pending_changed_commit(cx, &mut fixture, None, gate);
    let reader = page.read_with(cx, |page, _| page.reader().unwrap().entity_id());
    *fixture.transport.failed_path.lock().unwrap() = Some(gate.into());
    fixture.transport.tree_release.send_replace(true);
    settle(&page, cx, |page| page.error().is_some());
    assert!(cx.debug_bounds("reader-markdown").is_some());
    assert!(cx.debug_bounds("repository-refresh-error").is_some());
    page.read_with(cx, |page, _| {
        assert_eq!(page.reader().unwrap().entity_id(), reader);
        assert_eq!(page.fixed_commit().unwrap().as_str(), MAIN_COMMIT);
        assert!(page.is_showing_cached_snapshot());
        assert!(page.replacement.is_none());
    });
}

#[gpui::test]
fn continuity_replacement_resolves_the_latest_selection_and_reports_removed_files(
    cx: &mut TestAppContext,
) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture
        .transport
        .remove_main
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (_host, page, cx) = pending_changed_commit(
        cx,
        &mut fixture,
        None,
        "/git/trees/a000000000000000000000000000000000000002",
    );
    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    assert_eq!(
        page.read_with(cx, |page, _| page.current_path().map(str::to_string)),
        Some("src/main.rs".into())
    );
    fixture.transport.tree_release.send_replace(true);
    settle(&page, cx, |page| {
        page.fixed_commit()
            .is_some_and(|sha| sha.as_str() == FEATURE_COMMIT)
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.selection(), &Selection::File("src/main.rs".into()));
        assert!(matches!(
            page.reader_status(),
            ReaderStatus::SelectionUnavailable { .. }
        ));
        assert!(page.reader().is_none());
        assert!(page.tree("src").is_some());
    });
    assert!(cx.debug_bounds("repository-row-src/main.rs").is_none());
    assert!(
        cx.debug_bounds("repository-row-src/feature_note.rs")
            .is_some()
    );
}

#[gpui::test]
fn continuity_a_ref_switch_cancels_the_prepared_replacement(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    let (_host, page, cx) = pending_changed_commit(
        cx,
        &mut fixture,
        None,
        "/git/trees/a000000000000000000000000000000000000002",
    );
    let (generation, branch_request, context) = page.read_with(cx, |page, _| {
        (
            page.generation,
            page.branch_request,
            page.context.clone().unwrap(),
        )
    });
    page.update(cx, |page, cx| page.select_branch("feature/docs-polish", cx));
    frames(cx);
    page.read_with(cx, |page, _| {
        assert!(page.replacement.is_none());
        assert!(
            page.reader().is_none(),
            "the old ref must not be presented as the new ref"
        );
    });
    fixture.transport.tree_release.send_replace(true);
    settle(&page, cx, |page| {
        page.selected_branch() == Some("feature/docs-polish")
            && page.reader_status() == &ReaderStatus::Showing
    });
    // A fully queued failure from the superseded ref must not disturb the new page either.
    page.update_in(cx, |page, window, cx| {
        page.apply(
            crate::ui::repository::Message {
                generation,
                branch_request,
                context,
                authorization: None,
                preliminary: false,
                body: crate::ui::repository::Body::Revision {
                    branch: "main".into(),
                    request: branch_request,
                    result: Err(crate::ui::repository::PageError::new(
                        "late old-ref failure",
                        None,
                        true,
                    )),
                },
            },
            window,
            cx,
        );
    });
    page.read_with(cx, |page, _| {
        assert!(page.error().is_none());
        assert_eq!(page.selected_branch(), Some("feature/docs-polish"));
        assert_eq!(page.fixed_commit().unwrap().as_str(), FEATURE_COMMIT);
    });
}

#[gpui::test]
fn continuity_an_account_switch_retires_a_pending_replacement_and_its_late_reply(
    cx: &mut TestAppContext,
) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    let (_host, page, cx) = pending_changed_commit(
        cx,
        &mut fixture,
        None,
        "/git/trees/a000000000000000000000000000000000000002",
    );
    fixture.login(2);
    page.update_in(cx, |page, window, cx| page.drain(window, cx));
    frames(cx);
    page.read_with(cx, |page, _| {
        assert!(!page.is_active());
        assert!(page.details().is_none());
        assert!(page.reader().is_none());
        assert!(page.replacement.is_none());
    });
    assert!(cx.debug_bounds("reader-markdown").is_none());
    fixture.transport.tree_release.send_replace(true);
    page.update(cx, |page, cx| page.rebind(fixture.services(), cx));
    // The newly active account has no eligible persisted snapshot; it must read its own data.
    assert!(page.read_with(cx, |page, _| page.details().is_none()
        && page.reader().is_none()));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.context().unwrap().auth().account.id, 2);
        assert_eq!(page.fixed_commit().unwrap().as_str(), FEATURE_COMMIT);
    });
}

#[gpui::test]
fn continuity_revocation_during_manual_refresh_clears_retained_content_in_one_drain(
    cx: &mut TestAppContext,
) {
    let fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    fixture.transport.hold(true);
    page.update(cx, |page, cx| page.refresh_repository(cx));
    assert!(page.read_with(cx, |page, _| page.reader().is_some()));
    let store = fixture.store.as_ref().unwrap();
    let scope = store
        .saved_account(crate::domain::AccountKey::new(
            1,
            crate::domain::HostId::github(),
        ))
        .unwrap();
    store.revoke_repository(&scope, Some(42)).unwrap();
    page.update_in(cx, |page, window, cx| page.drain(window, cx));
    frames(cx);
    page.read_with(cx, |page, _| {
        assert!(!page.is_active());
        assert!(page.details().is_none() && page.reader().is_none() && page.overview().is_none());
    });
    assert!(cx.debug_bounds("reader-markdown").is_none());
    fixture.transport.release_all();
}

#[gpui::test]
fn continuity_a_moved_readme_reveals_its_new_path_in_the_replacement_tree(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture
        .transport
        .moved_readme
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (_host, page, cx) = pending_changed_commit(
        cx,
        &mut fixture,
        None,
        "/git/trees/a000000000000000000000000000000000000002",
    );
    fixture.transport.tree_release.send_replace(true);
    settle(&page, cx, |page| {
        page.fixed_commit()
            .is_some_and(|sha| sha.as_str() == FEATURE_COMMIT)
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.current_path(), Some("docs/README.md"));
        assert!(page.expanded().contains("docs"));
        assert_eq!(page.reader_status(), &ReaderStatus::Showing);
    });
    assert!(cx.debug_bounds("repository-row-docs/README.md").is_some());
    assert!(cx.debug_bounds("repository-row-README.md").is_none());
    assert!(reader_text(&page, cx).contains("docs polish"));
}

#[gpui::test]
fn continuity_a_recycled_locator_retires_old_repository_replies(cx: &mut TestAppContext) {
    let fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    fixture.transport.hold(true);
    page.update_in(cx, |page, window, cx| {
        let old_reply = crate::ui::repository::Message {
            generation: page.generation,
            branch_request: page.branch_request,
            authorization: None,
            context: page.context.clone().unwrap(),
            preliminary: false,
            body: crate::ui::repository::Body::Branches(Ok((
                page.branches().unwrap().clone(),
                None,
            ))),
        };
        let mut details = page.details().unwrap().clone();
        details.identity.id = 43;
        page.apply_repository(Ok((details, None)), page.star_write_generation, window, cx);
        page.apply(old_reply, window, cx);
        assert_eq!(page.details().unwrap().identity.id(), 43);
        assert!(page.branch.is_none() && page.reader().is_none());
        assert!(
            page.branches().is_none(),
            "the old repository's picker cannot be adopted under a recycled locator"
        );
    });
    fixture.transport.release_all();
}

#[gpui::test]
fn continuity_review_refresh_retries_failed_resources_on_the_same_commit(cx: &mut TestAppContext) {
    let fixture = CachedFixture::new(Scene::FileFailure);
    fixture.login(1);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
    settle(&page, cx, |page| {
        matches!(page.reader_status(), ReaderStatus::ContentFailed(_))
    });
    let verified_before = page.read_with(cx, |page, _| {
        page.revision_provenance().unwrap().verified_at_ms
    });
    fixture.transport.hold(true);
    // A separate failed directory must be retried by the same refresh too.
    page.update(cx, |page, cx| {
        page.branch.as_mut().unwrap().tree.fail_load(
            "docs",
            crate::ui::repository::PageError::new("synthetic directory failure", None, true),
        );
        page.intent.expanded.insert("docs".into());
        page.refresh_repository(cx);
        assert!(matches!(page.reader_status(), ReaderStatus::ContentFailed(_)),
            "until revalidation answers, retain an honest terminal state rather than an idle Loading");
    });
    fixture.transport.hold(false);
    fixture.transport.release_all();
    settle(&page, cx, |page| {
        page.revision_provenance()
            .is_some_and(|p| p.verified_at_ms > verified_before)
    });
    page.read_with(cx, |page, _| {
        assert!(
            !matches!(page.reader_status(), ReaderStatus::ContentFailed(_)),
            "successful same-commit revalidation must retry the failed document"
        );
        assert!(page.tree_failure("docs").is_none());
    });
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing && page.tree("docs").is_some()
    });
    assert!(reader_text(&page, cx).contains("fn main"));
    assert_eq!(
        page.read_with(cx, |page, _| page
            .fixed_commit()
            .unwrap()
            .as_str()
            .to_string()),
        MAIN_COMMIT
    );
}

#[gpui::test]
fn continuity_review_a_failed_summary_does_not_disable_independent_branch_retry(
    cx: &mut TestAppContext,
) {
    let fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    *fixture.transport.failed_path.lock().unwrap() = Some("/repos/octo/ledger".into());
    fixture
        .transport
        .failed_status
        .store(404, std::sync::atomic::Ordering::SeqCst);
    page.update(cx, |page, cx| page.refresh_repository(cx));
    settle(&page, cx, |page| page.error().is_some());
    page.read_with(cx, |page, _| {
        assert!(
            !page.branches_loading,
            "summary failure must release the branch retry gate"
        );
        assert_eq!(page.reader_status(), &ReaderStatus::Showing);
    });
    *fixture.transport.failed_path.lock().unwrap() = None;
    let before = fixture
        .transport
        .recorded()
        .iter()
        .filter(|url| url.contains("/branches"))
        .count();
    page.update(cx, |page, cx| page.retry_branches(cx));
    settle(&page, cx, |page| !page.branches_loading);
    assert!(
        fixture
            .transport
            .recorded()
            .iter()
            .filter(|url| url.contains("/branches"))
            .count()
            > before
    );
}

fn canceled_document_has_a_terminal_state_after_refresh_failure(
    cx: &mut TestAppContext,
    failed_path: &str,
) {
    let fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());
    *fixture.transport.held_path.lock().unwrap() =
        Some("/git/blobs/b000000000000000000000000000000000000004".into());
    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
    settle(&page, cx, |page| {
        page.pending_content.is_some()
            && fixture
                .transport
                .held_requests
                .load(std::sync::atomic::Ordering::SeqCst)
                >= 1
    });
    *fixture.transport.failed_path.lock().unwrap() = Some(failed_path.into());
    fixture
        .transport
        .failed_status
        .store(404, std::sync::atomic::Ordering::SeqCst);
    page.update(cx, |page, cx| page.refresh_repository(cx));
    settle(&page, cx, |page| page.error().is_some());
    page.read_with(cx, |page, _| {
        assert!(matches!(page.reader_status(), ReaderStatus::ContentFailed(error) if error.retryable),
            "a canceled file must offer retry after refresh fails, rather than leave an unowned Loading");
        assert!(page.pending_content.is_none());
        assert_eq!(page.fixed_commit().unwrap().as_str(), MAIN_COMMIT);
    });
    assert!(cx.debug_bounds("repository-content-error").is_some());
    assert!(cx.debug_bounds("repository-loading").is_none());
    *fixture.transport.failed_path.lock().unwrap() = None;
    fixture.transport.tree_release.send_replace(true);
    page.update(cx, |page, cx| page.retry_reader(cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    assert!(reader_text(&page, cx).contains("fn main"));
}

#[gpui::test]
fn continuity_canceled_file_settles_when_summary_refresh_fails(cx: &mut TestAppContext) {
    canceled_document_has_a_terminal_state_after_refresh_failure(cx, "/repos/octo/ledger");
}

#[gpui::test]
fn continuity_canceled_file_settles_when_revision_refresh_fails(cx: &mut TestAppContext) {
    canceled_document_has_a_terminal_state_after_refresh_failure(cx, "/commits/main");
}

#[gpui::test]
fn continuity_refresh_failure_keeps_a_newer_document_read_running(cx: &mut TestAppContext) {
    let fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());
    fixture.transport.hold(true);
    *fixture.transport.held_path.lock().unwrap() =
        Some("/git/blobs/b000000000000000000000000000000000000004".into());
    *fixture.transport.failed_path.lock().unwrap() = Some("/repos/octo/ledger".into());
    fixture
        .transport
        .failed_status
        .store(404, std::sync::atomic::Ordering::SeqCst);
    page.update(cx, |page, cx| {
        page.refresh_repository(cx);
        page.select_file("src/main.rs", cx);
    });
    settle(&page, cx, |page| {
        page.pending_content.is_some()
            && fixture
                .transport
                .held_requests
                .load(std::sync::atomic::Ordering::SeqCst)
                >= 1
    });
    fixture.transport.release_all();
    settle(&page, cx, |page| page.error().is_some());
    page.read_with(cx, |page, _| {
        assert!(page.pending_content.is_some());
        assert_eq!(
            page.reader_status(),
            &ReaderStatus::Loading,
            "a document request started after refresh still owns its loading state"
        );
    });
    *fixture.transport.failed_path.lock().unwrap() = None;
    fixture.transport.tree_release.send_replace(true);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    assert!(reader_text(&page, cx).contains("fn main"));
}

#[gpui::test]
fn continuity_failed_refresh_keeps_the_cached_document(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    warm_up(cx, &fixture);
    fixture.restart();
    fixture.transport.hold(true);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    let reader = page.read_with(cx, |page, _| page.reader().unwrap().entity_id());
    fixture.transport.set_offline(true);
    fixture.transport.hold(false);
    fixture.transport.release_all();
    settle(&page, cx, |page| !page.snapshot_preliminary);
    assert!(cx.debug_bounds("reader-markdown").is_some());
    page.read_with(cx, |page, _| {
        assert_eq!(page.reader().unwrap().entity_id(), reader);
        assert!(page.is_showing_cached_snapshot());
        assert_eq!(page.fixed_commit().unwrap().as_str(), MAIN_COMMIT);
    });
    page.update(cx, |page, cx| page.refresh_repository(cx));
    frames(cx);
    assert!(cx.debug_bounds("reader-markdown").is_some());
    settle(&page, cx, |page| !page.snapshot_preliminary);
    assert_eq!(
        page.read_with(cx, |page, _| page.reader().unwrap().entity_id()),
        reader
    );
}
