use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use gpui::{Entity, KeyUpEvent, Keystroke, TestAppContext, VisualContext, VisualTestContext};

use crate::app::AccountsHandle;
use crate::ui::Shell;
use crate::workspace::{Route, TabId};

use super::wait_for_shutdown;
use super::*;
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::domain::account::AuthorizationState;
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{
    HttpRequest, HttpResponse, MockTransport, Transport, TransportError,
};
use crate::storage::accounts::AccountDatabaseConfig;

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        crate::platform::fonts::register(cx).expect("test fonts");
        Shell::bind_keys(cx);
    });
}

/// The real production composition: an application-global account controller and the ordinary
/// Shell. Tests drive the same entity graph `app::run` creates; only the runtime is injected, so
/// nothing in the assertions depends on the preview window.
fn product_shell(
    cx: &mut TestAppContext,
    runtime: Option<AccountRuntime>,
) -> (Entity<Shell>, Entity<Accounts>, &mut VisualTestContext) {
    setup(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let accounts = cx.new(|cx| Accounts::new(runtime.map(Arc::new), window, cx));
        cx.set_global(AccountsHandle(accounts));
        Shell::new(window, cx)
    });
    let controller = cx.read(AccountsHandle::get).expect("installed controller");
    // Draw the Settings page so the form is mounted, then focus the masked field the way a user
    // does; the keyboard tests deliver real keystrokes into that same input.
    show_settings(&view, cx);
    focus_token(&controller, cx);
    (view, controller, cx)
}

fn user(id: u64, login: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![],
        body: serde_json::json!({"id": id, "login": login}).to_string(),
    }
}

fn repos(id: u64, login: &str, names: &[&str]) -> HttpResponse {
    let items: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            serde_json::json!({
                "id": index + 1, "name": name, "full_name": format!("{login}/{name}"),
                "owner": {"id": id, "login": login}, "visibility": "public"
            })
        })
        .collect();
    HttpResponse {
        status: 200,
        headers: vec![],
        body: serde_json::json!(items).to_string(),
    }
}

fn host_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("host runtime")
}

fn open(
    path: &std::path::Path,
    store: Arc<InMemoryCredentialStore>,
    transport: Arc<dyn Transport>,
) -> (AccountRuntime, Arc<BackgroundExecutor>) {
    let executor = Arc::new(BackgroundExecutor::new(4));
    let accounts = host_runtime()
        .block_on(AccountService::open(
            AccountDatabaseConfig::at(path.to_path_buf()),
            store,
            Arc::new(GitHubClient::new(GitHubConfig::default(), transport)),
            executor.clone(),
        ))
        .expect("open test service");
    (AccountRuntime::new(accounts, executor.clone()), executor)
}

/// Drains the controller the same way its 50ms background beat does, but synchronously, so a test
/// can assert a state transition rather than sleeping a fixed number of frames.
fn pump(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.drain(cx));
    cx.run_until_parked();
}

fn wait_for(
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    condition: impl Fn(&Snapshot) -> bool,
) {
    for _ in 0..400 {
        pump(controller, cx);
        std::thread::sleep(Duration::from_millis(5));
        if controller.read_with(cx, |accounts, _| condition(accounts.snapshot())) {
            return;
        }
    }
    let snapshot = controller.read_with(cx, |accounts, _| accounts.snapshot().clone());
    panic!("the account controller did not settle: {snapshot:?}");
}

fn put_token(controller: &Entity<Accounts>, cx: &mut VisualTestContext, token: &str) {
    let input = controller.read_with(cx, |accounts, _| accounts.token_input().clone());
    input.update(cx, |input, cx| input.set_text(token, cx));
}

fn focus_token(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let input = controller.read(cx).token_input().clone();
        let focus = input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.run_until_parked();
}

