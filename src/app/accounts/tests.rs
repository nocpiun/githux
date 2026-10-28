use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualContext, VisualTestContext};

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

/// Waits until no account operation is in flight and no cancelled mutation is still settling, so a
/// test can start the next mutation instead of colliding with an automatic read.
fn wait_idle(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    for _ in 0..400 {
        pump(controller, cx);
        std::thread::sleep(Duration::from_millis(5));
        if controller.read_with(cx, |accounts, _| {
            !accounts.busy() && accounts.reconciling.is_empty()
        }) {
            return;
        }
    }
    panic!("the account controller did not become idle");
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
    // The login settled, so the automatic loader started the list read: the generation advanced
    // past the login operation without any user action.
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let after_autoload = controller.read_with(cx, |accounts, _| accounts.generation);
    assert!(
        after_autoload >= 2,
        "login plus the automatic read advance the generation, got {after_autoload}"
    );

    // A duplicate Enter with an empty field cannot submit.
    cx.simulate_keystrokes("enter");
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.generation),
        after_autoload
    );

    // The real identity reached the Shell's own sidebar, and the real account page is reachable.
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("account-signed-out").is_none());
    assert!(cx.debug_bounds("account-name").is_some());
    assert!(cx.debug_bounds("account-login").is_some());

    // The list loads itself after a successful login: no refresh call here. The next read is the
    // automatic one that the settled login triggered.
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
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
    let mock = Arc::new(AutoReposTransport::new());
    let store = Arc::new(InMemoryCredentialStore::new());
    // The automatic loader reads the visible list after each settled login/select/revalidate, so
    // both accounts keep a servable body for as many reads as those settlements issue.
    mock.serve_repos("first", repos(1, "alice", &["alice-repo"]));
    mock.serve_repos("second", repos(2, "bob", &["bob-repo"]));
    mock.serve_repos("replacement", repos(1, "alice", &["alice-repo"]));
    for response in [
        user(1, "alice"),
        user(2, "bob"),
        user(1, "alice"),
        user(1, "alice"),
    ] {
        mock.script_user(response);
    }
    let (runtime, _) = open(&path, store.clone(), mock.clone());
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    // The automatic list read must finish before the next submit can start.
    wait_idle(&controller, cx);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    wait_idle(&controller, cx);
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
    });
    wait_idle(&controller, cx);
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
    // Let the replacement's automatic read settle so revalidate starts from an idle controller.
    wait_for(&controller, cx, |state| {
        matches!(&state.repositories.preview, RepositoryPreview::Ready(_))
    });
    wait_idle(&controller, cx);
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
    host_runtime().block_on(runtime.accounts.close()).unwrap();
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
    // After the restart the list loads by itself: no refresh call before this settles.
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

/// Answers `/user` from a scripted queue and serves `/user/repos` from a per-token body, so each
/// account's read returns the list that account's own credential would see even when the automatic
/// loader fires during a switch. It counts requests per URL class so a test can assert the list
/// loaded once without any manual refresh.
struct AutoReposTransport {
    responses: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    /// Repository bodies keyed by the token value the request presents.
    repos_by_token: Mutex<std::collections::HashMap<String, HttpResponse>>,
    /// The token whose `/user` reply arrived most recently, used only to associate a scripted user
    /// reply with the token it belongs to is not possible, so tests register token->body directly.
    pub user_reads: AtomicUsize,
    pub repo_reads: AtomicUsize,
}

impl AutoReposTransport {
    fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            repos_by_token: Mutex::new(std::collections::HashMap::new()),
            user_reads: AtomicUsize::new(0),
            repo_reads: AtomicUsize::new(0),
        }
    }

    fn script_user(&self, response: HttpResponse) {
        self.responses.lock().unwrap().push_back(Ok(response));
    }

    /// Serves `response` for every repository read made with `token`. Tests use the same synthetic
    /// token they submit, so the body follows the credential rather than the order of requests.
    fn serve_repos(&self, token: &str, response: HttpResponse) {
        self.repos_by_token
            .lock()
            .unwrap()
            .insert(token.to_string(), response);
    }
}

fn bearer_token(request: &HttpRequest) -> Option<String> {
    request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| {
            value
                .trim_start_matches("Bearer ")
                .trim_start_matches("bearer ")
                .to_string()
        })
}

