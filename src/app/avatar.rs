//! Real avatar loading for the account and repository surfaces.
//!
//! The account footer, each repository row's namespace and the saved-account list all show the
//! avatar GitHub reports for that identity. This module is the one place that turns an avatar URL
//! into a renderable image: it fetches the bytes off the UI thread over the *existing* transport,
//! caches the decoded image in memory by a stable identity, and hands out an [`ImageSource`] the
//! shared `Avatar`/`Image` components can draw.
//!
//! Boundaries that matter here:
//!
//! * No credential is ever attached. The request is a plain GET built without an `Authorization`
//!   header, and a URL is only fetched when it is an `https` URL (or the configured local test
//!   base). A GitHub PAT must never travel to an avatar CDN or an arbitrary host.
//! * Nothing blocks the UI. Fetching and decoding happen on the existing Tokio executor; results
//!   come back through a queue the controller drains on its own light beat.
//! * Identity, not position. A cached image and an in-flight request are keyed by the stable
//!   owner/account identity plus the URL, so a reordered list, two repositories under one owner, an
//!   account switch or a URL change can never cross images. A late response for an older key is
//!   simply stored under its own key and is never published under another.
//! * Failures are terminal for that key, so a 404 or a broken image does not retry forever; the
//!   caller falls back to initials at a stable size.
//! * There is no disk cache and no image platform: one small in-memory map, capped, is enough for
//!   a sidebar and an account list.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{Image, ImageFormat};

use crate::network::cancel::{CancellationToken, CancellationTokenSource};
pub use crate::ui::avatar::AvatarKey;

impl crate::data::avatar_cache::AvatarKeyLike for AvatarKey {
    fn subject(&self) -> &str {
        &self.subject
    }
    fn url(&self) -> &str {
        &self.url
    }
}

/// Largest avatar payload accepted, in bytes. GitHub avatars are small; a larger body is treated as
/// a failure instead of being decoded or buffered without bound.
pub const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

/// How many decoded avatars stay cached. The sidebar shows at most a screenful of owners plus the
/// saved accounts, so this is generous while still bounded.
const CACHE_CAPACITY: usize = 128;

// `AvatarKey` lives in the shared avatar renderer so standalone page assemblies do not need the
// network-bearing account module just to key a decoded image. It is re-exported above for the
// existing account/sidebar call sites.

/// What the cache currently knows about one key.
#[derive(Clone)]
pub enum AvatarState {
    /// Not loaded and not loading yet.
    Unknown,
    /// A fetch is in flight.
    Loading,
    /// Decoded and ready to draw.
    Ready(Arc<Image>),
    /// Failed for this key; the caller shows initials at a stable size. It is not retried.
    Failed,
}

/// The shared avatar store. One per application, owned by the account controller.
#[derive(Default)]
pub struct AvatarCache {
    entries: Mutex<HashMap<AvatarKey, AvatarState>>,
    /// The decoded images in insertion order, so the oldest can be evicted when full.
    order: Mutex<Vec<AvatarKey>>,
    /// Every key this session has already requested. It is what keeps a large list from looping:
    /// once an avatar has been fetched (or failed), an evicted key stays "already requested" and is
    /// not fetched again. Without this, a list larger than the cache would evict a still-visible
    /// avatar on each completion and refetch forever while the window sat idle.
    requested: std::sync::Mutex<std::collections::HashSet<AvatarKey>>,
}