/// Draws the shell and puts the active page on Settings so the form is mounted for real, exactly
/// as clicking Settings would do it.
fn show_settings(view: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update_window_entity(view, |shell, window, cx| {
        shell.navigate(Route::Settings, window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("account-token-form").is_some(),
        "the Settings page must mount the account form"
    );
}

fn submit(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    cx.update_window_entity(controller, |accounts, window, cx| {
        accounts.submit(window, cx)
    });
}

fn refresh(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.refresh(cx));
}

fn select(controller: &Entity<Accounts>, id: u64, cx: &mut VisualTestContext) {
    let key = AccountKey::new(id, HostId::github());
    controller.update(cx, |accounts, cx| accounts.select(key, cx));
}

fn snapshot(controller: &Entity<Accounts>, cx: &mut VisualTestContext) -> Snapshot {
    controller.read_with(cx, |accounts, _| accounts.snapshot().clone())
}

#[gpui::test]
fn pat_submit_is_masked_cleared_and_real_repos_reach_the_shell(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    let host = host_runtime();
    host.block_on(async {
        mock.enqueue(Ok(user(1, "alice"))).await;
        mock.enqueue(Ok(repos(1, "alice", &["alpha", "beta"])))
            .await;
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock.clone());
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;

    // Before signing in the product has no account identity and no repository rows at all.
    assert!(view.read_with(cx, |shell, _| shell.repositories().is_empty()));
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Settings
    );
    assert!(cx.debug_bounds("account-signed-out").is_some());
    assert!(cx.debug_bounds("account-status").is_none());

    focus_token(&controller, cx);
    cx.update(|window, cx| {
        let input = controller.read(cx).token_input().clone();
        assert!(input.read(cx).is_masked());
        window.focus(&input.read(cx).focus_handle(cx), cx);
    });
    put_token(&controller, cx, "synthetic-never-real");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(controller.read_with(cx, |accounts, cx| {
        accounts.token_input().read(cx).is_empty()
    }));
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.generation),
        1
    );

    // A duplicate Enter with an empty field cannot submit.
    cx.simulate_keystrokes("enter");
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.generation),
        1
    );

    // The real identity reached the Shell's own sidebar, and the real account page is reachable.
    assert_eq!(view.read_with(cx, |shell, _| shell.repositories().len()), 0);
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("account-signed-out").is_none());
    assert!(cx.debug_bounds("account-name").is_some());
    assert!(cx.debug_bounds("account-login").is_some());

    // The same controller drives the sidebar list and the palette; the fixed toolbar refresh is
    // not repurposed, only the list's own refresh reads repositories.
    refresh(&controller, cx);
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let snap = snapshot(&controller, cx);
    println!("snapshot after A refresh: {:?}", snap.repositories.preview);
    println!(
        "shell rows: {}",
        view.read_with(cx, |shell, _| shell.repositories().len())
    );
    assert_eq!(view.read_with(cx, |shell, _| shell.repositories().len()), 2);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repositories()[0].full_name.clone()),
        "alice/alpha"
    );
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    assert_eq!(
        cx.debug_bounds("repo-row-0")
            .map(|b| b.size.width > gpui::Pixels::ZERO),
        Some(true)
    );
    let debug = controller.read_with(cx, |accounts, _| format!("{:?}", accounts.snapshot()));
    assert!(!debug.contains("synthetic-never-real"));
}

#[gpui::test]
fn replace_switch_and_restart_restore_use_the_persistent_service(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("accounts.sqlite3");
    let mock = Arc::new(MockTransport::new());
    let store = Arc::new(InMemoryCredentialStore::new());
    let host = host_runtime();
    host.block_on(async {
        for response in [
            user(1, "alice"),
            user(2, "bob"),
            user(1, "alice"),
            user(1, "alice"),
        ] {
            mock.enqueue(Ok(response)).await;
        }
        mock.enqueue(Ok(repos(2, "bob", &["bob-repo"]))).await;
    });
    let (runtime, _) = open(&path, store.clone(), mock.clone());
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
    });
    controller.update(cx, |accounts, cx| {
        accounts.replace(AccountKey::new(1, HostId::github()), cx)
    });
    put_token(&controller, cx, "replacement");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| {
        state
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("Replacement PAT verified"))
    });
    controller.update(cx, |accounts, cx| {
        accounts.revalidate(AccountKey::new(1, HostId::github()), cx)
    });
    wait_for(&controller, cx, |state| {
        state
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("revalidated"))
    });

    // The service is closed and reopened on the same real SQLite lease; no state is restored from
    // the old controller.
    let runtime = controller.read_with(cx, |accounts, _| accounts.runtime.clone().unwrap());
    host.block_on(runtime.accounts.close()).unwrap();
    drop(runtime);
    controller.update(cx, |accounts, _| {
        if let Some(runtime) = accounts.runtime.take() {
            runtime.shutdown();
        }
    });
    cx.update(|window, _| window.remove_window());
    drop(view);

    let (restored, _) = open(&path, store, mock);
    let (restored_view, restored_controller, cx) = product_shell(cx, Some(restored));
    let state = snapshot(&restored_controller, cx);
    assert_eq!(state.accounts.len(), 2);
    assert_eq!(state.active, Some(AccountKey::new(1, HostId::github())));
    assert!(
        state
            .accounts
            .iter()
            .all(|summary| summary.authorization == AuthorizationState::PendingVerification)
    );
    // A restored directory is not verified access: the Shell does not present it as verified.
    assert!(restored_view.read_with(cx, |shell, _| shell.route()) != Route::Profile);
    refresh(&restored_controller, cx);
    wait_for(
        &restored_controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 1),
    );
}

