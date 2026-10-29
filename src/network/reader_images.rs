//! Credential-free HTTPS image fetching.
//!
//! README pictures may point at any public host, so this path is deliberately
//! separate from the authenticated GitHub transport:
//!
//! * No `Authorization`, `Cookie` or `Proxy-Authorization` header is ever
//!   attached, and the client runs with proxies disabled so a system proxy's
//!   credentials cannot travel either.
//! * Only `https` addresses are fetched in production. A plain `http` address
//!   is refused before any connection; the test-only constructor can allow
//!   loopback `http` so real sockets can be exercised without TLS.
//! * Redirects are followed manually, one hop at a time. Each target is parsed
//!   with the same URL parser the client connects with, and must stay `https`
//!   with no userinfo and no loopback/link-local host. `file:`, `data:`,
//!   `javascript:`, UNC and local paths can never become a request.
//! * The body is bounded while it streams, and one wall-clock deadline covers
//!   every redirect hop and the body read.

use std::pin::Pin;
use std::time::Duration;

use crate::network::cancel::CancellationToken;

/// Budgets for one external picture fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalImageLimits {
    /// Largest accepted body, enforced while chunks arrive.
    pub max_bytes: usize,
    /// Wall-clock deadline covering redirects, headers and the whole body.
    pub timeout: Duration,
    /// Maximum redirect hops followed.
    pub max_redirects: usize,
}

impl Default for ExternalImageLimits {
    fn default() -> Self {
        Self {
            max_bytes: 5 * 1024 * 1024,
            timeout: Duration::from_secs(20),
            max_redirects: 5,
        }
    }
}

/// Why an external address cannot be fetched at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalRefusal {
    /// A non-HTTPS address.
    Insecure,
    /// A scheme other than `http`/`https`.
    UnsupportedScheme,
    /// A missing host, userinfo, or a loopback/link-local target.
    LocalSource,
    /// The address does not parse as a URL.
    InvalidUrl,
    /// The redirect chain exceeded the configured hop budget.
    TooManyRedirects,
}

/// Failure of one external picture fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalImageError {
    Refused(ExternalRefusal),
    /// A non-success HTTP status.
    Status(u16),
    Timeout,
    Network,
    /// The streamed body exceeded the byte budget.
    TooLarge {
        limit: usize,
    },
    Cancelled,
}

/// Fetches raw bytes for one external picture address.
///
/// Implementations must never attach a credential. The future is `Send` so it
/// can be driven by the project's background executor.
pub trait ExternalImageBytes: Send + Sync + 'static {
    fn load<'a>(
        &'a self,
        url: &'a str,
        limits: &'a ExternalImageLimits,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ExternalImageError>> + Send + 'a>>;
}

/// The production fetcher: its own `reqwest` client with redirects disabled and
/// no proxy configuration.
pub struct ReqwestExternalImages {
    client: reqwest::Client,
    /// Allows loopback `http` for real-socket tests. Never enabled in
    /// production construction.
    allow_local_http: bool,
}

impl ReqwestExternalImages {
    /// A production fetcher. Only `https` addresses are accepted.
    pub fn new() -> Result<Self, ExternalImageError> {
        Self::build(false)
    }

    fn build(allow_local_http: bool) -> Result<Self, ExternalImageError> {
        // Every DNS answer is checked against the same local/private policy as
        // the URL host, so a public-looking name cannot point the client at an
        // internal address. The test constructor allows loopback on purpose.
        let resolver = GuardResolver {
            allow_local: allow_local_http,
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // No system proxy: neither the request nor its credentials can be
            // forwarded to a proxy the user configured for something else.
            .no_proxy()
            .dns_resolver(resolver)
            .build()
            .map_err(|_| ExternalImageError::Network)?;
        Ok(Self {
            client,
            allow_local_http,
        })
    }

    /// A fetcher that also accepts loopback `http`, for tests only.
    #[cfg(test)]
    pub fn with_local_http() -> Result<Self, ExternalImageError> {
        Self::build(true)
    }

    /// Whether `url` may be requested at all.
    fn check(&self, url: &reqwest::Url) -> Result<(), ExternalImageError> {
        match url.scheme() {
            "https" => {}
            "http" if self.allow_local_http => {}
            "http" => return Err(ExternalImageError::Refused(ExternalRefusal::Insecure)),
            _ => {
                return Err(ExternalImageError::Refused(
                    ExternalRefusal::UnsupportedScheme,
                ));
            }
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ExternalImageError::Refused(ExternalRefusal::LocalSource));
        }
        let host = url
            .host_str()
            .ok_or(ExternalImageError::Refused(ExternalRefusal::InvalidUrl))?;
        if is_local_host(host) && !self.allow_local_http {
            return Err(ExternalImageError::Refused(ExternalRefusal::LocalSource));
        }
        Ok(())
    }
}

