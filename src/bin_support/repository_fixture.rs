//! Deterministic development fixture for the repository page preview and its tests.
//!
//! This module is development-only material: it is compiled into the
//! `examples/repository.rs` preview and the `tests/repository.rs` suite through
//! `src/bin_support/repository_modules.rs`, and the product binary never
//! declares it. It is the *one* fixture both the preview and the acceptance
//! tests browse, so what the owner clicks is exactly what the tests exercise.
//!
//! The fixture is a small but real repository answered by request identity:
//! method, path, query and the branch/commit/tree/blob each request names. A
//! successful resource can be read again and again, in any order, and
//! concurrently — there is no one-shot queue. Anything the fixture does not
//! model is a **development configuration error** with a diagnostic message, not
//! a pretend GitHub permission failure.
//!
//! The only scripted failures are explicit scenes (`Scene::DirectoryFailure` and
//! friends), each of which fails exactly one resource once and then heals, so a
//! single bounded retry can recover.

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::sync::Mutex;

use crate::network::cancel::CancellationToken;
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};

/// What the preview/mounting code asks the fixture to demonstrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    /// Normal repository: two branches, README, docs, src and a nested directory.
    Normal,
    /// A real 409: the Git repository exists but has no commit data yet.
    EmptyRepository,
    /// README discovery 404 on a non-empty commit: absence vs. lost visibility.
    ReadmeUncertain,
    /// A complete, readable empty root tree proves there is no README.
    NoReadme,
    /// The first attempt to open `docs` fails; a retry succeeds.
    DirectoryFailure,
    /// The first attempt to read `src/main.rs` fails; a retry succeeds.
    FileFailure,
    /// A complete, empty branch list: no commit can be selected.
    NoBranches,
    /// A truncated (`truncated: true`) root tree with unknown/odd entry types.
    TruncatedTree,
    /// A very large root tree for scrolling and virtualization.
    LongTree,
    /// Every repository read takes 250ms; useful for branch/selection race checks.
    Delayed,
    /// Preferred README is a real file under docs/, not at the root.
    ReadmeInDocs,
    /// The first root directory read fails; Retry files recovers it.
    RootFailure,
    OverviewLoading,
    OverviewPartial,
    OverviewError,
    OverviewZero,
    OverviewLong,
    LatestUnavailable,
}

impl Scene {
    /// Every scene the preview toolbar offers, in display order.
    pub const ALL: [Scene; 18] = [
        Scene::Normal,
        Scene::EmptyRepository,
        Scene::ReadmeUncertain,
        Scene::NoReadme,
        Scene::DirectoryFailure,
        Scene::FileFailure,
        Scene::NoBranches,
        Scene::TruncatedTree,
        Scene::LongTree,
        Scene::Delayed,
        Scene::ReadmeInDocs,
        Scene::RootFailure,
        Scene::OverviewLoading,
        Scene::OverviewPartial,
        Scene::OverviewError,
        Scene::OverviewZero,
        Scene::OverviewLong,
        Scene::LatestUnavailable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Scene::OverviewLoading => "Overview loading",
            Scene::OverviewPartial => "Overview partial",
            Scene::OverviewError => "Overview error",
            Scene::OverviewZero => "Overview zero",
            Scene::OverviewLong => "Overview long",
            Scene::LatestUnavailable => "Latest unavailable",
            Scene::Normal => "Normal",
            Scene::EmptyRepository => "Empty repository",
            Scene::ReadmeUncertain => "README uncertain",
            Scene::NoReadme => "No README",
            Scene::DirectoryFailure => "Directory failure",
            Scene::FileFailure => "File failure",
            Scene::NoBranches => "No branches",
            Scene::TruncatedTree => "Truncated tree",
            Scene::LongTree => "Long tree",
            Scene::Delayed => "Delayed reads",
            Scene::ReadmeInDocs => "README in docs",
            Scene::RootFailure => "Root failure",
        }
    }

    /// One line the preview toolbar shows about what this scene demonstrates.
    pub fn description(self) -> &'static str {
        match self {
            Scene::OverviewLoading => {
                "Repository extras take 2 seconds; browse files while the compact strip loads."
            }
            Scene::OverviewPartial => {
                "Releases and contributors are partial observations; usable data remains visible."
            }
            Scene::OverviewError => {
                "Languages fail once; Retry restores only Languages and keeps the file selection."
            }
            Scene::OverviewZero => {
                "No latest observed, no contributors and zero language bytes; no distribution is invented."
            }
            Scene::OverviewLong => {
                "Long version, many contributors and languages stay in the bounded compact strip."
            }
            Scene::LatestUnavailable => {
                "The list contains releases but latest is 404: latest unavailable does not prove no releases."
            }
            Scene::Normal => {
                "Two branches, README, docs, src and a nested directory. Everything is browsable and repeatable."
            }
            Scene::EmptyRepository => {
                "GitHub answers 409: the Git repository is empty or unavailable, so no empty listing is invented."
            }
            Scene::ReadmeUncertain => {
                "README discovery is 404 on a non-empty commit: absence and lost visibility cannot be told apart."
            }
            Scene::NoReadme => {
                "A complete, readable empty root tree proves this commit has no README; the tree stays usable."
            }
            Scene::DirectoryFailure => {
                "Opening docs fails once; the directory row reports it and one retry succeeds."
            }
            Scene::FileFailure => {
                "Reading src/main.rs fails once; the reader reports it and one retry succeeds."
            }
            Scene::NoBranches => {
                "A complete, empty branch list: there is no commit to browse, so none is guessed."
            }
            Scene::TruncatedTree => {
                "The root tree is truncated and carries unknown entry types; it is never shown as a complete directory."
            }
            Scene::LongTree => "400 root files for scrolling and virtualization checks.",
            Scene::RootFailure => {
                "The root directory fails once. Retry files restores it while keeping the repository and README."
            }
            Scene::Delayed => {
                "Reads take 250ms. Switch branches or files while loading; only the current request may publish."
            }
            Scene::ReadmeInDocs => {
                "The preferred README lives at docs/README.md. Its ancestor opens and its real row is selected."
            }
        }
    }
}

