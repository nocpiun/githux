use super::*;
use crate::content::{PreparedBody, UnsupportedContent, prepare_content};
use crate::domain::repository::*;
use crate::network::transport::{MockTransport, json_response};
use std::sync::Arc;

fn sha(c: char) -> ObjectSha {
    ObjectSha::new(c.to_string().repeat(40)).unwrap()
}
fn snapshot() -> RepoSnapshot {
    RepoSnapshot {
        repository: RepositoryIdentity {
            id: 7,
            locator: RepositoryLocator::new("owner", "repo").unwrap(),
        },
        requested: RevisionSpec::Ref(DynamicRef::new("moving/branch").unwrap()),
        commit: sha('a'),
        root_tree: sha('b'),
        message: String::new(),
        author: None,
        committer: None,
    }
}
fn tree(size: Option<u64>, kind: TreeEntryKind) -> DirectoryTree {
    let snapshot = snapshot();
    let mut directory = SnapshotDirectory::root(&snapshot);
    directory.path = Some(RepositoryPath::new("src").unwrap());
    DirectoryTree {
        directory,
        entries: vec![TreeEntry {
            path: RepositoryPath::new("中文.md").unwrap(),
            sha: sha('c'),
            size,
            kind,
        }],
        truncated: Some(false),
        completeness: Completeness::Complete,
    }
}
fn file(size: Option<u64>) -> BlobIdentity {
    BlobIdentity::from_file(&tree(size, TreeEntryKind::File { executable: false }), 0).unwrap()
}
fn raw(bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({ "sha": sha('c').as_str(), "size": bytes.len(), "encoding": "base64", "content": STANDARD.encode(bytes), "url": "https://evil.invalid/pat-steal" })
}
async fn read(
    identity: &BlobIdentity,
    raw: serde_json::Value,
    limits: BlobLimits,
) -> Result<BlobRead, RepositoryError> {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(json_response(200, &raw.to_string()))).await;
    let client = GitHubClient::new(Default::default(), mock);
    fetch_blob(
        &client,
        &Pat::from_str("ghp_synthetic_blob_secret"),
        identity,
        &limits,
        CancellationToken::default(),
    )
    .await
}