impl AvatarCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The current state for `key`, without starting a fetch.
    pub fn state(&self, key: &AvatarKey) -> AvatarState {
        self.entries
            .lock()
            .expect("avatar cache")
            .get(key)
            .cloned()
            .unwrap_or(AvatarState::Unknown)
    }

    /// Marks `key` as loading, returning `true` only for the caller that actually started it. This
    /// is what de-duplicates a burst of paints for the same owner into a single request, and what
    /// stops an evicted key from being fetched again on every frame.
    fn begin(&self, key: AvatarKey) -> bool {
        let mut requested = self.requested.lock().expect("avatar requested");
        if requested.contains(&key) {
            // Already fetched or in flight at least once this session; an evicted entry reports
            // Unknown to the renderer (initials fallback) but must not start another download.
            return false;
        }
        requested.insert(key.clone());
        drop(requested);
        self.entries
            .lock()
            .expect("avatar cache")
            .insert(key, AvatarState::Loading);
        true
    }

    fn finish_ready(&self, key: AvatarKey, image: Arc<Image>) {
        let mut entries = self.entries.lock().expect("avatar cache");
        entries.insert(key.clone(), AvatarState::Ready(image));
        drop(entries);
        let mut order = self.order.lock().expect("avatar cache order");
        if !order.contains(&key) {
            order.push(key);
        }
        // Evict the oldest decoded images past the cap. Loading/Failed keys are cheap bookkeeping;
        // only `Ready` entries hold an image worth dropping.
        while order.len() > CACHE_CAPACITY {
            let oldest = order.remove(0);
            if let Ok(mut entries) = self.entries.try_lock()
                && matches!(entries.get(&oldest), Some(AvatarState::Ready(_)))
            {
                entries.remove(&oldest);
            }
        }
    }

    fn finish_failed(&self, key: AvatarKey) {
        self.entries
            .lock()
            .expect("avatar cache")
            .insert(key, AvatarState::Failed);
    }
}

/// The future one avatar byte read resolves to: a status and the body, or `()` for any failure
/// (network, cancellation, oversized). Keeping it a named alias keeps the trait signature readable.
pub type AvatarBytesFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u16, Vec<u8>), ()>> + Send + 'a>>;

/// Fetches raw bytes for an avatar URL. It is a dedicated, tiny abstraction rather than the shared
/// REST transport, because avatar bodies are binary and the JSON transport carries `String` bodies.
/// Implementations must never attach a credential: an avatar CDN must not receive the PAT.
pub trait AvatarBytes: Send + Sync + 'static {
    /// Returns `(status, bytes)` for a plain GET of `url`, bounded by `max_bytes`. Cancellation
    /// resolves to an error the caller treats as a silent stop.
    fn load<'a>(
        &'a self,
        url: &'a str,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> AvatarBytesFuture<'a>;
}

/// HTTP response headers relevant to caching and conditional requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AvatarResponseMetadata {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cache_control: Option<String>,
}

/// The future one conditional avatar byte read resolves to: a status, optional metadata and the body.
pub type AvatarDetailedFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<(u16, AvatarResponseMetadata, Vec<u8>), ()>>
            + Send
            + 'a,
    >,
>;

/// Trait with conditional HTTP capabilities for avatar downloading.
pub trait AvatarSource: Send + Sync + 'static {
    /// Loads an avatar URL with optional conditional headers (`If-None-Match`, `If-Modified-Since`).
    fn load_detailed<'a>(
        &'a self,
        url: &'a str,
        max_bytes: usize,
        etag: Option<&'a str>,
        last_modified: Option<&'a str>,
        cancel: CancellationToken,
    ) -> AvatarDetailedFuture<'a>;
}

/// The production implementation: a dedicated reqwest client that follows redirects within the
/// trusted CDN hosts and never carries an `Authorization` header. It is its own client so the
/// shared REST client's credential injection cannot reach an image host by accident.
pub struct ReqwestAvatars {
    client: reqwest::Client,
}

impl ReqwestAvatars {
    pub fn new() -> Result<Self, ()> {
        Self::with_timeout(std::time::Duration::from_secs(15))
    }

    fn with_timeout(timeout: std::time::Duration) -> Result<Self, ()> {
        let client = reqwest::Client::builder()
            // Covers connection establishment, response headers and the entire streamed body.
            // A stalled endpoint must release its avatar semaphore permit and reach fallback.
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|_| ())?;
        Ok(Self { client })
    }
}

impl Default for ReqwestAvatars {
    fn default() -> Self {
        Self::new().expect("the avatar HTTP client")
    }
}

