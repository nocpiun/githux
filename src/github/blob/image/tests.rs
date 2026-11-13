use super::*;
use crate::domain::repository::{
    ObjectSha, ReadmeDescriptor, RepositoryIdentity, RepositoryLocator, RepositoryPath,
};
use crate::network::transport::{HttpBytesResponse, HttpRequest, Transport};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};
struct BinaryTransport {
    body: Vec<u8>,
    requests: Mutex<Vec<HttpRequest>>,
}
impl Transport for BinaryTransport {
    fn execute<'a>(
        &'a self,
        _: HttpRequest,
        _: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async { panic!("raw transport must not request text") })
    }
    fn execute_bytes_bounded<'a>(
        &'a self,
        request: HttpRequest,
        _: CancellationToken,
        max: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpBytesResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            if self.body.len() > max {
                return Err(TransportError::ResponseTooLarge { limit: max });
            }
            Ok(HttpBytesResponse {
                status: 200,
                headers: vec![("cache-control".into(), "no-store".into())],
                body: self.body.clone(),
            })
        })
    }
}
fn identity(bytes: &[u8]) -> BlobIdentity {
    let mut sha = sha1_smol::Sha1::new();
    sha.update(format!("blob {}\0", bytes.len()).as_bytes());
    sha.update(bytes);
    BlobIdentity::from_readme(&ReadmeDescriptor {
        repository: RepositoryIdentity {
            id: 42,
            locator: RepositoryLocator::new("owner", "repo").unwrap(),
        },
        commit: ObjectSha::new("a".repeat(40)).unwrap(),
        path: RepositoryPath::new("image.png").unwrap(),
        blob: ObjectSha::new(sha.digest().to_string()).unwrap(),
        size: bytes.len() as u64,
    })
}
#[tokio::test]
async fn performance_raw_image_blob_checks_original_sha_and_keeps_metadata_and_auth_origin() {
    let bytes = vec![0x89, b'P', b'N', b'G', 0, 0xff, 0x80];
    let identity = identity(&bytes);
    let transport = Arc::new(BinaryTransport {
        body: bytes.clone(),
        requests: Mutex::default(),
    });
    let client = GitHubClient::new(Default::default(), transport.clone());
    let (read, metadata) = fetch_image_blob(
        &client,
        &Pat::from_str("synthetic-image-secret"),
        &identity,
        &BlobLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(read.body(), &BlobBody::Bytes(bytes));
    assert_eq!(metadata.cache_control.as_deref(), Some("no-store"));
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url,
        format!(
            "https://api.github.com/repos/owner/repo/git/blobs/{}",
            identity.blob().as_str()
        )
    );
    assert!(
        requests[0]
            .headers
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("accept")
                && value == "application/vnd.github.raw+json")
    );
    assert!(
        requests[0]
            .headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("authorization"))
    );
    assert!(!format!("{:?}", requests[0]).contains("synthetic-image-secret"));
}
#[tokio::test]
async fn performance_raw_image_blob_rejects_different_bytes_even_with_correct_length() {
    let expected = [0xff, 1, 2, 3];
    let transport = Arc::new(BinaryTransport {
        body: vec![0xff, 1, 2, 4],
        requests: Mutex::default(),
    });
    let client = GitHubClient::new(Default::default(), transport);
    assert!(matches!(
        fetch_image_blob(
            &client,
            &Pat::from_str("synthetic"),
            &identity(&expected),
            &BlobLimits::default(),
            CancellationToken::default()
        )
        .await,
        Err(RepositoryError::InvalidResponse { .. })
    ));
}
#[tokio::test]
async fn performance_binary_reqwest_keeps_non_utf8_bytes_and_receive_limit() {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n\xff\x00\x80\x01").unwrap();
        }
    });
    let transport =
        crate::network::transport::ReqwestTransport::new(std::time::Duration::from_secs(2))
            .unwrap();
    let request = || HttpRequest::get(format!("http://{address}/blob"));
    let response = transport
        .execute_bytes_bounded(request(), CancellationToken::default(), 4)
        .await
        .unwrap();
    assert_eq!(response.body, [0xff, 0, 0x80, 1]);
    assert!(matches!(
        transport
            .execute_bytes_bounded(request(), CancellationToken::default(), 3)
            .await,
        Err(TransportError::ResponseTooLarge { limit: 3 })
    ));
    server.join().unwrap();
}