#[tokio::test]
async fn fixed_sha_from_readme_and_file_survives_branch_movement() {
    let mock = Arc::new(MockTransport::new());
    let client = GitHubClient::new(Default::default(), mock.clone());
    let pat = Pat::from_str("ghp_synthetic_blob_secret");
    let snapshot_a = snapshot();
    let descriptor = ReadmeDescriptor {
        repository: snapshot_a.repository.clone(),
        commit: snapshot_a.commit.clone(),
        path: RepositoryPath::new("README.md").unwrap(),
        blob: sha('c'),
        size: 5,
    };
    let identities = [BlobIdentity::from_readme(&descriptor), file(Some(5))];
    let mut moved = snapshot_a;
    moved.commit = sha('d'); // the old handles remain tied to A
    for identity in identities {
        mock.enqueue(Ok(json_response(200, &raw(b"hello").to_string())))
            .await;
        let blob = fetch_blob(
            &client,
            &pat,
            &identity,
            &BlobLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(blob.identity().commit(), &sha('a'));
        assert_eq!(blob.identity().blob(), &sha('c'));
        assert_eq!(blob.body(), &BlobBody::Bytes(b"hello".to_vec()));
        assert!(!format!("{blob:?}").contains("ghp_synthetic_blob_secret"));
    }
    assert_eq!(moved.commit, sha('d'));
    for (url, headers) in mock.recorded().await {
        assert_eq!(
            url,
            format!(
                "https://api.github.com/repos/owner/repo/git/blobs/{}",
                sha('c').as_str()
            )
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Accept" && v == "application/vnd.github+json")
        );
    }
}

#[tokio::test]
async fn real_reqwest_blob_round_trip_uses_fixed_endpoint_and_rejects_truncated_json() {
    use crate::network::test_server::{LocalHttpServer, ScriptedResponse};
    use crate::network::transport::ReqwestTransport;
    use std::time::Duration;
    let server = LocalHttpServer::start(vec![
        ScriptedResponse::json(200, &raw("中文".as_bytes()).to_string()),
        ScriptedResponse::json(200, "{\"sha\":"),
    ]);
    let client = GitHubClient::new(
        crate::github::client::GitHubConfig {
            base_url: server.base_url().into(),
            ..Default::default()
        },
        Arc::new(ReqwestTransport::new(Duration::from_secs(2)).unwrap()),
    );
    let identity = file(None);
    let pat = Pat::from_str("synthetic_local_blob");
    let blob = fetch_blob(
        &client,
        &pat,
        &identity,
        &BlobLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        prepare_content(blob, 100).body,
        PreparedBody::Utf8 {
            text: "中文".into(),
            has_bom: false
        }
    );
    assert!(
        fetch_blob(
            &client,
            &pat,
            &identity,
            &BlobLimits::default(),
            CancellationToken::default()
        )
        .await
        .is_err()
    );
    let recorded = server.recorded();
    assert_eq!(
        recorded[0].request_line,
        format!(
            "GET /repos/owner/repo/git/blobs/{} HTTP/1.1",
            sha('c').as_str()
        )
    );
    assert_eq!(
        recorded[0].header("authorization"),
        Some("Bearer synthetic_local_blob")
    );
}

#[test]
fn only_ordinary_files_are_expandable() {
    for kind in [
        TreeEntryKind::Directory,
        TreeEntryKind::Symlink,
        TreeEntryKind::Submodule,
        TreeEntryKind::Unknown {
            git_type: "blob".into(),
            mode: "odd".into(),
        },
    ] {
        assert!(BlobIdentity::from_file(&tree(None, kind), 0).is_err());
    }
    let identity = file(None);
    assert_eq!(identity.path().as_str(), "src/中文.md");
    assert!(
        BlobIdentity::from_file(&tree(None, TreeEntryKind::File { executable: true }), 1).is_err()
    );
}

#[tokio::test]
async fn empty_chinese_bom_and_wrapped_base64_are_lossless() {
    for bytes in [
        b"".as_slice(),
        "你好\r\n源码\n".as_bytes(),
        "\u{feff}中文\n".as_bytes(),
        b"hello",
    ] {
        let mut value = raw(bytes);
        let encoded = value["content"].as_str().unwrap();
        value["content"] = serde_json::Value::String(format!(
            "{}\r\n{}\n",
            &encoded[..encoded.len() / 2],
            &encoded[encoded.len() / 2..]
        ));
        let blob = read(
            &file(Some(bytes.len() as u64)),
            value,
            BlobLimits::default(),
        )
        .await
        .unwrap();
        let prepared = prepare_content(blob, 100);
        if bytes.is_empty() {
            assert_eq!(prepared.body, PreparedBody::Empty);
        } else {
            assert_eq!(
                prepared.body,
                PreparedBody::Utf8 {
                    text: String::from_utf8(bytes.to_vec()).unwrap(),
                    has_bom: bytes.starts_with(&[0xef, 0xbb, 0xbf])
                }
            );
        }
    }
}

#[tokio::test]
async fn binary_md_and_unsupported_encoding_never_become_replacement_text() {
    for (bytes, reason) in [
        (vec![0xff, 0xfe, 0x41, 0], UnsupportedContent::NonUtf8),
        (
            b"a\0b".to_vec(),
            UnsupportedContent::BinaryControlCharacters,
        ),
        (vec![1, 2, 3], UnsupportedContent::BinaryControlCharacters),
    ] {
        let blob = read(&file(None), raw(&bytes), BlobLimits::default())
            .await
            .unwrap();
        assert_eq!(
            prepare_content(blob, 100).body,
            PreparedBody::Unsupported(reason)
        );
    }
}

#[tokio::test]
async fn invalid_api_data_is_an_error_and_does_not_echo_secrets() {
    for (key, value) in [
        ("sha", serde_json::json!(sha('d').as_str())),
        ("sha", serde_json::json!("ghp_synthetic_blob_secret")),
        ("encoding", serde_json::json!("utf-8")),
        ("size", serde_json::json!(1)),
        ("size", serde_json::json!(-1)),
        ("content", serde_json::json!("%%%=")),
        ("content", serde_json::json!("Zh==")), // noncanonical trailing bits
        ("content", serde_json::json!("Zg=")),
        ("content", serde_json::json!("Z g==")),
    ] {
        let mut value_raw = raw(b"hello");
        value_raw[key] = value;
        let error = read(&file(None), value_raw, BlobLimits::default())
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("ghp_synthetic_blob_secret"));
    }
    assert!(
        read(&file(Some(4)), raw(b"hello"), BlobLimits::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bounds_apply_before_network_decode_and_preparation() {
    let limits = BlobLimits {
        decoded_bytes: 4,
        response_bytes: 1024,
    };
    let mock = Arc::new(MockTransport::new());
    let client = GitHubClient::new(Default::default(), mock.clone());
    let early = fetch_blob(
        &client,
        &Pat::from_str("synthetic"),
        &file(Some(5)),
        &limits,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        early.body(),
        BlobBody::TooLarge { size: 5, limit: 4 }
    ));
    assert!(mock.recorded().await.is_empty());
    let over = read(&file(None), raw(b"hello"), limits).await.unwrap();
    assert!(matches!(
        over.body(),
        BlobBody::TooLarge { size: 5, limit: 4 }
    ));
    let mut lie = raw(b"hello");
    lie["size"] = serde_json::json!(1);
    let actual = read(&file(None), lie, limits).await.unwrap();
    assert!(matches!(
        actual.body(),
        BlobBody::TooLarge { size: 5, limit: 4 }
    ));
    let exact = read(&file(None), raw(b"four"), limits).await.unwrap();
    assert_eq!(
        prepare_content(exact, 3).body,
        PreparedBody::TooLarge { size: 4, limit: 3 }
    );
    let tiny_json = BlobLimits {
        response_bytes: 4,
        ..limits
    };
    assert!(matches!(
        read(&file(None), raw(b"hi"), tiny_json).await,
        Err(RepositoryError::LimitExceeded { .. })
    ));
    let zero = BlobLimits {
        decoded_bytes: 0,
        response_bytes: 1024,
    };
    assert_eq!(
        prepare_content(read(&file(None), raw(b""), zero).await.unwrap(), 0).body,
        PreparedBody::Empty
    );
}

#[tokio::test]
async fn default_two_mib_boundary_and_bom_only_are_explicit() {
    let limits = BlobLimits::default();
    let bytes = vec![b'x'; limits.decoded_bytes];
    let blob = read(&file(None), raw(&bytes), limits).await.unwrap();
    let prepared = prepare_content(blob, limits.decoded_bytes);
    match prepared.body {
        PreparedBody::Utf8 { text, .. } => assert_eq!(text.len(), limits.decoded_bytes),
        other => panic!("expected exact-budget text: {other:?}"),
    }
    let over = vec![b'x'; limits.decoded_bytes + 1];
    let mut understated = raw(&over);
    understated["size"] = serde_json::json!(0);
    assert!(matches!(
        read(&file(None), understated, limits).await.unwrap().body(),
        BlobBody::TooLarge { .. }
    ));
    let bom = read(&file(None), raw("\u{feff}".as_bytes()), limits)
        .await
        .unwrap();
    assert_eq!(
        prepare_content(bom, 3).body,
        PreparedBody::Utf8 {
            text: "\u{feff}".into(),
            has_bom: true
        }
    );
}

#[tokio::test]
async fn permission_rate_limit_cancellation_and_malformed_json_are_not_empty() {
    let mock = Arc::new(MockTransport::new());
    let client = GitHubClient::new(Default::default(), mock.clone());
    for status in [404, 401, 403, 429] {
        mock.enqueue(Ok(json_response(status, "{}"))).await;
        let err = fetch_blob(
            &client,
            &Pat::from_str("synthetic"),
            &file(None),
            &BlobLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            (status, err),
            (404, RepositoryError::GitHub(GitHubError::NotFound))
                | (401, RepositoryError::GitHub(GitHubError::Unauthorized))
                | (403, RepositoryError::GitHub(GitHubError::Forbidden))
                | (
                    429,
                    RepositoryError::GitHub(GitHubError::RateLimited { .. })
                )
        ));
    }
    mock.enqueue(Ok(json_response(200, "not json ghp_synthetic_blob_secret")))
        .await;
    let err = fetch_blob(
        &client,
        &Pat::from_str("synthetic"),
        &file(None),
        &BlobLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap_err();
    assert!(!format!("{err}").contains("ghp_synthetic_blob_secret"));
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    assert!(matches!(
        fetch_blob(
            &client,
            &Pat::from_str("synthetic"),
            &file(Some(u64::MAX)),
            &BlobLimits::default(),
            cancel
        )
        .await,
        Err(RepositoryError::GitHub(GitHubError::Cancelled))
    ));
}
