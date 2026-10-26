//! GPUI bridge for the opt-in real account preview.
//!
//! UI callbacks create short-lived operations. All service calls run on the
//! existing Tokio executor; results return through a channel drained by
//! `cx.spawn`. A monotonic generation and `RepositoryReadContext` protect both
//! successful and failed responses, including cancellation followed by the same
//! kind of operation.

mod runtime;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ely_gpui_component::{forms::TextInput, theme::ActiveTheme};
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, Styled, Subscription, Task, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use tokio::sync::watch;

use crate::auth::token::Pat;
use crate::data::{AccountError, LoginError, LoginOutcome, RepositoryReadContext};
use crate::domain::error::GitHubError;
use crate::domain::{AccountKey, AccountSummary, HostId};
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::ui::accounts::{
    AccountCallbacks, AccountsView, RepositoryPreview, StatusMessage, StatusTone,
};

use runtime::AccountRuntime;

#[cfg(test)]
mod tests;

const PAT_HELP: &str = "https://github.com/settings/personal-access-tokens/new?type=classic";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Add,
    Replace(u64),
    Select(u64),
    Revalidate(u64),
    Repositories,
}

struct Reply {
    generation: u64,
    operation: Operation,
    result: Result<Success, String>,
    accounts: Vec<AccountSummary>,
    active: Option<AccountKey>,
    /// Captured before a repository request. Only repository results are
    /// validated against it: account mutations intentionally advance the epoch.
    publication: Option<RepositoryReadContext>,
    recovery_pending: bool,
    repositories: Option<Result<crate::domain::repo::ReposOutcome, GitHubError>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Success {
    Added,
    Replaced,
    Selected,
    Revalidated,
    Repositories,
    CleanupPending,
}

enum Message {
    Opened(Result<Arc<AccountRuntime>, String>),
    Operation(Reply),
    /// The cancelled operation's background future ended. A durable account
    /// commit may already have happened, so the view reconciles the
    /// authoritative directory without publishing the cancelled reply.
    Settled(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Presentation {
    accounts: Vec<AccountSummary>,
    active: Option<AccountKey>,
    repositories: RepositoryPreview,
    status: Option<StatusMessage>,
}

#[derive(Clone)]
struct OperationControl {
    token: CancellationToken,
    source: CancellationTokenSource,
}

fn control() -> OperationControl {
    let (token, source) = CancellationToken::new();
    OperationControl { token, source }
}

pub(crate) struct AccountPreview {
    runtime: Option<Arc<AccountRuntime>>,
    input: Entity<TextInput>,
    root_focus: FocusHandle,
    scroll: ScrollHandle,
    inbox: Arc<Mutex<VecDeque<Message>>>,
    alive: Arc<watch::Sender<bool>>,
    operation: Option<OperationControl>,
    generation: u64,
    current_operation: Option<Operation>,
    reconciling: Option<u64>,
    replacing: Option<AccountKey>,
    presentation: Presentation,
    receiver_task: Option<Task<()>>,
    _keys: Subscription,
}

impl AccountPreview {
    #[cfg(test)]
    fn with_runtime(runtime: AccountRuntime, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(Some(Arc::new(runtime)), window, cx)
    }

    pub fn new(
        runtime: Option<Arc<AccountRuntime>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = Self::make_input(window, cx);
        window.focus(&input.read(cx).focus_handle(cx), cx);
        let root_focus = cx.focus_handle();
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let (alive, _) = watch::channel(true);
        let this = cx.entity();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            this.update(cx, |this, cx| this.on_key(event, window, cx));
        });
        let mut this = Self {
            runtime,
            input,
            root_focus,
            scroll: ScrollHandle::new(),
            inbox: inbox.clone(),
            alive: Arc::new(alive),
            operation: None,
            generation: 0,
            current_operation: None,
            reconciling: None,
            replacing: None,
            presentation: Presentation {
                accounts: Vec::new(),
                active: None,
                repositories: RepositoryPreview::Idle(
                    "Restore the saved directory, then refresh to read repositories.".into(),
                ),
                status: None,
            },
            receiver_task: None,
            _keys: keys,
        };
        this.start_receiver(cx);
        if this.runtime.is_none() {
            this.open(cx);
        } else {
            this.refresh_presentation(cx);
        }
        this
    }