#[gpui::test]
fn errors_and_partial_results_are_not_successful_empty_lists(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    let host = host_runtime();
    host.block_on(async {
        mock.enqueue(Ok(user(1, "alice"))).await;
        mock.enqueue(Ok(HttpResponse {
            status: 403,
            headers: vec![("x-ratelimit-remaining".into(), "0".into())],
            body: "{}".into(),
        }))
        .await;
        mock.enqueue(Ok(repos(1, "alice", &["alpha"]))).await;
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    refresh(&controller, cx);
    wait_for(&controller, cx, |state| {
        matches!(state.repositories.preview, RepositoryPreview::Failed(_))
    });
    assert_eq!(
        snapshot(&controller, cx).accounts[0].authorization,
        AuthorizationState::Verified
    );
    refresh(&controller, cx);
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 1),
    );
}

struct Gate {
    state: Mutex<bool>,
    wake: Condvar,
    response: HttpResponse,
}

impl Gate {
    fn new(response: HttpResponse) -> Self {
        Self {
            state: Mutex::new(false),
            wake: Condvar::new(),
            response,
        }
    }
    fn release(&self) {
        *self.state.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

struct GatedTransport {
    responses: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    gates: Mutex<VecDeque<Arc<Gate>>>,
    requests: AtomicUsize,
}

impl Transport for GatedTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.fetch_add(1, Ordering::SeqCst);
            // Only repository reads are gated; login/revalidate replies are
            // scripted and must not consume a repository gate.
            let gate = if request.url.contains("/user/repos") {
                self.gates.lock().unwrap().pop_front()
            } else {
                None
            };
            let immediate = if gate.is_none() {
                self.responses.lock().unwrap().pop_front()
            } else {
                None
            };
            if let Some(gate) = gate {
                let wait = tokio::task::spawn_blocking(move || {
                    let mut open = gate.state.lock().unwrap();
                    while !*open {
                        open = gate.wake.wait(open).unwrap();
                    }
                    Ok(gate.response.clone())
                });
                return tokio::select! {
                    _ = cancel.cancelled() => Err(TransportError::Cancelled),
                    result = wait => result.expect("gate"),
                };
            }
            immediate.expect("scripted response")
        })
    }
}

