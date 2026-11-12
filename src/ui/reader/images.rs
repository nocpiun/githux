//! Reader-side picture state and layout.
//!
//! The reader never fetches or decodes: [`MarkdownView`](super::markdown)
//! records which pictures a visible block wants, and the `ReaderView` pump
//! hands those requests to the host-supplied [`ImageProvider`]. Results come
//! back through an [`ImageSink`] and are applied only when they belong to the
//! current document generation.
//!
//! State is per reader and per document generation, so two readers showing the
//! same picture have independent loads, independent cancellation and
//! independent memory. The reader keeps a bounded number of decoded pictures;
//! older ones are released rather than accumulated.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, Image, ImageFormat as GpuiImageFormat, ImageSource, IntoElement, ObjectFit,
    ParentElement, Styled, div, img, prelude::*, px,
};

use crate::content::images::{
    ImageCompletion, ImageFailure, ImageFormat, ImageKey, ImageLimits, ImageRenderOptions,
    ImageRequest, ImageScope, ImageSink, ImageTarget,
};

use super::{LinkTarget, ResourceContext, links};

/// Dimensions an author asked for through HTML `width`/`height` attributes, in
/// CSS pixels. `None` means "use the picture's intrinsic size".
///
/// Markdown `![...](...)` images carry no hints; only parsed HTML `<img>`
/// attributes can request a size, so intrinsic-only pictures and explicit
/// requests stay distinguishable all the way to the renderer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImageHints {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl ImageHints {
    pub(crate) fn is_empty(&self) -> bool {
        self.width.is_none() && self.height.is_none()
    }
}

/// What the reader currently knows about one picture.
#[derive(Clone)]
pub(super) enum ImageState {
    /// A provider is fetching or decoding it.
    Loading,
    /// Ready to draw, with its intrinsic pixel size.
    Ready(ImageSource, u32, u32),
    /// The load failed or the address was refused.
    Failed(ImageFailure),
    /// Released to keep the document's decoded-picture budget.
    Released,
    /// No image provider is attached to this reader.
    Unavailable,
}

impl ImageState {
    pub(super) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(..))
    }

    /// A short status for tests and previews.
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Ready(..) => "ready",
            Self::Failed(failure) => failure.label(),
            Self::Released => "released",
            Self::Unavailable => "no image service attached",
        }
    }
}

/// The shared per-reader picture state. `ReaderView` owns it and hands a clone
/// to each `MarkdownView` it builds.
struct Playback {
    started: Option<std::time::Instant>,
    accepted: std::time::Instant,
    last_visible: Option<std::time::Instant>,
    duration_ms: u32,
    frames: Vec<(u32, ImageSource)>,
    final_image: ImageSource,
    frame: usize,
    extra_bytes: usize,
}

pub(crate) struct ImageBoard {
    animations: HashMap<ImageKey, Playback>,
    limits: ImageLimits,
    render_options: ImageRenderOptions,
    completed_options: HashMap<ImageKey, ImageRenderOptions>,
    attempted_options: HashMap<ImageKey, ImageRenderOptions>,
    density_dependent: HashSet<ImageKey>,
    /// This reader's own identity, so a shared provider never mixes two
    /// readers' work even when their document generations coincide.
    reader: u64,
    generation: u64,
    attached: bool,
    states: HashMap<ImageKey, ImageState>,
    ready_order: VecDeque<ImageKey>,
    ready_bytes: usize,
    ready_costs: HashMap<ImageKey, usize>,
    pending: VecDeque<ImageRequest>,
    /// Requests handed to the provider that have not completed or been
    /// cancelled. Their completions arrive through the sink.
    in_flight: usize,
    /// Picture key -> the top-level blocks that want it. A quote, list or
    /// footnote can hold several distinct pictures, so a single key per block
    /// would drop the others and leave their rows at the wrong height.
    blocks: HashMap<ImageKey, Vec<usize>>,
    sink: ImageSink,
}