    fn make_input(window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        cx.new(|cx| {
            TextInput::new(window, cx)
                .masked()
                .label("Classic personal access token")
                .placeholder("Paste a Classic PAT")
        })
    }

    fn reset_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A new entity also drops old edit/undo snapshots. The secret is never
        // copied into Presentation, operation replies, diagnostics or Debug.
        self.input = Self::make_input(window, cx);
        window.focus(&self.input.read(cx).focus_handle(cx), cx);
    }

    fn on_key(
        &mut self,
        event: &gpui::KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key != "enter"
            || !self.input.read(cx).focus_handle(cx).is_focused(window)
            || self.operation.is_some()
        {
            return;
        }
        let composing = self.input.update(cx, |input, cx| {
            gpui::EntityInputHandler::marked_text_range(input, window, cx).is_some()
        });
        if composing {
            return;
        }
        cx.stop_propagation();
        self.submit(window, cx);
    }

    /// Background work only enqueues; the foreground drains on its own clock.
    /// This keeps every entity update on the GPUI thread, never blocks render
    /// on I/O, and avoids scheduler assumptions about cross-thread wakeups.
    fn start_receiver(&mut self, cx: &mut Context<Self>) {
        self.receiver_task = Some(cx.spawn(async |this: gpui::WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let alive = this
                    .update(cx, |this, _| *this.alive.borrow())
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                this.update(cx, |this, cx| this.drain(cx)).ok();
            }
        }));
    }

    fn drain(&mut self, cx: &mut Context<Self>) {
        let messages: Vec<Message> = {
            let mut inbox = self.inbox.lock().expect("preview inbox");
            inbox.drain(..).collect()
        };
        for message in messages {
            self.receive(message, cx);
        }
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        self.presentation.status = Some(StatusMessage::new(
            StatusTone::Info,
            "Opening saved account services…",
        ));
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        std::thread::spawn(move || {
            let result = open_production_blocking().map(Arc::new);
            enqueue(&inbox, &alive, Message::Opened(result));
        });
        cx.notify();
    }

    fn receive(&mut self, message: Message, cx: &mut Context<Self>) {
        match message {
            Message::Opened(result) => {
                if self.generation != 0 || self.runtime.is_some() {
                    return;
                }
                match result {
                    Ok(runtime) => {
                        self.runtime = Some(runtime);
                        self.refresh_presentation(cx);
                        self.presentation.status = Some(StatusMessage::new(
                            StatusTone::Info,
                            "Saved account directory restored. Verification is pending until GitHub confirms it.",
                        ));
                    }
                    Err(message) => {
                        self.presentation.status =
                            Some(StatusMessage::new(StatusTone::Error, message));
                    }
                }
                cx.notify();
            }
            Message::Operation(reply) => self.apply(reply, cx),
            Message::Settled(generation) => self.reconcile(generation, cx),
        }
    }

    /// Re-read the authoritative account directory after a cancelled mutation
    /// settles. Repository results are never taken from the cancelled request.
    fn reconcile(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.reconciling != Some(generation) {
            return;
        }
        self.reconciling = None;
        self.refresh_presentation(cx);
        self.presentation.repositories = RepositoryPreview::Idle(
            "The cancelled request has settled. Review the current saved accounts, then refresh to read repositories."
                .into(),
        );
        self.presentation.status = Some(StatusMessage::new(
            StatusTone::Info,
            "Cancellation settled. Saved accounts and the current selection are shown as stored; an already committed change was not rolled back.",
        ));
        cx.notify();
    }

    fn refresh_presentation(&mut self, cx: &mut Context<Self>) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        self.presentation.accounts = runtime.accounts.summaries();
        self.presentation.active = runtime.accounts.active_context().map(|c| c.account);
        cx.notify();
    }

    fn apply(&mut self, reply: Reply, cx: &mut Context<Self>) {
        if self.generation != reply.generation || self.operation.is_none() {
            return;
        }
        self.operation = None;
        self.current_operation = None;
        self.input
            .update(cx, |input, cx| input.set_disabled(false, cx));
        if let (Some(publication), Some(runtime)) = (&reply.publication, &self.runtime)
            && !runtime.accounts.repo_service().is_current(publication)
        {
            // The authorization context moved between the request and this
            // reply. Nothing from the old account may be published.
            if reply.operation == Operation::Repositories {
                self.presentation.repositories = RepositoryPreview::Idle(
                    "The active account changed while this read was running; its result was discarded. Refresh to read the new account."
                        .into(),
                );
            }
            self.presentation.status = Some(StatusMessage::new(
                StatusTone::Warning,
                "The account or authorization changed while this request ran; its result was discarded.",
            ));
            cx.notify();
            return;
        }
        self.presentation.accounts = reply.accounts;
        self.presentation.active = reply.active;
        match reply.result {
            Ok(success) => {
                self.presentation.status = Some(match success {
                    Success::Added => StatusMessage::new(
                        StatusTone::Success,
                        "Account verified and saved; its PAT is in the platform credential store.",
                    ),
                    Success::Replaced => StatusMessage::new(
                        StatusTone::Success,
                        "Replacement PAT verified and saved.",
                    ),
                    Success::Selected => {
                        StatusMessage::new(StatusTone::Success, "Current account selection saved.")
                    }
                    Success::Revalidated => StatusMessage::new(
                        StatusTone::Success,
                        "Saved credential revalidated against the current GitHub identity.",
                    ),
                    Success::Repositories => StatusMessage::new(
                        StatusTone::Info,
                        "Repository response received; completeness is shown below.",
                    ),
                    Success::CleanupPending => StatusMessage::new(
                        StatusTone::Warning,
                        "The account change was committed, but retired credential cleanup is pending. Do not retry the same change blindly.",
                    ),
                });
                if matches!(success, Success::Added | Success::Replaced) {
                    self.replacing = None;
                }
                if success == Success::Repositories {
                    self.presentation.repositories = match reply.repositories {
                        Some(Ok(outcome)) => RepositoryPreview::Ready(outcome),
                        Some(Err(error)) => RepositoryPreview::Failed(error_text(&error)),
                        None => RepositoryPreview::Failed(
                            "The repository response was not returned.".into(),
                        ),
                    };
                } else {
                    self.presentation.repositories = RepositoryPreview::Idle(
                        "Authorization or current account changed. Refresh to read the new repository set."
                            .into(),
                    );
                }
            }
            Err(message) => {
                self.presentation.status = Some(StatusMessage::new(StatusTone::Error, message));
                if reply.operation == Operation::Repositories {
                    self.presentation.repositories = RepositoryPreview::Failed(
                        reply
                            .repositories
                            .and_then(|result| result.err())
                            .map(|error| error_text(&error))
                            .unwrap_or_else(|| {
                                "The request failed; no successful empty list is implied.".into()
                            }),
                    );
                } else {
                    // An errored mutation may still have committed: for example
                    // `CleanupPending` is returned after the directory and active
                    // selection were already persisted. Repository data read
                    // under the previous account must not stay on screen.
                    self.presentation.repositories = RepositoryPreview::Idle(
                        "An account change was attempted. Refresh to read the current account's repositories."
                            .into(),
                    );
                }
            }
        }
        if reply.recovery_pending {
            self.presentation.status = Some(StatusMessage::new(
                StatusTone::Warning,
                "The account change was committed, but retired credential cleanup remains pending.",
            ));
        }
        cx.notify();
    }

    fn start(&mut self, operation: Operation, cx: &mut Context<Self>) {
        if self.operation.is_some() {
            return;
        }
        let Some(runtime) = self.runtime.clone() else {
            self.presentation.status = Some(StatusMessage::new(
                StatusTone::Error,
                "Account services are not ready.",
            ));
            cx.notify();
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.current_operation = Some(operation);
        let control = control();
        self.operation = Some(control.clone());
        self.presentation.status = Some(StatusMessage::new(
            StatusTone::Info,
            operation_label(operation),
        ));
        if operation == Operation::Repositories {
            self.presentation.repositories = RepositoryPreview::Loading;
        }
        let publication = if operation == Operation::Repositories {
            runtime
                .accounts
                .repo_service()
                .session()
                .ok()
                .map(|session| session.context().clone())
        } else {
            None
        };
        self.input
            .update(cx, |input, cx| input.set_disabled(true, cx));
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let accounts = runtime.accounts.clone();
        let settled = settled_signal(self.inbox.clone(), self.alive.clone(), generation);
        let spawned = runtime.spawn(control.source, settled, async move {
            let (result, repositories) = match operation {
                Operation::Select(id) => (
                    accounts
                        .set_active(AccountKey::new(id, HostId::github()))
                        .await
                        .map(|_| Success::Selected)
                        .map_err(|error| account_error(&error)),
                    None,
                ),
                Operation::Revalidate(id) => (
                    // The existing data-layer revalidate has no cancellable
                    // variant and always commits its runtime evidence. Cancelling
                    // therefore stops publication, not the operation: the reply is
                    // discarded and `Settled` reconciles authoritative state.
                    accounts
                        .revalidate(AccountKey::new(id, HostId::github()))
                        .await
                        .map(|_| Success::Revalidated)
                        .map_err(|error| login_error(&error)),
                    None,
                ),
                Operation::Repositories => {
                    let repos = accounts.repo_service();
                    let result = repos.list_repos_cancellable(control.token).await;
                    let mapped = match &result {
                        Ok(_) => Ok(Success::Repositories),
                        Err(error) => Err(error_text(error)),
                    };
                    (mapped, Some(result))
                }
                Operation::Add | Operation::Replace(_) => unreachable!("token path"),
            };
            send_reply(
                &accounts,
                &inbox,
                &alive,
                Reply {
                    generation,
                    operation,
                    result,
                    accounts: Vec::new(),
                    active: None,
                    publication,
                    recovery_pending: false,
                    repositories,
                },
            )
            .await;
        });
        if spawned.is_err() {
            self.operation = None;
            self.current_operation = None;
            self.input
                .update(cx, |input, cx| input.set_disabled(false, cx));
            self.presentation.status = Some(StatusMessage::new(
                StatusTone::Error,
                "Background services are closing; the operation was not submitted.",
            ));
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation.is_some() {
            return;
        }
        let mut value = self.input.read(cx).text().trim().to_string();
        if value.is_empty() {
            self.presentation.status = Some(StatusMessage::new(
                StatusTone::Error,
                "Paste a Classic PAT before submitting.",
            ));
            cx.notify();
            return;
        }
        let operation = match &self.replacing {
            Some(key) => Operation::Replace(key.id),
            None => Operation::Add,
        };
        let Some(runtime) = self.runtime.clone() else {
            self.presentation.status = Some(StatusMessage::new(
                StatusTone::Error,
                "Account services are not ready.",
            ));
            cx.notify();
            return;
        };
        self.reset_input(window, cx);
        self.generation += 1;
        let generation = self.generation;
        self.current_operation = Some(operation);
        let control = control();
        self.operation = Some(control.clone());
        self.input
            .update(cx, |input, cx| input.set_disabled(true, cx));
        self.presentation.status = Some(StatusMessage::new(
            StatusTone::Info,
            operation_label(operation),
        ));
        let pat = Pat::new(std::mem::take(&mut value));
        let accounts = runtime.accounts.clone();
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let settled = settled_signal(self.inbox.clone(), self.alive.clone(), generation);
        let spawned = runtime.spawn(control.source.clone(), settled, async move {
            let pending = async {
                match operation {
                    Operation::Add => accounts.login_cancellable(pat, control.token.clone()).await,
                    Operation::Replace(id) => {
                        accounts
                            .replace(AccountKey::new(id, HostId::github()), pat)
                            .await
                    }
                    _ => unreachable!(),
                }
            };
            // Cancelling before a durable commit drops the request future. Once
            // the service submits its blocking commit, that commit finishes.
            let result = tokio::select! {
                biased;
                _ = control.token.clone().cancelled() => Err(LoginError::GitHub(GitHubError::Cancelled)),
                result = pending => result,
            };
            let result = match result {
                // The outcome, not the requested intent, decides what the user
                // is told: re-adding an existing identity is a replacement.
                Ok(LoginOutcome::SignedIn(_)) => Ok(Success::Added),
                Ok(LoginOutcome::Replaced(_)) => Ok(Success::Replaced),
                Err(LoginError::CleanupPending(_)) => Ok(Success::CleanupPending),
                Err(error) => Err(login_error(&error)),
            };
            send_reply(
                &accounts,
                &inbox,
                &alive,
                Reply {
                    generation,
                    operation,
                    result,
                    accounts: Vec::new(),
                    active: None,
                    publication: None,
                    recovery_pending: false,
                    repositories: None,
                },
            )
            .await;
        });
        if spawned.is_err() {
            self.operation = None;
            self.current_operation = None;
            self.input
                .update(cx, |input, cx| input.set_disabled(false, cx));
            self.presentation.status = Some(StatusMessage::new(
                StatusTone::Error,
                "The operation was not submitted because background services are closing.",
            ));
        }
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(operation) = self.operation.take() {
            operation.source.cancel();
            // Account mutations may already have submitted a durable commit, so
            // their authoritative state is reconciled when they settle. Their
            // own reply stays discarded because `operation` is now None.
            if !matches!(self.current_operation, Some(Operation::Repositories) | None) {
                self.reconciling = Some(self.generation);
            }
        }
        self.presentation.status = Some(StatusMessage::new(
            StatusTone::Info,
            "Cancellation requested. An in-flight account operation still settles in the service; saved accounts are reconciled when it does, and the preview will not claim it was rolled back.",
        ));
        self.presentation.repositories = RepositoryPreview::Idle(
            "Cancelled preview request; no new repository result was applied.".into(),
        );
        self.input
            .update(cx, |input, cx| input.set_disabled(false, cx));
        cx.notify();
    }

    fn select(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        self.replacing = None;
        self.start(Operation::Select(key.id), cx);
    }

    fn cancel_replace(&mut self, cx: &mut Context<Self>) {
        self.replacing = None;
        self.presentation.status = Some(StatusMessage::new(
            StatusTone::Info,
            "Replacement cancelled. The saved PAT is unchanged.",
        ));
        cx.notify();
    }

    fn replace(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        self.replacing = Some(key);
        self.presentation.status = Some(StatusMessage::new(
            StatusTone::Info,
            "Enter a PAT for this same account. A different identity will be rejected.",
        ));
        cx.notify();
    }

    fn revalidate(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        self.start(Operation::Revalidate(key.id), cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.start(Operation::Repositories, cx);
    }
}

impl Render for AccountPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let submit = this.clone();
        let cancel = this.clone();
        let select = this.clone();
        let replace = this.clone();
        let revalidate = this.clone();
        let refresh = this.clone();
        let cancel_replace = this;
        let callbacks = AccountCallbacks {
            submit: std::rc::Rc::new(move |window, cx| {
                submit.update(cx, |this, cx| this.submit(window, cx))
            }),
            cancel: std::rc::Rc::new(move |_, cx| cancel.update(cx, |this, cx| this.cancel(cx))),
            select: std::rc::Rc::new(move |key, _, cx| {
                select.update(cx, |this, cx| this.select(key, cx))
            }),
            replace: std::rc::Rc::new(move |key, _, cx| {
                replace.update(cx, |this, cx| this.replace(key, cx))
            }),
            revalidate: std::rc::Rc::new(move |key, _, cx| {
                revalidate.update(cx, |this, cx| this.revalidate(key, cx))
            }),
            refresh: std::rc::Rc::new(move |_, cx| refresh.update(cx, |this, cx| this.refresh(cx))),
            cancel_replace: std::rc::Rc::new(move |_, cx| {
                cancel_replace.update(cx, |this, cx| this.cancel_replace(cx))
            }),
            open_pat_help: std::rc::Rc::new(|_, cx| cx.open_url(PAT_HELP)),
        };
        div()
            .track_focus(&self.root_focus)
            .size_full()
            .bg(cx.theme().colors.bg)
            .child(
                div()
                    .id("account-preview-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(crate::ui::accounts::render(
                        AccountsView {
                            token_input: self.input.clone(),
                            replacing: self.replacing.is_some(),
                            busy: self.operation.is_some(),
                            status: self.presentation.status.clone(),
                            accounts: self.presentation.accounts.clone(),
                            active: self.presentation.active.clone(),
                            repositories: self.presentation.repositories.clone(),
                            callbacks,
                        },
                        cx,
                    )),
            )
    }
}

