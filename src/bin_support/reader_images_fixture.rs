//! Deterministic picture fixture for the `reader-images` preview and its tests.
//!
//! Development-only material: compiled through
//! `src/bin_support/reader_images_modules.rs` into `examples/reader_images.rs`
//! and `tests/reader_images.rs`, never into the product binary.
//!
//! The fixture answers by request identity, not from a one-shot queue, so the
//! same commit, tree or blob can be read again and again in any order. It has
//! two injected boundaries:
//!
//! * [`FixtureTransport`] answers the *authenticated* GitHub reads the real
//!   `AccountService`/`RepoService` pipeline performs (login, commit, trees,
//!   blobs). Every picture it serves is a real, decodable image generated here.
//! * [`MemoryExternalImages`] answers `https://images.invalid/...` addresses the
//!   reader treats as public external pictures. Nothing leaves the process.
//!
//! Anything the fixture does not model is reported as a configuration error, so
//! a preview that asks for an unknown resource says so instead of pretending a
//! GitHub permission result.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use crate::network::cancel::CancellationToken;
use crate::network::reader_images::{ExternalImageBytes, ExternalImageError, ExternalImageLimits};
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};

use std::sync::Arc;

use crate::auth::InMemoryCredentialStore;
use crate::auth::token::Pat;
use crate::content::PreparedContent;
use crate::data::AccountService;
use crate::data::RepositorySession;
use crate::data::reader_images::RepositoryImageService;
use crate::domain::blob::{BlobIdentity, BlobLimits};
use crate::domain::repository::{
    ObjectSha, RepositoryIdentity, RepositoryLocator, RevisionSpec, SnapshotDirectory,
    TreeEntryKind,
};
use crate::github::GitHubClient;
use crate::github::RepositoryLimits;
use crate::network::executor::BackgroundExecutor;
use crate::reader::{DocumentKind, ImageLimits, ReaderView, ResourceContext};
use crate::storage::accounts::AccountDatabaseConfig;

pub const OWNER: &str = "nocpiun";
pub const NAME: &str = "githux-images";
pub const COMMIT: &str = "5127a12a830f6f9b8a1e02a1cba4e0b1f47a5d33";
pub const ALT_COMMIT: &str = "9f0c7a2a830f6f9b8a1e02a1cba4e0b1f47a5d44";
pub const README_PATH: &str = "docs/README.md";
pub const ALT_README_PATH: &str = "docs/README-alt.md";

const ROOT_TREE: &str = "aaaa000000000000000000000000000000000001";
const DOCS_TREE: &str = "aaaa000000000000000000000000000000000002";
const IMAGES_TREE: &str = "aaaa000000000000000000000000000000000003";

const README_BLOB: &str = "bbbb000000000000000000000000000000000001";
const ALT_README_BLOB: &str = "bbbb000000000000000000000000000000000002";
const BANNER_BLOB: &str = "bbbb000000000000000000000000000000000010";
const DIAGRAM_BLOB: &str = "bbbb000000000000000000000000000000000011";
const HUGE_BLOB: &str = "bbbb000000000000000000000000000000000012";
const TEXT_BLOB: &str = "bbbb000000000000000000000000000000000013";
const ALT_ONLY_BLOB: &str = "bbbb000000000000000000000000000000000014";

/// How long `slow.png` takes to serve. Long enough that a document switch or a
/// hidden window lands while the load is genuinely in flight.
pub const SLOW_PICTURE_DELAY: Duration = Duration::from_millis(600);

/// A real text file, so the Failure scene proves "not a picture" rather than
/// "not found".
const TEXT_BYTES: &[u8] = b"this is a text file, not a picture\n";

pub const BANNER_URL: &str = "https://images.invalid/public/banner.png";
pub const LOGO_URL: &str = "https://images.invalid/public/logo.png";
pub const MISSING_URL: &str = "https://images.invalid/public/missing.png";
pub const SLOW_URL: &str = "https://images.invalid/public/slow.png";
/// Larger than the reader's encoded budget, so it must be refused.
pub const OVERSIZE_URL: &str = "https://images.invalid/public/oversize.png";

