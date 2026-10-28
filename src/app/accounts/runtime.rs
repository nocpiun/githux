//! Production and test composition for the account preview.
//!
//! The only production database, credential store, HTTP transport and executor
//! come from the existing service layer. Tests inject every external boundary.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::auth::credential::CredentialStore;
#[cfg(not(target_os = "windows"))]
use crate::auth::credential::UnsupportedStore;
use crate::data::AccountService;
use crate::github::client::{GitHubClient, GitHubConfig};
use crate::network::cancel::CancellationTokenSource;
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::ReqwestTransport;
use crate::storage::accounts::AccountDatabaseConfig;

/// Process-exit gate for background account teardown.
///
/// `AccountService::close` and the executor drain run on a detached thread so
/// GPUI never blocks. The process must not exit before they finish, or an
/// in-flight credential/SQLite commit is truncated; `app::run` waits here after
/// the event loop returns, bounded by `EXIT_WAIT`.
static EXIT: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();

const EXIT_WAIT: Duration = Duration::from_secs(10);

fn exit_gate() -> &'static (Mutex<usize>, Condvar) {
    EXIT.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

fn begin_drain() {
    let (pending, _) = exit_gate();
    *pending.lock().expect("exit gate") += 1;
}

fn end_drain() {
    let (pending, signal) = exit_gate();
    let mut count = pending.lock().expect("exit gate");
    *count = count.saturating_sub(1);
    signal.notify_all();
}

/// Block until every started teardown finished, bounded so a stuck platform
/// credential call cannot hang process exit forever.
pub(crate) fn wait_for_drain() {
    let (pending, signal) = exit_gate();
    let deadline = Instant::now() + EXIT_WAIT;
    let mut count = pending.lock().expect("exit gate");
    while *count > 0 {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        count = signal
            .wait_timeout(count, deadline - now)
            .expect("exit gate")
            .0;
    }
}

/// Owns the existing account service and its network executor. No UI state or
/// PAT input lives here. Clones may travel through background jobs; shutdown is
/// idempotent and always finishes off the GPUI thread.
pub(crate) struct AccountRuntime {
    pub accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
    jobs: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Whether shutdown has begun. Exposed so the production-composition tests can observe that
    /// dropping the controller really tears the account stack down.
    pub closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AccountRuntime {
    pub fn new(accounts: AccountService, executor: Arc<BackgroundExecutor>) -> Self {
        Self {
            accounts: Arc::new(accounts),
            executor,
            jobs: Mutex::new(Vec::new()),
            closed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// The executor every account/network call runs on. Pages mount their own reads on it instead
    /// of creating a second runtime thread.
    pub fn executor(&self) -> Arc<BackgroundExecutor> {
        self.executor.clone()
    }

    /// Run one tracked operation on the existing Tokio executor. The caller owns
    /// the operation generation and both the result and the completion signal.
    /// `settled` runs on the network thread after the future ends (including a
    /// durable commit that outlived cancellation). It must only enqueue cheap
    /// data; it must not touch GPUI or block.
    pub fn spawn(
        self: &Arc<Self>,
        cancel: CancellationTokenSource,
        settled: impl FnOnce() + Send + 'static,
        operation: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ()> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(());
        }
        let job = self
            .executor
            .handle()
            .spawn(async move {
                let _cancel_on_drop = CancelOnDrop(cancel);
                operation.await;
                settled();
            })
            .map_err(|_| ())?;
        self.jobs.lock().expect("preview jobs").push(job);
        Ok(())
    }

    /// Abort preview reads, drain already submitted account commits, then close
    /// SQLite and release the executor on a separate thread. The final Arc's
    /// Drop runs the existing drain/join shutdown; no blocking join runs on GPUI.
    pub fn shutdown(&self) {
        if self.closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        for job in self.jobs.lock().expect("preview jobs").drain(..) {
            job.abort();
        }
        // Counted before the thread starts so the exit gate cannot observe an
        // epoch between spawning teardown and registering it.
        begin_drain();
        let accounts = self.accounts.clone();
        let executor = self.executor.clone();
        std::thread::spawn(move || {
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let close = executor.handle().spawn(async move {
                let result = accounts.close().await;
                let _ = done_tx.send(result);
            });
            if close.is_ok() {
                // A submitted SQLite/credential commit is not interruptible;
                // the executor's normal grace period remains the final bound.
                let _ = done_rx.recv_timeout(Duration::from_secs(5));
            }
            drop(executor);
            // Release the process-exit gate only after SQLite is closed and the
            // executor has joined, so quitting cannot truncate the drain.
            end_drain();
        });
    }
}

impl Drop for AccountRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct CancelOnDrop(CancellationTokenSource);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Open the production stack inside a newly created background executor.
/// `AccountService::open` performs SQLite and credential restoration on its
/// blocking worker; no filesystem or credential API runs on GPUI.
pub(crate) async fn open_production() -> Result<(AccountRuntime, Arc<BackgroundExecutor>), String> {
    let config = AccountDatabaseConfig::application_data().map_err(|error| error.to_string())?;
    let executor = Arc::new(BackgroundExecutor::new(4));
    #[cfg(target_os = "windows")]
    let store: Arc<dyn CredentialStore> = Arc::new(crate::auth::WindowsCredentialStore::new());
    #[cfg(not(target_os = "windows"))]
    let store: Arc<dyn CredentialStore> = Arc::new(UnsupportedStore);
    let transport = Arc::new(
        ReqwestTransport::new(Duration::from_secs(30))
            .map_err(|_| "Could not initialize the GitHub HTTP client.".to_string())?,
    );
    let client = Arc::new(GitHubClient::new(GitHubConfig::default(), transport));
    let accounts = AccountService::open(config, store, client, executor.clone())
        .await
        .map_err(|error| format!("Could not open account storage: {error}"))?;
    let runtime = AccountRuntime::new(accounts, executor.clone());
    Ok((runtime, executor))
}