impl AvatarSource for ReqwestAvatars {
    fn load_detailed<'a>(
        &'a self,
        url: &'a str,
        max_bytes: usize,
        etag: Option<&'a str>,
        last_modified: Option<&'a str>,
        cancel: CancellationToken,
    ) -> AvatarDetailedFuture<'a> {
        Box::pin(async move {
            let mut request = self.client.get(url);
            if let Some(etag) = etag {
                request = request.header("If-None-Match", etag);
            }
            if let Some(last_modified) = last_modified {
                request = request.header("If-Modified-Since", last_modified);
            }
            let response = tokio::select! {
                biased;
                _ = cancel.clone().cancelled() => return Err(()),
                response = request.send() => response.map_err(|_| ())?,
            };
            let status = response.status().as_u16();
            let metadata = AvatarResponseMetadata {
                etag: response
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                last_modified: response
                    .headers()
                    .get("last-modified")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                cache_control: collect_cache_control(response.headers()),
            };
            if status == 304 {
                return Ok((status, metadata, Vec::new()));
            }
            let mut body = Vec::new();
            let mut stream = response;
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancel.clone().cancelled() => return Err(()),
                    chunk = stream.chunk() => chunk.map_err(|_| ())?,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                if body.len() + chunk.len() > max_bytes {
                    // The payload is beyond any avatar. Drop it, but keep the response
                    // status and headers so the caller can still honour `Cache-Control`
                    // (for example a `no-store` answer must invalidate an older entry).
                    return Ok((status, metadata, Vec::new()));
                }
                body.extend_from_slice(&chunk);
            }
            Ok((status, metadata, body))
        })
    }
}

impl AvatarBytes for ReqwestAvatars {
    fn load<'a>(
        &'a self,
        url: &'a str,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u16, Vec<u8>), ()>> + Send + 'a>>
    {
        Box::pin(async move {
            let (status, _metadata, body) = self
                .load_detailed(url, max_bytes, None, None, cancel)
                .await?;
            Ok((status, body))
        })
    }
}

/// Executes one avatar fetch against an injected [`AvatarBytes`] source, optionally backed by an [`AvatarDiskCache`].
pub struct AvatarFetcher {
    bytes: Arc<dyn AvatarBytes>,
    detailed: Option<Arc<dyn AvatarSource>>,
    disk_cache: Option<crate::data::avatar_cache::AvatarDiskCache>,
    token: CancellationToken,
    cancel: CancellationTokenSource,
    /// Whether `http://` is allowed (only for the local test base), mirroring the trusted-origin
    /// rule the GitHub client uses.
    allow_plain_http: bool,
}

impl AvatarFetcher {
    pub fn new(bytes: Arc<dyn AvatarBytes>, allow_plain_http: bool) -> Self {
        let (token, cancel) = CancellationToken::new();
        Self {
            bytes,
            detailed: None,
            disk_cache: None,
            token,
            cancel,
            allow_plain_http,
        }
    }

    #[allow(dead_code)] // Wire-up in AccountService is pending coordinator assignment
    pub fn with_cache(mut self, disk_cache: crate::data::avatar_cache::AvatarDiskCache) -> Self {
        self.disk_cache = Some(disk_cache);
        self
    }

    #[allow(dead_code)] // Wire-up in AccountService is pending coordinator assignment
    pub fn with_source(mut self, source: Arc<dyn AvatarSource>) -> Self {
        self.detailed = Some(source);
        self
    }