// ------------------------------------------------------------------ repository --

/// Branch names, in the order GitHub lists them.
const BRANCHES: [&str; 2] = ["main", "feature/docs-polish"];

/// One commit per branch: the fixture's two branches really differ.
const MAIN_COMMIT: &str = "c0ffee0000000000000000000000000000000001";
const FEATURE_COMMIT: &str = "c0ffee0000000000000000000000000000000002";
const MAIN_TREE: &str = "a000000000000000000000000000000000000001";
const FEATURE_TREE: &str = "a000000000000000000000000000000000000002";
/// Extra trees, all reachable from both commits.
const DOCS_TREE: &str = "a000000000000000000000000000000000000010";
const FEATURE_DOCS_TREE: &str = "a000000000000000000000000000000000000013";
const FEATURE_GUIDE_BLOB: &str = "b00000000000000000000000000000000000000b";
const DOCS_GUIDES_TREE: &str = "a000000000000000000000000000000000000011";
const SRC_TREE: &str = "a000000000000000000000000000000000000020";
const FEATURE_SRC_TREE: &str = "a000000000000000000000000000000000000021";

const README_BLOB: &str = "b000000000000000000000000000000000000001";
const FEATURE_README_BLOB: &str = "b00000000000000000000000000000000000000a";
const GUIDE_BLOB: &str = "b000000000000000000000000000000000000002";
const NESTED_BLOB: &str = "b000000000000000000000000000000000000003";
const MAIN_RS_BLOB: &str = "b000000000000000000000000000000000000004";
const BINARY_BLOB: &str = "b000000000000000000000000000000000000005";
const EMPTY_BLOB: &str = "b000000000000000000000000000000000000006";
const HUGE_BLOB: &str = "b000000000000000000000000000000000000007";
const FEATURE_ONLY_BLOB: &str = "b000000000000000000000000000000000000008";

/// How many files the `LongTree` scene's root holds.
pub const LONG_TREE_FILES: usize = 400;

const README_TEXT: &str = r#"# Ledger

A tiny double-entry ledger, kept small enough to browse: two branches, nested
folders, Markdown, source code and a few unusual files.

## What is here

- `README.md` — this file
- `docs/guide.md` — a short guide
- `docs/guides/nested/deep-note.md` — a note two levels down
- `src/main.rs` — the program entry point
- `assets/logo.bin` — a small binary file

## Try it

- open `docs/guide.md` from the tree
- open the [relative link](docs/guide.md) below
- open `src/main.rs` for the read-only source view

> `main` and `feature/docs-polish` really do differ: the second branch
> adds `src/feature_note.rs` and rewrites the guide.

```rust
fn main() {
    let total = split_in_half(9, 4.5);
    println!("{total:?}"); // 你好 · 🚀
}
```
"#;

const FEATURE_README_TEXT: &str = r#"# Ledger (docs polish)

This branch rewrites the guide and adds a note file, so its README differs from
`main` on purpose.

## What changed here

- `docs/guide.md` was rewritten
- `src/feature_note.rs` exists only on this branch

Read the [guide](docs/guide.md) for the long version.
"#;

const GUIDE_TEXT: &str = r#"# Guide

The guide shows a normal Markdown document with the pieces the reader supports.

## Steps

1. Open `README.md`.
2. Follow the relative link back to [`README.md`](../README.md).
3. Open `src/main.rs` for source.

## Notes

