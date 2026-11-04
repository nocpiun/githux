// Acceptance regressions adopted from the coordinator's basic acceptance run (run 5f274477), which
// reproduced these three gaps against ebc8dea. They are included by the `tests` module at the end of
// `profile.rs`; each one was verified to fail with the corresponding fix reverted.
#[gpui::test]
fn acceptance_revocation_also_clears_online_profile(cx: &mut TestAppContext) {
    let (harness, cx) = build_harness(cx, true);
    wait_for_profile(&harness, cx, |cx| harness.page.read_with(cx, |page, cx| {
        page.work.iter().all(Option::is_none) && page.view_for_test().is_some_and(|v| {
            matches!(v.read(cx).repositories(), SectionState::Ready(_))
                && matches!(v.read(cx).readme_state(), SectionState::Ready(()))
        })
    }));
    let session = harness.accounts.page_session().unwrap();
    let binding = harness.accounts.repo_service().cache(session.context()).unwrap();
    binding.store().revoke_repository(binding.scope(), Some(101)).unwrap();
    assert!(session.verify());
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        pump_page(&harness, cx);
        if harness.page.read_with(cx, |page, _| page.view_for_test().is_none()) { break; }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(harness.page.read_with(cx, |page, _| page.view_for_test().is_none()),
        "a confirmed repository revocation must invalidate displayed aggregates even when originally fetched online");
}

#[gpui::test]
fn acceptance_queued_historical_profile_is_rejected_after_revocation(cx: &mut TestAppContext) {
    let (harness, cx) = build_harness(cx, true);
    wait_for_profile(&harness, cx, |cx| harness.page.read_with(cx, |page, _| page.work.iter().all(Option::is_none)));
    let session = harness.accounts.page_session().unwrap();
    let repos = harness.accounts.repo_service();
    let binding = repos.cache(session.context()).unwrap();
    let cached = CachedPageReadSession::new(session.clone(), binding.store().clone(), binding.scope().clone()).unwrap();
    harness.transport.set_offline(true);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let outcome = rt.block_on(async {
        let stamp = prepare_gate_stamp(&repos, session.context()).await;
        read_profile_basic(&repos, &session, Some(&cached), stamp.as_ref(), "alice", Default::default()).await.unwrap()
    });
    assert!(outcome.provenance.as_ref().is_some_and(ProfileProvenance::is_historical));
    // The background read has completed successfully, but its reply has not reached the UI yet.
    binding.store().revoke_repository(binding.scope(), Some(101)).unwrap();
    cx.update_window_entity(&harness.page, |page, window, cx| {
        let reply = Reply {
            section: Section::Basic, generation: page.generation,
            request: page.requests[Section::Basic.index()], selection: page.selection,
            context: session.context().clone(), login: page.login.clone(),
            result: SectionResult::Basic(Ok(outcome)),
        };
        page.apply(reply, window, cx);
    });
    assert!(!harness.page.read_with(cx, |page, cx| page.view_for_test().is_some_and(|v|
        v.read(cx).basic_provenance().is_some_and(ProfileProvenance::is_historical))),
        "a reply read before revocation must not publish historical content after revocation, even before the first monitor tick");
}

#[gpui::test]
fn acceptance_profile_cache_write_failure_keeps_online_result(cx: &mut TestAppContext) {
    let (harness, cx) = build_harness(cx, true);
    wait_for_profile(&harness, cx, |cx| harness.page.read_with(cx, |page, _| page.work.iter().all(Option::is_none)));
    let session = harness.accounts.page_session().unwrap();
    let repos = harness.accounts.repo_service();
    let binding = repos.cache(session.context()).unwrap();
    let cached = CachedPageReadSession::new(session.clone(), binding.store().clone(), binding.scope().clone()).unwrap();
    // Deterministic failure in this test's disposable SQLite database only.
    let db = rusqlite::Connection::open(harness._temp.path().join("cache/cache.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER acceptance_fail_insert BEFORE INSERT ON entries BEGIN SELECT RAISE(ABORT, 'synthetic cache write failure'); END;").unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(read_profile_basic(&repos, &session, Some(&cached), None, "alice", Default::default()));
    let control = rt.block_on(session.profile("alice", Default::default())).unwrap();
    assert_eq!(control.value.login, "alice");
    assert!(result.is_ok(), "healthy online Profile must survive a cache write failure; actual={:?}", result.err());
}

#[tokio::test]
async fn acceptance_profile_reopens_service_and_store_without_online_revalidation() {
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let transport = Arc::new(ProfileFixtureTransport::new());
    let executor = Arc::new(BackgroundExecutor::new(4));
    let range = rolling_year();
    for offline in [false, true] {
        transport.set_offline(offline);
        let store = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        let accounts = AccountService::open_with_cache(
            AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")), credentials.clone(),
            Arc::new(GitHubClient::new(GitHubConfig::default(), transport.clone())), executor.clone(), store,
        ).await.unwrap();
        if !offline {
            accounts.login(Pat::from_str("synthetic-profile-acceptance")).await.unwrap();
        } else {
            assert_eq!(accounts.active_summary().unwrap().authorization, crate::domain::account::AuthorizationState::PendingVerification);
        }
        let session = accounts.page_session().unwrap();
        let repos = accounts.repo_service();
        let binding = repos.cache(session.context()).unwrap();
        let cached = CachedPageReadSession::new(session.clone(), binding.store().clone(), binding.scope().clone()).unwrap();
        let stamp = prepare_gate_stamp(&repos, session.context()).await;
        let basic = read_profile_basic(&repos, &session, Some(&cached), stamp.as_ref(), "alice", Default::default()).await.unwrap();
        let pinned = read_profile_pinned(&repos, &session, Some(&cached), stamp.as_ref(), "alice", Default::default()).await.unwrap();
        let (calendar, _) = read_profile_contributions(&repos, &session, Some(&cached), stamp.as_ref(), "alice", &range, Default::default()).await.unwrap();
        let readme = read_profile_readme(&repos, &session, Some(&cached), stamp.as_ref(), &basic.value, Default::default()).await.unwrap();
        for provenance in [basic.provenance.as_ref(), pinned.provenance.as_ref(), calendar.provenance.as_ref(), readme.provenance.as_ref()] {
            assert_eq!(provenance.is_some_and(ProfileProvenance::is_historical), offline);
        }
        assert!(matches!(readme.value, ProfileReadme::Prepared { .. }));
        accounts.close().await.unwrap();
    }
}