/// Whether an address is the local machine, the local network or a link-local
/// destination. Applied to IP literals and to every DNS answer, so a hostname
/// that resolves to an internal address cannot bypass the boundary either.
fn is_local_address(addr: std::net::IpAddr) -> bool {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    let v4_local = |ip: &Ipv4Addr| {
        ip.is_loopback()
            || ip.is_unspecified()
            || ip.is_link_local()
            || ip.is_private()
            // 100.64.0.0/10, carrier-grade NAT.
            || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
            // 0.0.0.0/8, "this network".
            || ip.octets()[0] == 0
    };
    match addr {
        IpAddr::V4(ip) => v4_local(&ip),
        IpAddr::V6(ip) => {
            // An IPv4-mapped address reaches the same machine as its IPv4 form.
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return v4_local(&mapped);
            }
            let segments = ip.segments();
            // `fe80::/10` is link-local; `fec0::/10` is the deprecated
            // site-local range, refused for the same reason.
            let site_local = (segments[0] & 0xffc0) == 0xfec0;
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unicast_link_local()
                || site_local
                || ip == Ipv6Addr::LOCALHOST
        }
    }
}

/// Whether a host is the local machine or a link-local address. Parsed as an
/// address when possible; otherwise only the `localhost` name is treated as
/// local here. A hostname is finally judged by the addresses it resolves to —
/// see [`GuardResolver`].
fn is_local_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match bare.parse::<std::net::IpAddr>() {
        Ok(addr) => is_local_address(addr),
        Err(_) => false,
    }
}

/// A DNS resolver that refuses to resolve a name to any local/private address.
///
/// Checking only the URL's host is not enough: `https://internal.example/` can
/// resolve to `127.0.0.1` or `10.0.0.5`, and the client would then connect to a
/// destination the address policy forbids. This resolver applies the same policy
/// to every answer and fails the lookup when *any* answer is local, so a
/// subsequent resolution cannot bypass the check. The client connects only to
/// the addresses returned here.
struct GuardResolver {
    allow_local: bool,
}

impl reqwest::dns::Resolve for GuardResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let allow_local = self.allow_local;
        Box::pin(async move {
            // Port 0 is replaced by the connector with the scheme's default.
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?
                .collect::<Vec<std::net::SocketAddr>>();
            if addrs.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "image host resolved to no address",
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            if !allow_local
                && let Some(local) = addrs.iter().find(|addr| is_local_address(addr.ip()))
            {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("image host resolves to a local address ({})", local.ip()),
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

impl ExternalImageBytes for ReqwestExternalImages {
    fn load<'a>(
        &'a self,
        url: &'a str,
        limits: &'a ExternalImageLimits,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ExternalImageError>> + Send + 'a>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ExternalImageError::Cancelled);
            }
            let mut url = reqwest::Url::parse(url)
                .map_err(|_| ExternalImageError::Refused(ExternalRefusal::InvalidUrl))?;
            self.check(&url)?;
            let deadline = tokio::time::Instant::now() + limits.timeout;

            let mut hops = 0usize;
            loop {
                let request = self.client.get(url.clone()).header(
                    "Accept",
                    "image/png,image/jpeg,image/webp,image/gif,*/*;q=0.8",
                );
                let response = tokio::select! {
                    biased;
                    _ = cancel.clone().cancelled() => return Err(ExternalImageError::Cancelled),
                    response = tokio::time::timeout_at(deadline, request.send()) => match response {
                        Err(_) => return Err(ExternalImageError::Timeout),
                        Ok(Err(_)) => return Err(ExternalImageError::Network),
                        Ok(Ok(response)) => response,
                    },
                };

                let status = response.status().as_u16();
                if (300..400).contains(&status) {
                    if hops >= limits.max_redirects {
                        return Err(ExternalImageError::Refused(
                            ExternalRefusal::TooManyRedirects,
                        ));
                    }
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    let Some(location) = location else {
                        return Err(ExternalImageError::Status(status));
                    };
                    let next = url
                        .join(&location)
                        .map_err(|_| ExternalImageError::Refused(ExternalRefusal::InvalidUrl))?;
                    // Validate the hop with the same parser the client uses to
                    // connect; a refusal here means the target is never contacted.
                    self.check(&next)?;
                    url = next;
                    hops += 1;
                    continue;
                }
                if !response.status().is_success() {
                    return Err(ExternalImageError::Status(status));
                }
                if response
                    .content_length()
                    .is_some_and(|size| size > limits.max_bytes as u64)
                {
                    return Err(ExternalImageError::TooLarge {
                        limit: limits.max_bytes,
                    });
                }

                let mut response = response;
                let mut body = Vec::new();
                loop {
                    let chunk = tokio::select! {
                        biased;
                        _ = cancel.clone().cancelled() => return Err(ExternalImageError::Cancelled),
                        chunk = tokio::time::timeout_at(deadline, response.chunk()) => match chunk {
                            Err(_) => return Err(ExternalImageError::Timeout),
                            Ok(Err(_)) => return Err(ExternalImageError::Network),
                            Ok(Ok(chunk)) => chunk,
                        },
                    };
                    let Some(chunk) = chunk else {
                        break;
                    };
                    if chunk.len() > limits.max_bytes.saturating_sub(body.len()) {
                        return Err(ExternalImageError::TooLarge {
                            limit: limits.max_bytes,
                        });
                    }
                    body.extend_from_slice(&chunk);
                }
                return Ok(body);
            }
        })
    }
}

#[cfg(test)]
#[path = "reader_images_tests.rs"]
mod tests;
