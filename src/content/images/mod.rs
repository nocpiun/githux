//! Bounded image policy shared by the reader and the image services it is given.
//!
//! This module is UI-independent: it carries the engineering budgets, the
//! format sniffing and the bounded decode/validation step, plus the small
//! contract the read-only reader uses to ask a host for one picture. It never
//! performs I/O itself. Network and credential work belongs to the data and
//! network layers, which implement [`ImageProvider`] over these types.
//!
//! Boundaries that matter here:
//!
//! * Every picture is bounded before it can allocate: the encoded response is
//!   capped while it streams, the header is read first, and the full decode
//!   runs under [`image::Limits`] so an oversized or malformed payload is
//!   rejected instead of becoming a broken-image state.
//! * Only PNG, JPEG, WebP and GIF are accepted, matching the formats the
//!   existing `image` dependency is built with. SVG is reported as
//!   [`ImageFailure::Unsupported`] rather than executed or silently accepted.
//! * The reader owns the document-generation check; a provider owns the
//!   account/authorization check. A completion carries the generation it was
//!   requested under so a stale result can never be published into a newer
//!   document.
//! * No cache is shared between readers. Two readers showing the same picture
//!   have independent state, independent cancellation and independent memory.

mod decode;

#[allow(unused_imports)]
pub use decode::{DecodedImage, ImageFormat, decode_image, detect_format, is_svg_like};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Allocates a process-unique reader id, so two readers never share a scope.
static NEXT_READER_ID: AtomicU64 = AtomicU64::new(1);

/// The next reader id. Readers are few and long-lived, so this never wraps in
/// practice; a wrapped id is still paired with a fresh generation.
pub fn next_reader_id() -> u64 {
    NEXT_READER_ID.fetch_add(1, Ordering::Relaxed)
}

/// Engineering budgets for one document's pictures.
///
/// The defaults are deliberately small enough for a README reader: one picture
/// is at most 5 MiB encoded and 8 megapixels decoded, a document keeps at most
/// 32 decoded pictures in memory, and at most three loads run at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageLimits {
    /// Largest accepted encoded body per picture, in bytes.
    pub max_response_bytes: usize,
    /// Largest accepted decoded buffer per picture, in bytes. Also the
    /// allocation ceiling passed to the decoder.
    pub max_decoded_bytes: usize,
    /// Largest accepted pixel count (`width * height`) per picture.
    pub max_pixels: u64,
    /// Largest accepted width or height per picture.
    pub max_dimension: u32,
    /// Wall-clock budget for one complete load, including redirects.
    pub total_timeout: Duration,
    /// Simultaneous loads per provider.
    pub max_concurrent: usize,
    /// Requests the reader may hand to one provider before waiting.
    pub max_in_flight: usize,
    /// Decoded pictures retained per reader document.
    pub max_ready_images: usize,
    /// Decoded bytes retained per reader document.
    pub max_ready_bytes: usize,
    /// Maximum redirect hops followed for one external picture.
    pub max_redirects: usize,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: 5 * 1024 * 1024,
            max_decoded_bytes: 32 * 1024 * 1024,
            max_pixels: 8_000_000,
            max_dimension: 8192,
            total_timeout: Duration::from_secs(20),
            max_concurrent: 3,
            max_in_flight: 12,
            max_ready_images: 32,
            max_ready_bytes: 64 * 1024 * 1024,
            max_redirects: 5,
        }
    }
}

/// Why an address cannot be turned into an image source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRefusal {
    /// An `http:` address: external pictures are fetched over HTTPS only.
    InsecureExternal,
    /// A `file:`, `data:`, `javascript:` or other non-web scheme.
    LocalOrUnsupportedScheme,
    /// A protocol-relative `//host/path` address with no repository meaning.
    ProtocolRelative,
    /// An empty address, a bad escape, a backslash or a control character.
    InvalidPath,
    /// A fragment-only address with no picture.
    Empty,
}

impl ImageRefusal {
    /// A short, stable label for the reader's placeholder.
    pub fn label(self) -> &'static str {
        match self {
            Self::InsecureExternal => "only HTTPS pictures load",
            Self::LocalOrUnsupportedScheme => "local or unsupported source",
            Self::ProtocolRelative => "protocol-relative source",
            Self::InvalidPath => "invalid picture path",
            Self::Empty => "empty picture address",
        }
    }
}

