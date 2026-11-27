//! Reproducible Windows reading-path baseline measurements on the official integrated Shell.
//!
//! Run noninteractively with:
//! `node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::tests::reading_baseline_tests -- --ignored --nocapture --test-threads=1`

use super::*;
use crate::app::accounts::runtime::open_accounts_with_cache;
use crate::auth::token::Pat;
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{CacheConfig, CacheStore};
use sha1_smol::Sha1;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const REPEAT_COUNT: usize = 5;
const COMMIT_SHA: &str = "1111111111111111111111111111111111111111";
const TREE_SHA: &str = "2222222222222222222222222222222222222222";
const README_BLOB_SHA: &str = "3333333333333333333333333333333333333333";

#[derive(Clone, Copy, Debug, Default)]
struct ProcessMem {
    working_set_bytes: usize,
    peak_working_set_bytes: usize,
    private_bytes: usize,
}

#[cfg(windows)]
fn read_process_memory() -> ProcessMem {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let mut counters = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    let size = counters.cb;
    let ok = unsafe { K32GetProcessMemoryInfo(-1, &mut counters, size) };
    if ok == 0 {
        return ProcessMem::default();
    }
    ProcessMem {
        working_set_bytes: counters.working_set_size,
        peak_working_set_bytes: counters.peak_working_set_size,
        private_bytes: counters.pagefile_usage,
    }
}

#[cfg(not(windows))]
fn read_process_memory() -> ProcessMem {
    ProcessMem::default()
}

fn dir_disk_usage(path: &Path) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let (f, b) = dir_disk_usage(&p);
            files += f;
            bytes += b;
        } else if let Ok(meta) = entry.metadata() {
            files += 1;
            bytes += meta.len();
        }
    }
    (files, bytes)
}

fn sqlite_disk_usage(db_path: &Path) -> u64 {
    let mut total = 0u64;
    for suffix in ["", "-wal", "-shm"] {
        let candidate = if suffix.is_empty() {
            db_path.to_path_buf()
        } else {
            let mut s = db_path.as_os_str().to_os_string();
            s.push(suffix);
            std::path::PathBuf::from(s)
        };
        if let Ok(meta) = std::fs::metadata(candidate) {
            total += meta.len();
        }
    }
    total
}

fn make_png(seed: u8, width: u32, height: u32) -> Vec<u8> {
    let rgba = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            ((x / 11 + y / 7 + seed as u32 * 37) % 256) as u8,
            ((x / 5 + seed as u32 * 53) % 256) as u8,
            ((y / 9 + seed as u32 * 71) % 256) as u8,
            255,
        ])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

fn git_blob_sha(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    hasher.update(bytes);
    hasher.digest().to_string()
}

fn long_readme_markdown(image_paths: &[&str]) -> String {
    let mut md = String::with_capacity(28 * 1024);
    md.push_str("# Githux Synthetic Reading Baseline Repository\n\n");
    md.push_str(
        "This deterministic document exercises Markdown headings, tables, code blocks, and embedded PNGs.\n\n",
    );
    for section in 0..12 {
        md.push_str(&format!(
            "## Section {section}: Architecture and Cache Flow\n\n"
        ));
        if section < image_paths.len() {
            md.push_str(&format!(
                "![Diagram {section}]({})\n\n",
                image_paths[section]
            ));
        }
        for para in 0..4 {
            md.push_str(&format!(
                "Paragraph {section}.{para}: Deterministic synthetic content for measuring layout, \
                 text shaping, scroll viewport updates, and image decoding without live network access. \
                 Accounts remain isolated per host and session token.\n\n"
            ));
        }
        md.push_str("```rust\npub fn sample_step(index: usize) -> usize {\n    index.saturating_mul(2).saturating_add(1)\n}\n```\n\n");
        if section % 4 == 0 {
            md.push_str("| Metric | Scope | Notes |\n| --- | --- | --- |\n| Startup | Cold/Warm | Isolated tempdir |\n| Scroll | Long doc | Synthetic PNGs |\n\n");
        }
    }
    md
}

struct BaselineFixture {
    readme_text: String,
    images: BTreeMap<String, (String, Vec<u8>)>,
    tree_entries: Vec<serde_json::Value>,
    repos_json: serde_json::Value,
    home_items_json: serde_json::Value,
    feed_events_json: serde_json::Value,
    repo_issues_p1_json: serde_json::Value,
    repo_issues_p2_json: serde_json::Value,
    repo_pulls_p1_json: serde_json::Value,
    repo_pulls_p2_json: serde_json::Value,
    issue_42_json: serde_json::Value,
    issue_42_comments_p1_json: serde_json::Value,
    issue_42_comments_p2_json: serde_json::Value,
    pr_5_json: serde_json::Value,
    pr_5_comments_p1_json: serde_json::Value,
    pr_5_comments_p2_json: serde_json::Value,
    pr_5_files_json: serde_json::Value,
    notifications_json: serde_json::Value,
}

