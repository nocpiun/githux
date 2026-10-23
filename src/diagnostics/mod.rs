//! Redacted diagnostics and performance evidence.
//!
//! This layer provides helpers that strip secret material before any diagnostic
//! record is formed. It never receives tokens directly: the PAT wrapper
//! ([`crate::auth::token::Pat`]) has no `Debug`/`Display`/`Serialize` impl, so
//! formatting it always renders `Pat(<redacted>)`.
//!
//! Status: first reusable R2 slice. No telemetry or update mechanism selected.

// The GPUI shell does not yet consume this service layer (that is the separate
// UI-integration task). This module-scoped allowance is limited to these
// not-yet-called modules so real warnings stay visible elsewhere; the crate
// root deliberately has no `#![allow]` blanket.
#![allow(dead_code)]

/// A diagnostic record that can be safely logged. Contains no secrets.
#[derive(Debug, Clone)]
pub struct DiagnosticEvent {
    pub kind: DiagnosticKind,
    pub message: String,
    pub url_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    RequestStarted,
    RequestSucceeded,
    RequestFailed,
    AuthEpochAdvanced,
    CredentialSaved,
    CredentialDeleted,
}

/// Strip the query string and userinfo from a URL for safe logging. Path and
/// host are preserved because they are needed to correlate failures.
pub fn safe_url(url: &str) -> String {
    let no_fragment = url.split('#').next().unwrap_or(url);
    let no_query = no_fragment.split('?').next().unwrap_or(no_fragment);
    if let Some(scheme_end) = no_query.find("://") {
        let after = &no_query[scheme_end + 3..];
        if let Some(at) = after.find('@') {
            let host_part = &after[at + 1..];
            return format!("{}://{}", &no_query[..scheme_end], host_part);
        }
    }
    no_query.to_string()
}

impl DiagnosticEvent {
    pub fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            url_hint: None,
        }
    }

    pub fn with_url(mut self, url: &str) -> Self {
        self.url_hint = Some(safe_url(url));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_url_strips_query() {
        assert_eq!(
            safe_url("https://api.github.com/user/repos?page=2&token=secret"),
            "https://api.github.com/user/repos"
        );
    }

    #[test]
    fn safe_url_strips_userinfo() {
        assert_eq!(
            safe_url("https://user:pass@api.github.com/user"),
            "https://api.github.com/user"
        );
    }

    #[test]
    fn diagnostic_event_has_no_secrets() {
        let event = DiagnosticEvent::new(DiagnosticKind::RequestFailed, "timeout")
            .with_url("https://api.github.com/user/repos?token=ghp_leaked");
        let rendered = format!("{event:?}");
        assert!(!rendered.contains("ghp_leaked"));
    }
}
