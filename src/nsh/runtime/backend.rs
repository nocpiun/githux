//! Service ownership and nonblocking teardown. No window or reactive handles cross this boundary.
use crate::auth::credential::CredentialStore;
use crate::data::AccountService;
use crate::github::client::{GitHubClient, GitHubConfig};
use crate::network::executor::{BackgroundExecutor, SpawnError};
use crate::network::transport::ReqwestTransport;
use crate::storage::accounts::{AccountDatabaseConfig, StorageError};
use crate::storage::cache::{CacheConfig, CacheError, CacheStore};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Clone before opening the window; wait only AFTER the native event loop returns.
/// A timeout returns false, never claims durable work finished.
#[derive(Clone, Default)]
pub struct ExitGate(Arc<(Mutex<usize>, Condvar)>);
impl ExitGate {
    pub fn pending(&self) -> usize {
        *self.0.0.lock().expect("exit gate")
    }
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut count = self.0.0.lock().expect("exit gate");
        while *count != 0 {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            count = self
                .0
                .1
                .wait_timeout(count, deadline - now)
                .expect("exit gate")
                .0;
        }
        true
    }
    pub(super) fn enter(&self) -> DrainGuard {
        *self.0.0.lock().expect("exit gate") += 1;
        DrainGuard(self.clone())
    }
}
pub(super) struct DrainGuard(ExitGate);
impl Drop for DrainGuard {
    fn drop(&mut self) {
        *self.0.0.0.lock().expect("exit gate") -= 1;
        self.0.0.1.notify_all();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    Storage(StorageError),
    HttpClient,
    Worker,
}

/// The single service/cache assembly for an application. All clones use its existing executor.
pub struct Backend {
    pub accounts: Arc<AccountService>,
    pub executor: Arc<BackgroundExecutor>,
    pub cache_open_error: Option<CacheError>,
    jobs: Mutex<Vec<tokio::task::AbortHandle>>,
    gate: ExitGate,
    closed: std::sync::atomic::AtomicBool,
}
impl Backend {
    /// Adopt an already assembled service. The caller must pass the SAME executor used by it.
    pub fn new(
        accounts: AccountService,
        executor: Arc<BackgroundExecutor>,
        gate: ExitGate,
    ) -> Self {
        Self {
            accounts: Arc::new(accounts),
            executor,
            cache_open_error: None,
            jobs: Mutex::new(Vec::new()),
            gate,
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn exit_gate(&self) -> ExitGate {
        self.gate.clone()
    }
    pub fn spawn<T: Send + 'static>(
        &self,
        work: impl Future<Output = T> + Send + 'static,
    ) -> Result<tokio::task::JoinHandle<T>, SpawnError> {
        let mut jobs = self.jobs.lock().expect("runtime jobs");
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(SpawnError);
        }
        let job = self.executor.handle().spawn(work)?;
        jobs.retain(|j| !j.is_finished());
        jobs.push(job.abort_handle());
        Ok(job)
    }
    pub fn shutdown(&self) {
        let mut jobs = self.jobs.lock().expect("runtime jobs");
        if self.closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        // Register synchronously before spawning, including when the final owner is the UI thread.
        let draining = self.gate.enter();
        let accounts = self.accounts.clone();
        let executor = self.executor.clone();
        let jobs = std::mem::take(&mut *jobs);
        std::thread::spawn(move || {
            // close marks the commit gate closed before awaiting the blocking commit lock. It
            // MUST finish before aborting futures: submitted credential commits are uninterruptible.
            let (tx, rx) = std::sync::mpsc::channel();
            let closing_accounts = accounts.clone();
            if executor
                .handle()
                .spawn(async move {
                    let _ = tx.send(closing_accounts.close().await);
                })
                .is_ok()
            {
                let _ = rx.recv();
            }
            for job in jobs {
                job.abort();
            }
            drop(accounts);
            drop(executor);
            drop(draining);
        });
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Opens exactly one application cache. A cache failure preserves online account functionality.
/// Invoke on the background runtime, never block a native event callback on this future.
pub async fn open_with_cache(
    config: AccountDatabaseConfig,
    store: Arc<dyn CredentialStore>,
    client: Arc<GitHubClient>,
    executor: Arc<BackgroundExecutor>,
    cache: Result<CacheStore, CacheError>,
    gate: ExitGate,
) -> Result<Backend, OpenError> {
    let cache_open_error = cache.as_ref().err().cloned();
    let accounts = match cache {
        Ok(cache) => {
            AccountService::open_with_cache(config, store, client, executor.clone(), cache).await
        }
        Err(_) => AccountService::open(config, store, client, executor.clone()).await,
    }
    .map_err(OpenError::Storage)?;
    let mut backend = Backend::new(accounts, executor, gate);
    backend.cache_open_error = cache_open_error;
    Ok(backend)
}

/// Run by RuntimeHandle's startup worker; retains existing application paths and credential targets.
pub(super) fn open_production(gate: ExitGate) -> Result<Backend, OpenError> {
    let executor = Arc::new(BackgroundExecutor::new(4));
    let config = AccountDatabaseConfig::application_data().map_err(OpenError::Storage)?;
    #[cfg(target_os = "windows")]
    let store: Arc<dyn CredentialStore> = Arc::new(crate::auth::WindowsCredentialStore::new());
    #[cfg(not(target_os = "windows"))]
    let store: Arc<dyn CredentialStore> = Arc::new(crate::auth::credential::UnsupportedStore);
    let transport = Arc::new(
        ReqwestTransport::new(Duration::from_secs(30)).map_err(|_| OpenError::HttpClient)?,
    );
    let client = Arc::new(GitHubClient::new(GitHubConfig::default(), transport));
    let worker_executor = executor.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    executor
        .handle()
        .spawn(async move {
            let cache = match CacheConfig::application_cache() {
                Ok(config) => tokio::task::spawn_blocking(move || CacheStore::open(config))
                    .await
                    .unwrap_or(Err(CacheError::Io)),
                Err(error) => Err(error),
            };
            let result = open_with_cache(config, store, client, worker_executor, cache, gate).await;
            let _ = tx.send(result);
        })
        .map_err(|_| OpenError::Worker)?;
    rx.recv().map_err(|_| OpenError::Worker)?
}
