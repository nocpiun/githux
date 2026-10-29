//! Real-socket tests for the credential-free external image path.

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::*;
use crate::network::test_server::{LocalHttpServer, ScriptedResponse};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn load(
    fetcher: &ReqwestExternalImages,
    url: &str,
    limits: &ExternalImageLimits,
) -> Result<Vec<u8>, ExternalImageError> {
    runtime().block_on(fetcher.load(url, limits, CancellationToken::default()))
}

#[test]
fn non_https_and_local_schemes_are_refused_before_any_request() {
    let fetcher = ReqwestExternalImages::new().expect("client");
    let limits = ExternalImageLimits::default();
    for url in [
        "file:///etc/passwd",
        "data:image/png;base64,AAAA",
        "javascript:alert(1)",
        "ftp://example.com/a.png",
        "\\\\server\\share\\a.png",
        "https://user:secret@example.com/a.png",
        "https://localhost/a.png",
        "https://127.0.0.1/a.png",
        "https://[::1]/a.png",
    ] {
        let result = load(&fetcher, url, &limits);
        assert!(
            matches!(result, Err(ExternalImageError::Refused(_))),
            "{url} must be refused, got {result:?}"
        );
    }
    // Plain http is refused in production, even for loopback.
    assert_eq!(
        load(&fetcher, "http://127.0.0.1:1/a.png", &limits),
        Err(ExternalImageError::Refused(ExternalRefusal::Insecure))
    );
}

/// The request carries no credential and a same-origin redirect is followed.
#[test]
fn no_credential_is_sent_and_a_same_origin_redirect_is_followed() {
    let server = LocalHttpServer::start(vec![
        ScriptedResponse::json(302, "").with_header("Location", "/next"),
        ScriptedResponse::json(200, "png-bytes"),
    ]);
    let fetcher = ReqwestExternalImages::with_local_http().expect("client");
    let limits = ExternalImageLimits::default();
    let body = load(&fetcher, &format!("{}a.png", server.base_url()), &limits).expect("body");
    assert_eq!(body, b"png-bytes");

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 2);
    for request in &recorded {
        for name in ["authorization", "cookie", "proxy-authorization"] {
            assert!(
                request.header(name).is_none(),
                "external image request must not send {name}: {request:?}"
            );
        }
    }
}

/// A redirect to `file:` or `data:` is refused; the target is never contacted.
#[test]
fn a_redirect_to_a_local_scheme_is_refused() {
    for location in ["file:///etc/passwd", "data:image/png;base64,AAAA"] {
        let server = LocalHttpServer::start(vec![
            ScriptedResponse::json(302, "").with_header("Location", location),
        ]);
        let fetcher = ReqwestExternalImages::with_local_http().expect("client");
        let limits = ExternalImageLimits::default();
        let result = load(&fetcher, &format!("{}a.png", server.base_url()), &limits);
        assert!(
            matches!(result, Err(ExternalImageError::Refused(_))),
            "{location} must be refused, got {result:?}"
        );
        assert_eq!(
            server.recorded().len(),
            1,
            "the refused redirect target must not be contacted"
        );
    }
}

#[test]
fn a_redirect_loop_stops_at_the_hop_budget() {
    let server = LocalHttpServer::start(vec![
        ScriptedResponse::json(302, "").with_header("Location", "/again"),
        ScriptedResponse::json(302, "").with_header("Location", "/again"),
        ScriptedResponse::json(302, "").with_header("Location", "/again"),
    ]);
    let fetcher = ReqwestExternalImages::with_local_http().expect("client");
    let limits = ExternalImageLimits {
        max_redirects: 2,
        ..ExternalImageLimits::default()
    };
    let result = load(&fetcher, &format!("{}a.png", server.base_url()), &limits);
    assert_eq!(
        result,
        Err(ExternalImageError::Refused(
            ExternalRefusal::TooManyRedirects
        ))
    );
    assert_eq!(server.recorded().len(), 3);
}

#[test]
fn a_non_success_status_is_reported() {
    let server = LocalHttpServer::start(vec![ScriptedResponse::json(404, "missing")]);
    let fetcher = ReqwestExternalImages::with_local_http().expect("client");
    let limits = ExternalImageLimits::default();
    assert_eq!(
        load(&fetcher, &format!("{}a.png", server.base_url()), &limits),
        Err(ExternalImageError::Status(404))
    );
}