#[gpui::test]
fn cancelled_and_stale_replies_never_overwrite_the_new_operation(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let first_gate = Arc::new(Gate::new(repos(1, "alice", &["old"])));
    let second_gate = Arc::new(Gate::new(repos(1, "alice", &["new"])));
    let transport = Arc::new(GatedTransport {
        responses: Mutex::new(VecDeque::from([
            Ok(user(1, "alice")),
            Ok(repos(1, "alice", &["latest"])),
        ])),
        gates: Mutex::new(VecDeque::from([first_gate.clone(), second_gate.clone()])),
        requests: AtomicUsize::new(0),
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, transport);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    refresh(&controller, cx);
    let first_generation = controller.read_with(cx, |accounts, _| accounts.generation);
    controller.update(cx, |accounts, cx| accounts.cancel(cx));
    refresh(&controller, cx);
    let second_generation = controller.read_with(cx, |accounts, _| accounts.generation);
    assert!(second_generation > first_generation);
    // The new operation completes while the older request is still blocked.
    second_gate.release();
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories[0].name == "new"),
    );
    // Releasing the older request afterwards must not overwrite the newer view.
    first_gate.release();
    for _ in 0..50 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(5));
    }
    let published = snapshot(&controller, cx);
    assert!(matches!(
        &published.repositories.preview,
        RepositoryPreview::Ready(outcome) if outcome.repositories[0].name == "new"
    ));
    assert!(!format!("{published:?}").contains("alice/old"));
    assert!(view.read_with(cx, |shell, _| {
        shell.repositories().len() == 1 && shell.repositories()[0].name == "new"
    }));
    // Duplicate submissions while busy do not advance the operation.
    put_token(&controller, cx, "duplicate-ignored");
    let generation = controller.read_with(cx, |accounts, _| accounts.generation);
    refresh(&controller, cx);
    refresh(&controller, cx);
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.generation),
        generation + 1
    );
    controller.update(cx, |accounts, cx| accounts.cancel(cx));
}

#[gpui::test]
fn partial_page_failure_is_shown_as_partial_with_the_data_received(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    let page2 = "http://127.0.0.1:1/user/repos?page=2";
    host_runtime().block_on(async {
        mock.enqueue(Ok(user(1, "alice"))).await;
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![("Link".into(), format!("<{page2}>; rel=\"next\""))],
            body: serde_json::json!([{
                "id": 1, "name": "first", "full_name": "alice/first",
                "owner": {"id": 1, "login": "alice"}, "visibility": "public"
            }])
            .to_string(),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 503,
            headers: vec![],
            body: "{}".into(),
        }))
        .await;
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    refresh(&controller, cx);
    wait_for(&controller, cx, |state| {
        matches!(
            &state.repositories.preview,
            RepositoryPreview::Ready(outcome)
                if outcome.repositories.len() == 1
                    && matches!(outcome.readiness, crate::domain::repo::ReposReadiness::Partial { .. })
        )
    });
}

#[gpui::test]
fn a_repository_reply_is_dropped_when_the_account_changes_mid_request(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let gate = Arc::new(Gate::new(repos(1, "alice", &["old-account"])));
    let transport = Arc::new(GatedTransport {
        responses: Mutex::new(VecDeque::from([Ok(user(1, "alice")), Ok(user(2, "bob"))])),
        gates: Mutex::new(VecDeque::from([gate.clone()])),
        requests: AtomicUsize::new(0),
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, transport);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    refresh(&controller, cx);
    let runtime = controller.read_with(cx, |accounts, _| accounts.runtime.clone().unwrap());
    host_runtime()
        .block_on(
            runtime
                .accounts
                .set_active(AccountKey::new(1, HostId::github())),
        )
        .unwrap();
    gate.release();
    for _ in 0..80 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(5));
    }
    let presentation = snapshot(&controller, cx);
    assert!(
        !format!("{presentation:?}").contains("old-account"),
        "a result for the previous active account must not be published"
    );
    assert!(
        matches!(
            presentation.repositories.preview,
            RepositoryPreview::Idle(_)
        ),
        "a discarded read must not leave the preview stuck in loading"
    );
}

/// Review round 2: a mutation that reports an error can still have committed
/// (CleanupPending), so repositories read under the previous account must not
/// remain displayed as a complete set.
#[gpui::test]
fn an_errored_mutation_clears_repository_results_for_the_previous_account(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(async {
        // login A, login B, repository read for B, then reselect A
        mock.enqueue(Ok(user(1, "alice"))).await;
        mock.enqueue(Ok(user(2, "bob"))).await;
        mock.enqueue(Ok(repos(2, "bob", &["bob-repo"]))).await;
        mock.enqueue(Ok(user(1, "alice"))).await;
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock.clone());
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    refresh(&controller, cx);
    wait_for(&controller, cx, |state| {
        matches!(state.repositories.preview, RepositoryPreview::Ready(_))
    });
    // Selecting another saved account changes the authoritative account, so
    // the repository set read under the previous account must be dropped.
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
    });
    assert!(matches!(
        snapshot(&controller, cx).repositories.preview,
        RepositoryPreview::Idle(_)
    ));
}

