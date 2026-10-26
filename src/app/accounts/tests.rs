use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use gpui::{Entity, KeyUpEvent, Keystroke, TestAppContext, VisualContext, VisualTestContext};

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
        ely_gpui_component::init_for_tests(cx);
        crate::platform::fonts::register(cx).expect("test fonts");
    });
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

fn preview(
    cx: &mut TestAppContext,
    runtime: AccountRuntime,
) -> (Entity<AccountPreview>, &mut VisualTestContext) {
    setup(cx);
    cx.add_window_view(|window, cx| AccountPreview::with_runtime(runtime, window, cx))
}

fn wait_for(
    view: &Entity<AccountPreview>,
    cx: &mut VisualTestContext,
    condition: impl Fn(&Presentation) -> bool,
) {
    for _ in 0..400 {
        view.update(cx, |view, cx| view.drain(cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        if view.read_with(cx, |view, _| condition(&view.presentation)) {
            return;
        }
    }
    let presentation = view.read_with(cx, |view, _| view.presentation.clone());
    panic!("preview did not settle: {presentation:?}");
}

fn put_token(view: &Entity<AccountPreview>, cx: &mut VisualTestContext, token: &str) {
    view.update(cx, |view, cx| {
        view.input.update(cx, |input, cx| input.set_text(token, cx));
    });
}

fn submit(view: &Entity<AccountPreview>, cx: &mut VisualTestContext) {
    cx.update_window_entity(view, |view, window, cx| view.submit(window, cx));
}

#[gpui::test]
fn pat_submit_is_masked_cleared_and_real_repos_reach_gpui(cx: &mut TestAppContext) {
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
    let (view, cx) = preview(cx, runtime);
    assert!(view.read_with(cx, |view, cx| view.input.read(cx).is_masked()));
    put_token(&view, cx, "synthetic-never-real");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, cx| view.input.read(cx).is_empty()));
    wait_for(&view, cx, |state| state.accounts.len() == 1);
    assert_eq!(view.read_with(cx, |view, _| view.generation), 1);
    // A duplicate Enter/click while idle with an empty field cannot submit.
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |view, _| view.generation), 1);
    view.update(cx, |view, cx| view.refresh(cx));
    wait_for(
        &view,
        cx,
        |state| matches!(&state.repositories, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let debug = view.read_with(cx, |view, _| format!("{:?}", view.presentation));
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
    let (view, cx) = preview(cx, runtime);
    put_token(&view, cx, "first");
    submit(&view, cx);
    wait_for(&view, cx, |state| state.accounts.len() == 1);
    put_token(&view, cx, "second");
    submit(&view, cx);
    wait_for(&view, cx, |state| state.accounts.len() == 2);
    let first = AccountKey::new(1, HostId::github());
    view.update(cx, |view, cx| view.select(first.clone(), cx));
    wait_for(&view, cx, |state| state.active == Some(first.clone()));
    view.update(cx, |view, cx| view.replace(first.clone(), cx));
    put_token(&view, cx, "replacement");
    submit(&view, cx);
    wait_for(&view, cx, |state| {
        state
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("Replacement PAT verified"))
    });
    view.update(cx, |view, cx| view.revalidate(first, cx));
    wait_for(&view, cx, |state| {
        state
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("revalidated"))
    });
    let runtime = view.read_with(cx, |view, _| view.runtime.clone().unwrap());
    host.block_on(runtime.accounts.close()).unwrap();
    drop(runtime);
    // Release the old view before reopening the SQLite lease.
    view.update(cx, |view, _| {
        view.presentation.accounts.clear();
        if let Some(runtime) = view.runtime.take() {
            runtime.shutdown();
        }
    });
    let (restored, _) = open(&path, store, mock);
    let (restored_view, cx) = preview(cx, restored);
    assert!(restored_view.read_with(cx, |view, _| {
        view.presentation.accounts.len() == 2
            && view.presentation.active == Some(AccountKey::new(1, HostId::github()))
            && view
                .presentation
                .accounts
                .iter()
                .all(|summary| summary.authorization == AuthorizationState::PendingVerification)
    }));
    restored_view.update(cx, |view, cx| view.refresh(cx));
    wait_for(
        &restored_view,
        cx,
        |state| matches!(&state.repositories, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 1),
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
    let (view, cx) = preview(cx, runtime);
    put_token(&view, cx, "synthetic");
    submit(&view, cx);
    wait_for(&view, cx, |state| state.accounts.len() == 1);
    view.update(cx, |view, cx| view.refresh(cx));
    wait_for(&view, cx, |state| {
        matches!(state.repositories, RepositoryPreview::Failed(_))
    });
    assert!(view.read_with(cx, |view, _| {
        view.presentation.accounts[0].authorization == AuthorizationState::Verified
    }));
    view.update(cx, |view, cx| view.refresh(cx));
    wait_for(
        &view,
        cx,
        |state| matches!(&state.repositories, RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 1),
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
    let (view, cx) = preview(cx, runtime);
    put_token(&view, cx, "synthetic");
    submit(&view, cx);
    wait_for(&view, cx, |state| state.accounts.len() == 1);
    view.update(cx, |view, cx| view.refresh(cx));
    let first_generation = view.read_with(cx, |view, _| view.generation);
    view.update(cx, |view, cx| view.cancel(cx));
    view.update(cx, |view, cx| view.refresh(cx));
    let second_generation = view.read_with(cx, |view, _| view.generation);
    assert!(second_generation > first_generation);
    // The new operation completes while the older request is still blocked.
    second_gate.release();
    wait_for(
        &view,
        cx,
        |state| matches!(&state.repositories, RepositoryPreview::Ready(outcome) if outcome.repositories[0].name == "new"),
    );
    // Releasing the older request afterwards must not overwrite the newer view.
    first_gate.release();
    for _ in 0..50 {
        view.update(cx, |view, cx| view.drain(cx));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(view.read_with(cx, |view, _| {
        matches!(&view.presentation.repositories, RepositoryPreview::Ready(outcome) if outcome.repositories[0].name == "new")
            && !format!("{:?}", view.presentation).contains("alice/old")
    }));
    // Duplicate submissions while busy do not advance the operation.
    put_token(&view, cx, "duplicate-ignored");
    let generation = view.read_with(cx, |view, _| view.generation);
    view.update(cx, |view, cx| view.refresh(cx));
    view.update(cx, |view, cx| view.refresh(cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.generation),
        generation + 1
    );
    view.update(cx, |view, cx| view.cancel(cx));
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
    let (view, cx) = preview(cx, runtime);
    put_token(&view, cx, "synthetic");
    submit(&view, cx);
    wait_for(&view, cx, |state| state.accounts.len() == 1);
    view.update(cx, |view, cx| view.refresh(cx));
    wait_for(&view, cx, |state| {
        matches!(
            &state.repositories,
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
    let (view, cx) = preview(cx, runtime);
    put_token(&view, cx, "first");
    submit(&view, cx);
    wait_for(&view, cx, |state| state.accounts.len() == 1);
    put_token(&view, cx, "second");
    submit(&view, cx);
    wait_for(&view, cx, |state| state.accounts.len() == 2);
    view.update(cx, |view, cx| view.refresh(cx));
    let runtime = view.read_with(cx, |view, _| view.runtime.clone().unwrap());
    host_runtime()
        .block_on(
            runtime
                .accounts
                .set_active(AccountKey::new(1, HostId::github())),
        )
        .unwrap();
    gate.release();
    for _ in 0..80 {
        view.update(cx, |view, cx| view.drain(cx));
        std::thread::sleep(Duration::from_millis(5));
    }
    let presentation = view.read_with(cx, |view, _| view.presentation.clone());
    assert!(
        !format!("{presentation:?}").contains("old-account"),
        "a result for the previous active account must not be published"
    );
    assert!(
        matches!(presentation.repositories, RepositoryPreview::Idle(_)),
        "a discarded read must not leave the preview stuck in loading"
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
    let (view, cx) = preview(cx, runtime);
    put_token(&view, cx, "synthetic");
    submit(&view, cx);
    wait_for(&view, cx, |state| state.accounts.len() == 1);
    view.update(cx, |view, cx| view.refresh(cx));
    let before = view.read_with(cx, |view, _| view.presentation.clone());
    view.update(cx, |view, _| {
        let _ = view.alive.send(false);
        if let Some(operation) = view.operation.take() {
            operation.source.cancel();
        }
        if let Some(runtime) = view.runtime.take() {
            runtime.shutdown();
        }
    });
    gate.release();
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(5));
    }
    // A worker that finishes after teardown has nowhere to publish a result.
    view.update(cx, |view, cx| view.drain(cx));
    assert!(view.read_with(cx, |view, _| {
        view.inbox.lock().unwrap().is_empty() && view.presentation == before
    }));
}

#[gpui::test]
fn tab_reaches_the_submit_button_and_enter_submits(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockTransport::new());
    host_runtime().block_on(mock.enqueue(Ok(user(1, "alice"))));
    let store = Arc::new(InMemoryCredentialStore::new());
    let (runtime, _) = open(&temp.path().join("accounts.sqlite3"), store, mock);
    let (view, cx) = preview(cx, runtime);
    put_token(&view, cx, "synthetic");
    // Password field, visibility toggle, then submit in Ely's focus order.
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("enter");
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").unwrap(),
    });
    wait_for(&view, cx, |state| state.accounts.len() == 1);
}