/// What the preview toolbar switches between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    /// Every source resolves: relative, root-relative, external and a picture
    /// whose bytes arrive after a delay.
    Healthy,
    /// A very wide banner and a very tall picture in the same document.
    Extremes,
    /// One source fails and one is refused by the reader's own boundary.
    Failure,
    /// A picture past the encoded and pixel budgets.
    Oversize,
    /// Long text with pictures far down, for scroll and measurement checks.
    LongDocument,
    /// The second document replaces the first while the first document's slow
    /// pictures are still in flight.
    Switch,
    /// The whole document expands to its natural height inside the host's own
    /// page scroll (the profile-README case), with a picture that widens it.
    Flow,
}

impl Scene {
    pub const ALL: [Scene; 7] = [
        Scene::Healthy,
        Scene::Extremes,
        Scene::Failure,
        Scene::Oversize,
        Scene::LongDocument,
        Scene::Switch,
        Scene::Flow,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Scene::Healthy => "Healthy",
            Scene::Extremes => "Extremes",
            Scene::Failure => "Failure",
            Scene::Oversize => "Oversize",
            Scene::LongDocument => "Long document",
            Scene::Switch => "Switch document",
            Scene::Flow => "Natural height (page scroll)",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Scene::Healthy => {
                "A relative picture, a root path, a public HTTPS picture and a delayed one."
            }
            Scene::Extremes => "A 1600x200 banner and a tall figure stay inside the column.",
            Scene::Failure => {
                "One picture 404s, one is a text file and one is a file: address the reader refuses."
            }
            Scene::Oversize => {
                "One picture is over the encoded budget and one is over the pixel budget."
            }
            Scene::LongDocument => "Pictures far down the document load as they scroll in.",
            Scene::Switch => {
                "Switching documents while a slow picture is in flight: the old result never lands."
            }
            Scene::Flow => {
                "The README has no inner scroll: it expands fully and the page scrolls it."
            }
        }
    }

    /// Whether this scene lays the reader out at its natural height.
    pub fn is_flow(self) -> bool {
        matches!(self, Scene::Flow)
    }

    /// The picture budgets a scene is opened with. The Oversize scene uses a
    /// small encoded budget so a real picture is genuinely over it.
    pub fn limits(self) -> ImageLimits {
        match self {
            Scene::Oversize => ImageLimits {
                max_response_bytes: 1_024,
                ..ImageLimits::default()
            },
            _ => ImageLimits::default(),
        }
    }
}

// ---- real, decodable synthetic pictures ----

/// A solid picture with a diagonal stripe, so it is visibly a picture rather
/// than a blank rectangle.
pub fn picture(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        let stripe = (x + y) % 64 < 12;
        let base = if stripe { 210u8 } else { 40u8 };
        image::Rgba([base, (x % 256) as u8, (y % 256) as u8, 255])
    });
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encode the fixture picture");
    cursor.into_inner()
}