#[test]
fn an_oversized_content_length_is_rejected_before_the_body() {
    let server = LocalHttpServer::start(vec![ScriptedResponse::json(200, "0123456789")]);
    let fetcher = ReqwestExternalImages::with_local_http().expect("client");
    let limits = ExternalImageLimits {
        max_bytes: 4,
        ..ExternalImageLimits::default()
    };
    assert_eq!(
        load(&fetcher, &format!("{}a.png", server.base_url()), &limits),
        Err(ExternalImageError::TooLarge { limit: 4 })
    );
}

#[test]
fn cancellation_resolves_without_contacting_the_server() {
    let server = LocalHttpServer::start(vec![]);
    let fetcher = ReqwestExternalImages::with_local_http().expect("client");
    let limits = ExternalImageLimits::default();
    let (token, source) = CancellationToken::new();
    source.cancel();
    let result =
        runtime().block_on(fetcher.load(&format!("{}a.png", server.base_url()), &limits, token));
    assert_eq!(result, Err(ExternalImageError::Cancelled));
    assert!(server.recorded().is_empty());
}

// ---- raw server for streaming and stalled responses ----

/// A one-shot localhost server that can write a body without `Content-Length`
/// (so the client must bound it while streaming) or stall after accepting.
struct RawServer {
    base: String,
    recorded: Arc<Mutex<Vec<String>>>,
    handle: Option<JoinHandle<()>>,
}

enum RawReply {
    /// Status line, optional extra headers, then the body with `Connection:
    /// close` and no `Content-Length`; the client must bound the stream.
    Streamed { body: Vec<u8> },
    /// Accept and never answer.
    Stall,
}

impl RawServer {
    fn start(reply: RawReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let recorded_thread = recorded.clone();
        let handle = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                    break;
                }
                let done = line == "\r\n" || line == "\n";
                head.push_str(&line);
                if done {
                    break;
                }
            }
            recorded_thread.lock().expect("recorded").push(head);
            match reply {
                RawReply::Stall => {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
                RawReply::Streamed { body } => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n",
                    );
                    let _ = stream.write_all(&body);
                    let _ = stream.flush();
                }
            }
        });
        Self {
            base: format!("http://{addr}/"),
            recorded,
            handle: Some(handle),
        }
    }
}

impl Drop for RawServer {
    fn drop(&mut self) {
        // Wake the accept loop if it has not accepted yet; a stalled handler is
        // detached rather than waited on.
        if let Ok(addr) = self
            .base
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .parse::<std::net::SocketAddr>()
        {
            let _ = TcpStream::connect(addr);
        }
        if let Some(handle) = self.handle.take() {
            drop(handle);
        }
    }
}

#[test]
fn a_streamed_body_without_content_length_is_bounded() {
    let server = RawServer::start(RawReply::Streamed {
        body: vec![b'x'; 4096],
    });
    let fetcher = ReqwestExternalImages::with_local_http().expect("client");
    let limits = ExternalImageLimits {
        max_bytes: 100,
        ..ExternalImageLimits::default()
    };
    assert_eq!(
        load(&fetcher, &format!("{}a.png", server.base), &limits),
        Err(ExternalImageError::TooLarge { limit: 100 })
    );
    assert_eq!(server.recorded.lock().unwrap().len(), 1);
}

#[test]
fn a_stalled_response_times_out() {
    let server = RawServer::start(RawReply::Stall);
    let fetcher = ReqwestExternalImages::with_local_http().expect("client");
    let limits = ExternalImageLimits {
        timeout: Duration::from_millis(150),
        ..ExternalImageLimits::default()
    };
    assert_eq!(
        load(&fetcher, &format!("{}a.png", server.base), &limits),
        Err(ExternalImageError::Timeout)
    );
}

/// Finding: alternate spellings of a loopback/link-local address must not slip
/// past the local-source boundary. IPv4-mapped IPv6 and IPv6 link-local forms
/// reach the same prohibited destinations as their plain IPv4 spellings.
#[test]
fn alternate_local_address_spellings_are_refused() {
    let fetcher = ReqwestExternalImages::new().expect("client");
    let limits = ExternalImageLimits::default();
    for url in [
        // IPv4-mapped IPv6: the same machine as 127.0.0.1.
        "https://[::ffff:127.0.0.1]/a.png",
        "https://[::ffff:169.254.169.254]/a.png",
        "https://[::ffff:10.0.0.1]/a.png",
        // Plain private / shared ranges are never a public picture.
        "https://10.0.0.1/a.png",
        "https://192.168.1.10/a.png",
        "https://172.16.0.1/a.png",
        "https://100.64.0.1/a.png",
        "https://0.0.0.0/a.png",
        // Native IPv6 loopback, unspecified and link-local.
        "https://[::1]/a.png",
        "https://[::]/a.png",
        "https://[fe80::1]/a.png",
        "https://[fec0::1]/a.png",
    ] {
        assert!(
            matches!(
                load(&fetcher, url, &limits),
                Err(ExternalImageError::Refused(_))
            ),
            "{url} must be refused as a local source"
        );
    }
    // A genuine public address is still accepted by the check itself (the
    // request then fails on network, which is not a refusal). `.invalid` never
    // resolves, so the assertion below can only fail on a policy mistake.
    assert!(!matches!(
        load(&fetcher, "https://images.invalid/a.png", &limits),
        Err(ExternalImageError::Refused(_))
    ));
}