- inline `code`, **bold**, *italic*
- a [nested note](guides/nested/deep-note.md)
- external: [Rust](https://www.rust-lang.org/)

> 中文段落：目录、分支、文件都可以反复打开。

```text
no syntax highlighting claim, only the generic highlighter
```
"#;

const FEATURE_GUIDE_TEXT: &str = r#"# Guide (rewritten)

On `feature/docs-polish` the guide says something different, which is how you
can tell the fixed commit of a switch apart.

Back to the [README](../README.md).
"#;

const NESTED_TEXT: &str = r#"# Deep note

This file lives two levels under `docs`, so its row has to be indented twice.

Back up to the [guide](../../guide.md) or the [README](../../../README.md).
"#;

const MAIN_RS_TEXT: &str = "//! A tiny ledger.\n\nfn split_in_half(amount: u32, shares: f64) -> f64 {\n    let each = amount as f64 / shares;\n    each\n}\n\nfn main() {\n    println!(\"{}\", split_in_half(9, 4.5));\n}\n";

const FEATURE_NOTE_TEXT: &str =
    "//! Only present on `feature/docs-polish`.\n\npub const NOTE: &str = \"你 好 · 🚀\";\n";

const BINARY_BYTES: [u8; 48] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05,
    0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00, 0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1,
    0x52, 0x49, 0x46, 0x46, 0x24, 0x00, 0x00, 0x00, 0x57, 0x41, 0x56, 0x45, 0x66, 0x6d, 0x74, 0x20,
];

/// Owner and name of the fixture repository.
pub const OWNER: &str = "octo";
/// Repository name.
pub const NAME: &str = "ledger";

fn locator_path() -> String {
    format!("/repos/{OWNER}/{NAME}")
}

// --------------------------------------------------------------------- server --

/// Which scripted, one-shot failure a scene arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArmedFailure {
    Languages,
    Root,
    Directory,
    File,
}

/// A deterministic, request-addressed repository.
///
/// `execute` answers by identity: the same request returns the same response as
/// often as it is asked. Only `armed` failures are consumed, and only once.
pub struct FixtureTransport {
    scene: Mutex<Scene>,
    armed: Mutex<Option<ArmedFailure>>,
    requests: Mutex<Vec<String>>,
    /// Development diagnostics: requests this fixture does not model.
    mismatches: Mutex<VecDeque<String>>,
    /// Which synthetic identity the next `/user` answers. Tests use it to make
    /// A→B→A a real account change instead of two names for one account.
    identity: Mutex<(u64, String)>,
}

impl FixtureTransport {
    pub fn new(scene: Scene) -> Self {
        Self {
            scene: Mutex::new(scene),
            armed: Mutex::new(armed_for(scene)),
            requests: Mutex::new(Vec::new()),
            mismatches: Mutex::new(VecDeque::new()),
            identity: Mutex::new((7, OWNER.to_string())),
        }
    }

    /// Sets the identity a later `/user` (i.e. login) will verify.
    pub fn set_identity(&self, id: u64, login: &str) {
        *self.identity.lock().unwrap() = (id, login.to_string());
    }

    /// Switches scene, clearing any half-consumed failure and the request log.
    pub fn set_scene(&self, scene: Scene) {
        *self.scene.lock().unwrap() = scene;
        *self.armed.lock().unwrap() = armed_for(scene);
        self.requests.lock().unwrap().clear();
        self.mismatches.lock().unwrap().clear();
    }

    pub fn scene(&self) -> Scene {
        *self.scene.lock().unwrap()
    }

    /// Every request URL the fixture answered, in order.
    pub fn recorded(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// How many requests the fixture answered whose path contains `needle`.
    pub fn count(&self, needle: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.contains(needle))
            .count()
    }

    pub fn clear_recorded(&self) {
        self.requests.lock().unwrap().clear();
    }

    /// Development configuration errors seen so far (unmodelled requests).
    pub fn mismatches(&self) -> Vec<String> {
        self.mismatches.lock().unwrap().iter().cloned().collect()
    }

    fn record(&self, url: &str) {
        self.requests.lock().unwrap().push(url.to_string());
    }

    fn mismatch(&self, url: &str) -> HttpResponse {
        let message = format!(
            "repository fixture does not model this request: {url}\n\
             The preview only answers the fixture repository's own resources. \
             Add the request to src/bin_support/repository_fixture.rs instead of \
             treating this as a GitHub permission result."
        );
        self.mismatches.lock().unwrap().push_back(message.clone());
        eprintln!("[repository fixture] {message}");
        json_response(599, &serde_json::json!({ "message": message }).to_string())
    }

