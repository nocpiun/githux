//! Lightweight cooperative cancellation token backed by a `tokio::watch`.
//!
//! A token can be cloned freely; cancelling any [`CancellationTokenSource`]
//! clone cancels every token created from the same pair. Request tasks
//! `select!` on [`CancellationToken::cancelled`] to stop early.
//!
//! The token keeps its own sender alive so a dropped source never masquerades as
//! a cancellation. Only an explicit [`CancellationTokenSource::cancel`] fires.

use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
pub struct CancellationToken {
    rx: watch::Receiver<bool>,
    /// Keeps the channel open even if all sources are dropped, so `cancelled()`
    /// only resolves on an explicit cancel rather than a closed channel.
    _keepalive: Arc<watch::Sender<bool>>,
}

impl CancellationToken {
    pub fn new() -> (Self, CancellationTokenSource) {
        let (tx, rx) = watch::channel(false);
        let tx = Arc::new(tx);
        (
            Self {
                rx,
                _keepalive: tx.clone(),
            },
            CancellationTokenSource { tx },
        )
    }

    /// Resolves when the token has been cancelled. Never resolves if the token
    /// is never cancelled (it is not the same as "all senders dropped").
    pub async fn cancelled(mut self) {
        if *self.rx.borrow() {
            return;
        }
        // A closed channel means every sender (including our keepalive) is gone,
        // which cannot happen while `self` is alive; treat it as never-cancelled.
        loop {
            match self.rx.changed().await {
                Ok(()) => {
                    if *self.rx.borrow() {
                        return;
                    }
                }
                Err(_) => std::future::pending::<()>().await,
            }
        }
    }

    /// Non-blocking check.
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }
}

impl Default for CancellationToken {
    /// A token that can never be cancelled. Used where a call site has no
    /// cancellation source.
    fn default() -> Self {
        Self::new().0
    }
}

/// The cancel handle. Dropping it does **not** cancel, so a spawned task can
/// outlive the spawner. Only an explicit [`CancellationTokenSource::cancel`]
/// triggers cancellation.
pub struct CancellationTokenSource {
    tx: Arc<watch::Sender<bool>>,
}

impl CancellationTokenSource {
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }
}

impl Clone for CancellationTokenSource {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancel_resolves_cancelled() {
        let (token, source) = CancellationToken::new();
        assert!(!token.is_cancelled());
        source.cancel();
        token.clone().cancelled().await;
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn clone_shares_cancellation() {
        let (a, source) = CancellationToken::new();
        let b = a.clone();
        source.cancel();
        a.cancelled().await;
        assert!(b.is_cancelled());
    }

    #[tokio::test]
    async fn default_never_cancels_and_dropped_source_does_not_cancel() {
        let token = CancellationToken::default();
        assert!(!token.is_cancelled());
        let (token2, source) = CancellationToken::new();
        drop(source);
        // Neither should resolve within a short window.
        let joined = tokio::time::timeout(Duration::from_millis(30), async {
            tokio::join!(token.cancelled(), token2.cancelled());
        })
        .await;
        assert!(joined.is_err(), "an uncancelled token must not resolve");
    }
}