/// A PNG whose declared canvas is enormous but whose payload is tiny. The
/// reader must refuse it from the header, before any pixel buffer exists.
pub fn oversized_canvas() -> Vec<u8> {
    let mut bytes = picture(2, 2);
    // Rewrite the IHDR width/height; PNG has a CRC over IHDR, so recompute it.
    bytes[16..20].copy_from_slice(&60_000u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&60_000u32.to_be_bytes());
    let crc = crc32(&bytes[12..29]);
    bytes[29..33].copy_from_slice(&crc.to_be_bytes());
    bytes
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---- the authenticated fixture transport ----

/// The repository's own content plus the scripted scene.
pub struct FixtureTransport {
    scene: Mutex<Scene>,
    /// Requests this fixture did not model.
    mismatches: Mutex<Vec<String>>,
    /// Recorded request URLs, in order.
    requests: Mutex<Vec<String>>,
    /// Synthetic identity returned by `GET /user`.
    identity: Mutex<(u64, String)>,
    pictures: Pictures,
}

struct Pictures {
    banner: Vec<u8>,
    diagram: Vec<u8>,
    huge: Vec<u8>,
    tall: Vec<u8>,
    slow: Vec<u8>,
    logo: Vec<u8>,
    oversize_external: Vec<u8>,
}

impl Pictures {
    fn new() -> Self {
        Self {
            banner: picture(1600, 200),
            diagram: picture(320, 180),
            // A real picture; the Oversize scene reads it under a small budget.
            huge: picture(200, 150),
            tall: picture(120, 900),
            slow: picture(240, 140),
            logo: picture(96, 96),
            oversize_external: oversized_canvas(),
        }
    }
}

impl FixtureTransport {
    pub fn new(scene: Scene) -> Self {
        Self {
            scene: Mutex::new(scene),
            mismatches: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            identity: Mutex::new((7, OWNER.to_string())),
            pictures: Pictures::new(),
        }
    }

    pub fn set_scene(&self, scene: Scene) {
        *self.scene.lock().unwrap() = scene;
        self.requests.lock().unwrap().clear();
        self.mismatches.lock().unwrap().clear();
    }

    pub fn scene(&self) -> Scene {
        *self.scene.lock().unwrap()
    }

    pub fn recorded(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    pub fn mismatches(&self) -> Vec<String> {
        self.mismatches.lock().unwrap().clone()
    }

    fn mismatch(&self, url: &str) -> HttpResponse {
        let message = format!("reader-images fixture does not model this request: {url}");
        self.mismatches.lock().unwrap().push(message.clone());
        eprintln!("[reader-images fixture] {message}");
        json_response(500, &serde_json::json!({"message": message}).to_string())
    }

    fn readme(&self, path: &str) -> Option<Vec<u8>> {
        let scene = self.scene();
        let text = match (path, scene) {
            (README_PATH, Scene::Healthy) => healthy_readme(),
            (README_PATH, Scene::Switch) => healthy_readme(),
            (README_PATH, Scene::Extremes) => extremes_readme(),
            (README_PATH, Scene::Failure) => failure_readme(),
            (README_PATH, Scene::Oversize) => oversize_readme(),
            (README_PATH, Scene::Flow) => flow_readme(),
            (README_PATH, _) => long_readme(),
            (ALT_README_PATH, _) => alt_readme(),
            _ => return None,
        };
        Some(text.into_bytes())
    }

    /// The bytes `diagram.png` serves in the current scene. The tree descriptor
    /// and the blob body must agree on the size, so both read this.
    fn diagram(&self) -> Vec<u8> {
        if self.scene() == Scene::Extremes {
            self.pictures.tall.clone()
        } else {
            self.pictures.diagram.clone()
        }
    }

    fn blob(&self, sha: &str, url: &str) -> HttpResponse {
        let body: Vec<u8> = match sha {
            README_BLOB => match self.readme(README_PATH) {
                Some(bytes) => bytes,
                None => return self.mismatch(url),
            },
            ALT_README_BLOB => match self.readme(ALT_README_PATH) {
                Some(bytes) => bytes,
                None => return self.mismatch(url),
            },
            BANNER_BLOB => self.pictures.banner.clone(),
            DIAGRAM_BLOB => self.diagram(),
            HUGE_BLOB => self.pictures.huge.clone(),
            TEXT_BLOB => b"this is a text file, not a picture\n".to_vec(),
            ALT_ONLY_BLOB => self.pictures.slow.clone(),
            _ => return self.mismatch(url),
        };
        json_response(
            200,
            &serde_json::json!({
                "sha": sha,
                "encoding": "base64",
                "size": body.len(),
                "content": base64(&body),
            })
            .to_string(),
        )
    }

    fn tree(&self, sha: &str, url: &str) -> HttpResponse {
        let entries = match sha {
            ROOT_TREE => serde_json::json!([
                {"path": "docs", "type": "tree", "mode": "040000", "sha": DOCS_TREE},
            ]),
            DOCS_TREE => {
                // Sizes come from the same text `blob` serves, so the fixed-tree
                // descriptor and the blob agree exactly as they must.
                let readme = self.readme(README_PATH).map_or(0, |bytes| bytes.len());
                let alt = self.readme(ALT_README_PATH).map_or(0, |bytes| bytes.len());
                serde_json::json!([
                    {"path": "README.md", "type": "blob", "mode": "100644", "sha": README_BLOB, "size": readme},
                    {"path": "README-alt.md", "type": "blob", "mode": "100644", "sha": ALT_README_BLOB, "size": alt},
                    {"path": "images", "type": "tree", "mode": "040000", "sha": IMAGES_TREE},
                ])
            }
            IMAGES_TREE => serde_json::json!([
                {"path": "banner.png", "type": "blob", "mode": "100644", "sha": BANNER_BLOB, "size": self.pictures.banner.len()},
                {"path": "diagram.png", "type": "blob", "mode": "100644", "sha": DIAGRAM_BLOB, "size": self.diagram().len()},
                {"path": "huge.png", "type": "blob", "mode": "100644", "sha": HUGE_BLOB, "size": self.pictures.huge.len()},
                {"path": "notes.txt", "type": "blob", "mode": "100644", "sha": TEXT_BLOB, "size": TEXT_BYTES.len()},
                {"path": "slow.png", "type": "blob", "mode": "100644", "sha": ALT_ONLY_BLOB, "size": self.pictures.slow.len()},
            ]),
            _ => return self.mismatch(url),
        };
        json_response(
            200,
            &serde_json::json!({"sha": sha, "truncated": false, "tree": entries}).to_string(),
        )
    }

    fn commit(&self, reference: &str, url: &str) -> HttpResponse {
        let (sha, tree) = match reference {
            COMMIT => (COMMIT, ROOT_TREE),
            ALT_COMMIT => (ALT_COMMIT, ROOT_TREE),
            _ => return self.mismatch(url),
        };
        json_response(
            200,
            &serde_json::json!({
                "sha": sha,
                "commit": {
                    "message": "fixture commit",
                    "tree": {"sha": tree},
                    "author": {"name": OWNER, "email": null, "date": "2026-10-29T10:00:00Z"},
                    "committer": {"name": OWNER, "email": null, "date": "2026-10-29T10:00:00Z"},
                }
            })
            .to_string(),
        )
    }
}

fn healthy_readme() -> String {
    format!(
        "# README pictures\n\n\
         A relative picture:\n\n\
         ![diagram](images/diagram.png)\n\n\
         A root path in the same commit:\n\n\
         ![banner](/{README_PATH_DIR}/images/banner.png)\n\n\
         A public HTTPS picture:\n\n\
         ![logo]({LOGO_URL})\n\n\
         A picture whose bytes arrive late:\n\n\
         ![slow](images/slow.png)\n\n\
         Trailing prose keeps the document from ending on a picture.\n",
        README_PATH_DIR = "docs",
    )
}

fn alt_readme() -> String {
    "# The second document\n\n\
     Only this document's picture belongs here:\n\n\
     ![diagram](images/diagram.png)\n\n\
     Trailing prose.\n"
        .to_string()
}

fn extremes_readme() -> String {
    "# Extremes\n\n\
     A 1600 pixel wide banner:\n\n\
     ![wide](images/banner.png)\n\n\
     A tall figure:\n\n\
     ![tall](images/diagram.png)\n"
        .to_string()
}

fn failure_readme() -> String {
    format!(
        "# Failures\n\n\
         This picture does not exist:\n\n\
         ![missing](images/missing.png)\n\n\
         This one is a text file:\n\n\
         ![notes](images/notes.txt)\n\n\
         This one is a local address the reader refuses:\n\n\
         ![local](file:///c:/secret.png)\n\n\
         This public address answers 404:\n\n\
         ![gone]({MISSING_URL})\n"
    )
}

fn oversize_readme() -> String {
    format!(
        "# Oversize\n\n\
         Over the encoded budget:\n\n\
         ![huge](images/huge.png)\n\n\
         A tiny payload declaring a giant canvas:\n\n\
         ![canvas]({OVERSIZE_URL})\n"
    )
}

/// A README that is long enough to scroll a page on its own, with pictures at
/// the top, middle and bottom, so a natural-height layout has to grow when each
/// one lands.
fn flow_readme() -> String {
    let mut text = String::from("# A profile README at natural height\n\n");
    text.push_str("This document has no viewport of its own: the page scrolls it.\n\n");
    text.push_str("![banner](images/banner.png)\n\n");
    for section in 0..10 {
        text.push_str(&format!(
            "## Section {section}\n\nA paragraph long enough to give the page real height, with 中文 and emoji 🚀.\n\n"
        ));
        if section == 4 {
            text.push_str("![diagram](images/diagram.png)\n\n");
        }
    }
    text.push_str(&format!("![public]({LOGO_URL})\n\n"));
    text.push_str("The end of the README.\n");
    text
}

fn long_readme() -> String {
    let mut text = String::from("# A long README\n\n");
    for section in 0..28 {
        text.push_str(&format!(
            "## Section {section}\n\nA paragraph of ordinary prose with 中文 and emoji 🚀 so the document has real height before the pictures.\n\n"
        ));
        if section % 7 == 0 {
            text.push_str("![diagram](images/diagram.png)\n\n");
        }
    }
    text.push_str(&format!("![banner at the end]({BANNER_URL})\n"));
    text
}

impl Transport for FixtureTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let url = request.url.clone();
            self.requests.lock().unwrap().push(url.clone());
            let Some((path, _query)) = split_url(&url) else {
                return Ok(self.mismatch(&url));
            };
            if path == "/user" {
                let (id, login) = self.identity.lock().unwrap().clone();
                return Ok(json_response(
                    200,
                    &serde_json::json!({"id": id, "login": login}).to_string(),
                ));
            }
            let base = format!("/repos/{OWNER}/{NAME}");
            let Some(rest) = path.strip_prefix(&base) else {
                return Ok(self.mismatch(&url));
            };
            if rest.is_empty() {
                // GET /repos/{owner}/{name}: the repository identity the page
                // and the picture provider share.
                return Ok(json_response(
                    200,
                    &serde_json::json!({
                        "id": 42,
                        "name": NAME,
                        "owner": {"id": 7, "login": OWNER},
                        "private": false,
                        "default_branch": "main",
                        "fork": false,
                        "archived": false,
                        "stargazers_count": 1,
                        "forks_count": 0,
                        "open_issues_count": 0,
                        "size": 4,
                        "html_url": format!("https://github.com/{OWNER}/{NAME}"),
                    })
                    .to_string(),
                ));
            }
            if let Some(sha) = rest.strip_prefix("/git/blobs/") {
                // `slow.png` is genuinely slow, so a document switch can happen
                // while one of its pictures is still in flight. The README text
                // itself is never delayed.
                if sha == ALT_ONLY_BLOB {
                    tokio::time::sleep(SLOW_PICTURE_DELAY).await;
                    if cancel.is_cancelled() {
                        return Err(TransportError::Cancelled);
                    }
                }
                return Ok(self.blob(sha, &url));
            }
            if let Some(sha) = rest.strip_prefix("/git/trees/") {
                return Ok(self.tree(sha, &url));
            }
            if let Some(reference) = rest.strip_prefix("/commits/") {
                return Ok(self.commit(reference, &url));
            }
            if rest == "/readme" {
                // Not used by the preview: it reads the README through the fixed
                // tree/blob path like any other file.
                return Ok(self.mismatch(&url));
            }
            Ok(self.mismatch(&url))
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

fn split_url(url: &str) -> Option<(String, String)> {
    let after_scheme = url.split_once("://")?.1;
    let rest = match after_scheme.find('/') {
        Some(index) => &after_scheme[index..],
        None => "/",
    };
    match rest.split_once('?') {
        Some((path, query)) => Some((path.to_string(), query.to_string())),
        None => Some((rest.to_string(), String::new())),
    }
}

// ---- the credential-free external picture source ----

/// Answers the fixture's public HTTPS addresses from memory.
pub struct MemoryExternalImages {
    bodies: HashMap<String, Vec<u8>>,
    reads: Mutex<Vec<String>>,
}

impl MemoryExternalImages {
    pub fn new() -> Self {
        let pictures = Pictures::new();
        let mut bodies = HashMap::new();
        bodies.insert(BANNER_URL.to_string(), pictures.banner);
        bodies.insert(LOGO_URL.to_string(), pictures.logo);
        bodies.insert(SLOW_URL.to_string(), pictures.slow);
        bodies.insert(OVERSIZE_URL.to_string(), pictures.oversize_external);
        Self {
            bodies,
            reads: Mutex::new(Vec::new()),
        }
    }

    pub fn reads(&self) -> Vec<String> {
        self.reads.lock().unwrap().clone()
    }
}

impl Default for MemoryExternalImages {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalImageBytes for MemoryExternalImages {
    fn load<'a>(
        &'a self,
        url: &'a str,
        _limits: &'a ExternalImageLimits,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ExternalImageError>> + Send + 'a>> {
        Box::pin(async move {
            self.reads.lock().unwrap().push(url.to_string());
            if url == SLOW_URL {
                tokio::time::sleep(SLOW_PICTURE_DELAY).await;
            }
            if cancel.is_cancelled() {
                return Err(ExternalImageError::Cancelled);
            }
            self.bodies
                .get(url)
                .cloned()
                .ok_or(ExternalImageError::Status(404))
        })
    }
}

/// The external addresses the fixture answers.
pub fn external_sources() -> &'static [&'static str] {
    &[BANNER_URL, LOGO_URL, SLOW_URL, OVERSIZE_URL]
}

// ---- the real pipeline assembly shared by the preview and the tests ----

/// Owns the fixture's account/executor stack and can open one document.
pub struct Pipeline {
    accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
    transport: Arc<FixtureTransport>,
    external: Arc<MemoryExternalImages>,
    _temp: tempfile::TempDir,
}

/// One opened document: its prepared README, its provider and the context.
pub struct Opened {
    pub content: PreparedContent,
    pub context: ResourceContext,
    pub provider: Arc<RepositoryImageService>,
    pub document: Document,
    /// The layout this scene asks for: bounded viewport, or natural height.
    pub layout: crate::reader::MarkdownLayout,
}

/// The fixed commit and path one scene shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Document {
    pub commit: &'static str,
    pub path: &'static str,
}

impl Document {
    /// The first document a scene opens.
    pub fn for_scene(_scene: Scene) -> Self {
        Self {
            commit: COMMIT,
            path: README_PATH,
        }
    }

    /// The second document the switch scene moves to.
    pub fn second() -> Self {
        Self {
            commit: ALT_COMMIT,
            path: ALT_README_PATH,
        }
    }

    pub fn kind(&self) -> DocumentKind {
        DocumentKind::for_path(self.path)
    }
}

impl Pipeline {
    /// Builds the real service stack over the injected boundaries: a temporary
    /// SQLite directory, an in-memory credential store, one synthetic PAT, the
    /// fixture transport and the in-memory external picture source.
    pub fn new(scene: Scene) -> Self {
        let temp = tempfile::tempdir().expect("temporary account directory");
        let transport = Arc::new(FixtureTransport::new(scene));
        let external = Arc::new(MemoryExternalImages::new());
        let executor = Arc::new(BackgroundExecutor::new(4));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime");
        let accounts = runtime.block_on(async {
            let accounts = AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(Default::default(), transport.clone())),
                executor.clone(),
            )
            .await
            .expect("open the fixture account service");
            accounts
                .login(Pat::from_str("synthetic-reader-images-token"))
                .await
                .expect("synthetic login");
            Arc::new(accounts)
        });
        Self {
            accounts,
            executor,
            transport,
            external,
            _temp: temp,
        }
    }

    pub fn transport(&self) -> &Arc<FixtureTransport> {
        &self.transport
    }

    pub fn external(&self) -> &Arc<MemoryExternalImages> {
        &self.external
    }

    pub fn executor(&self) -> &Arc<BackgroundExecutor> {
        &self.executor
    }

    pub fn accounts(&self) -> &Arc<AccountService> {
        &self.accounts
    }

    /// Opens one README through the real data path and builds its picture
    /// provider.
    ///
    /// This is the whole later-wiring contract in one function: resolve the
    /// fixed commit, walk the fixed tree to the README blob, prepare the
    /// content, and create one [`RepositoryImageService`] bound to that same
    /// verified session and repository.
    pub fn open(&self, scene: Scene, limits: ImageLimits) -> Opened {
        self.transport.set_scene(scene);
        self.open_document(Document::for_scene(scene), limits)
    }

    /// Whether the current scene asks for natural height.
    pub fn scene_is_flow(&self) -> bool {
        self.transport.scene().is_flow()
    }

    /// Opens one specific document, keeping the current scene.
    pub fn open_document(&self, document: Document, limits: ImageLimits) -> Opened {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("document runtime");
        let scene_is_flow = self.transport.scene().is_flow();
        runtime.block_on(async {
            let session = self
                .accounts
                .repo_service()
                .session()
                .expect("verified session");
            let limits_repo = RepositoryLimits::default();
            let repository = self
                .accounts
                .repo_service()
                .session()
                .expect("verified session")
                .repository(
                    RepositoryLocator::new(OWNER, NAME).expect("locator"),
                    &limits_repo,
                    Default::default(),
                )
                .await
                .map(|scoped| scoped.value.identity)
                .expect("repository identity");
            let identity = readme_identity(&session, &repository, document, &limits_repo).await;
            let content = session
                .content(
                    &identity,
                    &BlobLimits {
                        decoded_bytes: 256 * 1024,
                        response_bytes: 512 * 1024,
                    },
                    Default::default(),
                )
                .await
                .expect("fixture README")
                .value;
            let context = ResourceContext::from_prepared(&content);
            let provider = Arc::new(RepositoryImageService::new(
                session,
                repository,
                self.executor.clone(),
                self.external.clone(),
                limits,
            ));
            Opened {
                content,
                context,
                provider,
                document,
                layout: if scene_is_flow {
                    crate::reader::MarkdownLayout::Flow
                } else {
                    crate::reader::MarkdownLayout::Repository
                },
            }
        })
    }
}