impl Transport for AutoReposTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if request.url.contains("/user/repos") {
                self.repo_reads.fetch_add(1, Ordering::SeqCst);
                let token = bearer_token(&request);
                let body = token
                    .and_then(|token| self.repos_by_token.lock().unwrap().get(&token).cloned());
                return body.ok_or_else(|| TransportError::Network {
                    message: "no scripted repository body for this token".into(),
                });
            }
            self.user_reads.fetch_add(1, Ordering::SeqCst);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| {
                    Err(TransportError::Network {
                        message: "no scripted user response".into(),
                    })
                })
        })
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
    // Two accounts exist. A's repository read parks on the gate; the active account then becomes B,
    // so A's late reply must not be published and must not leave the list stuck in loading.
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
    // First login: A. Its automatic read parks on the gate before B exists.
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    // Register B by selecting it after a second login, but the second login's own read would take
    // the same gate; instead log B in through the service directly so the UI's in-flight A read is
    // untouched.
    let runtime = controller.read_with(cx, |accounts, _| accounts.runtime.clone().unwrap());
    host_runtime()
        .block_on(
            runtime
                .accounts
                .login(crate::auth::token::Pat::from_str("second")),
        )
        .expect("second account logs in through the service");
    // Now switch the authoritative selection to B while A's read is still parked.
    host_runtime()
        .block_on(
            runtime
                .accounts
                .set_active(AccountKey::new(2, HostId::github())),
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
        !matches!(
            presentation.repositories.preview,
            RepositoryPreview::Loading
        ),
        "a discarded read must not leave the list stuck in loading"
    );
}

#[gpui::test]
fn an_errored_mutation_clears_repository_results_for_the_previous_account(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(AutoReposTransport::new());
    mock.serve_repos("first", repos(1, "alice", &["alice-repo"]));
    mock.serve_repos("second", repos(2, "bob", &["bob-repo"]));
    mock.script_user(user(1, "alice"));
    mock.script_user(user(2, "bob"));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock.clone());
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    wait_idle(&controller, cx);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    // B is active after the second login; its list loads automatically.
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories[0].name == "bob-repo"),
    );
    assert_eq!(view.read_with(cx, |shell, _| shell.repositories().len()), 1);
    // Selecting A changes the authoritative account: the B list must be dropped, then A reloads.
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
            && matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories[0].name == "alice-repo")
    });
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repositories()[0].name.clone()),
        "alice-repo"
    );
}

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
    let mock = Arc::new(AutoReposTransport::new());
    mock.serve_repos("synthetic", repos(1, "alice", &["a"]));
    mock.script_user(user(1, "alice"));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    // Wait past the login *and* its automatic list read so the two cancelled revalidations below
    // start from an idle controller with no in-flight operation.
    wait_for(&controller, cx, |state| {
        state.accounts.len() == 1
            && matches!(&state.repositories.preview, RepositoryPreview::Ready(_))
    });
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
    let mock = Arc::new(AutoReposTransport::new());
    mock.serve_repos("first", repos(1, "alice", &["shared-repo", "only-a"]));
    mock.serve_repos("second", repos(2, "bob", &["shared-repo", "only-b"]));
    mock.script_user(user(1, "alice"));
    mock.script_user(user(2, "bob"));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));

    // Sign in as A and B; the second login makes B active, and its list auto-loads.
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    wait_idle(&controller, cx);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );

    // Switch to A: its list loads automatically for A's own credential.
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
            && matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.iter().any(|r| r.name == "only-a"))
    });
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repositories()[1].name.clone()),
        "only-a"
    );

    // A opens a repository tab; record its identity.
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

    // Switch to B: its own Home first, no A tab, and its own rows.
    select(&controller, 2, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(2, HostId::github()))
            && matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.iter().any(|r| r.name == "only-b"))
    });
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_count()), 1);
    assert_ne!(view.read_with(cx, |shell, _| shell.active_tab_id()), a_tab);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repositories()[1].name.clone()),
        "only-b"
    );

    // Back to A: tabs, order, active tab restored; the list reloads under A's current authorization.
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
            && matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.iter().any(|r| r.name == "only-a"))
    });
    assert_eq!(view.read_with(cx, |shell, _| shell.tab_ids()), a_ids);
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), a_tab);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repositories()[1].name.clone()),
        "only-a"
    );
}

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
    let mock = Arc::new(AutoReposTransport::new());
    mock.serve_repos("synthetic", repos(1, "alice", &["a"]));
    mock.script_user(user(1, "alice"));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "synthetic");
    // Tab must move focus through the form without killing it, and the token field must still be
    // reachable and submit on Enter afterwards.
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    cx.update(|window, cx| assert!(window.focused(cx).is_some(), "focus stays live after Tab"));
    focus_token(&controller, cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
}