impl Drop for AccountPreview {
    fn drop(&mut self) {
        let _ = self.alive.send(false);
        if let Some(operation) = self.operation.take() {
            operation.source.cancel();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown();
        }
    }
}

async fn send_reply(
    accounts: &Arc<crate::data::AccountService>,
    inbox: &Arc<Mutex<VecDeque<Message>>>,
    alive: &watch::Sender<bool>,
    mut reply: Reply,
) {
    reply.accounts = accounts.summaries();
    reply.active = accounts.active_context().map(|context| context.account);
    reply.recovery_pending = accounts.recovery_pending().await.unwrap_or(true);
    enqueue(inbox, alive, Message::Operation(reply));
}

/// Completion signal for one background operation. It runs on the network
/// thread and only pushes a marker; the foreground drain does the UI work.
fn settled_signal(
    inbox: Arc<Mutex<VecDeque<Message>>>,
    alive: Arc<watch::Sender<bool>>,
    generation: u64,
) -> impl FnOnce() + Send + 'static {
    move || enqueue(&inbox, &alive, Message::Settled(generation))
}

fn enqueue(inbox: &Arc<Mutex<VecDeque<Message>>>, alive: &watch::Sender<bool>, message: Message) {
    if *alive.borrow() {
        inbox.lock().expect("preview inbox").push_back(message);
    }
}

