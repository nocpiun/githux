//! UI-independent adapter to the existing content capability. No URL loader or disk cache.
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

/// Must only perform atomic/in-memory checks, e.g. a captured CacheAuthorization plus session guard.
/// Capture the token belonging to this document's reply, never reacquire a new token here.
pub type CapabilityCheck = Arc<dyn Fn() -> bool + Send + Sync>;
#[derive(Debug, Clone, PartialEq)]
pub enum Picture {
    Pending,
    Ready {
        source: String,
        width: u32,
        height: u32,
    },
    Failed(ImageFailure),
}
struct Entry {
    picture: Picture,
    target: ImageTarget,
    started: bool,
    bytes: usize,
}
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
    pub fn is_current(&self) -> bool {
        !self.retired && (self.guard)()
    }
    /// Permanently retire the captured document capability. Reauthorization requires a new bridge.
    pub fn retire(&mut self) {
        if let Some(provider) = &self.provider {
            provider.cancel(self.scope);
        }
        self.retired = true;
        self.entries.clear();
        self.ready.clear();
        self.sink.drain();
    }
    /// Called while making an image element. Only queues work; never invokes a provider.
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
        // Bound metadata as well as decoded memory. Unseen off-screen images can be requested later.
        if !self.entries.contains_key(&key)
            && self.entries.len()
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
        self.entries
            .entry(key)
            .or_insert(Entry {
                picture: Picture::Pending,
                target,
                started: false,
                bytes: 0,
            })
            .picture
            .clone()
    }
    /// Host UI drain: validate the captured ticket, accept current completions, then start queued work.
    /// Returns whether presentation changed. Call once per host drain, including after cached replies.
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
        let active = self
            .entries
            .values()
            .filter(|e| e.started && e.picture == Picture::Pending)
            .count();
        let requests: Vec<_> = self
            .entries
            .iter_mut()
            .filter(|(_, e)| !e.started)
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
        while self.ready.len() > self.limits.max_ready_images
            || self
                .entries
                .values()
                .map(|e| {
                    if matches!(e.picture, Picture::Ready { .. }) {
                        e.bytes
                    } else {
                        0
                    }
                })
                .sum::<usize>()
                > self.limits.max_ready_bytes
        {
            if let Some(key) = self.ready.pop_front() {
                self.entries.remove(&key);
            } else {
                break;
            }
        }
        true
    }
    pub fn has_work(&self) -> bool {
        self.entries.values().any(|e| e.picture == Picture::Pending)
    }
    /// Hiding preserves completed images; pending results get a new scope and cannot return to it.
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
        self.sink.drain();
    }
}
impl Drop for ImageBridge {
    fn drop(&mut self) {
        self.retire();
    }
}

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
