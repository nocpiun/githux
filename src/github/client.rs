//! GitHub REST client construction and trusted-host enforcement.

use crate::auth::token::Pat;
use crate::network::transport::{HttpRequest, Transport};
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
        let url = if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
            path_or_url.to_string()
        } else {
            format!(
                "{}{}",
                self.config.base_url,
                path_or_url.trim_start_matches('/')
            )
        };

        let mut req = HttpRequest::get(&url)
            .with_header("User-Agent", &self.config.user_agent)
            .with_header("Accept", &self.config.accept)
            .with_header("X-GitHub-Api-Version", &self.config.api_version);

        if self.url_is_trusted(&url) {
            req = req.with_header("Authorization", format!("Bearer {}", token.expose()));
        }

        req
    }

    /// A URL is trusted for credential injection iff it is HTTPS on the same
    /// host as the configured base URL. Localhost HTTP is trusted only when the
    /// base URL itself is a localhost mock (test mode).
    ///
    /// The URL is parsed structurally: scheme, optional userinfo, host and port
    /// are taken from the authority component only, so a path or query that
    /// happens to contain `@` or a host-looking string can never make an
    /// unrelated host look trusted.
    pub fn url_is_trusted(&self, url: &str) -> bool {
        let Some(candidate) = Authority::parse(url) else {
            return false;
        };
        let Some(base) = Authority::parse(&self.config.base_url) else {
            return false;
        };
        candidate.scheme == base.scheme && candidate.host == base.host
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }
}

/// Structural URL authority: scheme and host (port excluded from trust
/// comparison because the configured base URL determines the endpoint, while a
/// differing port on the same host is still the same origin family for GitHub's
/// purposes only if the base URL itself uses it).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Authority {
    scheme: String,
    host: String,
}

impl Authority {
    /// Parse `<scheme>://[userinfo@]host[:port][/path][?query][#fragment]`.
    /// Returns `None` for input without a scheme, so relative or malformed URLs
    /// are never considered trusted.
    fn parse(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        if scheme.is_empty() {
            return None;
        }

        // The authority ends at the first '/', '?' or '#'.
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];

        // Userinfo is terminated by the *last* '@' inside the authority. This is
        // bounded to the authority, so an '@' in the path cannot affect it.
        let host_port = match authority.rsplit_once('@') {
            Some((_userinfo, host_port)) => host_port,
            None => authority,
        };
        if host_port.is_empty() {
            return None;
        }

        // Strip a port. Handle bracketed IPv6 literals defensively: they contain
        // ':' but the closing ']' delimits the host.
        let host = if let Some(close) = host_port.strip_prefix('[') {
            let end = close.find(']')?;
            &close[..end]
        } else {
            host_port.split(':').next().unwrap_or(host_port)
        };
        if host.is_empty() {
            return None;
        }

        Some(Self {
            scheme: scheme.to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
        })
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
        ] {
            assert!(
                !client.url_is_trusted(hostile),
                "{hostile} must not be trusted"
            );
            let request = client.build_get(hostile, &pat);
            assert!(
                !request.headers.iter().any(|(k, _)| k == "Authorization"),
                "{hostile} must not receive Authorization"
            );
        }

        // Real userinfo on the real host is still the real host.
        assert!(client.url_is_trusted("https://someone@api.github.com/user"));
    }

    #[test]
    fn localhost_mock_is_trusted_only_when_base_is_localhost() {
        let cfg = GitHubConfig {
            base_url: "http://127.0.0.1:0/".to_string(),
            ..GitHubConfig::default()
        };
        let client = GitHubClient::new(
            cfg,
            Arc::new(crate::network::transport::MockTransport::new()),
        );
        assert!(client.url_is_trusted("http://127.0.0.1:12345/user"));
    }
}
