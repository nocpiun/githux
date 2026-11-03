use super::*;
use crate::auth::{memory::InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::github::client::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
#[path = "../../bin_support/notifications_fixture.rs"]
mod fixture;
use fixture::{Fixture, Scene};

async fn setup(scene: Scene) -> (AccountService, Notifications, Arc<Fixture>) {
    let fixture = Arc::new(Fixture::new(scene));
    let accounts = AccountService::new(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(GitHubClient::new(GitHubConfig::default(), fixture.clone())),
        Arc::new(BackgroundExecutor::new(2)),
    );
    accounts.login(Pat::from_str("synthetic")).await.unwrap();
    let inbox = accounts.repo_service().notifications().unwrap();
    (accounts, inbox, fixture)
}
async fn run(inbox: &Notifications, kind: RequestKind) {
    let request = inbox
        .begin_at(kind, Instant::now() + Duration::from_secs(120))
        .unwrap()
        .unwrap();
    inbox.execute(request).await;
}
#[tokio::test]
async fn first_once_append_dedup_server_order_and_retry() {
    let (_accounts, inbox, fixture) = setup(Scene::AppendFailure).await;
    run(&inbox, RequestKind::First).await;
    assert!(inbox.begin(RequestKind::First).unwrap().is_none());
    run(&inbox, RequestKind::More).await;
    let failed = inbox.snapshot();
    assert_eq!(failed.items.len(), 8);
    assert!(failed.error.is_some() && failed.has_more && !failed.is_empty());
    run(&inbox, RequestKind::More).await;
    let snapshot = inbox.snapshot();
    assert_eq!(
        snapshot
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        (9000..9014).map(|id| id.to_string()).collect::<Vec<_>>()
    );
    assert!(snapshot.error.is_none() && !snapshot.has_more);
    assert_eq!(fixture.requests().len(), 4);
}
#[tokio::test]
async fn empty_and_first_failure_are_distinct_and_entry_does_not_retry() {
    let (_accounts, empty, _) = setup(Scene::Empty).await;
    run(&empty, RequestKind::First).await;
    assert!(empty.snapshot().is_empty());
    let (_accounts, failed, _) = setup(Scene::FirstFailure).await;
    run(&failed, RequestKind::First).await;
    assert!(!failed.snapshot().is_empty());
    assert!(failed.snapshot().error.is_some());
    assert!(failed.begin(RequestKind::First).unwrap().is_none());
    run(&failed, RequestKind::Refresh).await;
    assert_eq!(failed.snapshot().items.len(), 8);
}
#[tokio::test]
async fn poll_cooldown_and_inflight_reject_duplicate_requests() {
    let (_accounts, inbox, fixture) = setup(Scene::Mixed).await;
    let first = inbox.begin(RequestKind::First).unwrap().unwrap();
    assert!(matches!(
        inbox.begin(RequestKind::Refresh),
        Err(NotificationError::Busy)
    ));
    inbox.execute(first).await;
    assert!(matches!(
        inbox.begin(RequestKind::Refresh),
        Err(NotificationError::Cooldown { .. })
    ));
    assert_eq!(fixture.requests().len(), 2);
    assert!(inbox.begin(RequestKind::More).unwrap().is_some());
    inbox.cancel();
}
#[tokio::test]
async fn refresh_supersedes_append_success_and_error() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    for failure in [false, true] {
        let old = inbox
            .begin_at(RequestKind::More, Instant::now() + Duration::from_secs(120))
            .unwrap()
            .unwrap();
        let new = inbox
            .begin_at(
                RequestKind::Refresh,
                Instant::now() + Duration::from_secs(120),
            )
            .unwrap()
            .unwrap();
        let result = if failure {
            Err(PageFailure {
                error: GitHubError::Unauthorized.into(),
                metadata: NotificationMetadata {
                    poll_interval_seconds: Some(999),
                    ..Default::default()
                },
            })
        } else {
            Ok(NotificationPage {
                items: vec![],
                next: None,
                partial: vec![],
                metadata: NotificationMetadata::default(),
                not_modified: false,
            })
        };
        inbox.finish(&old, result, Instant::now());
        assert_eq!(inbox.snapshot().busy, Some(RequestKind::Refresh));
        assert_eq!(inbox.snapshot().items.len(), 8);
        assert!(inbox.snapshot().error.is_none());
        inbox.execute(new).await;
    }
}
#[tokio::test]
async fn stale_account_success_error_and_replacement_cannot_publish() {
    let (accounts, inbox, fixture) = setup(Scene::Delayed).await;
    let first = inbox.begin(RequestKind::First).unwrap().unwrap();
    // The actual adapter is awaiting its delayed response when PAT replacement
    // advances the existing session's epoch.
    let read = inbox.execute(first);
    let replace = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        accounts
            .login(Pat::from_str("replacement-synthetic"))
            .await
            .unwrap();
    };
    tokio::join!(read, replace);
    assert!(!inbox.verify());
    assert!(!inbox.snapshot().loaded);
    assert!(inbox.snapshot().items.is_empty());
    fixture.set_scene(Scene::FirstFailure);
    assert!(matches!(
        inbox.begin(RequestKind::Refresh),
        Err(NotificationError::GitHub(GitHubError::StaleContext))
    ));
}
#[tokio::test]
async fn account_aba_remove_readd_close_and_service_identity_reject_old_results() {
    for mode in ["aba", "remove", "close", "service"] {
        let (accounts, inbox, _) = setup(Scene::Mixed).await;
        let old_context = inbox.context().clone();
        let request = inbox.begin(RequestKind::First).unwrap().unwrap();
        match mode {
            "aba" => {
                accounts
                    .login(Pat::from_str("synthetic-second"))
                    .await
                    .unwrap();
                accounts
                    .repo_service()
                    .set_active(crate::domain::AccountKey::new(
                        42,
                        crate::domain::HostId::github(),
                    ))
                    .unwrap();
            }
            "remove" => {
                accounts
                    .remove_saved(crate::domain::AccountKey::new(
                        42,
                        crate::domain::HostId::github(),
                    ))
                    .await
                    .unwrap();
                accounts.login(Pat::from_str("synthetic")).await.unwrap();
            }
            "close" => {
                accounts.close().await.unwrap();
            }
            _ => {
                let (other, _, _) = setup(Scene::Mixed).await;
                assert!(!other.repo_service().is_current(&old_context));
                continue;
            }
        }
        for error in [false, true] {
            let result = if error {
                Err(PageFailure {
                    error: GitHubError::Unauthorized.into(),
                    metadata: NotificationMetadata::default(),
                })
            } else {
                Ok(NotificationPage {
                    items: vec![],
                    next: None,
                    partial: vec![],
                    metadata: NotificationMetadata::default(),
                    not_modified: false,
                })
            };
            inbox.finish(&request, result, Instant::now());
            assert!(!inbox.snapshot().loaded);
            assert!(inbox.snapshot().error.is_none());
        }
    }
}
#[tokio::test]
async fn partial_never_has_validator_and_item_bound_is_explicit() {
    let (_accounts, mut inbox, _) = setup(Scene::SsoPartial).await;
    run(&inbox, RequestKind::First).await;
    assert!(inbox.state.lock().unwrap().validator.is_none());
    assert!(!inbox.snapshot().partial.is_empty());
    let (_accounts, bounded, _) = setup(Scene::Mixed).await;
    inbox = bounded;
    inbox.limits.max_items = 9;
    run(&inbox, RequestKind::First).await;
    run(&inbox, RequestKind::More).await;
    let snapshot = inbox.snapshot();
    assert_eq!(snapshot.items.len(), 9);
    assert_eq!(snapshot.partial, vec![NotificationPartial::ItemLimit]);
    assert!(!snapshot.has_more);
}
#[tokio::test]
async fn not_modified_keeps_appended_rows_and_continuation() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    run(&inbox, RequestKind::More).await;
    let request = inbox
        .begin_at(
            RequestKind::Refresh,
            Instant::now() + Duration::from_secs(120),
        )
        .unwrap()
        .unwrap();
    assert!(request.validator.is_some());
    inbox.finish(
        &request,
        Ok(NotificationPage {
            items: vec![],
            next: None,
            partial: vec![],
            metadata: NotificationMetadata::default(),
            not_modified: true,
        }),
        Instant::now(),
    );
    assert_eq!(inbox.snapshot().items.len(), 14);
}
#[tokio::test]
async fn rate_failure_headers_gate_append_and_refresh_and_keep_rows() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let request = inbox.begin(RequestKind::More).unwrap().unwrap();
    inbox.finish(
        &request,
        Err(PageFailure {
            error: GitHubError::RateLimited {
                retry: crate::domain::error::RetryAfter {
                    kind: crate::domain::error::RateLimitKind::Secondary,
                    seconds: Some(90),
                },
            }
            .into(),
            metadata: NotificationMetadata {
                rate_limit: NotificationRateLimit {
                    retry_after: Some(90),
                    ..Default::default()
                },
                ..Default::default()
            },
        }),
        Instant::now(),
    );
    assert_eq!(inbox.snapshot().items.len(), 8);
    for kind in [RequestKind::Refresh, RequestKind::More] {
        assert!(matches!(
            inbox.begin(kind),
            Err(NotificationError::Cooldown { .. })
        ));
    }
}

