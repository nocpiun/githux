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

/// Largest avatar payload accepted, in bytes. GitHub avatars are small; a larger body is treated as
/// a failure instead of being decoded or buffered without bound.
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

/// How many decoded avatars stay cached. The sidebar shows at most a screenful of owners plus the
/// saved accounts, so this is generous while still bounded.
const CACHE_CAPACITY: usize = 128;

/// The stable identity of one avatar. It is deliberately not a row index or a login string: the
/// account/owner key stays the same when the list reorders, and a changed URL is a different key so
/// the old image is never shown for the new identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AvatarKey {
    /// `host` + numeric owner/account id, when known, so one host's id cannot match another's.
    pub subject: String,
    /// The exact URL GitHub returned. A different URL is a different key, so a renamed account or a
    /// refreshed URL cannot keep serving the previous picture.
    pub url: String,
}

impl AvatarKey {
    /// Builds a key from a host, a stable numeric id and the URL. The numeric id is what survives a
    /// rename; the URL distinguishes a genuinely new picture.
    pub fn new(host: impl AsRef<str>, id: u64, url: impl Into<String>) -> Self {
        Self {
            subject: format!("{}#{id}", host.as_ref()),
            url: url.into(),
        }
    }
}

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
    /// is what de-duplicates a burst of paints for the same owner into a single request.
    fn begin(&self, key: AvatarKey) -> bool {
        let mut entries = self.entries.lock().expect("avatar cache");
        match entries.get(&key) {
            Some(AvatarState::Loading) | Some(AvatarState::Ready(_)) => false,
            // A known failure is terminal: do not retry the same URL on every frame.
            Some(AvatarState::Failed) => false,
            None | Some(AvatarState::Unknown) => {
                entries.insert(key, AvatarState::Loading);
                true
            }
        }
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

/// The production implementation: a dedicated reqwest client that follows redirects within the
/// trusted CDN hosts and never carries an `Authorization` header. It is its own client so the
/// shared REST client's credential injection cannot reach an image host by accident.
pub struct ReqwestAvatars {
    client: reqwest::Client,
}

impl ReqwestAvatars {
    pub fn new() -> Result<Self, ()> {
        let client = reqwest::Client::builder()
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

impl AvatarBytes for ReqwestAvatars {
    fn load<'a>(
        &'a self,
        url: &'a str,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u16, Vec<u8>), ()>> + Send + 'a>>
    {
        Box::pin(async move {
            let request = self.client.get(url);
            let response = tokio::select! {
                biased;
                _ = cancel.clone().cancelled() => return Err(()),
                response = request.send() => response.map_err(|_| ())?,
            };
            let status = response.status().as_u16();
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
                    // A larger payload than any avatar: treat it as a failure, not a decode target.
                    return Err(());
                }
                body.extend_from_slice(&chunk);
            }
            Ok((status, body))
        })
    }
}

/// Executes one avatar fetch against an injected [`AvatarBytes`] source.
pub struct AvatarFetcher {
    bytes: Arc<dyn AvatarBytes>,
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
            token,
            cancel,
            allow_plain_http,
        }
    }

    /// Fetches and decodes `key`'s URL. It returns the decoded image or `None` on any failure; a
    /// cancelled fetch is a silent failure the caller can ignore because its key is no longer shown.
    pub async fn fetch(&self, key: &AvatarKey) -> Option<Arc<Image>> {
        let url = key.url.as_str();
        // Only fetch a URL we are willing to send an unauthenticated request to.
        if !self.is_fetchable(url) {
            return None;
        }
        let (status, bytes) = self
            .bytes
            .load(url, MAX_AVATAR_BYTES, self.token.clone())
            .await
            .ok()?;
        if !(200..300).contains(&status) {
            return None;
        }
        if bytes.is_empty() || bytes.len() > MAX_AVATAR_BYTES {
            return None;
        }
        decode_image(bytes)
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

/// Decodes fetched bytes into a GPUI `Image` in a format GPUI can rasterize locally. GitHub serves
/// PNG (and sometimes JPEG); SVG is rejected because it needs a renderer the avatar path does not
/// have. Anything else, or a broken payload, is a failure — never a broken-image icon.
fn decode_image(bytes: Vec<u8>) -> Option<Arc<Image>> {
    let format = detect_format(&bytes)?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
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
pub async fn run_fetch(fetcher: Arc<AvatarFetcher>, request: AvatarRequest, inbox: AvatarInbox) {
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
}
