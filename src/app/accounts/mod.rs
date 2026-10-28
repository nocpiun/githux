//! Account and repository-read controller for the product Shell.
//!
//! The controller is the one place that turns UI callbacks into account-service calls. Every
//! service call runs on the existing Tokio executor owned by [`runtime::AccountRuntime`]; results
//! come back through a lock-protected queue that a GPUI task drains on a light 50ms beat, so no
//! I/O and no `block_on` ever runs on the UI thread.
//!
//! Two independent guards protect publication:
//!
//! * A monotonic operation generation, so a cancelled or superseded operation can never publish
//!   its own reply — including cancellation followed by the same kind of operation.
//! * A [`RepositoryReadContext`] captured before a repository read and rechecked with
//!   [`crate::data::RepoService::is_current`] before its success *or* error is published. Account
//!   mutations deliberately advance the authorization epoch, so only reads are validated this
//!   way, and a cancelled mutation is reconciled from authoritative service state instead.
//!
//! The controller also owns the repository list state of the active account. A repository result
//! is only ever published for the account that requested it, and every account change invalidates
//! the previous list *before* new content can appear.

pub(crate) mod runtime;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ely_core::forms::{InputEvent, TextInput};
use gpui::{App, AppContext, Context, Entity, FocusHandle, Focusable, Subscription, Task, Window};
use tokio::sync::watch;

use crate::app::avatar::{
    AvatarCache, AvatarFetcher, AvatarInbox, AvatarKey, AvatarRequest, AvatarState, run_fetch,
};
use crate::auth::token::Pat;
use crate::data::{AccountError, LoginError, LoginOutcome, RepositoryReadContext};
use crate::domain::error::GitHubError;
use crate::domain::repo::ReposOutcome;
use crate::domain::{AccountKey, AccountSummary, HostId, RepoSummary};
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::ui::accounts::{RepositoryPreview, StatusMessage, StatusTone};

use runtime::AccountRuntime;

#[cfg(test)]
mod tests;

pub(crate) const PAT_HELP: &str =
    "https://github.com/settings/personal-access-tokens/new?type=classic";

/// What the account UI currently shows for one account's repository access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Repositories {
    /// The account this state belongs to. A list is never published under another account.
    pub account: Option<AccountKey>,
    pub preview: RepositoryPreview,
}

impl Repositories {
    fn idle(account: Option<AccountKey>, message: impl Into<String>) -> Self {
        Self {
            account,
            preview: RepositoryPreview::Idle(message.into()),
        }
    }

    /// The rows that can be opened right now. A partial list is still usable; a failure, a loading
    /// state or an idle state exposes no rows at all, so a failed read cannot become a successful
    /// empty list.
    pub fn rows(&self) -> &[RepoSummary] {
        match &self.preview {
            RepositoryPreview::Ready(outcome) => &outcome.repositories,
            _ => &[],
        }
    }

    /// Number of rows, for the `Repositories (N)` heading.
    pub fn len(&self) -> usize {
        self.rows().len()
    }
}

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
    /// Captured before a repository request. Only repository results are validated against it:
    /// account mutations intentionally advance the epoch.
    publication: Option<RepositoryReadContext>,
    /// Account identity captured before a repository request, so a reply cannot be attributed to
    /// a different account even when the read context comparison alone would be inconclusive.
    for_account: Option<AccountKey>,
    /// Global journalled cleanup observed after this reply. Reported separately and never rewrites
    /// this operation's own outcome.
    recovery_pending: bool,
    repositories: Option<Result<ReposOutcome, GitHubError>>,
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
    /// A cancelled operation's background future ended. A durable commit may already have
    /// happened, so the foreground reconciles authoritative state without publishing its reply.
    Settled(u64),
    /// The directory keeps journalled secret cleanup that has not completed. A directory-level
    /// condition, never evidence that some operation committed.
    CleanupPending,
}

/// Everything the account surfaces need about the current service state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub accounts: Vec<AccountSummary>,
    pub active: Option<AccountKey>,
    pub repositories: Repositories,
    pub status: Option<StatusMessage>,
}

