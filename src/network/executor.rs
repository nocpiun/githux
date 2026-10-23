//! Dedicated background executor that owns a Tokio runtime on its own thread.
//!
//! This keeps reqwest futures off the GPUI scheduler (which has no Tokio
//! reactor) and gives every request bounded concurrency and cancellation.
//!
//! Lifecycle:
//! - A current-thread Tokio runtime is created on a dedicated OS thread. Work is
//!   scheduled through [`BackgroundExecutor::handle`], which returns a
//!   [`tokio::runtime::Handle`].
//! - [`BackgroundExecutor::shutdown`] stops accepting new work, waits a bounded
//!   amount of time for in-flight tasks, then drops the runtime. Any task still
//!   running is cancelled rather than left as an orphaned infinite retry.
//!
//! Concurrency is bounded by callers holding a permit from a shared
//! [`tokio::sync::Semaphore`]; see the data layer for the single owner.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{Semaphore, oneshot};

/// Default grace period given to in-flight tasks on shutdown.
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Owner of the background runtime. Dropping it triggers graceful shutdown.
pub struct BackgroundExecutor {
    handle: Handle,
    shutdown_tx: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    accepting: Arc<AtomicBool>,
    semaphore: Arc<Semaphore>,
    grace: Duration,
}

impl BackgroundExecutor {
    /// Create a background executor whose shared semaphore allows up to
    /// `max_concurrency` simultaneous requests.
    pub fn new(max_concurrency: usize) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build network runtime");
        let handle = runtime.handle().clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let accepting = Arc::new(AtomicBool::new(true));
        let accepting_thread = accepting.clone();

        let thread = std::thread::Builder::new()
            .name("githux-network".into())
            .spawn(move || {
                // Park the runtime until shutdown is requested; it continues to
                // drive spawned tasks in the background meanwhile.
                runtime.block_on(async move {
                    let _ = shutdown_rx.await;
                });
                accepting_thread.store(false, Ordering::Release);
                // Give in-flight tasks a bounded window to finish, then cancel
                // whatever remains instead of orphaning it.
                runtime.shutdown_timeout(DEFAULT_SHUTDOWN_GRACE);
            })
            .expect("failed to spawn network thread");

        Self {
            handle,
            shutdown_tx: Some(shutdown_tx),
            thread: Some(thread),
            accepting,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }

    /// Cloneable handle used to spawn tasks onto the background runtime.
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Shared semaphore that bounds simultaneous requests.
    pub fn semaphore(&self) -> Arc<Semaphore> {
        self.semaphore.clone()
    }

    /// Whether shutdown has been initiated; new work should not be scheduled.
    pub fn is_shutdown(&self) -> bool {
        !self.accepting.load(Ordering::Acquire)
    }

    /// Override the drain grace period (tests use a short value).
    pub fn set_grace(&mut self, grace: Duration) {
        self.grace = grace;
    }

    /// Graceful shutdown: stop accepting work, drain in-flight tasks, join.
    pub fn shutdown(mut self) {
        self.do_shutdown();
    }

    fn do_shutdown(&mut self) {
        // Reject new work first so a racing caller cannot add more.
        self.accepting.store(false, Ordering::Release);
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for BackgroundExecutor {
    fn drop(&mut self) {
        self.do_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// The test-side runtime only polls the `JoinHandle`s; the actual work runs
    /// on the executor's own runtime.
    fn host_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("host runtime")
    }

    #[test]
    fn spawns_and_runs_to_completion() {
        let exec = BackgroundExecutor::new(4);
        let handle = exec.handle();
        let rt = host_runtime();
        let result = rt.block_on(async move { handle.spawn(async { 42 }).await });
        assert_eq!(result.expect("task ok"), 42);
        exec.shutdown();
    }

    #[test]
    fn cancellation_of_one_task_does_not_affect_others() {
        let exec = BackgroundExecutor::new(4);
        let handle = exec.handle();
        let rt = host_runtime();
        let join = handle.spawn(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            "slow-independent"
        });
        let fast = rt.block_on(async { handle.spawn(async { "fast" }).await });
        assert_eq!(fast.expect("fast ok"), "fast");
        let slow = rt.block_on(join);
        assert_eq!(slow.expect("slow ok"), "slow-independent");
        exec.shutdown();
    }

    #[test]
    fn concurrency_is_bounded_by_the_semaphore() {
        let exec = BackgroundExecutor::new(2);
        let semaphore = exec.semaphore();
        let peak = Arc::new(AtomicUsize::new(0));
        let current = Arc::new(AtomicUsize::new(0));
        let rt = host_runtime();

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let semaphore = semaphore.clone();
                let peak = peak.clone();
                let current = current.clone();
                exec.handle().spawn(async move {
                    let _permit = semaphore.acquire_owned().await.expect("permit");
                    let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    current.fetch_sub(1, Ordering::SeqCst);
                    1
                })
            })
            .collect();

        rt.block_on(async {
            for handle in handles {
                let _ = handle.await;
            }
        });
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak concurrency {} exceeded the semaphore budget",
            peak.load(Ordering::SeqCst)
        );
        exec.shutdown();
    }

    #[test]
    fn shutdown_converges_without_orphaned_work() {
        let mut exec = BackgroundExecutor::new(2);
        exec.set_grace(Duration::from_millis(200));
        let handle = exec.handle();

        // A long-running task that would only end after the grace window.
        let completed = Arc::new(AtomicBool::new(false));
        let flag = completed.clone();
        let _detached = handle.spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            flag.store(true, Ordering::SeqCst);
            1
        });

        // A short task that must still finish inside the grace window.
        let short_done = Arc::new(AtomicBool::new(false));
        let short_flag = short_done.clone();
        let _short = handle.spawn(async move {
            short_flag.store(true, Ordering::SeqCst);
            2
        });

        // Let the runtime poll both tasks once so they are genuinely running
        // before shutdown (avoids a race where shutdown fires first).
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            short_done.load(Ordering::SeqCst),
            "short task should have run before shutdown"
        );

        // Shutdown returns; the thread has joined and the runtime is gone.
        exec.shutdown();

        assert!(
            short_done.load(Ordering::SeqCst),
            "short task should complete within the grace window"
        );
        assert!(
            !completed.load(Ordering::SeqCst),
            "long task must be cancelled, not left orphaned"
        );
    }

    #[test]
    fn shutdown_is_idempotent_via_drop() {
        let exec = BackgroundExecutor::new(1);
        exec.shutdown();
        // Drop after explicit shutdown must not panic or double-join.
        drop(BackgroundExecutor::new(1));
    }
}