async fn run_action(inbox: &Notifications, thread_id: &str, action: ThreadAction) {
    let request = inbox
        .begin_thread_action(thread_id, action)
        .unwrap()
        .unwrap();
    inbox.execute_thread_action(request).await;
}

#[tokio::test]
async fn done_uses_delete_and_only_204_removes_the_row() {
    let (_accounts, inbox, fixture) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    run_action(&inbox, "9000", ThreadAction::Done).await;
    let snapshot = inbox.snapshot();
    assert!(snapshot.items.iter().all(|item| item.id != "9000"));
    assert!(!snapshot.thread_states.contains_key("9000"));
    let writes: Vec<_> = fixture
        .requests()
        .into_iter()
        .filter(|request| request.method != crate::network::transport::HttpMethod::Get)
        .collect();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0].method,
        crate::network::transport::HttpMethod::Delete
    );
    assert!(writes[0].url.ends_with("/notifications/threads/9000"));
    assert!(writes[0].body.is_none());
}

#[tokio::test]
async fn done_failure_keeps_the_row_with_a_retryable_state() {
    let (_accounts, inbox, _) = setup(Scene::DoneFailure).await;
    run(&inbox, RequestKind::First).await;
    run_action(&inbox, "9000", ThreadAction::Done).await;
    let snapshot = inbox.snapshot();
    assert!(snapshot.items.iter().any(|item| item.id == "9000"));
    match snapshot.thread_states.get("9000") {
        Some(ThreadActionState::Failed { action, .. }) => assert_eq!(*action, ThreadAction::Done),
        other => panic!("expected a retryable Done failure, got {other:?}"),
    }
    // A retry after the scripted failure succeeds and removes the row.
    run_action(&inbox, "9000", ThreadAction::Done).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9000"));
}