    /// Answers one request by identity.
    fn answer(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let scene = self.scene();
        let url = &request.url;
        self.record(url);
        if request.method == crate::network::transport::HttpMethod::Post
            && url.ends_with("/graphql")
        {
            let body: serde_json::Value =
                serde_json::from_str(request.body.as_deref().unwrap_or("")).unwrap_or_default();
            if body["variables"]["owner"] != OWNER
                || body["variables"]["name"] != NAME
                || !body["query"].as_str().is_some_and(|query| {
                    query.contains("RepositoryOpenCounts")
                        && query.contains("states: OPEN")
                        && !query.contains("mutation")
                })
            {
                return Ok(self.mismatch(url));
            }
            let (issues, prs) = if scene == Scene::OverviewZero {
                (0, 0)
            } else if scene == Scene::OverviewLong {
                (123456, 98765)
            } else {
                (3, 1)
            };
            let mut value = serde_json::json!({"data":{"repository":{"databaseId":42,"issues":{"totalCount":issues},"pullRequests":{"totalCount":prs}}}});
            if scene == Scene::OverviewPartial {
                value["errors"] =
                    serde_json::json!([{"type":"FORBIDDEN","path":["repository","pullRequests"]}]);
            }
            if scene == Scene::OverviewError {
                return Ok(json_response(500, "{}"));
            }
            return Ok(json_response(200, &value.to_string()));
        }
        if request.method != crate::network::transport::HttpMethod::Get {
            return Ok(self.mismatch(url));
        }
        // The root repository path is `/repos/owner/name`; everything else hangs
        // off it, so strip the base once and dispatch on the remainder.
        let base = locator_path();
        let Some((path, query)) = split_url(url) else {
            return Ok(self.mismatch(url));
        };
        // The preview signs in through the real `AccountService`, so the
        // fixture also answers the identity endpoint that login verifies.
        if path == "/user" {
            let (id, login) = self.identity.lock().unwrap().clone();
            return Ok(json_response(
                200,
                &serde_json::json!({ "id": id, "login": login }).to_string(),
            ));
        }
        let Some(rest) = path.strip_prefix(&base) else {
            return Ok(self.mismatch(url));
        };

        match rest {
            // GET /repos/{owner}/{name}
            "" => Ok(json_response(200, &details_body(scene, url).to_string())),
            "/branches" => Ok(self.branches(scene, url)),
            "/releases" | "/releases/latest" | "/contributors" | "/languages" => {
                Ok(self.extras(rest, query, scene, url))
            }
            branch_path => {
                if let Some(reference) = branch_path.strip_prefix("/commits/") {
                    return Ok(self.commit(scene, reference, url));
                }
                if let Some(sha) = branch_path.strip_prefix("/git/trees/") {
                    return Ok(self.tree(scene, sha, url));
                }
                if let Some(sha) = branch_path.strip_prefix("/git/blobs/") {
                    return Ok(self.blob(scene, sha, url));
                }
                if branch_path == "/readme" {
                    return Ok(self.readme(scene, query, url));
                }
                Ok(self.mismatch(url))
            }
        }
    }

    fn branches(&self, scene: Scene, url: &str) -> HttpResponse {
        if scene == Scene::NoBranches {
            return json_response(200, "[]");
        }
        let names: Vec<&str> = BRANCHES
            .iter()
            .copied()
            .chain((scene == Scene::OverviewLong).then_some(
                "feature/a-very-long-branch-name-for-narrow-window-and-tooltip-acceptance",
            ))
            .collect();
        let value = serde_json::Value::Array(
            names
                .iter()
                .map(|name| {
                    let sha = commit_of(name);
                    serde_json::json!({
                        "name": name,
                        "commit": { "sha": sha },
                        "protected": *name == "main",
                    })
                })
                .collect(),
        );
        let _ = url;
        json_response(200, &value.to_string())
    }

    fn commit(&self, scene: Scene, reference: &str, url: &str) -> HttpResponse {
        if scene == Scene::EmptyRepository {
            return json_response(
                409,
                &serde_json::json!({"message": "Git Repository is empty."}).to_string(),
            );
        }
        // The revision endpoint encodes a ref that contains a slash, so the
        // fixture accepts both the raw and the percent-encoded spelling of the
        // two branches it publishes.
        let decoded = reference.replace("%2F", "/").replace("%2f", "/");
        let name = match decoded.as_str() {
            "main" => "main",
            "feature/docs-polish" => "feature/docs-polish",
            "feature/a-very-long-branch-name-for-narrow-window-and-tooltip-acceptance" => {
                "feature/docs-polish"
            }
            _ => return self.mismatch(url),
        };
        let sha = commit_of(name);
        let tree = tree_of(name);
        json_response(
            200,
            &serde_json::json!({
                "sha": sha,
                "commit": {
                    "message": format!("fixture commit on {name}"),
                    "tree": { "sha": tree },
                    "author": { "name": OWNER, "email": null, "date": "2026-10-20T10:00:00Z" },
                    "committer": { "name": OWNER, "email": null, "date": "2026-10-20T10:00:00Z" },
                }
            })
            .to_string(),
        )
    }

