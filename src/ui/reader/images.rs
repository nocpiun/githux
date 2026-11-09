//! UI-independent adapter to the existing content image capability.
//!
//! The reader never fetches or decodes: it resolves a picture against the fixed
//! repository context, asks the host-supplied [`ImageProvider`] for it, and
//! turns the provider's already-validated bytes into an in-memory source for
//! NSH to draw. A README address never reaches NSH's default image loader.
//!
//! State is per bridge and per scope. A completion is applied only when it
//! belongs to this bridge's current scope **and** the captured capability is
//! still valid; retiring is permanent, so a revoked account cannot be revived by
//! a late result or by the same account being re-authorized. Decoded memory is
//! bounded; an evicted picture stays visible as a released placeholder instead
//! of being re-requested every frame (which would thrash the provider).

use super::{
    ResourceContext,
    links::{self, LinkTarget, RefusalReason},
};
use crate::content::images::{
    ImageCompletion, ImageFailure, ImageFormat, ImageKey, ImageLimits, ImageProvider, ImageRefusal,
    ImageRequest, ImageScope, ImageSink, ImageTarget, next_reader_id,
};
use base64::Engine;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

/// Must only perform atomic/in-memory checks, e.g. a captured
/// `CacheAuthorization` plus a session/epoch guard. Capture the token belonging
/// to this document's reply, never reacquire a new one here. It runs on the UI
/// thread during `poll`, so it must not touch storage or the network.
pub type CapabilityCheck = Arc<dyn Fn() -> bool + Send + Sync>;

/// What the reader shows for one picture source.
#[derive(Debug, Clone, PartialEq)]
pub enum Picture {
    /// The provider is fetching or decoding it.
    Pending,
    /// Ready to draw: an in-memory data source the provider produced.
    Ready {
        source: String,
        width: u32,
        height: u32,
    },
    /// Released to stay inside the decoded-memory budget. It is not requested
    /// again for this document generation; hiding/resuming the reader (or
    /// replacing the document) demands it again.
    Released,
    /// The load failed, the address was refused, or no provider is attached.
    Failed(ImageFailure),
}

struct Entry {
    picture: Picture,
    target: ImageTarget,
    /// Handed to the provider and not completed/cancelled yet.
    started: bool,
    /// Decoded bytes while [`Picture::Ready`].
    bytes: usize,
}

/// The adapter between one reader document and the host's image provider.
pub struct ImageBridge {
    scope: ImageScope,
    provider: Option<Arc<dyn ImageProvider>>,
    guard: CapabilityCheck,
    retired: bool,
    sink: ImageSink,
    limits: ImageLimits,
    entries: HashMap<ImageKey, Entry>,
    ready: VecDeque<ImageKey>,
}

impl ImageBridge {
    pub fn new(
        provider: Option<Arc<dyn ImageProvider>>,
        guard: CapabilityCheck,
        limits: ImageLimits,
    ) -> Self {
        Self {
            scope: ImageScope {
                reader: next_reader_id(),
                generation: 1,
            },
            provider,
            guard,
            retired: false,
            sink: ImageSink::new(),
            limits,
            entries: HashMap::new(),
            ready: VecDeque::new(),
        }
    }

    pub fn scope(&self) -> ImageScope {
        self.scope
    }

    /// The budgets this bridge applies.
    pub fn limits(&self) -> &ImageLimits {
        &self.limits
    }

    /// Whether the captured capability still allows this document to be shown.
    pub fn is_current(&self) -> bool {
        !self.retired && (self.guard)()
    }

    /// Whether the bridge has been permanently retired (authorization loss or
    /// explicit retirement).
    pub fn is_retired(&self) -> bool {
        self.retired
    }

    /// Permanently retire the captured document capability: cancel in-flight
    /// work, drop every queued request and every displayed picture. A
    /// re-authorized account needs a new bridge.
    pub fn retire(&mut self) {
        if let Some(provider) = &self.provider {
            provider.cancel(self.scope);
        }
        self.retired = true;
        self.entries.clear();
        self.ready.clear();
        self.sink.drain();
    }

    /// Called while making an image element. Only queues work; it never invokes
    /// the provider. The request starts in the host's next [`Self::poll`].
    pub fn picture(&mut self, raw: &str, context: &ResourceContext) -> Picture {
        if !self.is_current() {
            self.retire();
            return Picture::Failed(ImageFailure::StaleContext);
        }
        let target = match resolve(raw, context) {
            Ok(target) => target,
            Err(error) => return Picture::Failed(error),
        };
        if self.provider.is_none() {
            return Picture::Failed(ImageFailure::NoProvider);
        }
        let key = target.key();
        if self.entries.contains_key(&key) {
            return self.entries[&key].picture.clone();
        }
        // Bound metadata as well as decoded memory: a document with hundreds of
        // pictures cannot accumulate an unbounded state table.
        if self.entries.len()
            >= self
                .limits
                .max_ready_images
                .saturating_add(self.limits.max_in_flight)
                .max(1)
        {
            return Picture::Failed(ImageFailure::TooLarge {
                limit: self.limits.max_ready_bytes,
            });
        }
        self.entries.insert(
            key,
            Entry {
                picture: Picture::Pending,
                target,
                started: false,
                bytes: 0,
            },
        );
        Picture::Pending
    }

