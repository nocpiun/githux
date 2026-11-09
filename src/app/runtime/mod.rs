//! Framework-independent account bridge for Native Shell. See README.md for the UI drain contract.
//!
pub mod backend;
pub mod page;
mod repositories;
pub mod sessions;

use crate::auth::{AuthContext, token::Pat};
use crate::data::{AccountError, LoginError, LoginOutcome, RepositoryReadContext};
use crate::domain::{AccountKey, AccountSummary, cache::CacheMiss, error::GitHubError};
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::storage::{
    accounts::StorageError,
    cache::{CacheAuthorization, CacheError},
};
pub use backend::{Backend, ExitGate, OpenError};
use page::Services as ServicesInner;
pub use page::{PageContext, PageSlot, PageTicket, Services};
use repositories::ListReply;
pub use repositories::{RepositoryList, RepositoryListOrigin};
pub use sessions::{
    PageIdentity, RepositoryNavigation, Route, RouteKey, Session, Sessions, TabId, Workspace,
};
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Login(LoginError),
    Account(AccountError),
    GitHub(GitHubError),
    Cache(CacheError),
    NotCached(CacheMiss),
    Worker,
}
/// Token inputs move into the command and never appear in a snapshot, reply or callback.
#[derive(Debug)]
pub enum Command {
    Add(Pat),
    Replace { account: AccountKey, token: Pat },
    Select(AccountKey),
    Revalidate(AccountKey),
    Remove(AccountKey),
    Recover,
    RetryRepositories,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Add,
    Replace,
    Select,
    Revalidate,
    Remove,
    Recover,
    Repositories,
}
impl Command {
    fn operation(&self) -> Operation {
        match self {
            Self::Add(_) => Operation::Add,
            Self::Replace { .. } => Operation::Replace,
            Self::Select(_) => Operation::Select,
            Self::Revalidate(_) => Operation::Revalidate,
            Self::Remove(_) => Operation::Remove,
            Self::Recover => Operation::Recover,
            Self::RetryRepositories => Operation::Repositories,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Success {
    Added,
    Replaced,
    Selected,
    Revalidated,
    Removed,
    Recovered,
    CommittedCleanupPending,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    Finished {
        operation: Operation,
        result: Result<Success, Failure>,
    },
    /// The durable service state is authoritative. Cancellation does not assert rollback.
    CancelledSettled(Operation),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Startup {
    Opening,
    Ready,
    Failed(OpenError),
    Closed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub startup: Startup,
    pub accounts: Vec<AccountSummary>,
    pub active: Option<AccountKey>,
    pub repositories: RepositoryList,
    pub busy: Option<Operation>,
    pub settling: bool,
    pub completion: Option<Completion>,
    pub recovery_pending: Result<bool, StorageError>,
    pub cache_open_error: Option<CacheError>,
    /// Changes on account selection, auth evidence, or an account mutation intent.
    pub binding_revision: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    NotReady,
    Busy,
    Closing,
    EmptyToken,
    Worker,
}

struct Pending {
    id: u64,
    operation: Operation,
    cancel: CancellationTokenSource,
    cancelled: bool,
}
enum Message {
    Opened(Result<Backend, OpenError>),
    Recovery {
        revision: u64,
        value: Result<bool, StorageError>,
    },
    Finished {
        id: u64,
        result: Result<Success, Failure>,
        recovery: Result<bool, StorageError>,
    },
    List {
        id: u64,
        reply: ListReply,
    },
}
struct Inbox {
    messages: Mutex<VecDeque<Message>>,
    alive: AtomicBool,
    /// Wake only schedules a UI drain. It must not call a UI signal from a background thread.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl Inbox {
    fn send(&self, message: Message) {
        let mut messages = self.messages.lock().expect("runtime inbox");
        if !self.alive.load(Ordering::Acquire) {
            return;
        }
        messages.push_back(message);
        drop(messages);
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

/// One owner per application. The NSH adapter holds this on the UI thread, calls drain on events
/// and a light foreground tick (for revocations), then binds the owned snapshot to native signals.
pub struct RuntimeHandle {
    backend: Option<Arc<Backend>>,
    services: Option<ServicesInner>,
    inbox: Arc<Inbox>,
    gate: ExitGate,
    snapshot: Snapshot,
    sessions: Sessions,
    pending: Option<Pending>,
    next: u64,
    bound: Option<(AccountSummary, AuthContext)>,
    loaded: Option<AuthContext>,
    displayed: Option<(RepositoryReadContext, Option<CacheAuthorization>)>,
}
impl RuntimeHandle {
    fn empty(gate: ExitGate, wake: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        Self {
            backend: None,
            services: None,
            inbox: Arc::new(Inbox {
                messages: Mutex::new(VecDeque::new()),
                alive: AtomicBool::new(true),
                wake,
            }),
            gate,
            snapshot: Snapshot {
                startup: Startup::Opening,
                accounts: Vec::new(),
                active: None,
                repositories: RepositoryList::Idle,
                busy: None,
                settling: false,
                completion: None,
                recovery_pending: Ok(false),
                cache_open_error: None,
                binding_revision: 0,
            },
            sessions: Sessions::new(),
            pending: None,
            next: 0,
            bound: None,
            loaded: None,
            displayed: None,
        }
    }
    /// Starts production initialization entirely off the UI thread, including executor construction.
    pub fn start(gate: ExitGate, wake: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let this = Self::empty(gate.clone(), wake);
        let inbox = this.inbox.clone();
        // Covers closing the window before initialization finishes.
        let starting = gate.enter();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                backend::open_production(gate)
            }))
            .unwrap_or(Err(OpenError::Worker));
            inbox.send(Message::Opened(result));
            drop(starting);
        });
        this
    }
    /// Injection path for isolated tests and alternate application assembly; no I/O here.
    pub fn from_backend(backend: Backend) -> Self {
        let mut this = Self::empty(backend.exit_gate(), None);
        this.adopt(backend);
        this
    }
    pub fn exit_gate(&self) -> ExitGate {
        self.gate.clone()
    }
    pub fn sessions(&self) -> &Sessions {
        &self.sessions
    }
    pub fn sessions_mut(&mut self) -> &mut Sessions {
        &mut self.sessions
    }
    pub fn services(&self) -> Option<Services> {
        self.services.as_ref().filter(|s| s.is_current()).cloned()
    }
    /// Checks revocations and authoritative account state before returning an owned presentation.
    /// The UI must replace/remove old signal values when the next snapshot clears them.
    pub fn snapshot(&mut self) -> Snapshot {
        self.drain();
        self.snapshot.clone()
    }
    fn invalidate(&mut self) {
        if let Some(services) = self.services.take() {
            services.invalidate();
        }
        self.snapshot.binding_revision = self
            .snapshot
            .binding_revision
            .checked_add(1)
            .expect("binding generation exhausted");
        self.snapshot.repositories = RepositoryList::Idle;
        self.displayed = None;
        self.loaded = None;
    }
    fn adopt(&mut self, backend: Backend) {
        self.snapshot.cache_open_error = backend.cache_open_error;
        self.backend = Some(Arc::new(backend));
        self.snapshot.startup = Startup::Ready;
        self.sync();
        let backend = self.backend.as_ref().expect("adopted backend").clone();
        let accounts = backend.accounts.clone();
        let inbox = self.inbox.clone();
        let revision = self.snapshot.binding_revision;
        let _ = backend.spawn(async move {
            inbox.send(Message::Recovery {
                revision,
                value: accounts.recovery_pending().await,
            });
        });
        self.autoload();
    }
    fn sync(&mut self) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let accounts = backend.accounts.summaries();
        let active = backend.accounts.active_context();
        let bound = backend.accounts.active_summary().zip(active.clone());
        if self.bound != bound {
            self.invalidate();
            self.bound = bound;
        }
        self.snapshot.active = active.as_ref().map(|c| c.account.clone());
        self.sessions.retain_accounts(&accounts);
        self.sessions.show(self.snapshot.active.clone());
        self.snapshot.accounts = accounts;
        if self.services.is_none()
            && active.is_some()
            && self
                .pending
                .as_ref()
                .is_none_or(|p| p.operation == Operation::Repositories)
        {
            self.services = Some(ServicesInner {
                backend,
                binding: Arc::new(AtomicBool::new(true)),
            });
        }
    }
    /// Cheap memory/atomic checks even with an empty inbox; never reads SQLite or the network.
    pub fn drain(&mut self) {
        if self.snapshot.startup == Startup::Closed {
            return;
        }
        self.sync();
        if self.displayed.as_ref().is_some_and(|(context, access)| {
            self.backend
                .as_ref()
                .is_none_or(|b| !b.accounts.repo_service().is_current(context))
                || access.as_ref().is_some_and(|a| !a.is_current())
        }) {
            self.displayed = None;
            self.snapshot.repositories =
                RepositoryList::Failed(Failure::Cache(CacheError::AccessDenied));
            // Keep the load marker: revocation is not an automatic retry loop.
        }
        let messages: Vec<_> = self
            .inbox
            .messages
            .lock()
            .expect("runtime inbox")
            .drain(..)
            .collect();
        for message in messages {
            match message {
                Message::Opened(result) if self.backend.is_none() => match result {
                    Ok(backend) => self.adopt(backend),
                    Err(error) => self.snapshot.startup = Startup::Failed(error),
                },
                Message::Opened(_) => {}
                Message::Recovery { revision, value } => {
                    if revision == self.snapshot.binding_revision {
                        self.snapshot.recovery_pending = value;
                    }
                }
                Message::Finished {
                    id,
                    result,
                    recovery,
                } => {
                    if self.pending.as_ref().is_none_or(|p| p.id != id) {
                        continue;
                    }
                    let pending = self.pending.take().expect("matching operation");
                    self.snapshot.busy = None;
                    self.snapshot.settling = false;
                    self.snapshot.completion = Some(if pending.cancelled {
                        Completion::CancelledSettled(pending.operation)
                    } else {
                        Completion::Finished {
                            operation: pending.operation,
                            result,
                        }
                    });
                    self.snapshot.recovery_pending = recovery;
                    self.sync();
                    self.autoload();
                }
                Message::List { id, reply } => {
                    if self.pending.as_ref().is_none_or(|p| p.id != id) {
                        continue;
                    }
                    let pending = self.pending.take().expect("matching read");
                    self.snapshot.busy = None;
                    if pending.cancelled {
                        continue;
                    }
                    let current = self
                        .backend
                        .as_ref()
                        .is_some_and(|b| b.accounts.repo_service().is_current(&reply.context));
                    if !current {
                        self.sync();
                        self.loaded = None;
                        self.autoload();
                        continue;
                    }
                    self.snapshot.repositories = match reply.result {
                        Ok((outcome, origin))
                            if reply
                                .authorization
                                .as_ref()
                                .is_none_or(CacheAuthorization::is_current) =>
                        {
                            self.displayed = Some((reply.context, reply.authorization));
                            RepositoryList::Ready { outcome, origin }
                        }
                        Ok(_) => RepositoryList::Failed(Failure::Cache(CacheError::AccessDenied)),
                        Err(error) => RepositoryList::Failed(error),
                    };
                }
            }
        }
    }
    fn autoload(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let Some(context) = self
            .backend
            .as_ref()
            .and_then(|b| b.accounts.active_context())
        else {
            return;
        };
        if self.loaded.as_ref() != Some(&context) {
            let _ = self.submit(Command::RetryRepositories);
        }
    }
    pub fn submit(&mut self, command: Command) -> Result<(), SubmitError> {
        if self.snapshot.startup == Startup::Closed {
            return Err(SubmitError::Closing);
        }
        let backend = self.backend.clone().ok_or(SubmitError::NotReady)?;
        if self.pending.is_some() {
            return Err(SubmitError::Busy);
        }
        if matches!(&command, Command::Add(token) | Command::Replace { token, .. } if token.is_empty())
        {
            return Err(SubmitError::EmptyToken);
        }
        let operation = command.operation();
        self.next = self
            .next
            .checked_add(1)
            .expect("operation generation exhausted");
        let id = self.next;
        let (cancel, source) = CancellationToken::new();
        self.pending = Some(Pending {
            id,
            operation,
            cancel: source,
            cancelled: false,
        });
        self.snapshot.busy = Some(operation);
        self.snapshot.settling = false;
        let accounts = backend.accounts.clone();
        let inbox = self.inbox.clone();
        let spawned = if operation == Operation::Repositories {
            let context = match accounts.repo_service().session() {
                Ok(session) => session.context().clone(),
                Err(_) => {
                    self.pending = None;
                    self.snapshot.busy = None;
                    return Err(SubmitError::NotReady);
                }
            };
            self.loaded = Some(context.auth().clone());
            self.displayed = None;
            self.snapshot.repositories = RepositoryList::Loading;
            backend.spawn(async move {
                let reply = repositories::read(accounts.repo_service(), context, cancel).await;
                inbox.send(Message::List { id, reply });
            })
        } else {
            self.invalidate();
            backend.spawn(async move {
                // Never drop a mutation future to implement cancellation: its blocking commit may
                // outlive that future. Wait for the real service result before marking settled.
                let result = match command {
                    Command::Add(token) => {
                        login_result(accounts.login_cancellable(token, cancel).await)
                    }
                    Command::Replace { account, token } => {
                        login_result(accounts.replace(account, token).await)
                    }
                    Command::Select(account) => {
                        account_result(accounts.set_active(account).await, Success::Selected)
                    }
                    Command::Revalidate(account) => accounts
                        .revalidate(account)
                        .await
                        .map(|_| Success::Revalidated)
                        .map_err(Failure::Login),
                    Command::Remove(account) => {
                        account_result(accounts.remove_saved(account).await, Success::Removed)
                    }
                    Command::Recover => {
                        account_result(accounts.recover().await, Success::Recovered)
                    }
                    Command::RetryRepositories => unreachable!(),
                };
                let recovery = accounts.recovery_pending().await;
                inbox.send(Message::Finished {
                    id,
                    result,
                    recovery,
                });
            })
        };
        match spawned {
            Ok(_) => Ok(()),
            Err(_) => {
                self.pending = None;
                self.snapshot.busy = None;
                self.snapshot.completion = Some(Completion::Finished {
                    operation,
                    result: Err(Failure::Worker),
                });
                self.sync();
                Err(SubmitError::Worker)
            }
        }
    }
    /// Cancelling a mutation suppresses its reply, but keeps the submit gate busy until it settles.
    pub fn cancel(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.cancel.cancel();
            pending.cancelled = true;
            if pending.operation == Operation::Repositories {
                self.pending = None;
                self.snapshot.busy = None;
                self.snapshot.repositories =
                    RepositoryList::Failed(Failure::GitHub(GitHubError::Cancelled));
            } else {
                self.snapshot.settling = true;
            }
        }
    }
    pub fn shutdown(&mut self) {
        if self.snapshot.startup == Startup::Closed {
            return;
        }
        self.invalidate();
        self.cancel();
        self.pending = None;
        self.snapshot.startup = Startup::Closed;
        self.snapshot.busy = None;
        self.snapshot.accounts.clear();
        self.snapshot.active = None;
        // Lock joins the enqueue check so an initializer cannot queue an unclosed backend after
        // shutdown has drained the inbox. Dropping queued Backend values schedules their teardown.
        let mut messages = self.inbox.messages.lock().expect("runtime inbox");
        self.inbox.alive.store(false, Ordering::Release);
        messages.clear();
        drop(messages);
        if let Some(backend) = self.backend.take() {
            backend.shutdown();
        }
    }
}
impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn login_result(result: Result<LoginOutcome, LoginError>) -> Result<Success, Failure> {
    match result {
        Ok(LoginOutcome::SignedIn(_)) => Ok(Success::Added),
        Ok(LoginOutcome::Replaced(_)) => Ok(Success::Replaced),
        Err(LoginError::CleanupPending(_)) => Ok(Success::CommittedCleanupPending),
        Err(error) => Err(Failure::Login(error)),
    }
}
fn account_result(result: Result<(), AccountError>, success: Success) -> Result<Success, Failure> {
    match result {
        Ok(()) => Ok(success),
        Err(AccountError::CleanupPending) => Ok(Success::CommittedCleanupPending),
        Err(error) => Err(Failure::Account(error)),
    }
}