/// Finding (policy coverage): the same check runs on every redirect hop, so the
/// classification itself must reject mapped, private and link-local forms. This
/// is the function `check()` uses for both the first request and each `Location`
/// target, so covering it covers both paths.
#[test]
fn the_local_source_policy_covers_mapped_private_and_link_local_forms() {
    for host in [
        "localhost",
        "[::1]",
        "::1",
        "[::ffff:127.0.0.1]",
        "[::ffff:7f00:1]",
        "[::ffff:169.254.169.254]",
        "[::ffff:10.1.2.3]",
        "[fe80::1]",
        "[fec0::1]",
        "[::]",
        "127.0.0.1",
        "0.0.0.0",
        "10.1.2.3",
        "172.16.0.1",
        "192.168.1.10",
        "169.254.169.254",
        "100.64.0.1",
    ] {
        assert!(is_local_host(host), "{host} must count as a local source");
    }
    for host in [
        "images.invalid",
        "cdn.example.com",
        // Real public addresses (a Cloudflare resolver and its IPv6 form).
        "1.1.1.1",
        "2606:4700:4700::1111",
        "[2606:4700:4700::1111]",
    ] {
        assert!(!is_local_host(host), "{host} must count as a public host");
    }
}

/// The refused forms never open a connection, so production mode rejects them
/// before the client is used at all.
#[test]
fn private_and_mapped_addresses_are_refused_in_production_mode() {
    let fetcher = ReqwestExternalImages::new().expect("client");
    let limits = ExternalImageLimits::default();
    for url in [
        "https://[::ffff:127.0.0.1]/a.png",
        "https://169.254.169.254/a.png",
        "https://10.1.2.3/a.png",
        "https://192.168.1.10/a.png",
    ] {
        assert!(
            matches!(
                load(&fetcher, url, &limits),
                Err(ExternalImageError::Refused(_))
            ),
            "{url} must be refused"
        );
    }
}

/// Finding: a public-looking hostname that resolves to a local/private address
/// must not be connected to. The guard resolver applies the same address policy
/// to every DNS answer, so this is checked where the addresses are produced, not
/// only on the URL's host text.
#[test]
fn the_resolver_refuses_a_name_that_resolves_to_a_local_address() {
    let runtime = runtime();
    // `localhost` is the one name guaranteed to resolve to loopback offline.
    let guarded = GuardResolver { allow_local: false };
    let outcome = runtime.block_on(async {
        reqwest::dns::Resolve::resolve(&guarded, "localhost".parse().expect("name")).await
    });
    assert!(
        outcome.is_err(),
        "a name resolving to loopback must fail the lookup instead of connecting"
    );

    // The test-only fetcher allows loopback on purpose, so the same lookup
    // succeeds there and real localhost sockets stay testable.
    let allowed = GuardResolver { allow_local: true };
    let outcome = runtime.block_on(async {
        reqwest::dns::Resolve::resolve(&allowed, "localhost".parse().expect("name")).await
    });
    let addrs: Vec<std::net::SocketAddr> = outcome.expect("allowed lookup").collect();
    assert!(!addrs.is_empty());
    assert!(addrs.iter().all(|addr| addr.ip().is_loopback()));
}

/// The address policy itself, applied to DNS answers and URL hosts alike.
#[test]
fn every_answer_address_is_classified_by_one_policy() {
    use std::net::IpAddr;
    for local in [
        "127.0.0.1",
        "10.1.2.3",
        "192.168.0.5",
        "172.20.1.1",
        "169.254.169.254",
        "100.64.1.1",
        "0.0.0.0",
        "::1",
        "::",
        "fe80::1",
        "fec0::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
    ] {
        let addr: IpAddr = local.parse().expect("ip");
        assert!(is_local_address(addr), "{local} must be local");
    }
    for public in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
        let addr: IpAddr = public.parse().expect("ip");
        assert!(!is_local_address(addr), "{public} must be public");
    }
}