/// Regression for review finding 1: cancelling an account mutation whose
/// durable commit already landed must reconcile the authoritative directory
/// instead of leaving a stale "no saved accounts" view.
#[gpui::test]
fn cancelling_after_a_committed_mutation_reconciles_saved_accounts(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(mock.enqueue(Ok(user(1, "alice"))));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock.clone());
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    // Cancel while the login is in flight; the login may still commit.
    controller.update(cx, |accounts, cx| accounts.cancel(cx));
    let runtime = controller.read_with(cx, |accounts, _| accounts.runtime.clone().unwrap());
    // Drive the login to completion through the service itself, then let the
    // view observe `Settled` and reconcile.
    for _ in 0..200 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(5));
        if !runtime.accounts.summaries().is_empty() {
            break;
        }
    }
    for _ in 0..200 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(5));
        if snapshot(&controller, cx).accounts.len() == 1 {
            break;
        }
    }
    let presentation = snapshot(&controller, cx);
    assert_eq!(
        presentation.accounts.len(),
        runtime.accounts.summaries().len(),
        "the displayed directory must match the service after cancellation settles"
    );
    assert!(
        !presentation
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("verified and saved")),
        "a cancelled operation must not publish its own success message"
    );
}

/// Review finding 2: a cancelled revalidation may still settle in the service,
/// but neither its reply nor its epoch change may be presented as the outcome
/// of a newer read.
#[gpui::test]
fn cancelling_revalidation_does_not_publish_its_late_success(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(async {
        mock.enqueue(Ok(user(1, "alice"))).await;
        mock.enqueue(Ok(user(1, "alice"))).await;
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    let key = AccountKey::new(1, HostId::github());
    controller.update(cx, |accounts, cx| accounts.revalidate(key, cx));
    controller.update(cx, |accounts, cx| accounts.cancel(cx));
    for _ in 0..200 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(5));
        if snapshot(&controller, cx)
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("Cancellations settled"))
        {
            break;
        }
    }
    let presentation = snapshot(&controller, cx);
    assert!(
        !presentation
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("revalidated against")),
        "a cancelled revalidation must not publish its own success"
    );
}

#[gpui::test]
fn closing_the_view_cancels_and_cannot_publish(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let gate = Arc::new(Gate::new(repos(1, "alice", &["late"])));
    let transport = Arc::new(GatedTransport {
        responses: Mutex::new(VecDeque::from([Ok(user(1, "alice"))])),
        gates: Mutex::new(VecDeque::from([gate.clone()])),
        requests: AtomicUsize::new(0),
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, transport);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    refresh(&controller, cx);
    let before = snapshot(&controller, cx);
    controller.update(cx, |accounts, _| {
        accounts.alive.send_replace(false);
        if let Some(operation) = accounts.operation.take() {
            operation.source.cancel();
        }
        if let Some(runtime) = accounts.runtime.take() {
            runtime.shutdown();
        }
    });
    gate.release();
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(5));
    }
    // A worker that finishes after teardown has nowhere to publish a result.
    pump(&controller, cx);
    assert!(controller.read_with(cx, |accounts, _| accounts.inbox.lock().unwrap().is_empty()));
    assert_eq!(snapshot(&controller, cx), before);
}

/// Review round 3: the parent owns the submit-enabled flag, so typing must
/// redraw it; otherwise the button stays disabled until an unrelated update.
#[gpui::test]
fn typing_a_token_enables_the_submit_button_without_another_operation(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(mock.enqueue(Ok(user(1, "alice"))));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    cx.simulate_input("synthetic-token");
    cx.run_until_parked();
    // A plain simulated click on the submit button must reach the service.
    let target = cx
        .debug_bounds("account-token-submit")
        .map(|bounds| bounds.center());
    if let Some(target) = target {
        cx.simulate_click(target, gpui::Modifiers::none());
    } else {
        submit(&controller, cx);
    }
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
}