async fn readme_identity(
    session: &RepositorySession,
    repository: &RepositoryIdentity,
    document: Document,
    limits: &RepositoryLimits,
) -> BlobIdentity {
    let snapshot = session
        .revision(
            repository,
            RevisionSpec::Commit(ObjectSha::new(document.commit).expect("fixture commit")),
            limits,
            Default::default(),
        )
        .await
        .expect("fixture snapshot")
        .value;
    let mut directory = SnapshotDirectory::root(&snapshot);
    let segments: Vec<&str> = document.path.split('/').collect();
    for (index, segment) in segments.iter().enumerate() {
        let tree = session
            .directory(&directory, limits, Default::default())
            .await
            .expect("fixture tree")
            .value;
        let at = tree
            .entries
            .iter()
            .position(|entry| entry.path.as_str() == *segment)
            .expect("fixture path segment");
        if index + 1 == segments.len() {
            assert!(matches!(tree.entries[at].kind, TreeEntryKind::File { .. }));
            return BlobIdentity::from_file(&tree, at).expect("blob identity");
        }
        directory = tree.child_directory(at).expect("child directory");
    }
    panic!("the fixture document has no blob");
}

/// The reader body for [`open_reader`], for hosts that create the entity
/// themselves (`cx.new(|cx| open_reader_entity(opened, window, cx))`).
pub fn open_reader_entity(
    opened: Opened,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<ReaderView>,
) -> ReaderView {
    open_reader(opened, window, cx).0
}

/// Attaches the opened document and its provider to a new reader in `cx`.
pub fn open_reader(
    opened: Opened,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<ReaderView>,
) -> (ReaderView, Arc<RepositoryImageService>) {
    let provider = opened.provider.clone();
    let mut reader = ReaderView::new(
        opened.content.body,
        opened.document.kind(),
        opened.context,
        window,
        cx,
    );
    reader.set_markdown_layout(opened.layout, cx);
    reader.set_image_provider(
        Some(provider.clone() as Arc<dyn crate::reader::ImageProvider>),
        cx,
    );
    (reader, provider)
}

#[cfg(test)]
#[path = "reader_images_fixture_tests.rs"]
mod tests;