impl ImageBoard {
    pub(super) fn new(limits: ImageLimits) -> Self {
        Self {
            animations: HashMap::new(),
            limits,
            render_options: ImageRenderOptions::default(),
            completed_options: HashMap::new(),
            attempted_options: HashMap::new(),
            density_dependent: HashSet::new(),
            reader: crate::content::images::next_reader_id(),
            generation: 0,
            attached: false,
            states: HashMap::new(),
            ready_order: VecDeque::new(),
            ready_bytes: 0,
            ready_costs: HashMap::new(),
            pending: VecDeque::new(),
            in_flight: 0,
            blocks: HashMap::new(),
            sink: ImageSink::new(),
        }
    }

    pub(super) fn painted(&mut self, key: &ImageKey) {
        if let Some(playback) = self.animations.get_mut(key) {
            let now = std::time::Instant::now();
            playback.started.get_or_insert(now);
            playback.last_visible = Some(now);
        }
    }

    pub(super) fn animate(&mut self, reduced_motion: bool) -> bool {
        let now = std::time::Instant::now();
        let mut finished = Vec::new();
        let mut changed = false;
        for (key, playback) in &mut self.animations {
            if reduced_motion {
                finished.push(key.clone());
                continue;
            }
            let Some(started) = playback.started else {
                if now.duration_since(playback.accepted).as_millis() >= 150 {
                    finished.push(key.clone());
                }
                continue;
            };
            if playback
                .last_visible
                .is_none_or(|at| now.duration_since(at).as_millis() >= 150)
            {
                finished.push(key.clone());
                continue;
            }
            let elapsed = now.duration_since(started).as_millis() as u32;
            if reduced_motion || elapsed >= playback.duration_ms {
                finished.push(key.clone());
                continue;
            }
            let index = playback
                .frames
                .partition_point(|(at, _)| *at <= elapsed)
                .saturating_sub(1);
            if playback.frame != index {
                playback.frame = index;
                if let Some(ImageState::Ready(image, ..)) = self.states.get_mut(key) {
                    *image = playback.frames[index].1.clone();
                    changed = true;
                }
            }
        }
        for key in finished {
            self.finish_animation(&key);
            changed = true;
        }
        changed
    }
    fn finish_animation(&mut self, key: &ImageKey) {
        if let Some(playback) = self.animations.remove(key) {
            if let Some(ImageState::Ready(image, ..)) = self.states.get_mut(key) {
                *image = playback.final_image;
            }
            self.ready_bytes = self.ready_bytes.saturating_sub(playback.extra_bytes);
            if let Some(cost) = self.ready_costs.get_mut(key) {
                *cost = cost.saturating_sub(playback.extra_bytes);
            }
        }
    }
    fn finish_animations(&mut self) {
        for key in self.animations.keys().cloned().collect::<Vec<_>>() {
            self.finish_animation(&key);
        }
    }

    pub(super) fn render_options(&self) -> ImageRenderOptions {
        self.render_options
    }
    pub(super) fn set_render_options(&mut self, options: ImageRenderOptions) {
        self.render_options = options;
    }
    /// Whether this generation already holds drawable pixels for `key`.
    ///
    /// Only a `Ready` picture paints; loading, failed, unavailable and released
    /// sources all draw nothing, so a block made only of them keeps no space.
    pub(super) fn is_ready(&self, key: &ImageKey) -> bool {
        self.states.get(key).is_some_and(ImageState::is_ready)
    }

    pub(super) fn sink(&self) -> ImageSink {
        self.sink.clone()
    }

    pub(super) fn scope(&self) -> ImageScope {
        ImageScope {
            reader: self.reader,
            generation: self.generation,
        }
    }

    pub(super) fn limits(&self) -> &ImageLimits {
        &self.limits
    }

    /// Advances the generation while keeping finished pictures: a suspension
    /// stops network work without forgetting what is already drawn, and a late
    /// completion of the previous generation is rejected.
    pub(super) fn advance(&mut self, generation: u64) {
        self.generation = generation;
        self.finish_animations();
        self.forget_unfinished();
    }