impl Snapshot {
    /// The state when no account controller is installed at all. Never used to claim signed-in
    /// access; it exists so a Shell built without the application composition still draws an
    /// honest logged-out surface instead of fixture data.
    pub fn logged_out_unavailable() -> Self {
        Self {
            accounts: Vec::new(),
            active: None,
            repositories: Repositories::idle(
                None,
                "Sign in from Settings to read your repositories.",
            ),
            status: None,
        }
    }
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

/// App-wide access point for the next integration step.
///
/// It is exercised by the production-composition integration tests; the product does not mount a
/// repository browser yet, so its narrow unused-warning allowance names that reason instead of a
/// module-wide blanket.
///
/// It exposes exactly what mounting a real repository page needs — the account service, the
/// repository service and the executor to run reads on — and nothing about the UI. A page must
/// still create its own `RepositoryReadContext`/`RepositorySession` through the service; it cannot
/// inherit one from here.
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct AccountServices {
    accounts: Arc<crate::data::AccountService>,
    executor: Arc<crate::network::executor::BackgroundExecutor>,
    /// Bumped on every account change, so a page can drop its own cached reads.
    selection: u64,
    account: Option<AccountKey>,
}

#[allow(dead_code)]
impl AccountServices {
    pub fn accounts(&self) -> &Arc<crate::data::AccountService> {
        &self.accounts
    }

    pub fn repositories(&self) -> crate::data::RepoService {
        self.accounts.repo_service()
    }

    pub fn executor(&self) -> &Arc<crate::network::executor::BackgroundExecutor> {
        &self.executor
    }

    /// The current stable account identity, or `None` while logged out.
    pub fn account(&self) -> Option<&AccountKey> {
        self.account.as_ref()
    }

    /// Monotonic change counter for the active account and its authorization.
    pub fn selection(&self) -> u64 {
        self.selection
    }
}

/// The app-global controller. One per application, created by `app::run` before the window opens.
pub(crate) struct Accounts {
    runtime: Option<Arc<AccountRuntime>>,
    input: Entity<TextInput>,
    /// Held so the masked field's changes redraw whoever renders the controller.
    input_changed: Option<Subscription>,
    /// The Enter interceptor for the masked field. It runs before key bindings so an input-method
    /// composition keeps Enter for its candidate list, exactly like the accepted preview.
    _keys: Option<Subscription>,
    root_focus: FocusHandle,
    inbox: Arc<Mutex<VecDeque<Message>>>,
    alive: Arc<watch::Sender<bool>>,
    operation: Option<OperationControl>,
    generation: u64,
    current_operation: Option<Operation>,
    /// Cancelled mutations whose durable work may still settle. Every outstanding generation is
    /// kept: a single slot would drop an earlier one when a second mutation is cancelled.
    reconciling: Vec<u64>,
    replacing: Option<AccountKey>,
    snapshot: Snapshot,
    selection: u64,
    opened: bool,
    /// The shared in-memory avatar store, so the sidebar, the account row and the account list draw
    /// the same picture once without each owning a loader.
    avatars: Arc<AvatarCache>,
    /// Finished avatar fetches, drained on the same beat as account messages.
    avatar_inbox: AvatarInbox,
    /// Pending avatar requests the renderer asked for but the controller has not started yet. The
    /// renderer only records intent; the launch happens off the paint path.
    avatar_queue: Vec<AvatarRequest>,
    /// The fetcher every avatar read runs on, built from the runtime's existing transport.
    avatar_fetcher: Option<Arc<AvatarFetcher>>,
    /// What the automatic loader has already satisfied. It records the `(account, auth epoch,
    /// selection generation)` triple and the outcome kind it loaded for, so a list is fetched once
    /// per settled authorization even though the view repaints, resizes or collapses constantly.
    /// A failed read records its key too, so a broken credential is not retried in a loop.
    loaded: Option<AutoLoadKey>,
    receiver_task: Option<Task<()>>,
}

/// The identity of one automatic repository read. It is deliberately not a row index or a login
/// string: the account is the stable host+id key, the epoch advances on every PAT replacement and
/// revalidation, and the selection generation advances on every active-account change.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AutoLoadKey {
    account: AccountKey,
    epoch: crate::domain::AuthEpoch,
    selection: u64,
}

impl AutoLoadKey {
    fn of(context: &crate::auth::AuthContext) -> Self {
        Self {
            account: context.account.clone(),
            epoch: context.epoch,
            selection: context.selection,
        }
    }
}

impl Accounts {
    /// The service state as the UI should render it.
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// The masked PAT field. The caller renders it; this controller owns its lifecycle.
    pub fn token_input(&self) -> &Entity<TextInput> {
        &self.input
    }