#[tokio::test]
async fn unsubscribe_ignores_then_dones_and_completes() {
    let (_accounts, inbox, fixture) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    run_action(&inbox, "9001", ThreadAction::Unsubscribe).await;
    let snapshot = inbox.snapshot();
    assert!(snapshot.items.iter().all(|item| item.id != "9001"));
    let writes: Vec<_> = fixture
        .requests()
        .into_iter()
        .filter(|request| request.method != crate::network::transport::HttpMethod::Get)
        .collect();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0].method, crate::network::transport::HttpMethod::Put);
    assert!(
        writes[0]
            .url
            .ends_with("/notifications/threads/9001/subscription")
    );
    assert!(
        writes[0]
            .body
            .as_deref()
            .is_some_and(|body| body.contains("\"ignored\":true"))
    );
    assert!(
        writes[0]
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "application/json")
    );
    assert_eq!(
        writes[1].method,
        crate::network::transport::HttpMethod::Delete
    );
    assert!(writes[1].url.ends_with("/notifications/threads/9001"));
}

#[tokio::test]
async fn unsubscribe_first_step_failure_touches_no_done_and_keeps_the_row() {
    let (_accounts, inbox, fixture) = setup(Scene::UnsubscribeStepOneFailure).await;
    run(&inbox, RequestKind::First).await;
    run_action(&inbox, "9000", ThreadAction::Unsubscribe).await;
    assert!(inbox.snapshot().items.iter().any(|item| item.id == "9000"));
    assert!(matches!(
        inbox.snapshot().thread_states.get("9000"),
        Some(ThreadActionState::Failed {
            action: ThreadAction::Unsubscribe,
            ..
        })
    ));
    let writes: Vec<_> = fixture
        .requests()
        .into_iter()
        .filter(|request| request.method != crate::network::transport::HttpMethod::Get)
        .collect();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].method, crate::network::transport::HttpMethod::Put);
}