    /// Starts a new document generation, dropping every picture of the old one.
    pub(super) fn begin(&mut self, generation: u64) {
        self.generation = generation;
        self.states.clear();
        self.completed_options.clear();
        self.attempted_options.clear();
        self.density_dependent.clear();
        self.animations.clear();
        self.ready_order.clear();
        self.ready_bytes = 0;
        self.ready_costs.clear();
        self.pending.clear();
        self.in_flight = 0;
        self.blocks.clear();
    }

    /// Records whether a provider is attached. Changing the attachment drops
    /// every non-ready picture so the next frame requests it again. Use
    /// [`Self::forget_unfinished`] when the provider changed but stays attached.
    pub(super) fn set_attached(&mut self, attached: bool) {
        if self.attached == attached {
            return;
        }
        self.attached = attached;
        self.forget_unfinished();
    }

    /// Drops unfinished loads and requests while keeping finished pictures, so
    /// a hidden window stops networking without forgetting what is drawn.
    pub(super) fn suspend(&mut self) {
        self.finish_animations();
        self.forget_unfinished();
    }

    /// Called while building a visible image block. Returns the state to draw
    /// and queues the request the first time this source is wanted.
    pub(super) fn want(&mut self, block: usize, target: ImageTarget) -> ImageState {
        let key = target.key();
        let blocks = self.blocks.entry(key.clone()).or_default();
        if !blocks.contains(&block) {
            blocks.push(block);
        }
        if let Some(state) = self.states.get(&key) {
            let state = state.clone();
            if self.attached
                && state.is_ready()
                && self.density_dependent.contains(&key)
                && self
                    .attempted_options
                    .get(&key)
                    .is_none_or(|known| *known < self.render_options)
            {
                self.attempted_options
                    .insert(key.clone(), self.render_options);
                self.pending.push_back(ImageRequest {
                    scope: self.scope(),
                    key,
                    target,
                });
            }
            return state;
        }
        if !self.attached {
            // A valid source may precede provider attachment. Keep its pending
            // visuals collapsed without registering work that cannot run yet.
            return ImageState::Loading;
        }
        self.states.insert(key.clone(), ImageState::Loading);
        self.attempted_options
            .insert(key.clone(), self.render_options);
        self.pending.push_back(ImageRequest {
            scope: self.scope(),
            key,
            target,
        });
        ImageState::Loading
    }