    fn tree(&self, scene: Scene, sha: &str, url: &str) -> HttpResponse {
        if scene == Scene::RootFailure && sha == MAIN_TREE && self.take_armed(ArmedFailure::Root) {
            return json_response(500, "{}");
        }
        if scene == Scene::DirectoryFailure
            && sha == DOCS_TREE
            && self.take_armed(ArmedFailure::Directory)
        {
            // A scripted, one-shot failure so a single bounded retry recovers.
            return json_response(
                500,
                &serde_json::json!({"message": "scripted directory failure"}).to_string(),
            );
        }
        // A proven missing README needs a complete, readable, empty root tree;
        // that is exactly what this scene serves.
        let entries = match sha {
            MAIN_TREE => root_entries(scene, false),
            FEATURE_TREE => root_entries(scene, true),
            FEATURE_DOCS_TREE => vec![
                entry("guides", DOCS_GUIDES_TREE, EntryKind::Directory, None),
                entry(
                    "guide.md",
                    FEATURE_GUIDE_BLOB,
                    EntryKind::Blob,
                    Some(FEATURE_GUIDE_TEXT.len() as u64),
                ),
            ],
            DOCS_TREE if scene == Scene::ReadmeInDocs => vec![
                entry(
                    "README.md",
                    README_BLOB,
                    EntryKind::Blob,
                    Some(README_TEXT.len() as u64),
                ),
                entry(
                    "guide.md",
                    GUIDE_BLOB,
                    EntryKind::Blob,
                    Some(GUIDE_TEXT.len() as u64),
                ),
            ],
            DOCS_TREE => vec![
                entry("guides", DOCS_GUIDES_TREE, EntryKind::Directory, None),
                entry(
                    "guide.md",
                    guide_blob_of(self.scene()),
                    EntryKind::Blob,
                    Some(guide_text(self.scene()).len() as u64),
                ),
            ],
            DOCS_GUIDES_TREE => vec![entry(
                "nested",
                "a000000000000000000000000000000000000012",
                EntryKind::Directory,
                None,
            )],
            "a000000000000000000000000000000000000012" => vec![entry(
                "deep-note.md",
                NESTED_BLOB,
                EntryKind::Blob,
                Some(NESTED_TEXT.len() as u64),
            )],
            "a000000000000000000000000000000000000030" => vec![
                entry(
                    "logo.bin",
                    BINARY_BLOB,
                    EntryKind::Blob,
                    Some(BINARY_BYTES.len() as u64),
                ),
                entry("empty.txt", EMPTY_BLOB, EntryKind::Blob, Some(0)),
                entry(
                    "empty-folder",
                    "a000000000000000000000000000000000000050",
                    EntryKind::Directory,
                    None,
                ),
                entry(
                    "很长的文件名-中文路径和目录缩进-完整路径可以通过提示查看.txt",
                    NESTED_BLOB,
                    EntryKind::Blob,
                    Some(NESTED_TEXT.len() as u64),
                ),
                entry(
                    "huge.log",
                    HUGE_BLOB,
                    EntryKind::Blob,
                    Some(3 * 1024 * 1024),
                ),
            ],
            SRC_TREE => vec![entry(
                "main.rs",
                MAIN_RS_BLOB,
                EntryKind::Blob,
                Some(MAIN_RS_TEXT.len() as u64),
            )],
            "a000000000000000000000000000000000000050" => Vec::new(),
            FEATURE_SRC_TREE => vec![
                entry(
                    "main.rs",
                    MAIN_RS_BLOB,
                    EntryKind::Blob,
                    Some(MAIN_RS_TEXT.len() as u64),
                ),
                entry(
                    "feature_note.rs",
                    FEATURE_ONLY_BLOB,
                    EntryKind::Blob,
                    Some(FEATURE_NOTE_TEXT.len() as u64),
                ),
            ],
            _ => return self.mismatch(url),
        };
        // `truncated` is only ever `true` in the scene that demonstrates it; every
        // other scene is a complete listing.
        let truncated = scene == Scene::TruncatedTree;
        let body = serde_json::json!({
            "sha": sha,
            "truncated": truncated,
            // `truncated: true` must never be read as a complete directory.
            "tree": entries,
        });
        json_response(200, &body.to_string())
    }

    fn blob(&self, scene: Scene, sha: &str, url: &str) -> HttpResponse {
        if scene == Scene::FileFailure && sha == MAIN_RS_BLOB && self.take_armed(ArmedFailure::File)
        {
            return json_response(
                500,
                &serde_json::json!({"message": "scripted file failure"}).to_string(),
            );
        }
        let bytes: Vec<u8> = match sha {
            README_BLOB => readme_text_for(true).as_bytes().to_vec(),
            FEATURE_README_BLOB => readme_text_for(false).as_bytes().to_vec(),
            FEATURE_ONLY_BLOB => FEATURE_NOTE_TEXT.as_bytes().to_vec(),
            GUIDE_BLOB => GUIDE_TEXT.as_bytes().to_vec(),
            FEATURE_GUIDE_BLOB => FEATURE_GUIDE_TEXT.as_bytes().to_vec(),
            NESTED_BLOB => NESTED_TEXT.as_bytes().to_vec(),
            MAIN_RS_BLOB => MAIN_RS_TEXT.as_bytes().to_vec(),
            BINARY_BLOB => BINARY_BYTES.to_vec(),
            EMPTY_BLOB => Vec::new(),
            HUGE_BLOB => huge_body(),
            _ => return self.mismatch(url),
        };
        // The fixture reports the same size it sends and the SHA the caller asked
        // for, so the real blob validation path is exercised, not bypassed.
        json_response(
            200,
            &serde_json::json!({
                "sha": sha,
                "encoding": "base64",
                "size": bytes.len(),
                "content": base64_encode(&bytes),
            })
            .to_string(),
        )
    }