    /// Whether a replacement PAT is being entered for a saved account.
    pub fn replacing(&self) -> Option<&AccountKey> {
        self.replacing.as_ref()
    }

    /// Whether an account operation is in flight.
    pub fn busy(&self) -> bool {
        self.operation.is_some()
    }

    /// Replaces the avatar byte source before any fetch starts. Used by tests to serve local bytes
    /// instead of contacting an avatar host; production never calls it.
    #[cfg(test)]
    pub fn install_avatar_source(&mut self, bytes: Arc<dyn crate::app::avatar::AvatarBytes>) {
        self.avatar_fetcher = Some(Arc::new(AvatarFetcher::new(bytes, true)));
    }

    /// The state of one avatar for a renderer. Reading the cache is cheap; when the key is unknown
    /// this queues a fetch (started off the paint path) so repeated paints do not refetch.
    pub fn avatar(&mut self, key: AvatarKey) -> AvatarState {
        let state = self.avatars.state(&key);
        match state {
            AvatarState::Unknown => {
                if let Some(request) = self.avatars.request(key) {
                    self.avatar_queue.push(request);
                }
                AvatarState::Loading
            }
            other => other,
        }
    }

    /// The cache state of one avatar without starting a fetch, for the avatar tests.
    #[cfg(test)]
    pub fn avatar_state_for_test(&self, key: &AvatarKey) -> Option<AvatarState> {
        Some(self.avatars.state(key))
    }

    /// The narrow production access point: the active account's services and executor.
    #[allow(dead_code)]
    pub fn services(&self) -> Option<AccountServices> {
        let runtime = self.runtime.as_ref()?;
        Some(AccountServices {
            accounts: runtime.accounts.clone(),
            executor: runtime.executor(),
            selection: self.selection,
            account: self.snapshot.active.clone(),
        })
    }

