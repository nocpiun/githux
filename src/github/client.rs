//! GitHub REST client construction and trusted-host enforcement.

use crate::auth::token::Pat;
use crate::network::transport::{HttpMethod, HttpRequest, Transport};
use std::sync::Arc;

/// Configuration for the GitHub client.
#[derive(Debug, Clone)]
pub struct GitHubConfig {
    /// Base URL including trailing slash, e.g. `https://api.github.com/`.
    /// Tests override with a localhost mock URL.
    pub base_url: String,
    pub user_agent: String,
    pub api_version: String,
    pub accept: String,
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.github.com/".to_string(),
            user_agent: "githux/0.1 (+https://github.com/nocpiun/githux)".to_string(),
            api_version: "2022-11-28".to_string(),
            accept: "application/vnd.github+json".to_string(),
        }
    }
}

/// GitHub REST client. Holds no secrets beyond the lifetime of a single request.
pub struct GitHubClient {
    config: GitHubConfig,
    transport: Arc<dyn Transport>,
}

impl GitHubClient {
    pub fn new(config: GitHubConfig, transport: Arc<dyn Transport>) -> Self {
        Self { config, transport }
    }

    pub fn config(&self) -> &GitHubConfig {
        &self.config
    }

    /// Build a GET request with fixed headers and an Authorization header
    /// injected only when the target URL belongs to the trusted host (HTTPS
    /// api.github.com). Pagination `next` URLs and any redirect target are
    /// validated by the caller via [`Self::url_is_trusted`].
    pub fn build_get(&self, path_or_url: &str, token: &Pat) -> HttpRequest {
        self.build(
            HttpMethod::Get,
            path_or_url,
            token,
            &self.config.accept,
            None,
        )
    }

    /// Build a read-only GraphQL POST request. The caller supplies the JSON body
    /// (a fixed query plus variables); this client never builds or replays a
    /// mutation. Authorization is injected only for the trusted origin, exactly
    /// as for GET, and the transport refuses to follow a POST redirect.
    pub fn build_post(&self, path_or_url: &str, token: &Pat, body: String) -> HttpRequest {
        self.build(
            HttpMethod::Post,
            path_or_url,
            token,
            "application/json",
            Some(body),
        )
        .with_header("Content-Type", "application/json")
    }

    fn build(
        &self,
        method: HttpMethod,
        path_or_url: &str,
        token: &Pat,
        accept: &str,
        body: Option<String>,
    ) -> HttpRequest {
        let url = if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
            path_or_url.to_string()
        } else {
            format!(
                "{}{}",
                self.config.base_url,
                path_or_url.trim_start_matches('/')
            )
        };

        let mut req = match method {
            HttpMethod::Get => HttpRequest::get(&url),
            HttpMethod::Post => HttpRequest::post(&url, body.unwrap_or_default()),
        }
        .with_header("User-Agent", &self.config.user_agent)
        .with_header("Accept", accept)
        .with_header("X-GitHub-Api-Version", &self.config.api_version);

        if self.url_is_trusted(&url) {
            req = req.with_header("Authorization", format!("Bearer {}", token.expose()));
        }