    fn readme(&self, scene: Scene, query: Option<&str>, url: &str) -> HttpResponse {
        let reference = query
            .and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("ref=")))
            .unwrap_or_default();
        let commit = match reference {
            MAIN_COMMIT => "main",
            FEATURE_COMMIT => "feature/docs-polish",
            _ => return self.mismatch(url),
        };
        let main_branch = commit == "main";
        match scene {
            Scene::ReadmeUncertain | Scene::NoReadme => json_response(
                404,
                &serde_json::json!({"message": "Not Found"}).to_string(),
            ),
            _ => json_response(
                200,
                &serde_json::json!({
                    "type": "file",
                    "path": if scene == Scene::ReadmeInDocs { "docs/README.md" } else { "README.md" },
                    "sha": readme_blob_of(!main_branch),
                    "size": readme_text_for(main_branch).len(),
                    "encoding": "base64",
                })
                .to_string(),
            ),
        }
    }

    fn extras(&self, resource: &str, query: Option<&str>, scene: Scene, url: &str) -> HttpResponse {
        let query: BTreeMap<_, _> = query
            .unwrap_or_default()
            .split('&')
            .filter_map(|part| part.split_once('='))
            .collect();
        let list = matches!(resource, "/releases" | "/contributors");
        if (list && query.get("per_page") != Some(&"100"))
            || (resource == "/contributors" && query.get("anon") != Some(&"true"))
            || (!list && !query.is_empty())
        {
            return self.mismatch(url);
        }
        let release = serde_json::json!({
            "id": 501, "tag_name": if scene == Scene::OverviewLong { "v1.0.0-ledger-preview-with-a-long-version-label" } else { "v1.0.0" },
            "name":"Ledger 1.0", "body":"Ledger release", "draft":false, "prerelease":false,
            "html_url":format!("https://github.com/{OWNER}/{NAME}/releases/tag/v1.0.0"),
            "created_at":"2026-10-20T10:00:00Z", "published_at":"2026-10-20T10:00:00Z"
        });
        let body = match resource {
            "/releases/latest" if matches!(scene, Scene::LatestUnavailable | Scene::OverviewZero) => return json_response(404,"{}"),
            "/releases/latest" => release,
            "/releases" => if scene == Scene::OverviewZero {serde_json::json!([])} else {serde_json::json!([release])},
            "/contributors" => serde_json::Value::Array((0..if scene == Scene::OverviewZero {0} else if scene == Scene::OverviewLong {35} else {4})
                .map(|index|serde_json::json!({"id":100+index,"login":format!("ledger-{index}"),"contributions":10+index,
                    "html_url":format!("https://github.com/ledger-{index}")})).collect()),
            "/languages" if scene == Scene::OverviewError && self.take_armed(ArmedFailure::Languages) => return json_response(500,"{}"),
            "/languages" if scene == Scene::OverviewZero => serde_json::json!({}),
            "/languages" if scene == Scene::OverviewLong => serde_json::json!({"Rust":75000,"TypeScript":15000,"A language with a very long display name":9000,"C":1000}),
            "/languages" => serde_json::json!({"Rust":92000,"Shell":6000,"Python":2000}),
            _ => return self.mismatch(url),
        };
        let mut response = json_response(200, &body.to_string());
        if scene == Scene::OverviewPartial && list {
            response.headers.push((
                "x-github-sso".into(),
                "partial-results; organizations=1234".into(),
            ));
        }
        response
    }

    /// Consumes one armed failure. Returns true when the failure should fire.
    fn take_armed(&self, failure: ArmedFailure) -> bool {
        let mut armed = self.armed.lock().unwrap();
        if *armed == Some(failure) {
            *armed = None;
            true
        } else {
            false
        }
    }
}

fn armed_for(scene: Scene) -> Option<ArmedFailure> {
    match scene {
        Scene::DirectoryFailure => Some(ArmedFailure::Directory),
        Scene::RootFailure => Some(ArmedFailure::Root),
        Scene::OverviewError => Some(ArmedFailure::Languages),
        Scene::FileFailure => Some(ArmedFailure::File),
        _ => None,
    }
}