fn open_production_blocking() -> Result<AccountRuntime, String> {
    // Production startup runs on its own Tokio runtime thread, then keeps the
    // resulting executor for all later operations.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not start the account runtime.".to_string())
            .and_then(|runtime| {
                runtime
                    .block_on(runtime::open_production())
                    .map(|(open, _)| open)
            });
        let _ = tx.send(result);
    });
    rx.recv()
        .map_err(|_| "Account startup did not return.".to_string())?
}

fn operation_label(operation: Operation) -> &'static str {
    match operation {
        Operation::Add => "Verifying the PAT before saving…",
        Operation::Replace(_) => "Verifying the replacement PAT…",
        Operation::Select(_) => "Saving the current account selection…",
        Operation::Revalidate(_) => "Revalidating the saved credential…",
        Operation::Repositories => "Reading all visible repository pages…",
    }
}

fn account_error(error: &AccountError) -> String {
    match error {
        AccountError::NotFound => "That saved account no longer exists.".into(),
        AccountError::CleanupPending => {
            "The selection was saved, but retired credential cleanup is still pending.".into()
        }
        other => format!("Account storage operation failed: {other:?}"),
    }
}

fn login_error(error: &LoginError) -> String {
    match error {
        LoginError::GitHub(error) => error_text(error),
        LoginError::CredentialSave(error) => {
            format!("The PAT was not saved: {error}. The account directory was not changed.")
        }
        LoginError::IdentityMismatch { .. } => {
            "This PAT belongs to a different account. Neither account was changed.".into()
        }
        LoginError::NotFound => "That saved account no longer exists.".into(),
        LoginError::Storage(error) => {
            format!("Account storage failed: {error}. No successful save is implied.")
        }
        LoginError::CleanupPending(_) => {
            "The change was committed, but retired credential cleanup is pending.".into()
        }
    }
}