#[tokio::test]
async fn unsubscribe_keeps_the_ignored_fact_when_the_followup_done_fails() {
    let (_accounts, inbox, fixture) = setup(Scene::UnsubscribeStepTwoFailure).await;
    run(&inbox, RequestKind::First).await;
    run_action(&inbox, "9000", ThreadAction::Unsubscribe).await;
    let snapshot = inbox.snapshot();
    // The row stays because Done did not confirm removal, but the subscription
    // is recorded as already ignored so the retry only issues Done.
    assert!(snapshot.items.iter().any(|item| item.id == "9000"));
    assert!(matches!(
        snapshot.thread_states.get("9000"),
        Some(ThreadActionState::UnsubscribedPendingDone { .. })
    ));
    run_action(&inbox, "9000", ThreadAction::Done).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9000"));
    let writes: Vec<_> = fixture
        .requests()
        .into_iter()
        .filter(|request| request.method != crate::network::transport::HttpMethod::Get)
        .collect();
    assert_eq!(writes.len(), 3, "PUT, failed DELETE, retried DELETE");
    assert_eq!(
        writes[1].method,
        crate::network::transport::HttpMethod::Delete
    );
    assert_eq!(
        writes[2].method,
        crate::network::transport::HttpMethod::Delete
    );
}

#[tokio::test]
async fn duplicate_action_clicks_send_one_request_per_thread() {
    let (_accounts, inbox, fixture) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let first = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap();
    assert!(first.is_some());
    assert!(matches!(
        inbox.begin_thread_action("9000", ThreadAction::Done),
        Err(NotificationError::Busy)
    ));
    assert!(matches!(
        inbox.begin_thread_action("9000", ThreadAction::Unsubscribe),
        Err(NotificationError::Busy)
    ));
    // Another thread is unaffected by the first thread's in-flight action.
    assert!(
        inbox
            .begin_thread_action("9001", ThreadAction::Done)
            .unwrap()
            .is_some()
    );
    inbox.execute_thread_action(first.unwrap()).await;
    let writes = fixture
        .requests()
        .into_iter()
        .filter(|request| request.method != crate::network::transport::HttpMethod::Get)
        .count();
    assert_eq!(writes, 1);
}

#[tokio::test]
async fn stale_account_reply_and_late_list_response_cannot_revive_a_done_row() {
    let (accounts, inbox, fixture) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    // An old Done reply is in flight while the account changes.
    let old = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap()
        .unwrap();
    accounts
        .login(Pat::from_str("synthetic-second"))
        .await
        .unwrap();
    // The late reply is applied after the account change: the controller must
    // drop it, so the thread is neither removed nor tombstoned (the new account
    // may legitimately own the same numeric thread id).
    inbox.execute_thread_action(old).await;
    assert!(!inbox.verify());
    let stale = inbox.snapshot();
    assert!(stale.items.is_empty(), "a stale snapshot publishes nothing");
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|request| { request.method != crate::network::transport::HttpMethod::Get })
            .count(),
        0,
        "a stale action must not send under the new account"
    );
}