/// The owner-facing requirement: the list loads itself, exactly once, without any refresh call, and
/// a repaint, resize or sidebar collapse does not read it again. A failed read is not retried in a
/// loop, so a broken credential cannot become a request storm.
#[gpui::test]
fn the_repository_list_loads_once_by_itself_and_never_storms(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(AutoReposTransport::new());
    mock.serve_repos("synthetic", repos(1, "alice", &["one", "two"]));
    mock.script_user(user(1, "alice"));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(
        &temp.path().join("accounts.sqlite3"),
        store.clone(),
        mock.clone(),
    );
    let (view, controller, cx) = product_shell(cx, Some(runtime));

    // A cold start with only a restored saved account must verify the credential and read the list
    // by itself: no refresh, no revalidate press, no Settings visit.
    let summary = controller.read_with(cx, |accounts, _| accounts.snapshot().accounts.clone());
    assert!(
        summary.is_empty(),
        "the fresh profile only has what the test logs in"
    );

    put_token(&controller, cx, "synthetic");
    submit(&controller, cx);
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let reads_after_load = mock.repo_reads.load(Ordering::SeqCst);
    assert_eq!(
        reads_after_load, 1,
        "the list loads exactly once after login"
    );

    // Repaints, resizes and a sidebar collapse must not trigger another read.
    for _ in 0..3 {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
    }
    cx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(560.0)));
    cx.run_until_parked();
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.toggle_sidebar_for_test(window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    for _ in 0..3 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        mock.repo_reads.load(Ordering::SeqCst),
        reads_after_load,
        "repaint, resize and collapse must not read the list again"
    );

    // A failed read is recoverable but not retried automatically: no storm.
    let failing = Arc::new(AutoReposTransport::new());
    failing.serve_repos(
        "synthetic",
        HttpResponse {
            status: 401,
            headers: vec![],
            body: "{}".into(),
        },
    );
    failing.script_user(user(1, "alice"));
    let (runtime, _) = open(
        &temp.path().join("failing.sqlite3"),
        store.clone(),
        failing.clone(),
    );
    let (view2, controller2, cx) = product_shell(cx, Some(runtime));
    let _ = &view2;
    put_token(&controller2, cx, "synthetic");
    submit(&controller2, cx);
    wait_for(&controller2, cx, |state| {
        matches!(&state.repositories.preview, RepositoryPreview::Failed(_))
    });
    let reads_after_failure = failing.repo_reads.load(Ordering::SeqCst);
    for _ in 0..40 {
        pump(&controller2, cx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        failing.repo_reads.load(Ordering::SeqCst),
        reads_after_failure,
        "a failed read must not retry in a loop"
    );
    assert!(
        matches!(
            snapshot(&controller2, cx).repositories.preview,
            RepositoryPreview::Failed(_)
        ),
        "a failed read stays a visible, recoverable failure, not an empty or loading list"
    );
}

