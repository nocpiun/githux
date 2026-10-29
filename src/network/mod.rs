//! Transport abstraction, background execution and cancellation for GitHub reads.
//!
//! Design boundaries:
//! - The background executor owns a dedicated Tokio runtime on its own thread.
//!   No reqwest future is ever polled on the GPUI scheduler, which does not host
//!   a Tokio reactor.
//! - Requests are cancellable and bounded by a concurrency semaphore.
//! - The executor drains in-flight work on [`executor::BackgroundExecutor::shutdown`]
//!   and joins the runtime thread without orphaned infinite retries.
//! - [`transport::Transport`] is trait-object-friendly so tests inject a local
//!   mock instead of depending on live GitHub network or real credentials.
//!
//! Decision (2026-10-23): use `reqwest` + `tokio`. Reqwest provides HTTP/2,
//! redirect control and a stable async surface; Tokio owns the reactor. GPUI's
//! own scheduler is smol-based and does not drive Tokio futures, so we isolate
//! networking in a separate runtime and bridge results back via channels (the
//! GPUI integration point is documented in `docs/auth-and-network.md`). This
//! slice only exercises the background runtime through tests, not the live app.
//!
//! Status: first reusable R2 slice. Not yet wired into UI.

// The GPUI shell does not yet consume this service layer (that is the separate
// UI-integration task). This module-scoped allowance is limited to these
// not-yet-called modules so real warnings stay visible elsewhere; the crate
// root deliberately has no `#![allow]` blanket.
#![allow(dead_code)]

pub mod cancel;
pub mod executor;
pub mod reader_images;
#[cfg(test)]
pub mod test_server;
pub mod transport;

#[allow(unused_imports)]
pub use cancel::CancellationToken;
#[allow(unused_imports)]
pub use executor::BackgroundExecutor;
#[allow(unused_imports)]
pub use transport::{
    HttpMethod, HttpRequest, HttpResponse, MockTransport, RateLimitHeaders, Transport,
    TransportError,
};
