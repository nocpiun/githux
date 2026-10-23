//! Typed GitHub read errors distinguishing authentication, authorization,
//! rate-limit, transport and parse failures so that callers never confuse a
//! transient/network/partial failure with a genuinely empty result set.
//!
//! No variant embeds a token or an Authorization header. Debug and Display
//! output never include secret material.

use thiserror::Error;

/// Origin of a rate-limit signal, used to choose backoff and messaging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKind {
    /// Primary limit bucket (`X-RateLimit-Remaining: 0`).
    Primary,
    /// Secondary limit (server-side abuse detection / `Retry-After`).
    Secondary,
}

/// Optional backoff hint parsed from response headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryAfter {
    pub kind: RateLimitKind,
    /// Seconds the server asked the client to wait, when present.
    pub seconds: Option<u32>,
}

/// Typed errors from the GitHub read layer. A normal empty list is **not** an
/// error — it is returned as `ReposReadiness::Complete` with zero items.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GitHubError {
    /// 401 — the token is missing, invalid or revoked. Credential replacement
    /// flow applies; the account is not automatically removed.
    #[error("authentication failed (token invalid or revoked)")]
    Unauthorized,

    /// 403 caused by an exhausted rate-limit budget. Retrying after the backoff
    /// window may succeed; the token itself is still valid.
    #[error("rate limited ({:?}); retry after {:?} seconds", .retry.kind, .retry.seconds)]
    RateLimited { retry: RetryAfter },

    /// 403 with an `X-GitHub-SSO` header requiring per-organization SAML SSO
    /// authorization. The URL points to the (time-limited) authorization page.
    #[error("organization SSO authorization required for one or more organizations")]
    SsoRequired { authorization_url: Option<String> },

    /// 403 without rate-limit or SSO signals — permission scope or org policy
    /// denied the resource. Does **not** delete the token.
    #[error("forbidden (insufficient scope or organization policy)")]
    Forbidden,

    /// 404 — resource is not visible to the current token. Distinct from a
    /// genuine empty result and from authentication failure.
    #[error("resource not found or not visible to this account")]
    NotFound,

    /// HTTP 5xx or other unexpected status not covered above.
    #[error("unexpected HTTP status {status}")]
    UnexpectedStatus { status: u16 },

    /// Network/connect failure (DNS, TLS, refused, reset). Retrying when back
    /// online may succeed.
    #[error("network error: {message}")]
    Network { message: String },

    /// Request exceeded its timeout. The response (if any) is unknown.
    #[error("request timed out after {seconds}s")]
    Timeout { seconds: u32 },

    /// The caller cancelled the request before it completed.
    #[error("request cancelled")]
    Cancelled,

    /// Response body could not be parsed into the expected type.
    #[error("failed to parse response: {message}")]
    Parse { message: String },

    /// The authorization context that produced this response is stale (account
    /// switched, PAT replaced or account removed), so the result is rejected.
    #[error("stale authorization context; result discarded")]
    StaleContext,

    /// Credential store is unavailable on this platform or returned an error.
    #[error("credential store error: {message}")]
    CredentialStore { message: String },
}