/// Review round 7: process exit must wait for the detached teardown instead of
/// truncating an in-flight commit.
#[gpui::test]
fn shutdown_waits_for_background_teardown(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    if let Some(runtime) = controller.read_with(cx, |accounts, _| accounts.runtime.clone()) {
        runtime.shutdown();
    }
    drop(view);
    let started = std::time::Instant::now();
    wait_for_shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "the exit gate must be bounded"
    );
    // A second call after the drain has completed returns immediately.
    let again = std::time::Instant::now();
    wait_for_shutdown();
    assert!(again.elapsed() < Duration::from_secs(1));
}

/// Review round 6: an operation that fails must keep its own error even when
/// the directory has unrelated journalled cleanup, and stale reply snapshots
/// must not overwrite a newer reconciliation.
#[gpui::test]
fn a_failed_operation_keeps_its_error_and_presentation_comes_from_the_service(
    cx: &mut TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(async {
        mock.enqueue(Ok(user(1, "alice"))).await;
        // The second login is rejected by GitHub.
        mock.enqueue(Ok(HttpResponse {
            status: 401,
            headers: vec![],
            body: "{}".into(),
        }))
        .await;
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "good");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    put_token(&controller, cx, "bad");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| {
        state
            .status
            .as_ref()
            .is_some_and(|status| status.tone == StatusTone::Error && status.text.contains("401"))
    });
    let presentation = snapshot(&controller, cx);
    assert!(
        !presentation
            .status
            .as_ref()
            .is_some_and(|status| status.text.starts_with("The account change was committed")),
        "an unrelated cleanup journal must not rewrite this operation's error"
    );
    assert_eq!(
        presentation.accounts.len(),
        1,
        "the failed login saved nothing"
    );
}

/// Review round 4: two cancelled mutations may settle in any order. Tracking
/// only the most recent generation would permanently drop the earlier one.
#[gpui::test]
fn every_cancelled_mutation_is_reconciled_independently(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(mock.enqueue(Ok(user(1, "alice"))));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    let key = AccountKey::new(1, HostId::github());
    // Cancel one mutation, then cancel a second one before the first settles.
    controller.update(cx, |accounts, cx| accounts.revalidate(key.clone(), cx));
    let first = controller.read_with(cx, |accounts, _| accounts.generation);
    controller.update(cx, |accounts, cx| accounts.cancel(cx));
    controller.update(cx, |accounts, cx| accounts.revalidate(key, cx));
    let second = controller.read_with(cx, |accounts, _| accounts.generation);
    controller.update(cx, |accounts, cx| accounts.cancel(cx));
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.reconciling.clone()),
        vec![first, second],
        "both cancelled mutations stay pending until they settle"
    );
    // Whichever order the two settlements arrive in, both are reconciled.
    controller.update(cx, |accounts, cx| accounts.reconcile(second, cx));
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.reconciling.clone()),
        vec![first]
    );
    controller.update(cx, |accounts, cx| accounts.reconcile(first, cx));
    assert!(controller.read_with(cx, |accounts, _| accounts.reconciling.is_empty()));
    assert!(
        snapshot(&controller, cx)
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("Cancellations settled"))
    );
}