/// Why a picture could not be produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageFailure {
    /// The address was refused before any request.
    Refused(ImageRefusal),
    /// No image service is attached to this reader.
    NoProvider,
    /// The account/authorization this read started under is no longer current.
    StaleContext,
    /// The object does not exist, or is not an ordinary file.
    NotFound,
    /// The read was cancelled (document replaced, reader hidden, window closed).
    Cancelled,
    /// The request exceeded its total deadline.
    Timeout,
    /// A network or transport failure.
    Network,
    /// The body exceeded the encoded budget.
    TooLarge { limit: usize },
    /// The decoded picture exceeded the pixel budget.
    TooManyPixels { width: u32, height: u32, limit: u64 },
    /// The bytes are not one of the supported picture formats.
    Unsupported,
    /// The bytes claim a supported format but do not decode.
    Malformed,
}

impl ImageFailure {
    /// A short, stable label for the reader's placeholder.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Refused(reason) => reason.label(),
            Self::NoProvider => "no image service attached",
            Self::StaleContext => "authorization changed",
            Self::NotFound => "picture not found",
            Self::Cancelled => "load cancelled",
            Self::Timeout => "picture timed out",
            Self::Network => "picture unavailable",
            Self::TooLarge { .. } => "picture too large",
            Self::TooManyPixels { .. } => "picture has too many pixels",
            Self::Unsupported => "unsupported picture format",
            Self::Malformed => "picture data is invalid",
        }
    }

    /// Whether the reader may show a retry affordance for this failure.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network | Self::Timeout | Self::Cancelled | Self::StaleContext
        )
    }
}

/// The stable identity of one picture source inside a document.
///
/// Two blocks pointing at the same source share one key, one request and one
/// completion. The key is derived from the resolved target, so a repository
/// path at one commit never collides with the same path at another commit.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageKey(String);

impl ImageKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a picture comes from, after the reader resolved the Markdown address
/// against the document's fixed repository context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageTarget {
    /// A file in the same repository at the fixed commit and resolved path.
    Repository {
        owner: String,
        name: String,
        commit: String,
        path: String,
    },
    /// A public HTTPS address fetched without any credential.
    External { url: String },
}

impl ImageTarget {
    /// The identity of this source, stable across blocks and readers.
    pub fn key(&self) -> ImageKey {
        match self {
            Self::Repository {
                owner,
                name,
                commit,
                path,
            } => ImageKey::new(format!("repo:{owner}/{name}@{commit}:{path}")),
            Self::External { url } => ImageKey::new(format!("url:{url}")),
        }
    }
}

/// The identity of one reader document generation.
///
/// `reader` is unique per reader entity, so two readers sharing one provider
/// still have separate work: cancelling one reader's document never cancels the
/// other's, even when their generation numbers coincide. `generation` advances
/// for every document a reader shows, so an old completion is always
/// distinguishable from a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageScope {
    pub reader: u64,
    pub generation: u64,
}

/// One picture request, bound to the reader document generation that wants it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRequest {
    pub scope: ImageScope,
    pub key: ImageKey,
    pub target: ImageTarget,
}

/// The result of one request.
#[derive(Debug)]
pub struct ImageCompletion {
    pub scope: ImageScope,
    pub key: ImageKey,
    pub result: Result<DecodedImage, ImageFailure>,
}

/// A thread-safe completion queue.
///
/// Providers push from their own thread; the reader drains it on its own pump
/// beat. The queue is never awaited, so a completion that lands between two
/// drains is simply picked up by the next one — it cannot be lost, and no
/// cross-thread wake-up is needed.
#[derive(Clone, Default)]
pub struct ImageSink {
    completions: Arc<Mutex<Vec<ImageCompletion>>>,
}

impl ImageSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, completion: ImageCompletion) {
        self.completions
            .lock()
            .expect("image completion queue")
            .push(completion);
    }

    pub fn drain(&self) -> Vec<ImageCompletion> {
        std::mem::take(&mut *self.completions.lock().expect("image completion queue"))
    }
}

/// The host-supplied image capability.
///
/// Implementations must return promptly, perform all network and decode work
/// on the project's background executor, and honour [`ImageProvider::cancel`].
/// The reader itself never performs I/O or decoding.
pub trait ImageProvider: Send + Sync + 'static {
    /// Starts one bounded load and delivers its single result through `sink`.
    /// Implementations must not block the calling thread.
    fn start(&self, request: ImageRequest, sink: ImageSink);

    /// Cancels every load started under `scope` (one reader's document
    /// generation). Safe to call repeatedly, and it must not affect any other
    /// scope, including the same generation number in another reader.
    fn cancel(&self, scope: ImageScope);
}

#[cfg(test)]
mod tests;