fn error_text(error: &GitHubError) -> String {
    match error {
        GitHubError::Unauthorized => "GitHub rejected this credential (401). Replace or revalidate the PAT; the account was not removed.".into(),
        GitHubError::RateLimited { retry } => format!("GitHub rate limited this request ({:?}). Retry after {:?} seconds; this is not an empty repository set.", retry.kind, retry.seconds),
        GitHubError::SsoRequired { authorization_url } => format!("Organization SSO authorization is required. Authorize the PAT on GitHub{} and retry.", authorization_url.as_deref().map(|url| format!(": {url}")).unwrap_or_default()),
        GitHubError::Forbidden => "GitHub denied access (403). The token scope or organization policy may be insufficient; no empty result is inferred.".into(),
        GitHubError::NotFound => "The resource is absent or not visible to this account (404).".into(),
        GitHubError::Network { .. } => "The network request failed. Check connectivity and retry; no repository result was established.".into(),
        GitHubError::Timeout { seconds } => format!("The request timed out after {seconds}s. Retry when ready."),
        GitHubError::Cancelled => "The request was cancelled; no result was applied.".into(),
        GitHubError::StaleContext => "The account or authorization changed while this request was running; its result was discarded.".into(),
        GitHubError::CredentialStore { .. } => "The platform credential is missing or unavailable. Revalidate or replace the PAT; this is not an empty repository set.".into(),
        other => format!("GitHub read failed: {other}"),
    }
}

/// Open only the opt-in preview, with the platform title bar. The normal Shell
/// startup path remains untouched.
pub(crate) fn open_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1120.0), px(850.0)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(760.0), px(540.0))),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(SharedString::from("Githux · Account wiring preview")),
                ..Default::default()
            }),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| AccountPreview::new(None, window, cx)),
    )
    .expect("failed to open account preview");
}