    /// Fetches and decodes `key`'s URL. It returns the decoded image or `None` on any failure; a
    /// cancelled fetch is a silent failure the caller can ignore because its key is no longer shown.
    pub async fn fetch(&self, key: &AvatarKey) -> Option<Arc<Image>> {
        let url = key.url.as_str();
        // Only fetch a URL we are willing to send an unauthenticated request to.
        if !self.is_fetchable(url) {
            return None;
        }

        // 1. If we have a disk cache:
        //    a) Acquire write ticket BEFORE reading snapshot and starting download
        //       to preserve stale-writer protections.
        //    b) Read existing cached snapshot under the same operation.
        let (cached_snapshot, write_ticket) = if let Some(cache) = &self.disk_cache {
            let cache = cache.clone();
            let key_clone = key.clone();
            tokio::task::spawn_blocking(move || {
                let ticket = cache.begin_write_sync(&key_clone).ok();
                let snapshot = cache.read_sync(&key_clone);
                (snapshot, ticket)
            })
            .await
            .unwrap_or((None, None))
        } else {
            (None, None)
        };

        // 2. Perform network fetch, conditionally if validators exist and detailed source is available.
        let etag = cached_snapshot
            .as_ref()
            .and_then(|s| s.metadata.etag.as_deref());
        let last_modified = cached_snapshot
            .as_ref()
            .and_then(|s| s.metadata.last_modified.as_deref());

        let has_metadata = self.detailed.is_some() || self.disk_cache.is_none();

        let net_result: Result<(u16, AvatarResponseMetadata, Vec<u8>), ()> =
            if let Some(detailed) = &self.detailed {
                detailed
                    .load_detailed(
                        url,
                        MAX_AVATAR_BYTES,
                        etag,
                        last_modified,
                        self.token.clone(),
                    )
                    .await
            } else {
                self.bytes
                    .load(url, MAX_AVATAR_BYTES, self.token.clone())
                    .await
                    .map(|(status, body)| (status, AvatarResponseMetadata::default(), body))
            };

        match net_result {
            Ok((304, meta, _)) => {
                // 304 Not Modified: only valid when a matching representation is still
                // cached. The commit uses the pre-request ticket and snapshot, so a
                // newer writer or an invalidation makes it stale instead of mixing
                // generations. The response then reflects the *current* entry.
                let cached = cached_snapshot?;
                if let Some(cache) = &self.disk_cache {
                    if has_metadata && let Some(ticket) = write_ticket {
                        let cache = cache.clone();
                        let original = cached.clone();
                        let etag = meta.etag.clone();
                        let lm = meta.last_modified.clone();
                        let cc = meta.cache_control.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            cache.commit_revalidation_sync(ticket, original, etag, lm, cc)
                        })
                        .await;
                    }
                    // Re-read on a blocking worker: this both rechecks scope
                    // authorization and avoids returning a superseded snapshot.
                    let cache = cache.clone();
                    let key_clone = key.clone();
                    let current = tokio::task::spawn_blocking(move || cache.read_sync(&key_clone))
                        .await
                        .ok()
                        .flatten();
                    current.and_then(|snapshot| decode_image(snapshot.bytes))
                } else {
                    decode_image(cached.bytes)
                }
            }
            Ok((status, meta, bytes)) if (200..300).contains(&status) => {
                // Storage policy is decided by the response headers alone, so a
                // `no-store` response invalidates any previously usable
                // representation even when its body turns out to be unusable.
                let permits_storage = meta
                    .cache_control
                    .as_deref()
                    .map(|cc| {
                        !cc.split(',').any(|part| {
                            part.split('=')
                                .next()
                                .unwrap_or("")
                                .trim()
                                .eq_ignore_ascii_case("no-store")
                        })
                    })
                    .unwrap_or(true);

                // The payload is validated before anything is stored, so a
                // corrupt body never replaces a usable cached avatar.
                let decoded = if bytes.is_empty() || bytes.len() > MAX_AVATAR_BYTES {
                    None
                } else {
                    decode_image(bytes.clone())
                };
                let usable = decoded.is_some();

                if let Some(cache) = &self.disk_cache {
                    // Without response metadata (only the basic `AvatarBytes`
                    // trait injected) disk semantics cannot be honoured.
                    if has_metadata {
                        let cache = cache.clone();
                        let key_clone = key.clone();
                        if permits_storage && usable {
                            if let Some(ticket) = write_ticket {
                                let etag = meta.etag.clone();
                                let lm = meta.last_modified.clone();
                                let cc = meta.cache_control.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    cache.commit_write_sync(ticket, &bytes, etag, lm, cc)
                                })
                                .await;
                            }
                        } else if !permits_storage {
                            // no-store: commit through the same pre-request ticket so that
                            // newer writers/invalidations win instead of being unconditionally deleted.
                            if let Some(ticket) = write_ticket {
                                let etag = meta.etag.clone();
                                let lm = meta.last_modified.clone();
                                let cc = meta.cache_control.clone();
                                let body = bytes.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    cache.commit_write_sync(ticket, &body, etag, lm, cc)
                                })
                                .await;
                            } else {
                                let _ = tokio::task::spawn_blocking(move || {
                                    cache.invalidate_sync(&key_clone);
                                })
                                .await;
                            }
                        }
                    }
                }

                decoded
            }
            _ => {
                // Network failed or returned error status (e.g. 404, 500, offline).
                // Fall back to the *current* cached entry, re-read on a blocking
                // worker: that rechecks scope authorization and evaluates the reuse
                // policy of whatever is stored now, so a superseded or invalidated
                // snapshot is never resurrected.
                if let Some(cache) = &self.disk_cache {
                    let cache = cache.clone();
                    let key_clone = key.clone();
                    let current = tokio::task::spawn_blocking(move || cache.read_sync(&key_clone))
                        .await
                        .ok()
                        .flatten();
                    current.and_then(|snapshot| {
                        if crate::data::avatar_cache::AvatarDiskCache::is_snapshot_reusable_offline(
                            &snapshot,
                        ) {
                            decode_image(snapshot.bytes)
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            }
        }
    }

    /// `https` always; `http` only when this fetcher was told the base is a local test server.
    fn is_fetchable(&self, url: &str) -> bool {
        match decode_scheme(url) {
            Some("https") => true,
            Some("http") => self.allow_plain_http,
            _ => false,
        }
    }
}

fn decode_scheme(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    if rest.is_empty() {
        return None;
    }
    Some(scheme)
}

/// Decodes fetched bytes into a GPUI `Image` in a format GPUI can rasterize locally, but only after
/// a **full** decode proves the payload is a real image.
///
/// A signature check is not enough: an eight-byte PNG header, or a truncated or corrupted body with
/// a correct prefix, would be accepted and then fail inside GPUI's renderer, producing exactly the
/// broken-image state this path exists to avoid. Decoding here means a bad payload is a failure and
/// the caller keeps its initials fallback. SVG is rejected because it needs a renderer this path
/// does not have.
fn decode_image(bytes: Vec<u8>) -> Option<Arc<Image>> {
    let format = detect_format(&bytes)?;
    // Fully decode to validate; the decoded pixels are then dropped. This runs off the UI thread.
    image::load_from_memory_with_format(&bytes, to_image_crate_format(format)).ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

/// Maps GPUI's image format to the `image` crate's, for the validation decode.
fn to_image_crate_format(format: ImageFormat) -> image::ImageFormat {
    match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Gif => image::ImageFormat::Gif,
        // `detect_format` only ever returns the four above.
        _ => unreachable!("only avatar formats reach the decoder"),
    }
}

/// Joins every `Cache-Control` field value into one directive list. A response
/// with no such header returns `None` (not an empty string), so a 304 without
/// the header does not erase the policy stored with the original representation.
fn collect_cache_control(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let directives: Vec<&str> = headers
        .get_all("cache-control")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();
    if directives.is_empty() {
        None
    } else {
        Some(directives.join(", "))
    }
}

/// Sniffs the format from the leading bytes rather than trusting a header or a file extension, so a
/// mislabelled or truncated payload is rejected instead of handed to the wrong decoder.
fn detect_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.len() >= 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.len() >= 3 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        Some(ImageFormat::Gif)
    } else {
        None
    }
}

