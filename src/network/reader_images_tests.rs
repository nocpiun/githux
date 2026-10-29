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