        req
    }

    /// A URL is trusted for credential injection iff it has the same scheme and
    /// host as the configured base URL. Localhost HTTP is trusted only when the
    /// base URL itself is a localhost mock (test mode).
    ///
    /// Parsing deliberately uses the **same** standards-compliant URL parser that
    /// reqwest uses ([`reqwest::Url`]), not a hand-rolled one. A custom parser
    /// inevitably disagrees somewhere (for example, WHATWG treats `\` as a path
    /// separator for special schemes, so `https://evil.example\@api.github.com/x`
    /// resolves to host `evil.example` while a naive split on `@` would say
    /// `api.github.com`). Using one parser for both the trust decision and the
    /// actual connection removes that class of mismatch.
    pub fn url_is_trusted(&self, url: &str) -> bool {
        let Ok(candidate) = reqwest::Url::parse(url) else {
            return false;
        };
        let Ok(base) = reqwest::Url::parse(&self.config.base_url) else {
            return false;
        };
        // `host_str` is `None` for hosts that cannot be compared (e.g. `file:`),
        // which must never count as trusted. The port must also match (using the
        // scheme's default when absent) so a `:8443` variant on the same host is
        // treated as a different origin rather than receiving the credential.
        match (candidate.host_str(), base.host_str()) {
            (Some(candidate_host), Some(base_host)) => {
                candidate.scheme() == base.scheme()
                    && candidate_host == base_host
                    && candidate.port_or_known_default() == base.port_or_known_default()
            }
            _ => false,
        }
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_host_logic() {
        let cfg = GitHubConfig::default();
        let client = GitHubClient::new(
            cfg,
            Arc::new(crate::network::transport::MockTransport::new()),
        );
        assert!(client.url_is_trusted("https://api.github.com/user"));
        assert!(!client.url_is_trusted("http://api.github.com/user"));
        assert!(!client.url_is_trusted("https://evil.com/user"));
        assert!(!client.url_is_trusted("https://api.github.com.evil.com/user"));
    }

    #[test]
    fn authorization_only_for_trusted_host() {
        let cfg = GitHubConfig::default();
        let client = GitHubClient::new(
            cfg,
            Arc::new(crate::network::transport::MockTransport::new()),
        );
        let pat = Pat::from_str("ghp_secret123");
        let trusted = client.build_get("user", &pat);
        assert!(
            trusted
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer ghp_secret123")
        );

        // A non-trusted absolute URL must not carry Authorization.
        let untrusted = client.build_get("https://evil.com/steal", &pat);
        assert!(!untrusted.headers.iter().any(|(k, _)| k == "Authorization"));
    }

    /// A GraphQL POST is a POST with a JSON content type, and Authorization is
    /// injected only for the trusted origin.
    #[test]
    fn post_authorization_only_for_trusted_host() {
        let client = GitHubClient::new(
            GitHubConfig::default(),
            Arc::new(crate::network::transport::MockTransport::new()),
        );
        let pat = Pat::from_str("ghp_secret123");
        let trusted = client.build_post("graphql", &pat, "{}".to_string());
        assert_eq!(trusted.method, HttpMethod::Post);
        assert!(
            trusted
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer ghp_secret123")
        );
        assert!(
            trusted
                .headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/json")
        );

        let untrusted = client.build_post("https://evil.com/graphql", &pat, "{}".to_string());
        assert!(!untrusted.headers.iter().any(|(k, _)| k == "Authorization"));
    }

    /// Regression: an `@` or host-looking text in the *path* must not make an
    /// attacker host look trusted. The earlier implementation searched for
    /// userinfo across the whole URL remainder, so
    /// `https://attacker.example/https://api.github.com/user` was mis-parsed and
    /// the PAT was injected toward `attacker.example`.
    #[test]
    fn path_content_does_not_make_a_host_trusted() {
        let cfg = GitHubConfig::default();
        let client = GitHubClient::new(
            cfg,
            Arc::new(crate::network::transport::MockTransport::new()),
        );
        let pat = Pat::from_str("ghp_secret123");
        for hostile in [
            "https://attacker.example/https://api.github.com/user",
            "https://attacker.example/api.github.com/user",
            "https://attacker.example/?next=https://api.github.com/user",
            "https://someuser@attacker.example/user",
            "https://attacker.example#@api.github.com/user",
            "https://api.github.com.evil.example/user",
            "http://api.github.com/user",
            // WHATWG URL parsing treats `\\` as a path separator for special
            // schemes, so this connects to evil.example even though it contains
            // `@api.github.com`.
            "https://evil.example\\@api.github.com/user",
            "https://evil.example\\\\api.github.com/user",
            // Scheme-relative and non-http forms must not be trusted either.
            "//api.github.com/user",
            "ftp://api.github.com/user",
            "https://api.github.com:8443/user",
        ] {
            assert!(
                !client.url_is_trusted(hostile),
                "{hostile} must not be trusted"
            );
            // An absolute URL is used verbatim, so it must never carry the
            // credential. A non-absolute string is treated as a path relative to
            // the trusted base, which is safe by construction.
            if hostile.starts_with("http://") || hostile.starts_with("https://") {
                let request = client.build_get(hostile, &pat);
                assert!(
                    !request.headers.iter().any(|(k, _)| k == "Authorization"),
                    "{hostile} must not receive Authorization"
                );
            }
        }

        // Real userinfo on the real host is still the real host.
        assert!(client.url_is_trusted("https://someone@api.github.com/user"));
    }

    #[test]
    fn localhost_mock_is_trusted_only_on_the_configured_origin() {
        let cfg = GitHubConfig {
            base_url: "http://127.0.0.1:54321/".to_string(),
            ..GitHubConfig::default()
        };
        let client = GitHubClient::new(
            cfg,
            Arc::new(crate::network::transport::MockTransport::new()),
        );
        // Same scheme, host and port as the configured mock base.
        assert!(client.url_is_trusted("http://127.0.0.1:54321/user"));
        // A different port is a different origin and must not receive the PAT.
        assert!(!client.url_is_trusted("http://127.0.0.1:12345/user"));
        // HTTPS on the localhost mock port is a scheme mismatch.
        assert!(!client.url_is_trusted("https://127.0.0.1:54321/user"));
    }
}