impl BaselineFixture {
    fn build() -> Self {
        let image_names = ["img1.png", "img2.png", "img3.png", "img4.png"];
        let mut images = BTreeMap::new();
        for (idx, name) in image_names.iter().enumerate() {
            let bytes = make_png((idx + 1) as u8, 1200, 800);
            let sha = git_blob_sha(&bytes);
            images.insert((*name).to_string(), (sha, bytes));
        }
        let readme_text = long_readme_markdown(&image_names);

        let mut tree_entries = vec![serde_json::json!({
            "path": "README.md",
            "mode": "100644",
            "type": "blob",
            "sha": README_BLOB_SHA,
            "size": readme_text.len(),
        })];
        for (name, (sha, bytes)) in &images {
            tree_entries.push(serde_json::json!({
                "path": name,
                "mode": "100644",
                "type": "blob",
                "sha": sha,
                "size": bytes.len(),
            }));
        }
        for i in 0..55 {
            tree_entries.push(serde_json::json!({
                "path": format!("module_{i:02}.rs"),
                "mode": "100644",
                "type": "blob",
                "sha": format!("{i:040x}"),
                "size": 1024 + i * 32,
            }));
        }

        let repos_json = serde_json::json!(
            (0..12)
                .map(|i| {
                    let name = if i == 0 {
                        "demo".to_string()
                    } else {
                        format!("repo-{i:02}")
                    };
                    serde_json::json!({
                        "id": 100 + i,
                        "name": name,
                        "full_name": format!("alice/{name}"),
                        "visibility": "public",
                        "private": false,
                        "fork": false,
                        "archived": false,
                        "default_branch": "main",
                        "description": format!("Synthetic repository {i}"),
                        "html_url": format!("https://github.com/alice/{name}"),
                        "stargazers_count": 42 + i,
                        "forks_count": 7 + i,
                        "open_issues_count": 50,
                        "size": 1024,
                        "owner": { "login": "alice", "id": 1, "avatar_url": null }
                    })
                })
                .collect::<Vec<_>>()
        );

        let make_search_item = |id: u64, number: u64, is_pr: bool, title_prefix: &str| {
            let mut obj = serde_json::json!({
                "id": id,
                "number": number,
                "title": format!("{title_prefix} #{number}: Deterministic baseline item"),
                "state": "open",
                "html_url": format!("https://github.com/alice/demo/{}/{}", if is_pr { "pull" } else { "issues" }, number),
                "repository_url": "https://api.github.com/repos/alice/demo",
                "user": { "login": "alice", "id": 1, "avatar_url": "" },
                "labels": [{ "name": "baseline", "color": "0e8a16" }],
                "comments": 3,
                "updated_at": "2026-11-27T10:00:00Z",
                "created_at": "2026-11-20T10:00:00Z",
            });
            if is_pr {
                obj.as_object_mut()
                    .unwrap()
                    .insert("pull_request".into(), serde_json::json!({ "url": "..." }));
            }
            obj
        };

        let home_items_json = serde_json::json!({
            "total_count": 25,
            "incomplete_results": false,
            "items": (1..=25).map(|i| make_search_item(10_000 + i, i, i % 3 == 0, "Home")).collect::<Vec<_>>()
        });
        let repo_issues_p1_json = serde_json::json!({
            "total_count": 50,
            "incomplete_results": false,
            "items": (1..=25).map(|i| make_search_item(20_000 + i, if i == 25 { 42 } else { i }, false, "Issue")).collect::<Vec<_>>()
        });
        let repo_issues_p2_json = serde_json::json!({
            "total_count": 50,
            "incomplete_results": false,
            "items": (26..=50).map(|i| make_search_item(20_000 + i, i, false, "Issue")).collect::<Vec<_>>()
        });
        let repo_pulls_p1_json = serde_json::json!({
            "total_count": 40,
            "incomplete_results": false,
            "items": (1..=20).map(|i| make_search_item(30_000 + i, if i == 1 { 5 } else { i + 100 }, true, "PR")).collect::<Vec<_>>()
        });
        let repo_pulls_p2_json = serde_json::json!({
            "total_count": 40,
            "incomplete_results": false,
            "items": (21..=40).map(|i| make_search_item(30_000 + i, i + 100, true, "PR")).collect::<Vec<_>>()
        });

        let feed_events_json = serde_json::json!((0..25)
            .map(|i| serde_json::json!({
                "id": format!("{}", 9000 + i),
                "type": "WatchEvent",
                "actor": { "id": 2, "login": "bob", "avatar_url": "" },
                "repo": { "id": 100, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo" },
                "payload": { "action": "started" },
                "public": true,
                "created_at": "2026-11-27T09:00:00Z"
            }))
            .collect::<Vec<_>>());

        let long_issue_body = "Detailed issue description.\n\n".repeat(120);
        let issue_42_json = serde_json::json!({
            "id": 42_042,
            "number": 42,
            "title": "Issue #42: Long discussion baseline",
            "state": "open",
            "body": long_issue_body,
            "user": { "login": "alice", "id": 1, "avatar_url": "" },
            "labels": [{ "name": "perf", "color": "1d76db" }],
            "comments": 25,
            "created_at": "2026-11-20T09:00:00Z",
            "updated_at": "2026-11-27T09:00:00Z"
        });
        let make_comments = |start: u64, count: u64| {
            serde_json::json!((start..start + count)
                .map(|id| serde_json::json!({
                    "id": id,
                    "user": { "login": "bob", "id": 2, "avatar_url": "" },
                    "body": format!("Comment {id}: Deterministic synthetic discussion entry for reading baseline measurement."),
                    "created_at": "2026-11-25T10:00:00Z",
                    "updated_at": "2026-11-25T10:00:00Z"
                }))
                .collect::<Vec<_>>())
        };
        let issue_42_comments_p1_json = make_comments(4201, 15);
        let issue_42_comments_p2_json = make_comments(4216, 10);

        let pr_5_json = serde_json::json!({
            "id": 5005,
            "number": 5,
            "title": "PR #5: Baseline conversation and files changed",
            "state": "open",
            "draft": false,
            "merged": false,
            "mergeable_state": "clean",
            "body": long_issue_body,
            "user": { "login": "alice", "id": 1, "avatar_url": "" },
            "head": { "ref": "feat/baseline", "sha": COMMIT_SHA, "label": "alice:feat/baseline" },
            "base": { "ref": "main", "sha": COMMIT_SHA, "label": "alice:main" },
            "labels": [{ "name": "reading", "color": "0e8a16" }],
            "comments": 20,
            "commits": 3,
            "additions": 180,
            "deletions": 45,
            "changed_files": 15,
            "created_at": "2026-11-21T09:00:00Z",
            "updated_at": "2026-11-27T09:30:00Z"
        });
        let pr_5_comments_p1_json = make_comments(501, 12);
        let pr_5_comments_p2_json = make_comments(513, 8);
        let pr_5_files_json = serde_json::json!((0..15)
            .map(|i| serde_json::json!({
                "sha": format!("f{i:039x}"),
                "filename": format!("src/file_{i:02}.rs"),
                "status": "modified",
                "additions": 12,
                "deletions": 3,
                "changes": 15,
                "patch": "@@ -1,5 +1,14 @@\n fn baseline() {\n-    let old = 1;\n+    let new = 2;\n+    println!(\"line {}\", new);\n }\n"
            }))
            .collect::<Vec<_>>());

        let notifications_json = serde_json::json!(
            (0..20)
                .map(|i| serde_json::json!({
                    "id": format!("{}", 8000 + i),
                    "unread": i < 5,
                    "reason": "mention",
                    "updated_at": "2026-11-27T08:00:00Z",
                    "subject": {
                        "title": format!("Notification item {i}"),
                        "url": "https://api.github.com/repos/alice/demo/issues/42",
                        "type": "Issue"
                    },
                    "repository": {
                        "id": 100,
                        "name": "demo",
                        "full_name": "alice/demo",
                        "owner": { "login": "alice", "id": 1, "avatar_url": "" }
                    }
                }))
                .collect::<Vec<_>>()
        );

        Self {
            readme_text,
            images,
            tree_entries,
            repos_json,
            home_items_json,
            feed_events_json,
            repo_issues_p1_json,
            repo_issues_p2_json,
            repo_pulls_p1_json,
            repo_pulls_p2_json,
            issue_42_json,
            issue_42_comments_p1_json,
            issue_42_comments_p2_json,
            pr_5_json,
            pr_5_comments_p1_json,
            pr_5_comments_p2_json,
            pr_5_files_json,
            notifications_json,
        }
    }
}

struct BaselineTransport {
    fixture: Arc<BaselineFixture>,
    delay_ms: AtomicU64,
    request_count: AtomicUsize,
    in_flight: AtomicUsize,
    urls: std::sync::Mutex<Vec<String>>,
}

impl BaselineTransport {
    fn new(fixture: Arc<BaselineFixture>, delay_ms: u64) -> Self {
        Self {
            fixture,
            delay_ms: AtomicU64::new(delay_ms),
            request_count: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            urls: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn reset_count(&self) -> usize {
        self.urls.lock().unwrap().clear();
        self.request_count.swap(0, Ordering::SeqCst)
    }

    fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }
}

struct InFlightGuard<'a>(&'a AtomicUsize);
impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Transport for BaselineTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max_body_bytes {
                return Err(TransportError::ResponseTooLarge {
                    limit: max_body_bytes,
                });
            }
            Ok(response)
        })
    }

    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.request_count.fetch_add(1, Ordering::SeqCst);
            self.in_flight.fetch_add(1, Ordering::SeqCst);
            self.urls.lock().unwrap().push(request.url.clone());
            let _guard = InFlightGuard(&self.in_flight);

            let delay = self.delay_ms.load(Ordering::SeqCst);
            if delay > 0 && !request.url.ends_with("/user") {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(TransportError::Cancelled),
                    _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                }
            }

            let url = request.url.as_str();
            let body_str = request.body.as_deref().unwrap_or("");

            if url.ends_with("/user") {
                return Ok(json(
                    200,
                    serde_json::json!({ "id": 1, "login": "alice", "avatar_url": "" }),
                ));
            }
            if url.contains("/user/repos") {
                return Ok(json(200, self.fixture.repos_json.clone()));
            }
            if url.contains("/user/following") {
                return Ok(json(
                    200,
                    serde_json::json!([{ "id": 2, "login": "bob", "avatar_url": "" }]),
                ));
            }
            if url.contains("/received_events") {
                return Ok(json(200, self.fixture.feed_events_json.clone()));
            }
            if url.contains("/notifications") {
                return Ok(json(200, self.fixture.notifications_json.clone()));
            }
            if url.contains("/repos/alice/demo/issues?") {
                let page2 = url.contains("page=2");
                let range = if page2 { 26..=50 } else { 1..=25 };
                let rows: Vec<_> = range
                    .map(|number| {
                        serde_json::json!({
                            "number": number,
                            "title": format!("Issue #{number}: Deterministic baseline item"),
                            "state": "open",
                            "updated_at": "2026-11-27T10:00:00Z",
                            "user": { "login": "alice", "id": 1, "avatar_url": null },
                            "labels": [{ "name": "baseline", "color": "0e8a16" }],
                            "comments": 3
                        })
                    })
                    .collect();
                let mut response = json(200, serde_json::json!(rows));
                if !page2 {
                    response.headers.push((
                        "link".into(),
                        "<https://api.github.com/repos/alice/demo/issues?state=open&sort=created&direction=desc&per_page=30&page=2>; rel=\"next\"".into(),
                    ));
                }
                return Ok(response);
            }
            if url.contains("/search/issues") {
                if url.contains("repo%3Aalice%2Fdemo") && url.contains("is%3Apr") {
                    let page2 = url.contains("page=2");
                    return Ok(json(
                        200,
                        if page2 {
                            self.fixture.repo_pulls_p2_json.clone()
                        } else {
                            self.fixture.repo_pulls_p1_json.clone()
                        },
                    ));
                }
                if url.contains("repo%3Aalice%2Fdemo") {
                    let page2 = url.contains("page=2");
                    return Ok(json(
                        200,
                        if page2 {
                            self.fixture.repo_issues_p2_json.clone()
                        } else {
                            self.fixture.repo_issues_p1_json.clone()
                        },
                    ));
                }
                return Ok(json(200, self.fixture.home_items_json.clone()));
            }
            if url.ends_with("/repos/alice/demo") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 100,
                        "name": "demo",
                        "full_name": "alice/demo",
                        "visibility": "public",
                        "private": false,
                        "fork": false,
                        "archived": false,
                        "default_branch": "main",
                        "description": "Synthetic repository 0",
                        "html_url": "https://github.com/alice/demo",
                        "stargazers_count": 42,
                        "forks_count": 7,
                        "open_issues_count": 50,
                        "size": 1024,
                        "owner": { "login": "alice", "id": 1, "avatar_url": null }
                    }),
                ));
            }
            if url.ends_with("/repos/alice/demo/branches")
                || url.contains("/repos/alice/demo/branches?")
            {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "name": "main",
                        "commit": { "sha": COMMIT_SHA }
                    }]),
                ));
            }
            if url.ends_with("/repos/alice/demo/releases")
                || url.contains("/repos/alice/demo/releases?")
            {
                return Ok(json(200, serde_json::json!([])));
            }
            if url.ends_with("/repos/alice/demo/contributors")
                || url.contains("/repos/alice/demo/contributors?")
            {
                return Ok(json(
                    200,
                    serde_json::json!([{ "id": 1, "login": "alice", "contributions": 42, "avatar_url": "" }]),
                ));
            }
            if url.ends_with("/repos/alice/demo/languages") {
                return Ok(json(
                    200,
                    serde_json::json!({ "Rust": 90000, "Markdown": 10000 }),
                ));
            }
            if url.ends_with("/repos/alice/demo/commits/main")
                || url.ends_with(&format!("/repos/alice/demo/commits/{COMMIT_SHA}"))
            {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "sha": COMMIT_SHA,
                        "commit": {
                            "message": "Baseline commit",
                            "tree": { "sha": TREE_SHA },
                            "author": { "name": "alice", "date": "2026-11-27T09:00:00Z" }
                        }
                    }),
                ));
            }
            if url.contains(&format!("/repos/alice/demo/git/trees/{TREE_SHA}")) {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "sha": TREE_SHA,
                        "truncated": false,
                        "tree": self.fixture.tree_entries
                    }),
                ));
            }
            if url.contains("/repos/alice/demo/readme") {
                use base64::Engine as _;
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(&self.fixture.readme_text);
                return Ok(json(
                    200,
                    serde_json::json!({
                        "name": "README.md",
                        "path": "README.md",
                        "sha": README_BLOB_SHA,
                        "size": self.fixture.readme_text.len(),
                        "type": "file",
                        "encoding": "base64",
                        "content": encoded
                    }),
                ));
            }
            if url.ends_with(&format!("/repos/alice/demo/git/blobs/{README_BLOB_SHA}")) {
                use base64::Engine as _;
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(&self.fixture.readme_text);
                return Ok(json(
                    200,
                    serde_json::json!({
                        "sha": README_BLOB_SHA,
                        "size": self.fixture.readme_text.len(),
                        "encoding": "base64",
                        "content": encoded
                    }),
                ));
            }
            for (sha, bytes) in self.fixture.images.values() {
                if url.ends_with(&format!("/repos/alice/demo/git/blobs/{sha}")) {
                    use base64::Engine as _;
                    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "sha": sha,
                            "size": bytes.len(),
                            "encoding": "base64",
                            "content": encoded
                        }),
                    ));
                }
            }
            if url.ends_with("/repos/alice/demo/issues/42") {
                return Ok(json(200, self.fixture.issue_42_json.clone()));
            }
            if url.contains("/repos/alice/demo/issues/42/comments") {
                let page2 = url.contains("page=2");
                return Ok(json(
                    200,
                    if page2 {
                        self.fixture.issue_42_comments_p2_json.clone()
                    } else {
                        self.fixture.issue_42_comments_p1_json.clone()
                    },
                ));
            }
            if url.ends_with("/repos/alice/demo/pulls/5") {
                return Ok(json(200, self.fixture.pr_5_json.clone()));
            }
            if url.contains("/repos/alice/demo/issues/5/comments") {
                let page2 = url.contains("page=2");
                return Ok(json(
                    200,
                    if page2 {
                        self.fixture.pr_5_comments_p2_json.clone()
                    } else {
                        self.fixture.pr_5_comments_p1_json.clone()
                    },
                ));
            }
            if url.contains("/repos/alice/demo/pulls/5/files") {
                return Ok(json(200, self.fixture.pr_5_files_json.clone()));
            }
            if url.ends_with("/users/alice") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 1,
                        "login": "alice",
                        "name": "Alice",
                        "bio": "Baseline profile",
                        "followers": 10,
                        "following": 5,
                        "public_repos": 12,
                        "avatar_url": ""
                    }),
                ));
            }
            if url.ends_with("/graphql") {
                if body_str.contains("query PRConversation") {
                    return Ok(graphql_ok(serde_json::json!({
                        "repository": {
                            "nameWithOwner": "alice/demo",
                            "databaseId": 100,
                            "pullRequest": {
                                "number": 5,
                                "title": "PR #5: Baseline conversation and files changed",
                                "state": "OPEN",
                                "isDraft": false,
                                "author": { "login": "alice", "avatarUrl": "" },
                                "body": self.fixture.pr_5_json["body"],
                                "baseRefName": "main",
                                "headRefName": "feat/baseline",
                                "headRef": null,
                                "headRepository": null
                            }
                        }
                    })));
                }
                if body_str.contains("pinnedItems") {
                    return Ok(graphql_ok(serde_json::json!({
                        "user": {
                            "pinnedItems": { "nodes": [] },
                            "gists": { "totalCount": 0 },
                            "repositories": { "nodes": [] }
                        }
                    })));
                }
                if body_str.contains("contributionCalendar") {
                    return Ok(graphql_ok(serde_json::json!({
                        "user": {
                            "contributionsCollection": {
                                "contributionCalendar": {
                                    "totalContributions": 42,
                                    "weeks": []
                                }
                            }
                        }
                    })));
                }
                return Ok(graphql_ok(serde_json::json!({
                    "repository": {
                        "id": "R_100",
                        "nameWithOwner": "alice/demo",
                        "defaultBranchRef": { "name": "main" },
                        "stargazerCount": 42,
                        "forkCount": 7,
                        "viewerHasStarred": false,
                        "issues": { "totalCount": 50 },
                        "pullRequests": { "totalCount": 40 }
                    }
                })));
            }
            if url.contains("/user/starred/") {
                return Ok(json(404, serde_json::json!({})));
            }
            Ok(json(200, serde_json::json!([])))
        })
    }
}