    /// Takes queued requests, bounded by the reader's in-flight budget. Each
    /// taken request counts as in flight until it completes or a generation
    /// change drops it.
    pub(super) fn take_pending(&mut self, max: usize) -> Vec<ImageRequest> {
        let free = max.saturating_sub(self.in_flight);
        let mut taken = Vec::new();
        while taken.len() < free {
            match self.pending.pop_front() {
                Some(request) => taken.push(request),
                None => break,
            }
        }
        self.in_flight += taken.len();
        taken
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Dropped by a provider replacement, a document change or suspension.
    /// A block that still wants a picture asks again on the next frame.
    pub(super) fn forget_unfinished(&mut self) {
        self.finish_animations();
        self.pending.clear();
        self.in_flight = 0;
        self.blocks.clear();
        self.states.retain(|_, state| state.is_ready());
        self.attempted_options = self.completed_options.clone();
        self.ready_order.retain(|key| self.states.contains_key(key));
    }

    /// Whether anything is queued or outstanding. The pump stops when this is
    /// false, so a settled document runs no background timer.
    pub(super) fn work_outstanding(&self) -> bool {
        self.in_flight > 0 || !self.pending.is_empty() || !self.animations.is_empty()
    }

    pub(super) fn clear_pending(&mut self) {
        self.pending.clear();
        self.in_flight = 0;
    }

    /// Applies one completion. Returns the top-level blocks that need a fresh
    /// measurement because their picture changed size.
    pub(super) fn apply(&mut self, completion: ImageCompletion) -> Vec<usize> {
        // Reject a stale completion before touching the current generation's
        // accounting: `begin`/`forget_unfinished` reset the count, so an old
        // completion arriving afterwards must not consume a current slot (that
        // would stop the pump while a live request is still outstanding).
        if completion.scope != self.scope() {
            return Vec::new();
        }
        self.in_flight = self.in_flight.saturating_sub(1);
        let key = completion.key;
        if self.states.get(&key).is_some_and(ImageState::is_ready) {
            self.finish_animation(&key);
        }
        let Some(state) = self.states.get_mut(&key) else {
            // The source was dropped by a document/attachment change.
            return Vec::new();
        };
        let upgrading = state.is_ready();
        match completion.result {
            Ok(decoded) => {
                let format = gpui_format(decoded.format);
                let mut bytes = decoded.decoded_bytes();
                // Input rasters retain their header dimensions. Only a scaled
                // SVG grid or finite frame bank can benefit from rerasterizing;
                // do not refetch PNGs or restart GIF playback on a DPI change.
                let native = (decoded.width as usize)
                    .saturating_mul(decoded.height as usize)
                    .saturating_mul(4);
                if decoded.animation.is_some() || bytes > native {
                    self.density_dependent.insert(key.clone());
                } else {
                    self.density_dependent.remove(&key);
                }
                let mut displayed;
                if let Some(animation) = decoded.animation {
                    let final_cost = animation.final_pixels.bgra.len();
                    let image = decoded_pixels_source(animation.final_pixels);
                    displayed = image.clone();
                    let frames = animation
                        .frames
                        .into_iter()
                        .map(|f| (f.at_ms, decoded_pixels_source(f.pixels)))
                        .collect::<Vec<_>>();
                    if upgrading {
                        bytes = final_cost;
                    }
                    if let Some((_, first)) = frames.first().filter(|_| !upgrading) {
                        displayed = first.clone();
                        self.animations.insert(
                            key.clone(),
                            Playback {
                                started: None,
                                accepted: std::time::Instant::now(),
                                last_visible: None,
                                duration_ms: animation.duration_ms,
                                frame: 0,
                                extra_bytes: bytes.saturating_sub(final_cost),
                                frames,
                                final_image: image,
                            },
                        );
                    }
                } else {
                    displayed =
                        ImageSource::Image(Arc::new(Image::from_bytes(format, decoded.bytes)));
                }
                *state = ImageState::Ready(displayed, decoded.width, decoded.height);
                self.completed_options
                    .insert(key.clone(), self.render_options);
                self.ready_bytes = self
                    .ready_bytes
                    .saturating_sub(self.ready_costs.get(&key).copied().unwrap_or_default());
                self.ready_costs.insert(key.clone(), bytes);
                self.ready_bytes = self.ready_bytes.saturating_add(bytes);
                self.ready_order.retain(|known| known != &key);
                self.ready_order.push_back(key.clone());
            }
            Err(failure) => {
                *state = ImageState::Failed(failure);
                self.ready_bytes = self
                    .ready_bytes
                    .saturating_sub(self.ready_costs.remove(&key).unwrap_or_default());
                self.ready_order.retain(|entry| entry != &key);
                self.completed_options.remove(&key);
            }
        }
        self.blocks.get(&key).cloned().unwrap_or_default()
    }

    /// Releases the oldest decoded pictures until the budget holds again.
    /// Returns the top-level blocks whose picture disappeared, so their rows are
    /// measured at the new height instead of keeping the released box.
    pub(super) fn evict(&mut self) -> Vec<usize> {
        let mut affected = Vec::new();
        while self.ready_order.len() > self.limits.max_ready_images
            || self.ready_bytes > self.limits.max_ready_bytes
        {
            let Some(key) = self.ready_order.pop_front() else {
                break;
            };
            let Some(state) = self.states.get_mut(&key) else {
                continue;
            };
            if let ImageState::Ready(..) = state {
                let bytes = self.ready_costs.remove(&key).unwrap_or_default();
                self.animations.remove(&key);
                self.ready_bytes = self.ready_bytes.saturating_sub(bytes);
                *state = ImageState::Released;
                if let Some(blocks) = self.blocks.get(&key) {
                    affected.extend(blocks.iter().copied());
                }
            }
        }
        affected.sort_unstable();
        affected.dedup();
        affected
    }

    #[cfg(test)]
    pub(super) fn state(&self, key: &ImageKey) -> Option<ImageState> {
        self.states.get(key).cloned()
    }

    #[cfg(test)]
    pub(super) fn known_states(&self) -> usize {
        self.states.len()
    }

    #[cfg(test)]
    pub(super) fn in_flight_debug(&self) -> usize {
        self.in_flight
    }

    #[cfg(test)]
    pub(super) fn blocks_for_test(&self) -> HashMap<ImageKey, Vec<usize>> {
        self.blocks.clone()
    }

    #[cfg(test)]
    pub(super) fn debug_states(&self) -> Vec<(String, &'static str)> {
        let mut states: Vec<(String, &'static str)> = self
            .states
            .iter()
            .map(|(key, state)| (key.as_str().to_string(), state.label()))
            .collect();
        states.sort();
        states
    }
}

fn decoded_pixels_source(pixels: crate::content::images::DecodedPixels) -> ImageSource {
    let buffer = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.bgra)
        .expect("validated BGRA frame dimensions");
    ImageSource::Render(Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
        buffer,
    )])))
}