    /// Host UI drain: validate the captured ticket, accept current completions,
    /// release over-budget pictures, then start queued work. Returns whether
    /// presentation changed. Call once per host drain, including after cached
    /// replies and after the window becomes visible again.
    pub fn poll(&mut self) -> bool {
        if !self.is_current() {
            let changed = !self.retired;
            self.retire();
            return changed;
        }
        let mut changed = false;
        for completion in self.sink.drain() {
            changed |= self.apply(completion);
        }
        changed |= self.evict();
        let active = self
            .entries
            .values()
            .filter(|e| e.started && e.picture == Picture::Pending)
            .count();
        let requests: Vec<_> = self
            .entries
            .iter_mut()
            .filter(|(_, e)| !e.started && e.picture == Picture::Pending)
            .take(self.limits.max_in_flight.saturating_sub(active))
            .map(|(key, e)| {
                e.started = true;
                ImageRequest {
                    scope: self.scope,
                    key: key.clone(),
                    target: e.target.clone(),
                }
            })
            .collect();
        if !requests.is_empty() {
            changed |= true;
        }
        if let Some(provider) = &self.provider {
            for request in requests {
                provider.start(request, self.sink.clone());
            }
        }
        changed
    }

    fn apply(&mut self, completion: ImageCompletion) -> bool {
        if completion.scope != self.scope || !self.is_current() {
            return false;
        }
        let Some(entry) = self.entries.get_mut(&completion.key) else {
            return false;
        };
        if !entry.started || entry.picture != Picture::Pending {
            return false;
        }
        entry.picture = match completion.result {
            Ok(image) => {
                entry.bytes = image.decoded_bytes();
                if entry.bytes > self.limits.max_ready_bytes {
                    Picture::Failed(ImageFailure::TooLarge {
                        limit: self.limits.max_ready_bytes,
                    })
                } else {
                    let mime = match image.format {
                        ImageFormat::Png => "image/png",
                        ImageFormat::Jpeg => "image/jpeg",
                        ImageFormat::Webp => "image/webp",
                        ImageFormat::Gif => "image/gif",
                    };
                    let source = format!(
                        "data:{mime};base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(&image.bytes)
                    );
                    self.ready.push_back(completion.key.clone());
                    Picture::Ready {
                        source,
                        width: image.width,
                        height: image.height,
                    }
                }
            }
            Err(error) => Picture::Failed(error),
        };
        true
    }

    /// Releases the oldest decoded pictures until both budgets hold. A released
    /// picture keeps its entry so a frame that still draws the block does not
    /// queue the same load again on every drain.
    fn evict(&mut self) -> bool {
        let mut changed = false;
        while self.ready.len() > self.limits.max_ready_images
            || self.ready_bytes() > self.limits.max_ready_bytes
        {
            let Some(key) = self.ready.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.picture = Picture::Released;
                entry.bytes = 0;
                entry.started = false;
                changed = true;
            }
        }
        changed
    }

    fn ready_bytes(&self) -> usize {
        self.entries
            .values()
            .map(|entry| {
                if matches!(entry.picture, Picture::Ready { .. }) {
                    entry.bytes
                } else {
                    0
                }
            })
            .sum()
    }

    pub fn has_work(&self) -> bool {
        self.entries.values().any(|e| e.picture == Picture::Pending)
    }

    /// Hiding preserves completed images and drops unfinished demand; pending
    /// results get a new scope so they cannot return into the hidden reader.
    /// Released pictures are dropped, so showing the reader again demands them.
    pub fn suspend(&mut self) {
        if let Some(provider) = &self.provider {
            provider.cancel(self.scope);
        }
        self.scope.generation = self
            .scope
            .generation
            .checked_add(1)
            .expect("reader generation exhausted");
        self.entries
            .retain(|_, e| matches!(e.picture, Picture::Ready { .. }));
        self.ready.retain(|key| self.entries.contains_key(key));
        self.sink.drain();
    }
}

impl Drop for ImageBridge {
    fn drop(&mut self) {
        self.retire();
    }
}

/// Resolves a Markdown picture address against the document's fixed context.
///
/// The same refusal boundary as links applies: relative and root paths stay in
/// the repository at the fixed commit, `http:` is refused because external
/// pictures are HTTPS-only, and local/data/protocol-relative sources never
/// become a request.
pub fn resolve(raw: &str, context: &ResourceContext) -> Result<ImageTarget, ImageFailure> {
    match links::resolve(raw, context) {
        LinkTarget::Repository(link) => Ok(ImageTarget::Repository {
            owner: link.owner,
            name: link.name,
            commit: link.commit,
            path: link.path,
        }),
        LinkTarget::External { url }
            if url
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://")) =>
        {
            Ok(ImageTarget::External { url })
        }
        LinkTarget::External { .. } => Err(ImageFailure::Refused(ImageRefusal::InsecureExternal)),
        LinkTarget::Anchor { .. } => Err(ImageFailure::Refused(ImageRefusal::Empty)),
        LinkTarget::Refused { reason, .. } => Err(ImageFailure::Refused(match reason {
            RefusalReason::Empty => ImageRefusal::Empty,
            RefusalReason::Scheme => ImageRefusal::LocalOrUnsupportedScheme,
            RefusalReason::ProtocolRelative => ImageRefusal::ProtocolRelative,
            RefusalReason::InvalidPath => ImageRefusal::InvalidPath,
        })),
    }
}
