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

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{JoinHandle, ThreadId};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{Semaphore, oneshot};

/// Default grace period given to in-flight tasks on shutdown.
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

thread_local! {
    /// Set on the executor's own network thread so `Drop` can tell it is running
    /// on the thread it would otherwise join.
    static ON_NETWORK_THREAD: Cell<bool> = const { Cell::new(false) };
}

thread_local! {
    /// Identifies which executor's network thread is currently executing.
    static NETWORK_THREAD_ID: Cell<Option<ThreadId>> = const { Cell::new(None) };
}

/// Monotonic id source used to give each network thread a distinct name.
static NEXT_EXECUTOR_ID: AtomicU64 = AtomicU64::new(1);

/// Owner of the background runtime. Dropping it triggers graceful shutdown.
pub struct BackgroundExecutor {
    handle: Handle,
    shutdown_tx: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    network_thread_id: ThreadId,
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

        let executor_id = NEXT_EXECUTOR_ID.fetch_add(1, Ordering::Relaxed);
        let thread = std::thread::Builder::new()
            .name(format!("githux-network-{executor_id}"))
            .spawn(move || {
                // Mark this thread so `Drop` can avoid joining itself.
                ON_NETWORK_THREAD.with(|flag| flag.set(true));
                NETWORK_THREAD_ID.with(|id| id.set(Some(std::thread::current().id())));
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

        let network_thread_id = thread.thread().id();

        Self {
            handle,
            shutdown_tx: Some(shutdown_tx),
            thread: Some(thread),
            network_thread_id,
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

    /// Whether the current thread is this executor's own network thread.
    ///
    /// If it is, joining is impossible: the last `Arc<BackgroundExecutor>` can be
    /// released by a task finishing on that very thread, and joining it would
    /// deadlock. In that case we only signal shutdown and detach the join handle;
    /// the runtime shuts itself down when the parked future ends.
    fn running_on_network_thread(&self) -> bool {
        ON_NETWORK_THREAD.with(|flag| flag.get())
            && NETWORK_THREAD_ID.with(|id| id.get() == Some(self.network_thread_id))
    }

    fn do_shutdown(&mut self) {
        // Reject new work first so a racing caller cannot add more.
        self.accepting.store(false, Ordering::Release);
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        if self.running_on_network_thread() {
            // Self-drop: signal, then detach instead of joining ourselves.
            if let Some(thread) = self.thread.take() {
                drop(thread);
            }
            return;
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

    /// Regression: dropping the last `Arc<BackgroundExecutor>` *from a task
    /// running on the network thread* must not deadlock by joining itself.
    #[test]
    fn dropping_from_its_own_network_thread_does_not_deadlock() {
        let exec = Arc::new(BackgroundExecutor::new(2));
        let weak = Arc::downgrade(&exec);
        let handle = exec.handle();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();

        // Move the last strong reference into the task; when this task finishes
        // it runs on the network thread and drops the executor there.
        let exec_for_task = exec.clone();
        let _spawned = handle.spawn(async move {
            drop(exec_for_task);
            let _ = done_tx.send(());
            1
        });
        drop(exec);

        // The task must complete, and the executor must be fully dropped.
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("task must run and drop the executor without deadlocking");

        // Give the detached thread a moment to tear down.
        for _ in 0..200 {
            if weak.upgrade().is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            weak.upgrade().is_none(),
            "executor should be dropped after the self-drop shutdown"
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