fn pump_shell(shell: &Entity<Shell>, controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.drain_for_test(cx));
    cx.update_window_entity(shell, |shell, window, cx| {
        shell.drain_all_pages_for_test(window, cx);
    });
    cx.update(|window, cx| {
        window.refresh();
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
}

fn wait_until(
    shell: &Entity<Shell>,
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    timeout: Duration,
    mut pred: impl FnMut(&mut VisualTestContext) -> bool,
) -> Duration {
    let start = Instant::now();
    loop {
        pump_shell(shell, controller, cx);
        if pred(cx) {
            return start.elapsed();
        }
        assert!(
            start.elapsed() < timeout,
            "baseline condition timed out after {:?}",
            timeout
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[allow(clippy::too_many_arguments)]
fn wait_settled_from(
    t0: Instant,
    shell: &Entity<Shell>,
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    transport: &BaselineTransport,
    selector: &'static str,
    timeout: Duration,
    mut extra_ready: impl FnMut(&Entity<Shell>, &mut VisualTestContext) -> bool,
) -> (Duration, Duration) {
    let mut first_content = None;
    loop {
        pump_shell(shell, controller, cx);
        if first_content.is_none() && cx.debug_bounds(selector).is_some() {
            first_content = Some(t0.elapsed());
        }
        if let Some(first) = first_content
            && transport.in_flight() == 0
            && extra_ready(shell, cx)
        {
            pump_shell(shell, controller, cx);
            return (first, t0.elapsed());
        }
        assert!(
            t0.elapsed() < timeout,
            "baseline settle timed out for {selector} (in_flight={}, reqs={}, home_view={}, services={}, urls={:?})",
            transport.in_flight(),
            transport.request_count.load(Ordering::SeqCst),
            cx.debug_bounds("home-view").is_some(),
            controller.read_with(cx, |a, _| a.services().is_some()),
            transport.urls.lock().unwrap().as_slice(),
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn summarize_ms(samples: &[f64]) -> (f64, f64, f64) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let min = *sorted.first().unwrap_or(&0.0);
    let max = *sorted.last().unwrap_or(&0.0);
    let median = sorted[sorted.len() / 2];
    (min, median, max)
}

#[test]
#[ignore = "windows reading baseline measurement"]
fn windows_reading_baseline_harness() {
    let fixture = Arc::new(BaselineFixture::build());
    let total_image_bytes: usize = fixture.images.values().map(|(_, b)| b.len()).sum();
    eprintln!(
        "BASELINE_FIXTURE: readme_bytes={} image_count={} total_image_png_bytes={} tree_entries={}",
        fixture.readme_text.len(),
        fixture.images.len(),
        total_image_bytes,
        fixture.tree_entries.len()
    );

    let mut cold_home_first_ms = Vec::new();
    let mut cold_home_complete_ms = Vec::new();
    let mut warm_home_first_ms = Vec::new();
    let mut warm_home_complete_ms = Vec::new();
    let mut cold_repo_first_ms = Vec::new();
    let mut cold_repo_complete_ms = Vec::new();
    let mut warm_repo_first_ms = Vec::new();
    let mut warm_repo_complete_ms = Vec::new();
    let mut readme_scroll_ms = Vec::new();
    let mut issues_page2_ms = Vec::new();
    let mut issue_detail_ms = Vec::new();
    let mut pr_files_ms = Vec::new();
    let mut tab_switch_ms = Vec::new();

    let mut last_cold_home_reqs = 0usize;
    let mut last_warm_home_reqs = 0usize;
    let mut last_cold_repo_reqs = 0usize;
    let mut last_warm_repo_reqs = 0usize;
    let mut last_tab_switch_reqs = 0usize;
    let mut last_cache_stats = (0usize, 0u64, 0u64);

    let mut mem_before = ProcessMem::default();
    let mut mem_after_home = ProcessMem::default();
    let mut mem_after_all_tabs = ProcessMem::default();
    let mut mem_after_close_tabs = ProcessMem::default();
    let mut open_tab_count = 0usize;
    let mut closed_tab_count = 0usize;

    for run in 0..REPEAT_COUNT {
        let temp = tempfile::tempdir().unwrap();
        let clock = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let creds = Arc::new(InMemoryCredentialStore::new());

        // --- Phase 1: Cold Start (Fresh Cache) ---
        let mut cold_cx = gpui::TestAppContext::build(gpui::TestDispatcher::new(0), None);
        setup(&mut cold_cx);
        let cold_transport = Arc::new(BaselineTransport::new(fixture.clone(), 0));
        let executor = Arc::new(BackgroundExecutor::new(4));
        let t0 = Instant::now();
        let store = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        let service = clock
            .block_on(open_accounts_with_cache(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                creds.clone(),
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    cold_transport.clone(),
                )),
                executor.clone(),
                Ok(store.clone()),
            ))
            .unwrap();
        clock
            .block_on(service.login(Pat::new("synthetic-baseline-pat".to_string())))
            .unwrap();
        let _service_login_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let runtime = Arc::new(AccountRuntime::new(service, executor));

        if run == 0 {
            mem_before = read_process_memory();
        }
        cold_transport.reset_count();
        let mount_t0 = Instant::now();
        let mut controller_slot = None;
        let (shell, vcx) = cold_cx.add_window_view(|window, cx| {
            let controller = cx.new(|cx| Accounts::new(Some(runtime.clone()), window, cx));
            controller_slot = Some(controller.clone());
            cx.set_global(AccountsHandle(controller));
            Shell::new(window, cx)
        });
        let controller = controller_slot.unwrap();
        vcx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(900.)));
        wait_until(&shell, &controller, vcx, Duration::from_secs(10), |cx| {
            controller.read_with(cx, |a, _| a.services().is_some())
        });
        vcx.update_window_entity(&shell, |shell, window, cx| {
            shell.home_mount_for_test(window, cx);
        });
        let (home_first, home_complete) = wait_settled_from(
            mount_t0,
            &shell,
            &controller,
            vcx,
            &cold_transport,
            "home-item-row-0",
            Duration::from_secs(15),
            |shell, cx| shell.read_with(cx, |s, cx| s.home_is_ready_for_test(cx)),
        );
        cold_home_first_ms.push(home_first.as_secs_f64() * 1000.0);
        cold_home_complete_ms.push(home_complete.as_secs_f64() * 1000.0);
        last_cold_home_reqs = cold_transport.reset_count();
        if run == 0 {
            mem_after_home = read_process_memory();
        }

        // --- Phase 2: Cold Repository (Long README + 4 PNGs + 60-entry Tree) ---
        let repo_t0 = Instant::now();
        vcx.update_window_entity(&shell, |shell, window, cx| {
            shell.navigate(
                Route::Repository {
                    namespace: "alice".into(),
                    name: "demo".into(),
                },
                window,
                cx,
            );
        });
        let (repo_first, repo_complete) = wait_settled_from(
            repo_t0,
            &shell,
            &controller,
            vcx,
            &cold_transport,
            "markdown-heading-0",
            Duration::from_secs(20),
            |_, cx| cx.debug_bounds("repository-page").is_some(),
        );
        cold_repo_first_ms.push(repo_first.as_secs_f64() * 1000.0);
        cold_repo_complete_ms.push(repo_complete.as_secs_f64() * 1000.0);
        last_cold_repo_reqs = cold_transport.reset_count();

        // Scroll long README (20 down + 20 up)
        let scroll_t0 = Instant::now();
        if let Some(repo_bounds) = vcx.debug_bounds("repository-page") {
            let center = repo_bounds.center();
            for _ in 0..20 {
                vcx.simulate_event(gpui::ScrollEvent {
                    position: center,
                    delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-240.0))),
                    modifiers: gpui::Modifiers::none(),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                });
                vcx.run_until_parked();
            }
            for _ in 0..20 {
                vcx.simulate_event(gpui::ScrollEvent {
                    position: center,
                    delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(240.0))),
                    modifiers: gpui::Modifiers::none(),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                });
                vcx.run_until_parked();
            }
        }
        readme_scroll_ms.push(scroll_t0.elapsed().as_secs_f64() * 1000.0);

        // --- Phase 3: Repository Issues List (50 items) + Load More ---
        if let Some(tab) = vcx.debug_bounds("repository-tab-issues") {
            vcx.simulate_click(tab.center(), gpui::Modifiers::none());
        }
        let issues_t0 = Instant::now();
        wait_settled_from(
            issues_t0,
            &shell,
            &controller,
            vcx,
            &cold_transport,
            "repository-issues-more",
            Duration::from_secs(15),
            |_, _| true,
        );
        let p2_t0 = Instant::now();
        if let Some(more) = vcx.debug_bounds("repository-issues-more") {
            vcx.simulate_click(more.center(), gpui::Modifiers::none());
        }
        wait_until(&shell, &controller, vcx, Duration::from_secs(15), |cx| {
            cx.debug_bounds("repository-issues-more").is_none() && cold_transport.in_flight() == 0
        });
        if let Some(viewport) = vcx.debug_bounds("repository-issues-viewport") {
            vcx.simulate_event(gpui::ScrollEvent {
                position: viewport.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-3000.0))),
                modifiers: gpui::Modifiers::none(),
                touch_phase: gpui::TouchPhase::Moved,
                ..Default::default()
            });
        }
        wait_until(&shell, &controller, vcx, Duration::from_secs(15), |cx| {
            cx.debug_bounds("repository-issue-title-50").is_some()
        });
        issues_page2_ms.push(p2_t0.elapsed().as_secs_f64() * 1000.0);

        // --- Phase 4: Issue #42 Detail + PR #5 Conversation + Files Changed ---
        let issue_t0 = Instant::now();
        vcx.update_window_entity(&shell, |shell, window, cx| {
            shell.navigate(
                Route::Issue {
                    namespace: "alice".into(),
                    name: "demo".into(),
                    number: 42,
                },
                window,
                cx,
            );
        });
        let (_, issue_complete) = wait_settled_from(
            issue_t0,
            &shell,
            &controller,
            vcx,
            &cold_transport,
            "issue-detail-body",
            Duration::from_secs(15),
            |_, _| true,
        );
        issue_detail_ms.push(issue_complete.as_secs_f64() * 1000.0);

        let pr_t0 = Instant::now();
        vcx.update_window_entity(&shell, |shell, window, cx| {
            shell.navigate(
                Route::PullRequest {
                    namespace: "alice".into(),
                    name: "demo".into(),
                    number: 5,
                },
                window,
                cx,
            );
        });
        wait_settled_from(
            pr_t0,
            &shell,
            &controller,
            vcx,
            &cold_transport,
            "pr-conversation-body",
            Duration::from_secs(15),
            |_, _| true,
        );
        let files_t0 = Instant::now();
        if let Some(files_tab) = vcx.debug_bounds("pr-tab-files-changed") {
            vcx.simulate_click(files_tab.center(), gpui::Modifiers::none());
        }
        wait_until(&shell, &controller, vcx, Duration::from_secs(15), |cx| {
            cold_transport.in_flight() == 0 && cx.debug_bounds("pr-file-0").is_some()
        });
        pr_files_ms.push(files_t0.elapsed().as_secs_f64() * 1000.0);

        // --- Phase 5: Open remaining tabs for Memory & Repeated Navigation ---
        for route in [
            Route::Profile,
            Route::Notifications,
            Route::Issues,
            Route::PullRequests,
            Route::Settings,
        ] {
            vcx.update_window_entity(&shell, |shell, window, cx| {
                shell.navigate(route, window, cx);
            });
            wait_until(&shell, &controller, vcx, Duration::from_secs(15), |_| {
                cold_transport.in_flight() == 0
            });
        }
        if run == 0 {
            open_tab_count =
                vcx.update_window_entity(&shell, |s, _, _| s.workspace().sessions().len());
            assert_eq!(open_tab_count, 9, "expected Home + 8 opened tabs = 9");
            mem_after_all_tabs = read_process_memory();
        }

        // Repeated warm tab switching (Home -> Repo -> Issue -> PR -> Home)
        cold_transport.reset_count();
        let switch_t0 = Instant::now();
        for route in [
            Route::Home,
            Route::Repository {
                namespace: "alice".into(),
                name: "demo".into(),
            },
            Route::Issue {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 42,
            },
            Route::PullRequest {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 5,
            },
            Route::Home,
        ] {
            vcx.update_window_entity(&shell, |shell, window, cx| {
                shell.navigate(route, window, cx);
            });
            vcx.run_until_parked();
        }
        tab_switch_ms.push(switch_t0.elapsed().as_secs_f64() * 1000.0);
        last_tab_switch_reqs = cold_transport.reset_count();

        // Close all non-Home tabs
        vcx.update_window_entity(&shell, |shell, window, cx| {
            let tabs: Vec<_> = shell
                .workspace()
                .sessions()
                .iter()
                .filter(|t| *t.route() != Route::Home)
                .map(|t| t.id())
                .collect();
            for id in tabs {
                shell.close_tab(id, window, cx);
            }
        });
        for _ in 0..5 {
            vcx.run_until_parked();
        }
        if run == 0 {
            closed_tab_count =
                vcx.update_window_entity(&shell, |s, _, _| s.workspace().sessions().len());
            assert_eq!(closed_tab_count, 1, "expected only Home tab remaining");
            mem_after_close_tabs = read_process_memory();
        }

        // Close window, clear global AccountsHandle, quit cold_cx to drain on_quit, then wait for shutdown
        let weak_runtime = Arc::downgrade(&runtime);
        vcx.update(|window, cx| {
            window.remove_window();
            if cx.has_global::<AccountsHandle>() {
                cx.remove_global::<AccountsHandle>();
            }
        });
        drop(shell);
        drop(controller);
        drop(runtime);
        drop(store);
        cold_cx.quit();
        drop(cold_cx);
        crate::app::accounts::wait_for_shutdown();
        eprintln!(
            "AFTER_COLD_QUIT weak_runtime_strong_count={}",
            weak_runtime.strong_count()
        );

        // --- Phase 6: Warm Reopen (Real CacheStore Reopen, 150ms Delayed Transport) ---
        let mut warm_cx = gpui::TestAppContext::build(gpui::TestDispatcher::new(0), None);
        setup(&mut warm_cx);
        let warm_transport = Arc::new(BaselineTransport::new(fixture.clone(), 150));
        let warm_exec = Arc::new(BackgroundExecutor::new(4));
        let warm_store = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        let warm_service = clock
            .block_on(open_accounts_with_cache(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                creds,
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    warm_transport.clone(),
                )),
                warm_exec.clone(),
                Ok(warm_store.clone()),
            ))
            .unwrap();
        let warm_runtime = Arc::new(AccountRuntime::new(warm_service, warm_exec));
        warm_transport.reset_count();

        let warm_mount_t0 = Instant::now();
        let mut warm_controller_slot = None;
        let (warm_shell, wcx) = warm_cx.add_window_view(|window, cx| {
            let controller = cx.new(|cx| Accounts::new(Some(warm_runtime.clone()), window, cx));
            warm_controller_slot = Some(controller.clone());
            cx.set_global(AccountsHandle(controller));
            Shell::new(window, cx)
        });
        let warm_controller = warm_controller_slot.unwrap();
        wcx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(900.)));
        wait_until(
            &warm_shell,
            &warm_controller,
            wcx,
            Duration::from_secs(10),
            |cx| warm_controller.read_with(cx, |a, _| a.services().is_some()),
        );
        wcx.update_window_entity(&warm_shell, |shell, window, cx| {
            shell.home_mount_for_test(window, cx);
        });
        let (w_home_first, w_home_complete) = wait_settled_from(
            warm_mount_t0,
            &warm_shell,
            &warm_controller,
            wcx,
            &warm_transport,
            "home-item-row-0",
            Duration::from_secs(15),
            |shell, cx| shell.read_with(cx, |s, cx| s.home_is_ready_for_test(cx)),
        );
        warm_home_first_ms.push(w_home_first.as_secs_f64() * 1000.0);
        warm_home_complete_ms.push(w_home_complete.as_secs_f64() * 1000.0);
        last_warm_home_reqs = warm_transport.reset_count();

        let warm_repo_t0 = Instant::now();
        wcx.update_window_entity(&warm_shell, |shell, window, cx| {
            shell.navigate(
                Route::Repository {
                    namespace: "alice".into(),
                    name: "demo".into(),
                },
                window,
                cx,
            );
        });
        let (w_repo_first, w_repo_complete) = wait_settled_from(
            warm_repo_t0,
            &warm_shell,
            &warm_controller,
            wcx,
            &warm_transport,
            "markdown-heading-0",
            Duration::from_secs(20),
            |_, cx| cx.debug_bounds("repository-page").is_some(),
        );
        warm_repo_first_ms.push(w_repo_first.as_secs_f64() * 1000.0);
        warm_repo_complete_ms.push(w_repo_complete.as_secs_f64() * 1000.0);
        last_warm_repo_reqs = warm_transport.reset_count();

        let (cache_files, cache_bytes) = dir_disk_usage(&temp.path().join("cache"));
        let db_bytes = sqlite_disk_usage(&temp.path().join("accounts.sqlite3"));
        last_cache_stats = (cache_files, cache_bytes, db_bytes);

        wcx.update(|window, cx| {
            window.remove_window();
            if cx.has_global::<AccountsHandle>() {
                cx.remove_global::<AccountsHandle>();
            }
        });
        drop(warm_shell);
        drop(warm_controller);
        drop(warm_runtime);
        drop(warm_store);
        warm_cx.quit();
        drop(warm_cx);
        crate::app::accounts::wait_for_shutdown();
    }

    let report = serde_json::json!({
        "platform": "gpui_test_platform_headless",
        "native_window": false,
        "repeat_count": REPEAT_COUNT,
        "cold_home_first_ms": summarize_ms(&cold_home_first_ms),
        "cold_home_settled_idle_ms": summarize_ms(&cold_home_complete_ms),
        "warm_home_first_ms": summarize_ms(&warm_home_first_ms),
        "warm_home_settled_idle_ms": summarize_ms(&warm_home_complete_ms),
        "cold_repo_first_ms": summarize_ms(&cold_repo_first_ms),
        "cold_repo_settled_idle_ms": summarize_ms(&cold_repo_complete_ms),
        "warm_repo_first_ms": summarize_ms(&warm_repo_first_ms),
        "warm_repo_settled_idle_ms": summarize_ms(&warm_repo_complete_ms),
        "readme_scroll_40steps_ms": summarize_ms(&readme_scroll_ms),
        "issues_page2_ms": summarize_ms(&issues_page2_ms),
        "issue_42_detail_ms": summarize_ms(&issue_detail_ms),
        "pr_5_files_changed_ms": summarize_ms(&pr_files_ms),
        "warm_tab_switch_5x_ms": summarize_ms(&tab_switch_ms),
        "mock_requests": {
            "cold_home": last_cold_home_reqs,
            "warm_home": last_warm_home_reqs,
            "cold_repo": last_cold_repo_reqs,
            "warm_repo": last_warm_repo_reqs,
            "warm_tab_switch_5x": last_tab_switch_reqs,
        },
        "disk_usage": {
            "cache_files": last_cache_stats.0,
            "cache_bytes": last_cache_stats.1,
            "accounts_sqlite_bytes": last_cache_stats.2,
        },
        "tab_counts": {
            "opened_total": open_tab_count,
            "after_close": closed_tab_count,
        },
        "memory_run0_bytes": {
            "before_shell": [mem_before.working_set_bytes, mem_before.peak_working_set_bytes, mem_before.private_bytes],
            "after_home": [mem_after_home.working_set_bytes, mem_after_home.peak_working_set_bytes, mem_after_home.private_bytes],
            "after_all_tabs": [mem_after_all_tabs.working_set_bytes, mem_after_all_tabs.peak_working_set_bytes, mem_after_all_tabs.private_bytes],
            "after_close_tabs": [mem_after_close_tabs.working_set_bytes, mem_after_close_tabs.peak_working_set_bytes, mem_after_close_tabs.private_bytes],
        }
    });
    eprintln!("READING_BASELINE_REPORT={report}");
}