impl Transport for FixtureTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            if self.scene() == Scene::OverviewLoading
                && [
                    "/releases",
                    "/releases/latest",
                    "/contributors",
                    "/languages",
                ]
                .iter()
                .any(|end| split_url(&request.url).is_some_and(|(path, _)| path.ends_with(end)))
            {
                tokio::select! {
                    biased;
                    _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                }
            }
            if self.scene() == Scene::Delayed {
                tokio::select! {
                    biased;
                    _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
                }
            }
            self.answer(&request)
        })
    }

    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
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
}

// ---------------------------------------------------------------- repository --

fn commit_of(branch: &str) -> &'static str {
    match branch {
        "feature/docs-polish" => FEATURE_COMMIT,
        _ => MAIN_COMMIT,
    }
}

fn tree_of(branch: &str) -> &'static str {
    match branch {
        "feature/docs-polish" => FEATURE_TREE,
        _ => MAIN_TREE,
    }
}

fn guide_blob_of(scene: Scene) -> &'static str {
    let _ = scene;
    GUIDE_BLOB
}

/// The README object id, which differs per branch because the file changed.
fn readme_blob_of(feature: bool) -> &'static str {
    match feature {
        true => FEATURE_README_BLOB,
        false => README_BLOB,
    }
}

/// The README body for the commit being browsed; the two branches differ.
fn readme_text_for(main_commit: bool) -> &'static str {
    match main_commit {
        true => README_TEXT,
        false => FEATURE_README_TEXT,
    }
}

fn guide_text(scene: Scene) -> &'static str {
    let _ = scene;
    GUIDE_TEXT
}

fn feature_note(scene: Scene) -> &'static str {
    let _ = scene;
    FEATURE_NOTE_TEXT
}

#[derive(Clone, Copy)]
enum EntryKind {
    Blob,
    Directory,
    Symlink,
    Submodule,
    Unknown,
}

fn entry(path: &str, sha: &str, kind: EntryKind, size: Option<u64>) -> serde_json::Value {
    let (git_type, mode) = match kind {
        EntryKind::Blob => ("blob", "100644"),
        EntryKind::Directory => ("tree", "040000"),
        EntryKind::Symlink => ("blob", "120000"),
        EntryKind::Submodule => ("commit", "160000"),
        EntryKind::Unknown => ("blob", "100664"),
    };
    let mut value = serde_json::json!({
        "path": path,
        "mode": mode,
        "type": git_type,
        "sha": sha,
    });
    if let Some(size) = size {
        value["size"] = serde_json::json!(size);
    }
    value
}

fn root_entries(scene: Scene, feature: bool) -> Vec<serde_json::Value> {
    if scene == Scene::LongTree {
        return long_tree_entries();
    }
    if scene == Scene::NoReadme {
        // Absence is only proven by an empty tree, so this scene really is empty.
        return Vec::new();
    }
    // A tree entry's `path` is one segment; nesting is expressed by trees, not
    // slashes. The branches differ in their `src` tree, not in the root.
    let src = if feature { FEATURE_SRC_TREE } else { SRC_TREE };
    let mut entries = vec![
        entry(
            "README.md",
            readme_blob_of(feature),
            EntryKind::Blob,
            Some(readme_text_for(!feature).len() as u64),
        ),
        entry(
            "docs",
            if feature {
                FEATURE_DOCS_TREE
            } else {
                DOCS_TREE
            },
            EntryKind::Directory,
            None,
        ),
        entry("src", src, EntryKind::Directory, None),
        entry(
            "assets",
            "a000000000000000000000000000000000000030",
            EntryKind::Directory,
            None,
        ),
        // Known-but-not-text entries are honest rows in a complete listing.
        entry("link-to-readme", README_BLOB, EntryKind::Symlink, Some(9)),
        entry(
            "vendor",
            "a000000000000000000000000000000000000040",
            EntryKind::Submodule,
            Some(0),
        ),
    ];
    if scene == Scene::TruncatedTree {
        // An unknown Git type makes the whole listing partial, so it is only
        // present where that is the point of the scene.
        entries.push(entry(
            "odd-entry",
            "b000000000000000000000000000000000000009",
            EntryKind::Unknown,
            Some(12),
        ));
    }
    if scene == Scene::ReadmeInDocs {
        entries.retain(|entry| entry["path"] != "README.md");
    }
    entries
}

fn long_tree_entries() -> Vec<serde_json::Value> {
    (0..LONG_TREE_FILES)
        .map(|index| {
            entry(
                &format!("file-{index:04}.txt"),
                MAIN_RS_BLOB,
                EntryKind::Blob,
                Some(MAIN_RS_TEXT.len() as u64),
            )
        })
        .collect()
}