#[tokio::test]
async fn a_done_row_is_not_resurrected_by_a_late_refresh_page() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    run_action(&inbox, "9000", ThreadAction::Done).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9000"));
    // Refresh returns the same server page; the confirmed Done must not come back.
    run(&inbox, RequestKind::Refresh).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9000"));
    assert_eq!(inbox.snapshot().items.len(), 7);
}

#[tokio::test]
async fn list_requests_never_cancel_an_in_flight_thread_action() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let action = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap()
        .unwrap();
    // Starting a refresh supersedes the page-one list token, but the action
    // holds a separate lifecycle and must stay live.
    // A refresh is in poll cooldown here; More still supersedes the list token.
    let _more = inbox
        .begin_at(RequestKind::More, Instant::now() + Duration::from_secs(120))
        .unwrap();
    assert!(!action.cancel.is_cancelled());
    inbox.execute_thread_action(action).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9000"));
}

#[tokio::test]
async fn actions_work_after_the_view_was_cancelled_and_reentered() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    // Leaving/hiding a retained view cancels in-flight work and rotates tokens.
    inbox.cancel();
    let action = inbox
        .begin_thread_action("9001", ThreadAction::Done)
        .unwrap()
        .expect("a fresh action token is issued after cancel");
    assert!(!action.cancel.is_cancelled());
    inbox.execute_thread_action(action).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9001"));
}