/// Review round 3: the preview must not be retained by its own keystroke
/// subscription, otherwise releasing the view never runs `Drop` and the
/// runtime/request teardown is skipped.
/// A/B accounts opening the same repository keep isolated tabs, order, active selection and
/// scroll. Switching A → B → A restores A exactly; the remembered navigation is not silently
/// re-authorized (the workspace incarnation advances), and a delayed reply cannot change B.
#[gpui::test]
fn account_switching_isolates_workspaces_and_restores_a(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(async {
        mock.enqueue(Ok(user(1, "alice"))).await;
        mock.enqueue(Ok(user(2, "bob"))).await;
        mock.enqueue(Ok(repos(1, "alice", &["shared-repo", "only-a"])))
            .await;
        mock.enqueue(Ok(repos(2, "bob", &["shared-repo", "only-b"])))
            .await;
        mock.enqueue(Ok(repos(1, "alice", &["shared-repo", "only-a"])))
            .await;
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;

    // Sign in as A and as B; A is selected first.
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
    });

    // A reads repositories and opens the same repo B will also open.
    refresh(&controller, cx);
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        let route = Route::Repository {
            namespace: "alice".to_string(),
            name: "shared-repo".to_string(),
        };
        shell.navigate(route, window, cx);
    });
    cx.run_until_parked();
    let a_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    let a_ids: Vec<TabId> = view.read_with(cx, |shell, _| shell.tab_ids());
    assert_eq!(view.read_with(cx, |shell, _| shell.repositories().len()), 2);

    // Switch to B: its own Home, no A tabs, no A repository list published.
    select(&controller, 2, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(2, HostId::github()))
    });
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 1);
    assert!(view.read_with(cx, |shell, _| shell.repositories().is_empty()));
    assert_ne!(view.read_with(cx, |shell, _| shell.active_tab_id()), a_tab);

    // B reads its own list, which contains the same repository plus one only B sees.
    refresh(&controller, cx);
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repositories()[1].name.clone()),
        "only-b"
    );

    // Switch back to A: its tabs, order, active tab and rows are restored.
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
    });
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_ids()), a_ids);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), a_tab);
    // The list was invalidated by the switch (scope 4): it is not silently re-published from the
    // old authorization, and A must refresh under its current context to read again.
    assert!(view.read_with(cx, |shell, _| shell.repositories().is_empty()));
    refresh(&controller, cx);
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    assert_eq!(view.read_with(cx, |shell, _| shell.repositories().len()), 2);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repositories()[1].name.clone()),
        "only-a"
    );
}

/// The controller's keystroke interceptor holds only a weak reference, so dropping the window's
/// root view and the app-global must release the controller and run its `Drop` teardown. The
/// observable proof is the account runtime shutting down, not a raw strong-count probe.
#[gpui::test]
fn dropping_the_window_runs_the_controller_teardown(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    // The runtime's `closed` flag is swapped on shutdown; observe it from outside.
    let closed = runtime.closed.clone();
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    let weak = controller.downgrade();
    assert!(
        weak.upgrade().is_some(),
        "the controller is alive before teardown"
    );

    // Simulate the application quit path: close the window, drop handles, clear the global.
    cx.update(|window, _| window.remove_window());
    drop(view);
    drop(controller);
    cx.cx.update(|cx| cx.clear_globals());

    // The controller's Drop runs `shutdown`, which closes the account runtime off the UI thread.
    let mut released = false;
    for _ in 0..400 {
        if weak.upgrade().is_none() {
            released = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        released,
        "the interceptor must hold a weak reference, not the entity"
    );
    let closed = closed.load(std::sync::atomic::Ordering::Acquire);
    assert!(
        closed,
        "the account runtime must be shut down with the controller"
    );
}

/// The migrated Ely field must still accept pasted and CJK text, keep it
/// masked, and expose the same value to the service on submit.
#[gpui::test]
fn paste_and_cjk_text_stay_masked_and_submit(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(mock.enqueue(Ok(user(1, "alice"))));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    // Focus the masked field and paste from the clipboard, as Ctrl+V does.
    focus_token(&controller, cx);
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("ghp_pasted".into()));
    });
    cx.simulate_keystrokes("ctrl-v");
    cx.update_window_entity(&controller, |accounts, _, cx| {
        let input = accounts.token_input().read(cx);
        assert_eq!(input.text(), "ghp_pasted", "paste reaches the field");
        assert!(input.is_masked(), "the field stays masked after pasting");
    });
    // A CJK composition replaces the value through the same input handler.
    cx.simulate_input("中文口令");
    cx.update_window_entity(&controller, |accounts, _, cx| {
        let input = accounts.token_input().read(cx);
        assert!(input.text().contains("中文口令"), "CJK text is accepted");
        assert!(input.is_masked(), "the field stays masked with CJK text");
    });
    // No unmasked copy path: the field never renders the value as aria text.
    put_token(&controller, cx, "ghp_final");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    assert!(controller.read_with(cx, |accounts, cx| {
        accounts.token_input().read(cx).is_empty()
    }));
}

#[gpui::test]
fn tab_reaches_the_submit_button_and_enter_submits(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(mock.enqueue(Ok(user(1, "alice"))));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    // Password field, visibility toggle, then submit in Ely's focus order.
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("enter");
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").unwrap(),
    });
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
}