fn gpui_format(format: ImageFormat) -> GpuiImageFormat {
    match format {
        ImageFormat::Png => GpuiImageFormat::Png,
        ImageFormat::Jpeg => GpuiImageFormat::Jpeg,
        ImageFormat::Webp => GpuiImageFormat::Webp,
        ImageFormat::Gif => GpuiImageFormat::Gif,
    }
}

/// Resolves a Markdown picture address against the document's fixed context.
///
/// The same refusal boundary as links applies: relative and root paths stay in
/// the repository at the fixed commit, `http:` is refused because external
/// pictures are HTTPS-only, and local/data/protocol-relative sources never
/// become a request.
pub(super) fn resolve_target(
    raw: &str,
    context: &ResourceContext,
) -> Result<ImageTarget, ImageFailure> {
    match links::resolve(raw, context) {
        LinkTarget::Repository(link) => Ok(ImageTarget::Repository {
            owner: link.owner,
            name: link.name,
            commit: link.commit,
            path: link.path,
        }),
        LinkTarget::External { url } => {
            if url
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
            {
                Ok(ImageTarget::External { url })
            } else {
                Err(ImageFailure::Refused(
                    crate::content::images::ImageRefusal::InsecureExternal,
                ))
            }
        }
        LinkTarget::Anchor { .. } => Err(ImageFailure::Refused(
            crate::content::images::ImageRefusal::Empty,
        )),
        LinkTarget::Refused { reason, .. } => Err(ImageFailure::Refused(match reason {
            links::RefusalReason::Empty => crate::content::images::ImageRefusal::Empty,
            links::RefusalReason::Scheme => {
                crate::content::images::ImageRefusal::LocalOrUnsupportedScheme
            }
            links::RefusalReason::ProtocolRelative => {
                crate::content::images::ImageRefusal::ProtocolRelative
            }
            links::RefusalReason::InvalidPath => crate::content::images::ImageRefusal::InvalidPath,
        })),
    }
}