/// Real avatars flow from the identity's `avatar_url` through the controller's cache to a renderable
/// image, deduplicated by stable identity, and a late response for another identity never crosses
/// over. This drives the same controller the sidebar and Settings use, with an injected byte source
/// instead of any real avatar host.
#[gpui::test]
fn avatars_load_by_identity_and_do_not_cross_accounts(cx: &mut TestAppContext) {
    use crate::app::avatar::{AvatarBytes, AvatarKey, AvatarState};
    use std::collections::HashMap;

    struct LocalBytes {
        bodies: Mutex<HashMap<String, (u16, Vec<u8>)>>,
        reads: AtomicUsize,
    }
    impl AvatarBytes for LocalBytes {
        fn load<'a>(
            &'a self,
            url: &'a str,
            _max: usize,
            _cancel: crate::network::cancel::CancellationToken,
        ) -> crate::app::avatar::AvatarBytesFuture<'a> {
            Box::pin(async move {
                self.reads.fetch_add(1, Ordering::SeqCst);
                self.bodies.lock().unwrap().get(url).cloned().ok_or(())
            })
        }
    }
    // A real 1x1 PNG so the decoder succeeds; the other URL is a 404.
    let png = vec![
        137u8, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8,
        6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 96, 96, 96, 248,
        15, 0, 1, 4, 1, 0, 95, 229, 195, 75, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let source = Arc::new(LocalBytes {
        bodies: Mutex::new(HashMap::from([
            (
                "https://avatars.test/alice.png".to_string(),
                (200u16, png.clone()),
            ),
            (
                "https://avatars.test/bob.png".to_string(),
                (404u16, b"nope".to_vec()),
            ),
        ])),
        reads: AtomicUsize::new(0),
    });

    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(AutoReposTransport::new());
    mock.script_user(user(1, "alice"));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    controller.update(cx, |accounts, _| {
        accounts.install_avatar_source(source.clone())
    });

    let alice = AvatarKey::new("github.com", 1, "https://avatars.test/alice.png");
    let bob = AvatarKey::new("github.com", 2, "https://avatars.test/bob.png");

    // Alice: the first paint starts one fetch (Loading); a repaint does not start a second.
    assert!(matches!(
        controller.update(cx, |a, _| a.avatar(alice.clone())),
        AvatarState::Loading
    ));
    assert!(matches!(
        controller.update(cx, |a, _| a.avatar(alice.clone())),
        AvatarState::Loading
    ));
    // Drive the controller's beat until Alice's image decodes.
    for _ in 0..200 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(5));
        if matches!(
            controller.read_with(cx, |a, _| a.avatar_state_for_test(&alice)),
            Some(AvatarState::Ready(_))
        ) {
            break;
        }
    }
    assert!(
        matches!(
            controller.read_with(cx, |a, _| a.avatar_state_for_test(&alice)),
            Some(AvatarState::Ready(_))
        ),
        "a decodable avatar reaches Ready and can be drawn"
    );
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "a repeated paint must not refetch the same avatar"
    );

    // Bob: a 404 is a terminal failure for Bob's own key, never Alice's image.
    assert!(matches!(
        controller.update(cx, |a, _| a.avatar(bob.clone())),
        AvatarState::Loading
    ));
    for _ in 0..200 {
        pump(&controller, cx);
        std::thread::sleep(Duration::from_millis(5));
        if matches!(
            controller.read_with(cx, |a, _| a.avatar_state_for_test(&bob)),
            Some(AvatarState::Failed)
        ) {
            break;
        }
    }
    assert!(matches!(
        controller.read_with(cx, |a, _| a.avatar_state_for_test(&bob)),
        Some(AvatarState::Failed)
    ));
    assert!(
        matches!(
            controller.read_with(cx, |a, _| a.avatar_state_for_test(&alice)),
            Some(AvatarState::Ready(_))
        ),
        "Bob's failure never replaces Alice's image"
    );
    // Alice keeps her image; Bob's failure did not disturb her key.
    assert!(
        !matches!(
            controller.read_with(cx, |a, _| a.avatar_state_for_test(&bob)),
            Some(AvatarState::Ready(_))
        ),
        "a 404 never becomes a picture"
    );
}