fn details_body(scene: Scene, url: &str) -> serde_json::Value {
    let _ = (scene, url);
    serde_json::json!({
        "id": 42,
        "name": NAME,
        "owner": { "id": 7, "login": OWNER },
        "description": "A tiny double-entry ledger, kept small enough to browse.",
        "visibility": "private",
        "private": true,
        "default_branch": "main",
        "fork": false,
        "archived": false,
        "stargazers_count": 128,
        "forks_count": 9,
        "open_issues_count": 4,
        "subscribers_count": 12,
        "size": 64,
        "html_url": "https://example.invalid/octo/ledger",
        "clone_url": "https://example.invalid/octo/ledger.git",
        "pushed_at": "2026-10-20T10:00:00Z"
    })
}

fn huge_body() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(3 * 1024 * 1024);
    while bytes.len() < 3 * 1024 * 1024 {
        bytes.extend_from_slice(
            b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        );
    }
    bytes
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn split_url(url: &str) -> Option<(&str, Option<&str>)> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let path_start = after_scheme.find('/')?;
    let path_and_query = &after_scheme[path_start..];
    let (path, query) = match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    };
    Some((path, query))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &str) -> HttpRequest {
        HttpRequest::get(format!("https://api.github.com{path}"))
    }

    #[test]
    fn the_same_resource_can_be_read_repeatedly_and_in_any_order() {
        let fixture = FixtureTransport::new(Scene::Normal);
        let document = request(&format!("{}/git/blobs/{README_BLOB}", locator_path()));
        let root = request(&format!("{}/git/trees/{MAIN_TREE}", locator_path()));
        for _ in 0..3 {
            assert_eq!(
                fixture.answer(&document).unwrap().status,
                200,
                "a blob stays readable"
            );
            assert_eq!(fixture.answer(&root).unwrap().status, 200);
        }
        assert!(fixture.mismatches().is_empty());
    }

    #[test]
    fn two_branches_resolve_to_different_commits_trees_and_bodies() {
        let fixture = FixtureTransport::new(Scene::Normal);
        let main = fixture
            .answer(&request(&format!("{}/commits/main", locator_path())))
            .unwrap();
        // The real client percent-encodes the slash inside a branch name.
        let feature = fixture
            .answer(&request(&format!(
                "{}/commits/feature%2Fdocs-polish",
                locator_path()
            )))
            .unwrap();
        assert_eq!(main.status, 200);
        assert_eq!(feature.status, 200, "the encoded branch name is accepted");
        let main_body: serde_json::Value = serde_json::from_str(&main.body).unwrap();
        let feature_body: serde_json::Value = serde_json::from_str(&feature.body).unwrap();
        assert_eq!(main_body["sha"], MAIN_COMMIT);
        assert_eq!(feature_body["sha"], FEATURE_COMMIT);
        assert_ne!(
            main_body["commit"]["tree"]["sha"], feature_body["commit"]["tree"]["sha"],
            "the two branches point at different root trees"
        );
    }

    #[test]
    fn the_readme_scenes_are_distinguishable_and_repeatable() {
        let fixture = FixtureTransport::new(Scene::NoReadme);
        let url = format!("{}/readme?ref={MAIN_COMMIT}", locator_path());
        assert_eq!(fixture.answer(&request(&url)).unwrap().status, 404);
        assert_eq!(fixture.answer(&request(&url)).unwrap().status, 404);
        fixture.set_scene(Scene::ReadmeUncertain);
        assert_eq!(fixture.answer(&request(&url)).unwrap().status, 404);
        fixture.set_scene(Scene::Normal);
        assert_eq!(fixture.answer(&request(&url)).unwrap().status, 200);
    }

    #[test]
    fn scripted_failures_fire_once_so_a_single_retry_recovers() {
        let fixture = FixtureTransport::new(Scene::DirectoryFailure);
        let tree = request(&format!("{}/git/trees/{DOCS_TREE}", locator_path()));
        assert_eq!(fixture.answer(&tree).unwrap().status, 500, "armed failure");
        let second = fixture.answer(&tree).unwrap();
        assert_eq!(second.status, 200, "the retry succeeds");
        let third = fixture.answer(&tree).unwrap();
        assert_eq!(third.status, 200, "and stays successful");
    }

    #[test]
    fn an_unmodelled_request_is_a_development_error_not_a_permission_result() {
        let fixture = FixtureTransport::new(Scene::Normal);
        let response = fixture
            .answer(&request(&format!("{}/tags", locator_path())))
            .unwrap();
        assert_eq!(response.status, 599);
        assert!(response.body.contains("does not model this request"));
        assert_eq!(fixture.mismatches().len(), 1);
    }

    #[test]
    fn the_long_tree_exposes_enough_rows_to_scroll() {
        let fixture = FixtureTransport::new(Scene::LongTree);
        let response = fixture
            .answer(&request(&format!(
                "{}/git/trees/{MAIN_TREE}",
                locator_path()
            )))
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(body["tree"].as_array().unwrap().len(), LONG_TREE_FILES);
    }
}
