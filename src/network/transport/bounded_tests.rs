//! Real sockets with deliberately incomplete HTTP bodies. A bounded read must
//! finish before EOF/terminating chunk, proving it did not buffer then check.
use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

struct RawServer {
    url: String,
    sent: Option<tokio::sync::oneshot::Receiver<()>>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl RawServer {
    fn new(wire: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/blob", listener.local_addr().unwrap());
        let (stop_tx, stop_rx) = mpsc::channel();
        let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if stop_rx.recv_timeout(Duration::from_millis(10)).is_ok() {
                            return;
                        }
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = stream.write_all(&wire);
            let _ = stream.flush();
            let _ = sent_tx.send(());
            // Hold open until client completion; no EOF or chunk terminator.
            let _ = stop_rx.recv_timeout(Duration::from_secs(5));
        });
        Self {
            url,
            sent: Some(sent_rx),
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }
    async fn sent(&mut self) {
        self.sent.take().unwrap().await.unwrap();
    }
}
impl Drop for RawServer {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.thread.take().unwrap().join().unwrap();
    }
}
fn request(url: &str) -> HttpRequest {
    HttpRequest::get(url).with_header("Authorization", "Bearer synthetic_bounded_secret")
}
async fn bounded_wire(wire: &str, limit: usize) -> Result<HttpResponse, TransportError> {
    let server = RawServer::new(wire.as_bytes().to_vec());
    let transport = ReqwestTransport::new(Duration::from_secs(3)).unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        transport.execute_bounded(request(&server.url), CancellationToken::default(), limit),
    )
    .await
    .expect("must finish before EOF")
}

#[tokio::test]
async fn chunked_and_no_length_stop_before_eof_on_success_and_errors() {
    for status in ["200 OK", "401 Unauthorized", "500 Internal Server Error"] {
        let chunked = format!(
            "HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n1234\r\n5\r\n56789\r\n"
        );
        assert_eq!(
            bounded_wire(&chunked, 8).await,
            Err(TransportError::ResponseTooLarge { limit: 8 })
        );
        let no_length = format!("HTTP/1.1 {status}\r\nConnection: close\r\n\r\n123456789");
        assert_eq!(
            bounded_wire(&no_length, 8).await,
            Err(TransportError::ResponseTooLarge { limit: 8 })
        );
    }
}

#[tokio::test]
async fn length_is_only_a_hint_and_malformed_framing_cannot_bypass_bound() {
    // Reject on headers, even though no body is sent.
    assert_eq!(
        bounded_wire("HTTP/1.1 200 OK\r\nContent-Length: 999999\r\n\r\n", 8).await,
        Err(TransportError::ResponseTooLarge { limit: 8 })
    );
    assert_eq!(
        bounded_wire(
            "HTTP/1.1 404 Not Found\r\nContent-Length: 999999\r\n\r\n",
            8
        )
        .await,
        Err(TransportError::ResponseTooLarge { limit: 8 })
    );
    // HTTP framing exposes only the declared byte; trailing data cannot enter
    // the body or a blob JSON parser as hidden additional content.
    let low = bounded_wire(
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\nxextra",
        8,
    )
    .await
    .unwrap();
    assert_eq!(low.body, "x");
    // With TE+CL, either reject invalid framing or count the actual chunks;
    // trusting the low CL and reading an unbounded chunk stream is forbidden.
    let ambiguous = bounded_wire("HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n9\r\n123456789\r\n",8).await;
    assert!(matches!(
        ambiguous,
        Err(TransportError::ResponseTooLarge { limit: 8 }) | Err(TransportError::Network { .. })
    ));
    assert!(matches!(
        bounded_wire(
            "HTTP/1.1 200 OK\r\nContent-Length: nope\r\n\r\n123456789",
            8
        )
        .await,
        Err(TransportError::Network { .. })
    ));
    let exact = bounded_wire(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n8\r\n12345678\r\n0\r\n\r\n",
        8,
    )
    .await
    .unwrap();
    assert_eq!(exact.body, "12345678");
}

#[tokio::test]
async fn bounded_body_cancellation_and_timeout_are_typed_and_join_servers() {
    for should_cancel in [true, false] {
        let mut server = RawServer::new(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n".to_vec(),
        );
        let transport = ReqwestTransport::new(Duration::from_millis(250)).unwrap();
        let (cancel, source) = CancellationToken::new();
        let url = server.url.clone();
        let pending =
            tokio::spawn(async move { transport.execute_bounded(request(&url), cancel, 8).await });
        server.sent().await;
        if should_cancel {
            source.cancel();
        }
        let result = tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result,
            Err(if should_cancel {
                TransportError::Cancelled
            } else {
                TransportError::Timeout { seconds: 1 }
            })
        );
    }
}

#[tokio::test]
async fn redirect_keeps_budget_and_credentials_on_same_origin() {
    use crate::network::test_server::{LocalHttpServer, ScriptedResponse};
    let target = LocalHttpServer::start(vec![ScriptedResponse::json(200, "stolen")]);
    let server = LocalHttpServer::start(vec![
        ScriptedResponse::json(302, "").with_header("Location", target.base_url()),
    ]);
    let transport = ReqwestTransport::new(Duration::from_secs(2)).unwrap();
    let response = transport
        .execute_bounded(request(server.base_url()), CancellationToken::default(), 8)
        .await
        .unwrap();
    assert_eq!(response.status, 302);
    assert!(target.recorded().is_empty());
    // No request consumed the target's fixture. Let its drop wake-up exit.
    target.script(vec![]);
    let same = LocalHttpServer::start(vec![
        ScriptedResponse::json(302, "").with_header("Location", "/fixed"),
        ScriptedResponse::json(200, "123456789"),
    ]);
    assert_eq!(
        transport
            .execute_bounded(request(same.base_url()), CancellationToken::default(), 8)
            .await,
        Err(TransportError::ResponseTooLarge { limit: 8 })
    );
    let recorded = same.recorded();
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        recorded[1].header("authorization"),
        Some("Bearer synthetic_bounded_secret")
    );
}

#[tokio::test]
async fn injected_client_cannot_enable_automatic_cross_origin_redirects() {
    let transport = ReqwestTransport::with_client(reqwest::Client::new(), Duration::from_secs(1));
    assert_eq!(
        transport
            .execute_bounded(
                request("http://127.0.0.1:1/"),
                CancellationToken::default(),
                8
            )
            .await,
        Err(TransportError::BoundedReadUnsupported)
    );
}