#[tokio::test]
async fn confirmed_unsubscribe_survives_a_failed_done_retry() {
    let (_accounts, inbox, _) = setup(Scene::DoneAlwaysFails).await;
    run(&inbox, RequestKind::First).await;
    // Confirm the ignore, then fail Done twice.
    let ignored = inbox
        .begin_thread_action("9000", ThreadAction::Unsubscribe)
        .unwrap()
        .unwrap();
    // The fixture's unsubscribe path uses the always-failing Done.
    inbox.execute_thread_action(ignored).await;
    assert!(matches!(
        inbox.snapshot().thread_states.get("9000"),
        Some(ThreadActionState::UnsubscribedPendingDone { .. })
    ));
    let retry = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap()
        .unwrap();
    inbox.execute_thread_action(retry).await;
    assert!(
        matches!(
            inbox.snapshot().thread_states.get("9000"),
            Some(ThreadActionState::UnsubscribedPendingDone { .. })
        ),
        "the confirmed ignore must not degrade to an ordinary Done failure"
    );
    // Unsubscribe is not offered again; the ignore is already confirmed.
    assert!(
        inbox
            .begin_thread_action("9000", ThreadAction::Unsubscribe)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_shorter_rate_hint_never_shortens_an_outstanding_cooldown() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let rate = |seconds: u32| NotificationMetadata {
        rate_limit: NotificationRateLimit {
            retry_after: Some(seconds),
            ..Default::default()
        },
        ..Default::default()
    };
    let first = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap()
        .unwrap();
    let second = inbox
        .begin_thread_action("9001", ThreadAction::Done)
        .unwrap()
        .unwrap();
    // A long restriction lands first, then a shorter concurrent completion.
    inbox.finish_thread_action(
        &first,
        Err(PageFailure {
            error: GitHubError::RateLimited {
                retry: crate::domain::error::RetryAfter {
                    kind: crate::domain::error::RateLimitKind::Secondary,
                    seconds: Some(90),
                },
            }
            .into(),
            metadata: rate(90),
        }),
        Instant::now(),
    );
    inbox.finish_thread_action(
        &second,
        Err(PageFailure {
            error: GitHubError::RateLimited {
                retry: crate::domain::error::RetryAfter {
                    kind: crate::domain::error::RateLimitKind::Secondary,
                    seconds: Some(5),
                },
            }
            .into(),
            metadata: rate(5),
        }),
        Instant::now(),
    );
    let cooldown = inbox.snapshot().action_cooldown_seconds;
    assert!(
        cooldown > 80,
        "the 90s restriction must survive the later 5s hint, got {cooldown}s"
    );
    assert!(matches!(
        inbox.begin_thread_action("9002", ThreadAction::Done),
        Err(NotificationError::Cooldown { .. })
    ));
}

#[tokio::test]
async fn pending_done_survives_leaving_and_returning_to_a_retained_view() {
    let (_accounts, inbox, _) = setup(Scene::UnsubscribeStepTwoFailure).await;
    run(&inbox, RequestKind::First).await;
    run_action(&inbox, "9000", ThreadAction::Unsubscribe).await;
    assert!(matches!(
        inbox.snapshot().thread_states.get("9000"),
        Some(ThreadActionState::UnsubscribedPendingDone { .. })
    ));
    // Leaving the page cancels in-flight work and drops transient state.
    inbox.cancel();
    let snapshot = inbox.snapshot();
    assert!(
        matches!(
            snapshot.thread_states.get("9000"),
            Some(ThreadActionState::UnsubscribedPendingDone { .. })
        ),
        "the confirmed ignore must stay visible after leaving"
    );
    // Unsubscribe is still not offered again; only Done may be retried.
    assert!(
        inbox
            .begin_thread_action("9000", ThreadAction::Unsubscribe)
            .unwrap()
            .is_none()
    );
    assert!(
        inbox
            .begin_thread_action("9000", ThreadAction::Done)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn newer_activity_reenters_the_inbox_after_a_done() {
    let (_accounts, inbox, fixture) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let old_version = inbox
        .snapshot()
        .items
        .iter()
        .find(|item| item.id == "9000")
        .unwrap()
        .updated_at
        .clone();
    run_action(&inbox, "9000", ThreadAction::Done).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9000"));
    // The same refresh that returns the old version stays suppressed...
    run(&inbox, RequestKind::Refresh).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9000"));
    // ...but a newer server version of the same thread is a new notification.
    fixture.set_clock(fixture::PREVIEW_NOW + 3600);
    run(&inbox, RequestKind::Refresh).await;
    let revived = inbox
        .snapshot()
        .items
        .iter()
        .find(|item| item.id == "9000")
        .cloned();
    assert!(revived.is_some(), "newer activity must re-enter the inbox");
    assert_ne!(revived.unwrap().updated_at, old_version);
}

#[tokio::test]
async fn a_late_done_does_not_remove_a_newer_version_already_loaded() {
    let (_accounts, inbox, fixture) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let old_version = inbox
        .snapshot()
        .items
        .iter()
        .find(|item| item.id == "9000")
        .unwrap()
        .updated_at
        .clone();
    // Done for version A is in flight.
    let done = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap()
        .unwrap();
    // A concurrent refresh loads newer activity (version B) for the same id.
    fixture.set_clock(fixture::PREVIEW_NOW + 3600);
    run(&inbox, RequestKind::Refresh).await;
    assert!(
        inbox
            .snapshot()
            .items
            .iter()
            .any(|item| item.id == "9000" && item.updated_at != old_version)
    );
    // The late Done completion for A must not remove B.
    inbox.execute_thread_action(done).await;
    assert!(
        inbox.snapshot().items.iter().any(|item| item.id == "9000"),
        "newer activity must survive an older Done completion"
    );
}

#[tokio::test]
async fn a_cancelled_old_action_cannot_overwrite_a_new_action_or_unlock_it() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let old = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap()
        .unwrap();
    inbox.cancel();
    let current = inbox
        .begin_thread_action("9000", ThreadAction::Unsubscribe)
        .unwrap()
        .unwrap();
    assert_ne!(old.generation, current.generation);
    inbox.finish_thread_action(
        &old,
        Err(PageFailure {
            error: GitHubError::Cancelled.into(),
            metadata: NotificationMetadata::default(),
        }),
        Instant::now(),
    );
    assert!(matches!(
        inbox.snapshot().thread_states.get("9000"),
        Some(ThreadActionState::Busy(ThreadAction::Unsubscribe))
    ));
    inbox.execute_thread_action(current).await;
    assert!(inbox.snapshot().items.iter().all(|item| item.id != "9000"));
}

#[tokio::test]
async fn dropping_the_last_controller_cancels_the_action_lifecycle() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let action = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap()
        .unwrap();
    drop(inbox);
    assert!(
        action.cancel.is_cancelled(),
        "dropping the last owner must cancel queued writes; dropping a source alone does not cancel"
    );
}

#[tokio::test]
async fn nested_done_rate_limit_and_put_metadata_preserve_a_long_cooldown() {
    let (_accounts, inbox, fixture) = setup(Scene::UnsubscribeDoneRateLimited).await;
    run(&inbox, RequestKind::First).await;
    run_action(&inbox, "9000", ThreadAction::Unsubscribe).await;
    let snapshot = inbox.snapshot();
    assert!(matches!(
        snapshot.thread_states.get("9000"),
        Some(ThreadActionState::UnsubscribedPendingDone {
            error: NotificationError::GitHub(GitHubError::RateLimited { .. })
        })
    ));
    assert!(
        snapshot.action_cooldown_seconds >= 89,
        "nested 429 fallback/PUT restriction must be preserved, got {}",
        snapshot.action_cooldown_seconds
    );
    assert!(
        snapshot.cooldown_seconds >= 119,
        "the successful PUT poll interval must survive the failing Done, got {}",
        snapshot.cooldown_seconds
    );
    assert!(matches!(
        inbox.begin(RequestKind::More),
        Err(NotificationError::Cooldown { .. })
    ));
    assert!(matches!(
        inbox.begin_thread_action("9000", ThreadAction::Done),
        Err(NotificationError::Cooldown { .. })
    ));
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|request| request.method != crate::network::transport::HttpMethod::Get)
            .count(),
        2
    );
}

