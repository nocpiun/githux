//! Ordered publication identity shared by providers and the reader.
use super::{DecodedFrame, DecodedImage, ImageFailure, ImageKey, ImageScope};
use std::{fmt, sync::Arc, time::Instant};

#[derive(Clone)]
pub struct ImageValidity {
    check: Arc<dyn Fn() -> bool + Send + Sync>,
    pub expires: Option<Instant>,
}
impl ImageValidity {
    pub fn new(check: impl Fn() -> bool + Send + Sync + 'static, expires: Option<Instant>) -> Self {
        Self {
            check: Arc::new(check),
            expires,
        }
    }
    pub fn is_current(&self) -> bool {
        self.expires.is_none_or(|at| Instant::now() < at) && (self.check)()
    }
}
impl fmt::Debug for ImageValidity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageValidity")
            .field("expires", &self.expires)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum ImageUpdate {
    Ready(DecodedImage),
    Frames(Vec<DecodedFrame>),
    /// Same representation successfully validated; keep its pixels/timeline.
    Validated,
    /// An old provisional representation must be dropped before replacement.
    Discard,
    Finished,
}
#[derive(Debug)]
pub struct ImageEvent {
    pub scope: ImageScope,
    pub key: ImageKey,
    pub request: u64,
    pub phase: u32,
    pub terminal: bool,
    pub validity: Option<ImageValidity>,
    pub(crate) _admission: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
    pub result: Result<ImageUpdate, ImageFailure>,
}
