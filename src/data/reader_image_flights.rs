//! Per-session in-flight reads. Completed/failed values never become an extra byte cache.
use crate::{
    content::images::ImageFailure,
    network::{
        cancel::{CancellationToken, CancellationTokenSource},
        executor::BackgroundExecutor,
    },
};
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::watch;

type Outcome<T> = Result<Arc<T>, ImageFailure>;
struct Flight<T> {
    result: watch::Sender<Option<Outcome<T>>>,
    waiters: AtomicUsize,
    cancel: CancellationToken,
    source: CancellationTokenSource,
}
struct Waiter<T> {
    flight: Arc<Flight<T>>,
    map: Arc<Mutex<HashMap<String, Arc<Flight<T>>>>>,
    key: String,
}
impl<T> Drop for Waiter<T> {
    fn drop(&mut self) {
        // Joining and last-waiter cancellation share the map lock. Otherwise
        // a new subscriber could increment 0 -> 1 between fetch_sub and cancel.
        let mut map = self.map.lock().expect("image flights");
        if self.flight.waiters.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.flight.source.cancel();
            if map
                .get(&self.key)
                .is_some_and(|flight| Arc::ptr_eq(flight, &self.flight))
            {
                map.remove(&self.key);
            }
        }
    }
}
pub(super) struct Flights<T>(Arc<Mutex<HashMap<String, Arc<Flight<T>>>>>);
impl<T> Default for Flights<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}
impl<T: Send + Sync + 'static> Flights<T> {
    #[cfg(test)]
    pub(super) fn waiters(&self, key: &str) -> usize {
        self.0
            .lock()
            .expect("image flights")
            .get(key)
            .map_or(0, |flight| flight.waiters.load(Ordering::Acquire))
    }

    pub async fn read<F, Fut>(
        &self,
        key: String,
        executor: &BackgroundExecutor,
        cancel: CancellationToken,
        timeout: std::time::Duration,
        work: F,
    ) -> Outcome<T>
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ImageFailure>> + Send + 'static,
    {
        let (flight, leader) = {
            let mut map = self.0.lock().expect("image flights");
            if let Some(flight) = map.get(&key).filter(|f| !f.cancel.is_cancelled()) {
                flight.waiters.fetch_add(1, Ordering::AcqRel);
                (flight.clone(), false)
            } else {
                let (cancel, source) = CancellationToken::new();
                let (result, _) = watch::channel(None);
                let flight = Arc::new(Flight {
                    result,
                    waiters: AtomicUsize::new(1),
                    cancel,
                    source,
                });
                map.insert(key.clone(), flight.clone());
                (flight, true)
            }
        };
        let _waiter = Waiter {
            flight: flight.clone(),
            map: self.0.clone(),
            key: key.clone(),
        };
        let mut result = flight.result.subscribe();
        if leader {
            let map = self.0.clone();
            let running = flight.clone();
            let task = executor.handle().spawn(async move {
                let outcome = tokio::select! {
                    biased;
                    _ = running.cancel.clone().cancelled() => Err(ImageFailure::Cancelled),
                    outcome = tokio::time::timeout(timeout, work(running.cancel.clone())) => outcome.unwrap_or(Err(ImageFailure::Timeout)).map(Arc::new),
                };
                running.result.send_replace(Some(outcome));
                let mut map = map.lock().expect("image flights");
                if map.get(&key).is_some_and(|f| Arc::ptr_eq(f, &running)) { map.remove(&key); }
            });
            if task.is_err() {
                flight.result.send_replace(Some(Err(ImageFailure::Network)));
            }
        }
        loop {
            if cancel.is_cancelled() {
                return Err(ImageFailure::Cancelled);
            }
            if let Some(outcome) = result.borrow().as_ref() {
                return outcome.clone();
            }
            tokio::select! {
                biased;
                _ = cancel.clone().cancelled() => return Err(ImageFailure::Cancelled),
                changed = result.changed() => if changed.is_err() { return Err(ImageFailure::Network); },
            }
        }
    }
}