#[tokio::test]
async fn cancelling_invalidates_the_in_flight_action_generation() {
    let (_accounts, inbox, _) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let old = inbox
        .begin_thread_action("9000", ThreadAction::Done)
        .unwrap()
        .unwrap();
    inbox.cancel();
    // Even without starting a new action on the same thread, the old reply must
    // lose publication rights: a late success cannot remove the row, and a late
    // cancellation cannot install a failure state or cooldown.
    inbox.finish_thread_action(
        &old,
        Ok(notifications::UnsubscribeOutcome::Completed(
            NotificationMetadata::default(),
        )),
        Instant::now(),
    );
    let snapshot = inbox.snapshot();
    assert!(snapshot.items.iter().any(|item| item.id == "9000"));
    assert!(!snapshot.thread_states.contains_key("9000"));
    assert_eq!(snapshot.action_cooldown_seconds, 0);
}

#[tokio::test]
async fn unsubscribe_revalidates_authorization_before_the_done_step() {
    let (accounts, inbox, fixture) = setup(Scene::Mixed).await;
    run(&inbox, RequestKind::First).await;
    let release = fixture.gate_next_subscription();
    let action = inbox
        .begin_thread_action("9000", ThreadAction::Unsubscribe)
        .unwrap()
        .unwrap();
    let handle = tokio::spawn({
        let inbox = inbox.clone();
        async move { inbox.execute_thread_action(action).await }
    });
    // Wait until the PUT is parked, then switch the account before releasing it.
    for _ in 0..200 {
        let writes = fixture
            .requests()
            .into_iter()
            .filter(|request| request.method == crate::network::transport::HttpMethod::Put)
            .count();
        if writes >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    accounts
        .login(Pat::from_str("synthetic-second"))
        .await
        .unwrap();
    let _ = release.send(());
    handle.await.unwrap();
    assert!(!inbox.verify());
    let deletes = fixture
        .requests()
        .into_iter()
        .filter(|request| request.method == crate::network::transport::HttpMethod::Delete)
        .count();
    assert_eq!(
        deletes, 0,
        "the follow-up Done must not run after the account changed"
    );
}