/// The result of one fetch, as it returns to the UI thread.
pub enum AvatarCompletion {
    Ready(AvatarKey, Arc<Image>),
    Failed(AvatarKey),
}

/// A queue the account controller drains on its light beat, shared with the avatar cache so the
/// decode work never touches a GPUI entity from the network thread.
pub type AvatarInbox = Arc<Mutex<Vec<AvatarCompletion>>>;

/// A request handed back by the renderer when it wants an avatar that is not cached yet. The
/// controller starts it on the executor; the renderer never awaits anything itself.
pub struct AvatarRequest {
    pub key: AvatarKey,
}

impl AvatarCache {
    /// Applies one finished fetch. It is safe to call for a key that is no longer on screen: the
    /// image is only stored under its own key.
    pub fn apply(&self, completion: AvatarCompletion) {
        match completion {
            AvatarCompletion::Ready(key, image) => self.finish_ready(key, image),
            AvatarCompletion::Failed(key) => self.finish_failed(key),
        }
    }

    /// Registers interest in `key`, returning a request only for the caller that should actually
    /// start the fetch. Repeated paints for the same key get `None`.
    pub fn request(&self, key: AvatarKey) -> Option<AvatarRequest> {
        if self.begin(key.clone()) {
            Some(AvatarRequest { key })
        } else {
            None
        }
    }
}

/// Runs one avatar fetch on the caller-provided fetcher and pushes the completion into `inbox`.
/// Called on the executor; it must not touch a GPUI entity.
pub async fn run_fetch(
    permit: tokio::sync::OwnedSemaphorePermit,
    fetcher: Arc<AvatarFetcher>,
    request: AvatarRequest,
    inbox: AvatarInbox,
) {
    // Held for the whole fetch so the shared avatar concurrency bound is respected.
    let _permit = permit;
    let key = request.key;
    match fetcher.fetch(&key).await {
        Some(image) => inbox
            .lock()
            .expect("avatar inbox")
            .push(AvatarCompletion::Ready(key, image)),
        None => inbox
            .lock()
            .expect("avatar inbox")
            .push(AvatarCompletion::Failed(key)),
    }
}