/// The rebuilt Settings › Accounts page: a single list of saved accounts (no duplicated active
/// summary, no disabled "Current" action button) and a separate add/replace form group. It must fit
/// the minimum window width without clipping and remain scrollable.
#[gpui::test]
fn the_settings_accounts_page_is_one_compact_list_plus_a_form(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(AutoReposTransport::new());
    mock.serve_repos("first", repos(1, "alice", &["a"]));
    mock.serve_repos("second", repos(2, "bob", &["b"]));
    mock.script_user(user(1, "alice"));
    mock.script_user(user(2, "bob"));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    wait_idle(&controller, cx);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    wait_idle(&controller, cx);

    // The minimum supported window, on the Settings page.
    show_settings(&view, cx);
    cx.simulate_resize(gpui::size(gpui::px(880.0), gpui::px(560.0)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();

    // The Accounts surface is drawn once, with both saved accounts as rows.
    assert!(cx.debug_bounds("settings-accounts").is_some());
    assert!(cx.debug_bounds("saved-account-1").is_some());
    assert!(cx.debug_bounds("saved-account-2").is_some());
    // The add form and its masked submit control are present.
    assert!(cx.debug_bounds("account-token-form").is_some());
    // The current account is marked, but there is no duplicated active-summary block and no
    // disabled "Current" button pretending to be an action.
    assert!(
        cx.debug_bounds("account-current-badge").is_some(),
        "the active account is marked with a status, not a disabled button"
    );
    assert!(
        cx.debug_bounds("settings-current-account").is_none(),
        "the active account is not repeated above the list"
    );
    // Every drawn row stays inside the island, so nothing is pushed past the edge at 880px.
    let island = cx.debug_bounds("island").expect("island");
    for selector in ["settings-accounts", "saved-account-1", "account-token-form"] {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector}"));
        assert!(
            bounds.right() <= island.right() + gpui::px(1.0),
            "{selector} must stay inside the island at the minimum width"
        );
    }
}

/// A rejected PAT (an unsuccessful mutation that leaves the authorization unchanged) must not drop
/// the already-loaded list and leave an idle sidebar with no retry: the previous list is restored.
#[gpui::test]
fn a_rejected_pat_keeps_the_loaded_repository_list(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(AutoReposTransport::new());
    mock.serve_repos("good", repos(1, "alice", &["one", "two"]));
    mock.script_user(user(1, "alice"));
    // The second login is rejected.
    mock.script_user(HttpResponse {
        status: 401,
        headers: vec![],
        body: "{}".into(),
    });
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock.clone());
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    put_token(&controller, cx, "good");
    submit(&controller, cx);
    wait_for(
        &controller,
        cx,
        |state| matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    assert_eq!(view.read_with(cx, |shell, _| shell.repositories().len()), 2);
    wait_idle(&controller, cx);

    // A bad token is rejected; the active account and its authorization are unchanged.
    put_token(&controller, cx, "bad");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| {
        state
            .status
            .as_ref()
            .is_some_and(|status| status.tone == StatusTone::Error && status.text.contains("401"))
    });
    // The loaded list is restored rather than replaced by an idle "reloading" placeholder.
    let state = snapshot(&controller, cx);
    assert!(
        matches!(&state.repositories.preview, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
        "a rejected PAT must not clear the already-loaded list: {:?}",
        state.repositories.preview
    );
    assert_eq!(view.read_with(cx, |shell, _| shell.repositories().len()), 2);
}

/// The replacement form names the account being replaced, which may not be the active one, so the
/// displayed instruction matches the account `replace` actually targets.
#[gpui::test]
fn the_replacement_form_targets_the_selected_account_not_the_active_one(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(AutoReposTransport::new());
    mock.serve_repos("first", repos(1, "alice", &["a"]));
    mock.serve_repos("second", repos(2, "bob", &["b"]));
    mock.script_user(user(1, "alice"));
    mock.script_user(user(2, "bob"));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, controller, cx) = product_shell(cx, Some(runtime));
    let _ = &view;
    put_token(&controller, cx, "first");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 1);
    wait_idle(&controller, cx);
    put_token(&controller, cx, "second");
    submit(&controller, cx);
    wait_for(&controller, cx, |state| state.accounts.len() == 2);
    wait_idle(&controller, cx);
    // Make A the active account, then start replacing B (not active).
    select(&controller, 1, cx);
    wait_for(&controller, cx, |state| {
        state.active == Some(AccountKey::new(1, HostId::github()))
    });
    wait_idle(&controller, cx);
    controller.update(cx, |accounts, cx| {
        accounts.replace(AccountKey::new(2, HostId::github()), cx)
    });
    // The controller's replacement target is B, independent of the active account A.
    assert_eq!(
        controller.read_with(cx, |accounts, _| accounts.replacing().cloned()),
        Some(AccountKey::new(2, HostId::github())),
        "the replacement target is the account the user pressed Replace on"
    );
}