    /// Builds the controller and starts restoring the saved account directory off the UI thread.
    pub fn new(
        runtime: Option<Arc<AccountRuntime>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (input, input_changed) = Self::make_input(window, cx);
        let root_focus = cx.focus_handle();
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let (alive, _) = watch::channel(true);
        // Downgrade: the subscription is owned by the controller, so holding a strong entity here
        // would keep it alive forever and skip `Drop`.
        let weak = cx.weak_entity();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |accounts, cx| accounts.on_key(event, window, cx))
                .ok();
        });
        let mut this = Self {
            runtime,
            input,
            input_changed: Some(input_changed),
            _keys: Some(keys),
            root_focus,
            inbox: inbox.clone(),
            alive: Arc::new(alive),
            operation: None,
            generation: 0,
            current_operation: None,
            reconciling: Vec::new(),
            replacing: None,
            snapshot: Snapshot {
                accounts: Vec::new(),
                active: None,
                repositories: Repositories::idle(
                    None,
                    "Sign in from Settings to read your repositories.",
                ),
                status: Some(StatusMessage::new(
                    StatusTone::Info,
                    "Restoring the saved account directory…",
                )),
            },
            selection: 0,
            opened: false,
            avatars: Arc::new(AvatarCache::new()),
            avatar_inbox: Arc::new(Mutex::new(Vec::new())),
            avatar_queue: Vec::new(),
            avatar_fetcher: None,
            loaded: None,
            receiver_task: None,
        };
        this.start_receiver(cx);
        if this.runtime.is_none() {
            this.open(cx);
        } else {
            this.adopt_runtime_state(cx);
        }
        this
    }

    /// Enter in the masked field submits, unless an input method is composing text or an operation
    /// is already in flight. The interceptor runs before key bindings, so the input's own Enter
    /// binding cannot run a stale action and an IME candidate confirmation is left alone.
    fn on_key(
        &mut self,
        event: &gpui::KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key != "enter"
            || self.operation.is_some()
            || !self.input.read(cx).focus_handle(cx).is_focused(window)
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

    fn make_input(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<TextInput>, Subscription) {
        let input = cx.new(|cx| {
            TextInput::new(window, cx)
                .masked()
                .label("Classic personal access token")
                .placeholder("Paste a Classic PAT")
        });
        // The parent owns the submit-enabled flag, so a text change must redraw it.
        let changed = cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
            if *event == InputEvent::Changed {
                cx.notify();
            }
        });
        (input, changed)
    }

    fn reset_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A new entity also drops the old edit/undo history. The secret is never copied into the
        // snapshot, a reply, diagnostics or Debug output.
        let (input, changed) = Self::make_input(window, cx);
        self.input = input;
        self.input_changed = Some(changed);
        window.focus(&self.input.read(cx).focus_handle(cx), cx);
    }

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
            let mut inbox = self.inbox.lock().expect("account inbox");
            inbox.drain(..).collect()
        };
        for message in messages {
            self.receive(message, cx);
        }
        self.drain_avatars(cx);
    }

    /// Applies finished avatar fetches and starts the ones the renderer queued. Decoding happened
    /// on the executor; this only swaps cache entries and repaints once.
    fn drain_avatars(&mut self, cx: &mut Context<Self>) {
        let completions = crate::app::avatar::drain(&self.avatar_inbox);
        let had_completions = !completions.is_empty();
        for completion in completions {
            self.avatars.apply(completion);
        }
        // Start anything the last frames asked for, but only while the runtime is alive.
        if !self.avatar_queue.is_empty()
            && let Some(runtime) = self.runtime.clone()
            && let Some(fetcher) = self.avatar_fetcher.clone()
        {
            let queued: Vec<AvatarRequest> = std::mem::take(&mut self.avatar_queue);
            let inbox = self.avatar_inbox.clone();
            for request in queued {
                let fetcher = fetcher.clone();
                let inbox = inbox.clone();
                // One tracked job per avatar; they share the executor's concurrency bound, so a long
                // list cannot launch an unbounded burst.
                let _ = runtime.spawn(
                    crate::network::cancel::CancellationToken::new().1,
                    || {},
                    async move { run_fetch(fetcher, request, inbox).await },
                );
            }
        }
        if had_completions {
            cx.notify();
        }
    }

    fn open(&mut self, cx: &mut Context<Self>) {
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
                if self.opened || self.runtime.is_some() {
                    return;
                }
                self.opened = true;
                match result {
                    Ok(runtime) => {
                        self.runtime = Some(runtime);
                        self.adopt_runtime_state(cx);
                    }
                    Err(message) => {
                        // Account storage is unavailable: the shell keeps working, but it says so
                        // plainly instead of pretending the directory is empty.
                        self.snapshot.status = Some(StatusMessage::new(
                            StatusTone::Error,
                            format!("Account storage could not be opened: {message}"),
                        ));
                        self.snapshot.repositories = Repositories::idle(
                            None,
                            "Account services are unavailable, so no repository list can be read.",
                        );
                    }
                }
                cx.notify();
            }
            Message::Operation(reply) => self.apply(reply, cx),
            Message::Settled(generation) => self.reconcile(generation, cx),
            Message::CleanupPending => {
                self.append_cleanup_warning();
                cx.notify();
            }
        }
    }

    /// Publish the authoritative directory, selection and pending-cleanup state from the service.
    fn adopt_runtime_state(&mut self, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        if self.avatar_fetcher.is_none() {
            // Avatar bodies are binary, so they use a dedicated byte source rather than the
            // JSON REST transport; production installs a reqwest client that never carries the PAT.
            let bytes: Arc<dyn crate::app::avatar::AvatarBytes> =
                Arc::new(crate::app::avatar::ReqwestAvatars::new().unwrap_or_default());
            self.avatar_fetcher = Some(Arc::new(AvatarFetcher::new(bytes, false)));
        }
        self.refresh_presentation();
        let restored = self.snapshot.accounts.iter().any(|summary| {
            summary.authorization == crate::domain::account::AuthorizationState::PendingVerification
        });
        if self.snapshot.active.is_none() {
            self.snapshot.repositories = Repositories::idle(
                None,
                if restored {
                    "Saved accounts were restored. Selecting or revalidating one establishes the current authorization."
                } else {
                    "Sign in from Settings to read your repositories."
                },
            );
            self.snapshot.status = Some(if restored {
                StatusMessage::new(
                    StatusTone::Info,
                    "Saved account directory restored. Nothing is verified access yet; restoring does not treat a stored identity as authorization.",
                )
            } else {
                StatusMessage::new(
                    StatusTone::Info,
                    "No account is signed in. A Classic PAT can be added from Settings.",
                )
            });
        } else if matches!(
            self.snapshot.repositories.preview,
            RepositoryPreview::Idle(_)
        ) {
            // The list loads itself once an account is active; no "refresh to read" instruction.
            self.snapshot.repositories = Repositories::idle(
                self.snapshot.active.clone(),
                "Loading this account's repositories…",
            );
        }
        // A restored saved account is only a stored identity. The automatic loader below runs a
        // real credential verification and read, so the user does not have to revalidate first.
        self.autoload_if_needed(cx);
        // The pending-cleanup journal is a property of the directory, not of one operation, so it
        // is observed separately and reported without rewriting any operation's outcome.
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let accounts = runtime.accounts.clone();
        let _ = runtime.spawn(control().source, || {}, async move {
            if matches!(accounts.recovery_pending().await, Ok(true)) {
                enqueue(&inbox, &alive, Message::CleanupPending);
            }
        });
        cx.notify();
    }

    /// Re-read the authoritative directory and selection; never publishes a repository list.
    fn refresh_presentation(&mut self) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        let accounts = runtime.accounts.clone();
        self.snapshot.accounts = accounts.summaries();
        let active = accounts.active_context().map(|context| context.account);
        if self.snapshot.active != active {
            // The published account changed: any list on screen belonged to the previous one.
            self.selection = self.selection.saturating_add(1);
            self.snapshot.repositories = Repositories::idle(
                active.clone(),
                "Switching to the current account's repositories…",
            );
        }
        self.snapshot.active = active;
    }

    fn reconcile(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some(at) = self
            .reconciling
            .iter()
            .position(|pending| *pending == generation)
        else {
            return;
        };
        self.reconciling.remove(at);
        let previous = self.snapshot.active.clone();
        self.refresh_presentation();
        // Each settlement may have changed this account's authorization evidence, so the displayed
        // repository set is invalidated every time, not only when the last pending one finishes.
        if previous == self.snapshot.active {
            self.snapshot.repositories = Repositories::idle(
                self.snapshot.active.clone(),
                "Reloading repositories under the settled authorization…",
            );
        }
        self.snapshot.status = Some(if self.reconciling.is_empty() {
            StatusMessage::new(
                StatusTone::Info,
                "Cancellations settled. Saved accounts and the current selection are shown as stored; an already committed change was not rolled back.",
            )
        } else {
            StatusMessage::new(
                StatusTone::Info,
                "One cancelled request settled; another is still settling. Saved accounts are shown as currently stored.",
            )
        });
        // Once every cancelled mutation has settled, load the (now final) authorization's list
        // automatically. The discarded read is not reused: this starts a fresh one.
        self.autoload_if_needed(cx);
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
            if reply.operation == Operation::Repositories {
                self.snapshot.repositories = Repositories::idle(
                    reply.for_account.clone(),
                    "The authorization changed while this read was running; its result was discarded. Reloading…",
                );
            }
            self.snapshot.status = Some(StatusMessage::new(
                StatusTone::Warning,
                "The account or authorization changed while this request ran; its result was discarded.",
            ));
            cx.notify();
            return;
        }
        if let (Some(expected), Some(actual)) = (&reply.for_account, &self.snapshot.active)
            && expected != actual
            && reply.operation == Operation::Repositories
        {
            // Defence in depth: the read context check above already covers this, but a reply is
            // never attributed to an account it was not requested for.
            self.snapshot.repositories = Repositories::idle(
                Some(actual.clone()),
                "This repository result belonged to another account and was discarded. Reloading…",
            );
            self.snapshot.status = Some(StatusMessage::new(
                StatusTone::Warning,
                "A repository result for the previous account was discarded.",
            ));
            cx.notify();
            return;
        }
        // Read authoritative state now: an unrelated cancelled mutation may have reconciled the
        // directory after this reply's work finished, and a stale snapshot must not roll it back.
        let previous = self.snapshot.active.clone();
        self.refresh_presentation();
        let switched = previous != self.snapshot.active;
        match reply.result {
            Ok(success) => {
                // A successful repository read is the automatic consequence of an account action;
                // it must not replace that action's own status (e.g. "revalidated") with noise.
                if success != Success::Repositories {
                    self.snapshot.status = Some(match success {
                        Success::Added => StatusMessage::new(
                            StatusTone::Success,
                            "Account verified and saved; its PAT is in the platform credential store.",
                        ),
                        Success::Replaced => StatusMessage::new(
                            StatusTone::Success,
                            "Replacement PAT verified and saved.",
                        ),
                        Success::Selected => StatusMessage::new(
                            StatusTone::Success,
                            "Current account selection saved.",
                        ),
                        Success::Revalidated => StatusMessage::new(
                            StatusTone::Success,
                            "Saved credential revalidated against the current GitHub identity.",
                        ),
                        // Handled above: an automatic read never publishes its own status line.
                        Success::Repositories => unreachable!("read status is not published"),
                        Success::CleanupPending => StatusMessage::new(
                            StatusTone::Warning,
                            "The account change was committed, but retired credential cleanup is pending. Do not retry the same change blindly.",
                        ),
                    });
                }
                if matches!(success, Success::Added | Success::Replaced) {
                    self.replacing = None;
                }
                if success == Success::Repositories {
                    self.snapshot.repositories = match reply.repositories {
                        Some(Ok(outcome)) => Repositories {
                            account: reply.for_account.clone(),
                            preview: RepositoryPreview::Ready(outcome),
                        },
                        Some(Err(error)) => Repositories {
                            account: reply.for_account.clone(),
                            preview: RepositoryPreview::Failed(error_text(&error)),
                        },
                        None => Repositories {
                            account: reply.for_account.clone(),
                            preview: RepositoryPreview::Failed(
                                "The repository response was not returned.".into(),
                            ),
                        },
                    };
                } else if !switched {
                    self.snapshot.repositories = Repositories::idle(
                        self.snapshot.active.clone(),
                        "Authorization or current account changed. Reloading repositories…",
                    );
                }
            }
            Err(message) => {
                self.snapshot.status = Some(StatusMessage::new(StatusTone::Error, message));
                if reply.operation == Operation::Repositories {
                    self.snapshot.repositories = Repositories {
                        account: reply.for_account.clone(),
                        preview: RepositoryPreview::Failed(
                            reply
                                .repositories
                                .and_then(|result| result.err())
                                .map(|error| error_text(&error))
                                .unwrap_or_else(|| {
                                    "The request failed; no successful empty list is implied."
                                        .into()
                                }),
                        ),
                    };
                } else if !switched {
                    // An errored mutation may still have committed: `CleanupPending` is returned
                    // after the directory and active selection were already persisted, so data
                    // read under the previous account must not stay on screen.
                    self.snapshot.repositories = Repositories::idle(
                        self.snapshot.active.clone(),
                        "An account change was in progress; reloading the current account's repositories…",
                    );
                }
            }
        }
        if reply.recovery_pending {
            self.append_cleanup_warning();
        }
        // Any settled account mutation (add/select/replace/revalidate) may have changed the current
        // authorization, so the list is loaded once for the resulting settled triple. A completed
        // repository read is a no-op here because its key is already recorded.
        self.autoload_if_needed(cx);
        cx.notify();
    }

    fn append_cleanup_warning(&mut self) {
        let current = self
            .snapshot
            .status
            .as_ref()
            .map(|status| status.text.clone())
            .unwrap_or_default();
        self.snapshot.status = Some(StatusMessage::new(
            StatusTone::Warning,
            format!(
                "{current} Retired credential cleanup remains pending for the saved account directory."
            ),
        ));
    }

    fn start(&mut self, operation: Operation, cx: &mut Context<Self>) {
        if self.operation.is_some() {
            return;
        }
        let Some(runtime) = self.runtime.clone() else {
            self.snapshot.status = Some(StatusMessage::new(
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
        self.snapshot.status = Some(StatusMessage::new(
            StatusTone::Info,
            operation_label(operation),
        ));
        let for_account = self.snapshot.active.clone();
        let publication = if operation == Operation::Repositories {
            self.snapshot.repositories = Repositories {
                account: for_account.clone(),
                preview: RepositoryPreview::Loading,
            };
            // Record the settled authorization this read belongs to, so a later plain
            // `autoload_if_needed` (from a repaint-driven settlement) will not repeat it.
            if let Some(context) = runtime.accounts.active_context() {
                self.loaded = Some(AutoLoadKey::of(&context));
            }
            runtime
                .accounts
                .repo_service()
                .session()
                .ok()
                .map(|session| session.context().clone())
        } else {
            // An account change invalidates the previous list before anything is published.
            self.snapshot.repositories = Repositories::idle(
                for_account.clone(),
                "An account change is in progress. Repositories load once it settles.",
            );
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
                    // The data-layer revalidate has no cancellable variant and always commits its
                    // runtime evidence. Cancelling stops publication, not the operation: the reply
                    // is discarded and `Settled` reconciles authoritative state.
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
                    publication,
                    for_account,
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
            self.snapshot.status = Some(StatusMessage::new(
                StatusTone::Error,
                "Background services are closing; the operation was not submitted.",
            ));
        }
        cx.notify();
    }

    /// Verifies the masked field. The PAT is moved out of the field and the field is rebuilt before
    /// the request starts, so no later frame can render or copy it.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation.is_some() {
            return;
        }
        let mut value = self.input.read(cx).text().trim().to_string();
        if value.is_empty() {
            self.snapshot.status = Some(StatusMessage::new(
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
            self.snapshot.status = Some(StatusMessage::new(
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
        self.snapshot.status = Some(StatusMessage::new(
            StatusTone::Info,
            operation_label(operation),
        ));
        self.snapshot.repositories = Repositories::idle(
            self.snapshot.active.clone(),
            "An account change is in progress. Repositories load once it settles.",
        );
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
            // Cancelling before a durable commit drops the request future. Once the service submits
            // its blocking commit, that commit finishes.
            let result = tokio::select! {
                biased;
                _ = control.token.clone().cancelled() => Err(LoginError::GitHub(GitHubError::Cancelled)),
                result = pending => result,
            };
            let result = match result {
                // The outcome, not the requested intent, decides what the user is told: re-adding
                // an existing identity is a replacement.
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
                    publication: None,
                    for_account: None,
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
            self.snapshot.status = Some(StatusMessage::new(
                StatusTone::Error,
                "The operation was not submitted because background services are closing.",
            ));
        }
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(operation) = self.operation.take() {
            operation.source.cancel();
            // Account mutations may already have submitted a durable commit, so their authoritative
            // state is reconciled when they settle. Their own reply stays discarded because
            // `operation` is now `None`.
            if !matches!(self.current_operation, Some(Operation::Repositories) | None)
                && !self.reconciling.contains(&self.generation)
            {
                self.reconciling.push(self.generation);
            }
        }
        self.snapshot.status = Some(StatusMessage::new(
            StatusTone::Info,
            "Cancellation requested. An in-flight account operation still settles in the service; saved accounts are reconciled when it does, and nothing will claim it was rolled back.",
        ));
        self.snapshot.repositories = Repositories::idle(
            self.snapshot.active.clone(),
            "Cancelled. The current authorization's repositories reload when it settles.",
        );
        self.input
            .update(cx, |input, cx| input.set_disabled(false, cx));
        cx.notify();
    }

    pub fn select(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        self.replacing = None;
        self.start(Operation::Select(key.id), cx);
    }

    pub fn cancel_replace(&mut self, cx: &mut Context<Self>) {
        self.replacing = None;
        self.snapshot.status = Some(StatusMessage::new(
            StatusTone::Info,
            "Replacement cancelled. The saved PAT is unchanged.",
        ));
        cx.notify();
    }

    pub fn replace(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        self.replacing = Some(key);
        self.snapshot.status = Some(StatusMessage::new(
            StatusTone::Info,
            "Enter a PAT for this same account. A different identity will be rejected.",
        ));
        cx.notify();
    }

    pub fn revalidate(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        self.start(Operation::Revalidate(key.id), cx);
    }

    /// Starts the current account's repository read exactly once per settled authorization.
    ///
    /// Called from every place where the authoritative account or its authorization may have just
    /// settled (startup restore, login, select, replace, revalidate, cancellation reconcile, and
    /// the end of a read). It never runs on a plain repaint, resize or sidebar collapse, because it
    /// is only invoked from those settlements and records the triple it loaded for.
    ///
    /// A stored identity is not a verified authorization: when the service has not verified this
    /// credential in this run, the read goes through the normal authenticated path, which loads the
    /// platform credential and fails with the matching recoverable error if it is missing or bad.
    fn autoload_if_needed(&mut self, cx: &mut Context<Self>) {
        // Never interrupt an in-flight operation, and never act while a cancelled mutation is still
        // settling: the authorization evidence it may change is not final yet.
        if self.operation.is_some() || !self.reconciling.is_empty() {
            return;
        }
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let Some(context) = runtime.accounts.active_context() else {
            return;
        };
        let key = AutoLoadKey::of(&context);
        if self.loaded.as_ref() == Some(&key) {
            return;
        }
        // Mark before starting so a repaint or a second settlement cannot queue a duplicate.
        self.loaded = Some(key);
        // Keep the settlement/action message the caller just set: the automatic read is a
        // consequence of it, not a new user-visible event of its own.
        let previous_status = self.snapshot.status.clone();
        self.start(Operation::Repositories, cx);
        if self.operation.is_some() {
            self.snapshot.status = previous_status;
        }
    }

    /// Explicit refresh of the current account's repository list. Scoped to that list: it never
    /// refreshes a page, a tab or anything global. It is reachable only from a failure's scoped
    /// retry, not from a permanent control.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.start(Operation::Repositories, cx);
    }

    /// Opens the PAT creation page in the user's browser. No token is created or read here.
    pub fn open_pat_help(&self, cx: &mut App) {
        cx.open_url(PAT_HELP);
    }

    /// Stops all in-flight account work when the controller is dropped.
    fn shutdown(&mut self) {
        // Stop avatar reads first: they hold the shared transport and must not outlive teardown.
        if let Some(fetcher) = &self.avatar_fetcher {
            fetcher.cancel();
        }
        // `send` fails with no receiver held, which is exactly our case: `send_replace` still
        // stores the value so late workers observe it.
        self.alive.send_replace(false);
        if let Some(operation) = self.operation.take() {
            operation.source.cancel();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown();
        }
    }
}

impl Drop for Accounts {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Focusable for Accounts {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.root_focus.clone()
    }
}

/// The account callbacks the presentation layer invokes. One closure set, so the product Settings
/// page and the example cannot drift apart.
pub(crate) fn callbacks(
    controller: Entity<Accounts>,
    _status: Option<StatusMessage>,
) -> crate::ui::accounts::AccountCallbacks {
    let submit = controller.clone();
    let cancel = controller.clone();
    let select = controller.clone();
    let replace = controller.clone();
    let revalidate = controller.clone();
    let cancel_replace = controller.clone();
    let help = controller.clone();
    crate::ui::accounts::AccountCallbacks {
        submit: std::rc::Rc::new(move |window, cx| {
            submit.update(cx, |accounts, cx| accounts.submit(window, cx))
        }),
        cancel: std::rc::Rc::new(move |_, cx| {
            cancel.update(cx, |accounts, cx| accounts.cancel(cx))
        }),
        select: std::rc::Rc::new(move |key, _, cx| {
            select.update(cx, |accounts, cx| accounts.select(key, cx))
        }),
        replace: std::rc::Rc::new(move |key, _, cx| {
            replace.update(cx, |accounts, cx| accounts.replace(key, cx))
        }),
        revalidate: std::rc::Rc::new(move |key, _, cx| {
            revalidate.update(cx, |accounts, cx| accounts.revalidate(key, cx))
        }),
        cancel_replace: std::rc::Rc::new(move |_, cx| {
            cancel_replace.update(cx, |accounts, cx| accounts.cancel_replace(cx))
        }),
        open_pat_help: std::rc::Rc::new(move |_, cx| {
            help.update(cx, |accounts, cx| accounts.open_pat_help(cx))
        }),
    }
}

async fn send_reply(
    accounts: &Arc<crate::data::AccountService>,
    inbox: &Arc<Mutex<VecDeque<Message>>>,
    alive: &watch::Sender<bool>,
    mut reply: Reply,
) {
    reply.recovery_pending = accounts.recovery_pending().await.unwrap_or(true);
    enqueue(inbox, alive, Message::Operation(reply));
}

/// Completion signal for one background operation. It runs on the network thread and only pushes
/// a marker; the foreground drain does the UI work.
fn settled_signal(
    inbox: Arc<Mutex<VecDeque<Message>>>,
    alive: Arc<watch::Sender<bool>>,
    generation: u64,
) -> impl FnOnce() + Send + 'static {
    move || enqueue(&inbox, &alive, Message::Settled(generation))
}

fn enqueue(inbox: &Arc<Mutex<VecDeque<Message>>>, alive: &watch::Sender<bool>, message: Message) {
    if *alive.borrow() {
        inbox.lock().expect("account inbox").push_back(message);
    }
}

fn open_production_blocking() -> Result<AccountRuntime, String> {
    // Production startup runs on its own Tokio runtime thread and keeps the resulting executor for
    // all later operations.
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

/// Turns a typed GitHub error into a user-facing sentence. Every branch states that no successful
/// empty list is implied, because a failed read is never an empty repository set.
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

/// Keep the process alive until background account teardown finishes. Called by `app::run` after
/// the GPUI event loop returns, so quitting cannot truncate an in-flight credential or SQLite
/// commit.
pub(crate) fn wait_for_shutdown() {
    runtime::wait_for_drain();
}