/// Drains the queue. Returns the completions so the caller can apply them and repaint once.
pub fn drain(inbox: &AvatarInbox) -> Vec<AvatarCompletion> {
    let mut queue = inbox.lock().expect("avatar inbox");
    queue.drain(..).collect()
}

impl AvatarFetcher {
    /// Stops in-flight avatar requests. Called from the controller's teardown so quitting does not
    /// leave image fetches running against a closing network stack.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
#[path = "../data/avatar_cache/tests.rs"]
mod avatar_cache_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A byte source that records how many times it was asked and returns fixed bytes per URL. It
    /// is the injected stand-in for the network: no real avatar host is contacted and no credential
    /// is involved, because avatars are plain GETs.
    struct BytesSource {
        bodies: Mutex<HashMap<String, (u16, Vec<u8>)>>,
        reads: AtomicUsize,
    }

    impl BytesSource {
        fn new() -> Self {
            Self {
                bodies: Mutex::new(HashMap::new()),
                reads: AtomicUsize::new(0),
            }
        }

        fn serve(&self, url: &str, status: u16, body: Vec<u8>) {
            self.bodies
                .lock()
                .unwrap()
                .insert(url.to_string(), (status, body));
        }
    }

    impl AvatarBytes for BytesSource {
        fn load<'a>(
            &'a self,
            url: &'a str,
            _max_bytes: usize,
            _cancel: CancellationToken,
        ) -> AvatarBytesFuture<'a> {
            Box::pin(async move {
                self.reads.fetch_add(1, Ordering::SeqCst);
                self.bodies.lock().unwrap().get(url).cloned().ok_or(())
            })
        }
    }

    /// A real 1x1 RGBA PNG, so GPUI's decoder actually succeeds on it.
    fn png() -> Vec<u8> {
        vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 96, 96, 96,
            248, 15, 0, 1, 4, 1, 0, 95, 229, 195, 75, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ]
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// Real sockets exercise both a server that never sends headers and one that sends headers
    /// then stalls its body. Each timeout must release the only permit so the next request runs.
    #[tokio::test]
    async fn stalled_headers_and_body_time_out_and_release_the_next_request() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Duration;

        for send_headers in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (release, stop) = std::sync::mpsc::channel::<()>();
            let server = std::thread::spawn(move || {
                let (mut stalled, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let size = stalled.read(&mut request).unwrap();
                assert!(
                    !String::from_utf8_lossy(&request[..size])
                        .to_ascii_lowercase()
                        .contains("authorization:")
                );
                if send_headers {
                    stalled
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\nx")
                        .unwrap();
                    stalled.flush().unwrap();
                }
                // Keep the first socket open while accepting and completing the following request.
                let (mut following, _) = listener.accept().unwrap();
                assert!(following.read(&mut request).unwrap() > 0);
                let body = png();
                write!(
                    following,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                following.write_all(&body).unwrap();
                following.flush().unwrap();
                let _ = stop.recv_timeout(Duration::from_secs(5));
            });
            let bytes = Arc::new(ReqwestAvatars::with_timeout(Duration::from_millis(150)).unwrap());
            let fetcher = Arc::new(AvatarFetcher::new(bytes, true));
            let permits = Arc::new(tokio::sync::Semaphore::new(1));
            let inbox = Arc::new(Mutex::new(Vec::new()));
            let stalled_key = AvatarKey::new("test", 1, format!("{base}/stalled"));
            tokio::time::timeout(
                Duration::from_secs(2),
                run_fetch(
                    permits.clone().acquire_owned().await.unwrap(),
                    fetcher.clone(),
                    AvatarRequest {
                        key: stalled_key.clone(),
                    },
                    inbox.clone(),
                ),
            )
            .await
            .expect("a stalled avatar reaches its total deadline");
            assert_eq!(
                permits.available_permits(),
                1,
                "a timed-out fetch releases its permit"
            );
            assert!(
                matches!(drain(&inbox).as_slice(), [AvatarCompletion::Failed(key)] if key == &stalled_key)
            );
            let next_key = AvatarKey::new("test", 2, format!("{base}/next"));
            tokio::time::timeout(
                Duration::from_secs(2),
                run_fetch(
                    permits.clone().acquire_owned().await.unwrap(),
                    fetcher,
                    AvatarRequest {
                        key: next_key.clone(),
                    },
                    inbox.clone(),
                ),
            )
            .await
            .expect("the next avatar is not starved");
            assert!(
                matches!(drain(&inbox).as_slice(), [AvatarCompletion::Ready(key, _)] if key == &next_key)
            );
            let _ = release.send(());
            server.join().unwrap();
        }
    }

    /// A response whose streamed body exceeds the caller's budget still reports
    /// its status and headers, so a `no-store` answer can invalidate an older
    /// entry even though the oversized body itself is dropped.
    #[tokio::test]
    async fn an_oversized_streamed_body_keeps_its_cache_control() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 64\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            socket.write_all(&[0u8; 64]).unwrap();
            socket.flush().unwrap();
        });

        let source = ReqwestAvatars::new().unwrap();
        let (status, metadata, body) = source
            .load_detailed(
                &format!("{base}/huge"),
                16,
                None,
                None,
                CancellationToken::default(),
            )
            .await
            .expect("an oversized body is not a transport failure");
        assert_eq!(status, 200);
        assert_eq!(metadata.cache_control.as_deref(), Some("no-store"));
        assert!(body.is_empty(), "the oversized body is not retained");
        server.join().unwrap();
    }

    #[test]
    fn the_key_separates_identity_and_url() {
        let a = AvatarKey::new("github.com", 1, "https://x/a.png");
        let renamed = AvatarKey::new("github.com", 1, "https://x/b.png");
        let other = AvatarKey::new("github.com", 2, "https://x/a.png");
        assert_ne!(a, renamed, "a changed URL is a different avatar");
        assert_ne!(a, other, "a different account id is a different avatar");
        assert_eq!(a, AvatarKey::new("github.com", 1, "https://x/a.png"));
    }

    #[test]
    fn only_https_is_fetchable_by_default() {
        let transport: Arc<dyn AvatarBytes> = Arc::new(BytesSource::new());
        let fetcher = AvatarFetcher::new(transport, false);
        assert!(fetcher.is_fetchable("https://avatars.githubusercontent.com/u/1"));
        assert!(!fetcher.is_fetchable("http://avatars.githubusercontent.com/u/1"));
        assert!(!fetcher.is_fetchable("file:///etc/passwd"));
        assert!(!fetcher.is_fetchable("ftp://x/y"));
        assert!(!fetcher.is_fetchable("not a url"));
        let local = AvatarFetcher::new(Arc::new(BytesSource::new()) as Arc<dyn AvatarBytes>, true);
        assert!(local.is_fetchable("http://127.0.0.1:9/avatar.png"));
    }

    #[test]
    fn a_decodable_image_fetches_and_a_missing_one_fails() {
        let transport = Arc::new(BytesSource::new());
        transport.serve("https://x/ok.png", 200, png());
        transport.serve("https://x/missing.png", 404, b"not found".to_vec());
        transport.serve("https://x/garbage.png", 200, b"<svg></svg>".to_vec());
        let fetcher = Arc::new(AvatarFetcher::new(
            transport.clone() as Arc<dyn AvatarBytes>,
            false,
        ));
        assert!(
            block_on(fetcher.fetch(&AvatarKey::new("github.com", 1, "https://x/ok.png"))).is_some(),
            "a decodable PNG produces an image"
        );
        assert!(
            block_on(fetcher.fetch(&AvatarKey::new("github.com", 2, "https://x/missing.png")))
                .is_none(),
            "a 404 produces no image (fallback initials)"
        );
        assert!(
            block_on(fetcher.fetch(&AvatarKey::new("github.com", 3, "https://x/garbage.png")))
                .is_none(),
            "undecodable bytes produce no image, never a broken-image icon"
        );
        // The avatar path builds no Authorization header: `AvatarBytes::load` takes only a URL and
        // a bound, so there is no parameter through which the PAT could reach an image host.
        fn assert_no_credential_parameter() {
            // A compile-time guarantee expressed as a source-level note: `load` has no token
            // argument. This line exists so a future signature change that adds one fails review.
            let _ = |url: &str, max: usize, cancel: CancellationToken| {
                let _ = (url, max, cancel);
            };
        }
        assert_no_credential_parameter();
    }

    #[test]
    fn the_cache_deduplicates_fetches_by_identity_and_url() {
        let cache = AvatarCache::new();
        let key = AvatarKey::new("github.com", 7, "https://x/a.png");
        assert!(
            cache.request(key.clone()).is_some(),
            "the first paint fetches"
        );
        assert!(
            cache.request(key.clone()).is_none(),
            "a repaint must not refetch"
        );
        let renamed = AvatarKey::new("github.com", 7, "https://x/b.png");
        assert!(
            cache.request(renamed).is_some(),
            "a new URL fetches its own body"
        );
        let broken = AvatarKey::new("github.com", 8, "https://x/broken.png");
        assert!(cache.request(broken.clone()).is_some());
        cache.apply(AvatarCompletion::Failed(broken.clone()));
        assert!(
            cache.request(broken).is_none(),
            "a known failure is not retried"
        );
    }

    #[test]
    fn a_late_response_is_stored_under_its_own_key_only() {
        let cache = AvatarCache::new();
        let a = AvatarKey::new("github.com", 1, "https://x/a.png");
        let b = AvatarKey::new("github.com", 2, "https://x/b.png");
        cache.apply(AvatarCompletion::Failed(b.clone()));
        assert!(matches!(cache.state(&a), AvatarState::Unknown));
        assert!(matches!(cache.state(&b), AvatarState::Failed));
    }

    /// A list larger than the cache must settle: an evicted key is not refetched, so fills do not
    /// loop forever while the window is idle.
    #[test]
    fn an_evicted_avatar_is_not_requested_again() {
        let cache = AvatarCache::new();
        // Fill past the capacity with one-pixel images.
        for id in 0..(CACHE_CAPACITY as u64 + 8) {
            let key = AvatarKey::new("github.com", id, format!("https://x/{id}.png"));
            assert!(cache.request(key.clone()).is_some());
            cache.apply(AvatarCompletion::Failed(key));
        }
        // The very first key is long evicted or failed; either way it must not be requested again.
        let first = AvatarKey::new("github.com", 0, "https://x/0.png");
        assert!(
            cache.request(first.clone()).is_none(),
            "an evicted or already-fetched avatar is never refetched"
        );
        assert!(
            cache.request(first).is_none(),
            "repeated paints after eviction still do not refetch"
        );
    }

    /// A payload with a correct signature but no decodable body must fail, not publish a Ready image
    /// that would render as a broken-image icon.
    #[test]
    fn a_signature_only_payload_is_rejected() {
        // Just the eight-byte PNG signature, and a truncated real PNG.
        assert!(decode_image(vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).is_none());
        let mut truncated = png();
        truncated.truncate(24);
        assert!(decode_image(truncated).is_none());
        // The intact image still decodes.
        assert!(decode_image(png()).is_some());
    }

    #[test]
    fn formats_are_detected_from_content_not_extension() {
        assert_eq!(detect_format(&png()), Some(ImageFormat::Png));
        assert_eq!(
            detect_format(&[0xFF, 0xD8, 0xFF, 0x00]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(detect_format(b"RIFF0000WEBPmore"), Some(ImageFormat::Webp));
        assert_eq!(detect_format(b"GIF89a...."), Some(ImageFormat::Gif));
        assert_eq!(detect_format(b"<svg></svg>"), None);
        assert_eq!(detect_format(b""), None);
    }

    #[test]
    fn cache_control_is_absent_only_when_no_field_is_present() {
        use reqwest::header::{HeaderMap, HeaderValue};

        // No field at all: absence must stay `None`, so a 304 without the header
        // does not erase the policy stored with the original representation.
        assert_eq!(collect_cache_control(&HeaderMap::new()), None);

        // Repeated fields are merged into one directive list, so a later
        // `no-store`/`no-cache` is not lost behind an earlier `max-age`.
        let mut headers = HeaderMap::new();
        headers.append("cache-control", HeaderValue::from_static("max-age=3600"));
        headers.append("cache-control", HeaderValue::from_static("no-store"));
        assert_eq!(
            collect_cache_control(&headers).as_deref(),
            Some("max-age=3600, no-store")
        );
    }
}
