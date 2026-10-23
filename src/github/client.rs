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
    pub fn url_is_trusted(&self, url: &str) -> bool {
        let base_host = extract_host(&self.config.base_url);
        let base_scheme = extract_scheme(&self.config.base_url);
        let host = extract_host(url);
        let scheme = extract_scheme(url);
        if host.is_empty() || host != base_host {
            return false;
        }
        scheme == base_scheme && !scheme.is_empty()
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }
}

fn extract_scheme(url: &str) -> &str {
    if let Some(end) = url.find("://") {
        &url[..end]
    } else {
        ""
    }
}

fn extract_host(url: &str) -> &str {
    let after_scheme = if let Some(idx) = url.find("://") {
        &url[idx + 3..]
    } else {
        url
    };
    let after_userinfo = after_scheme
        .find('@')
        .map(|i| &after_scheme[i + 1..])
        .unwrap_or(after_scheme);
    after_userinfo
        .find(['/', ':', '?', '#'])
        .map(|i| &after_userinfo[..i])
        .unwrap_or(after_userinfo)
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
