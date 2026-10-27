//! Test-only minimal HTTP/1.1 server over `std::net::TcpListener`.
//!
//! Used to exercise the real [`crate::network::transport::ReqwestTransport`]
//! (TLS-free localhost) end to end without touching GitHub or any credential.
//! Each server answers a fixed scripted sequence of responses and records the
//! request headers it received, so tests can assert Authorization injection and
//! reject cross-origin redirects with real sockets.

#![cfg(test)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One scripted HTTP response.
#[derive(Debug, Clone)]
pub struct ScriptedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl ScriptedResponse {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.to_string(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// A recorded inbound request: method, target, header names/values and body.
#[derive(Debug, Clone, Default)]
pub struct RecordedRequest {
    pub request_line: String,
    pub headers: Vec<(String, String)>,
    /// Request body when one was framed by `Content-Length` (POST). Empty for
    /// GET requests.
    pub body: String,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct LocalHttpServer {
    base_url: String,
    script: Arc<Mutex<Vec<ScriptedResponse>>>,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
    handle: Option<JoinHandle<()>>,
}

impl LocalHttpServer {
    /// Start a server that answers the given responses in order. After the
    /// script is exhausted it replies `500` so tests fail loudly instead of
    /// hanging.
    pub fn start(responses: Vec<ScriptedResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind localhost");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}/");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let recorded_thread = recorded.clone();
        let script = Arc::new(Mutex::new(responses));
        let script_thread = script.clone();

        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let next = {
                    let mut guard = script_thread.lock().expect("script lock");
                    if guard.is_empty() {
                        None
                    } else {
                        Some(guard.remove(0))
                    }
                };
                let Some(response) = next else { break };
                serve_one(stream, response, &recorded_thread);
            }
        });

        Self {
            base_url,
            script,
            recorded,
            handle: Some(handle),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Replace the remaining script after the server has started. Needed when a
    /// response header depends on the real ephemeral port (e.g. a `Link` header).
    pub fn script(&self, responses: Vec<ScriptedResponse>) {
        let mut guard = self.script.lock().expect("script lock");
        guard.clear();
        guard.extend(responses);
    }

    pub fn recorded(&self) -> Vec<RecordedRequest> {
        self.recorded.lock().expect("recorded lock").clone()
    }
}

impl Drop for LocalHttpServer {
    fn drop(&mut self) {
        // Trigger an accept() wake-up by connecting once, then join.
        if let Ok(addr) = self
            .base_url
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .parse::<std::net::SocketAddr>()
        {
            let _ = TcpStream::connect(addr);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve_one(
    mut stream: TcpStream,
    response: ScriptedResponse,
    recorded: &Arc<Mutex<Vec<RecordedRequest>>>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    // GET requests have no body. For a framed POST body, read exactly the
    // declared `Content-Length` bytes so the request completes before the
    // response is written; never read to EOF (the client keeps the connection
    // open until it has read our response).
    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 {
        let _ = reader.read_exact(&mut body_bytes);
    }
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    recorded
        .lock()
        .expect("recorded lock")
        .push(RecordedRequest {
            request_line: request_line.trim_end().to_string(),
            headers,
            body,
        });

    let reason = status_reason(response.status);
    let mut out = format!("HTTP/1.1 {} {}\r\n", response.status, reason);
    for (name, value) in &response.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    // Deliberately close the connection so the client reads a complete body
    // without needing chunked encoding.
    out.push_str("Connection: close\r\n\r\n");
    out.push_str(&response.body);
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.flush();
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}