/// Draws one picture block: real pixels when ready, nothing at all otherwise.
///
/// Owner direction (2026-11-12): an image that cannot be displayed shows no
/// failure placeholder, alt caption, diagnostic label or reserved panel. Alt
/// text stays in the document/copy model and failure reasons stay internal
/// state, so a missing or unreachable picture is simply absent from the page.
/// Every non-ready state therefore draws an empty zero-size element; callers
/// are expected to drop it from their row so it cannot contribute gaps or
/// height, and to remeasure the containing block when the state changes.
pub(super) fn image_element(
    id: gpui::ElementId,
    selector: String,
    state: &ImageState,
    hints: ImageHints,
    frame_selector: String,
) -> AnyElement {
    match state {
        ImageState::Ready(image, width, height) => {
            // The real pixels are drawn at their intrinsic or author-requested
            // size, never upscaled and never wrapped in a placeholder panel. A
            // ready picture keeps no background, no rounding and no forced
            // frame: success is pixels and alpha only.
            //
            // The box only reserves the displayed area (its width and aspect
            // ratio), so a picture wider than the reading column shrinks to
            // fit while a small picture keeps its own size. HTML `width`/
            // `height` hints are honoured but still bounded by the intrinsic
            // dimensions, so an author can shrink a picture without upscaling
            // or distorting it.
            let intrinsic_width = *width as f32;
            let intrinsic_height = (*height as f32).max(1.0);
            let intrinsic_ratio = intrinsic_width / intrinsic_height;
            let requested_width = hints
                .width
                .map(|w| (w as f32).min(intrinsic_width).max(1.0));
            let requested_height = hints
                .height
                .map(|h| (h as f32).min(intrinsic_height).max(1.0));
            let (display_width, display_ratio) = match (requested_width, requested_height) {
                (Some(width), Some(height)) => (width, width / height),
                (Some(width), None) => (width, intrinsic_ratio),
                (None, Some(height)) => (height * intrinsic_ratio, intrinsic_ratio),
                (None, None) => (intrinsic_width, intrinsic_ratio),
            };
            let frame = div()
                .debug_selector(move || frame_selector.clone())
                .relative()
                .w_full()
                .max_w(px(display_width))
                .aspect_ratio(display_ratio);
            let picture = img(image.clone())
                .id(id)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Contain);
            // Retain #81's requested/intrinsic sizing and transparent frame.
            // A successful image has an inline-sized box and no forced caption.
            div()
                .debug_selector(move || selector.clone())
                .w(px(display_width))
                .max_w_full()
                .flex()
                .flex_col()
                .items_start()
                .child(frame.child(picture))
                .into_any_element()
        }
        // Loading/unrequested, failed/refused, unavailable and released all
        // render the same empty element: the page shows nothing where the
        // picture would be. The selector stays attached so tests and the
        // renderer can still find the position, but it has no size, content,
        // background, border or padding.
        ImageState::Loading
        | ImageState::Failed(_)
        | ImageState::Unavailable
        | ImageState::Released => div()
            .debug_selector(move || selector.clone())
            .w(px(0.0))
            .h(px(0.0))
            .overflow_hidden()
            .into_any_element(),
    }
}

/// Wraps a drawn picture so a finite SVG animation starts and keeps playing
/// only while the picture is actually on screen.
///
/// `visibility` is the board and source key the picture belongs to; without an
/// attached provider there is nothing to report. The window's content mask
/// excludes rows scrolled out of the viewport, so a picture that is built for
/// overdraw does not start playing off-screen.
pub(super) fn painted_when_visible(
    picture: AnyElement,
    visibility: Option<(Rc<RefCell<ImageBoard>>, ImageKey)>,
) -> AnyElement {
    div()
        .max_w_full()
        .child(picture)
        .on_children_prepainted(move |bounds, window, _| {
            if let Some((board, key)) = &visibility
                && bounds.iter().any(|bounds| {
                    let visible = window.content_mask().bounds.intersect(bounds);
                    visible.size.width > px(0.0) && visible.size.height > px(0.0)
                })
            {
                board.borrow_mut().painted(key);
            }
        })
        .into_any_element()
}

/// A debug selector for a drawn picture block, stable per top-level block.
///
/// It is attached only when the picture really paints, so its absence is the
/// evidence that a non-ready picture reserved no space.
pub(super) fn image_selector(ix: usize) -> String {
    format!("markdown-picture-{ix}")
}

/// The selector of a ready picture's frame, for layout checks.
pub(super) fn frame_selector(ix: usize) -> String {
    format!("markdown-picture-frame-{ix}")
}

#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
