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
    ImageCompletion, ImageEvent, ImageFailure, ImageFormat, ImageKey, ImageLimits,
    ImageRenderOptions, ImageRequest, ImageScope, ImageSink, ImageTarget, ImageUpdate,
    ImageValidity,
};

use super::{LinkTarget, ResourceContext, links};
#[path = "image_demand.rs"]
mod demand;
#[path = "image_measurement.rs"]
mod measurement;
#[path = "image_viewport.rs"]
mod viewport;
pub(super) use measurement::{PendingDemand, pending_container};
pub(super) use viewport::viewport;
#[path = "image_resources.rs"]
mod resources;
pub(crate) use resources::release_retired;
#[path = "image_publication.rs"]
mod publication;

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
    settled_animations: HashSet<ImageKey>,
    limits: ImageLimits,
    render_options: ImageRenderOptions,
    completed_options: HashMap<ImageKey, ImageRenderOptions>,
    attempted_options: HashMap<ImageKey, ImageRenderOptions>,
    density_dependent: HashSet<ImageKey>,
    static_rasters: HashSet<ImageKey>,
    /// This reader's own identity, so a shared provider never mixes two
    /// readers' work even when their document generations coincide.
    reader: u64,
    generation: u64,
    attached: bool,
    states: HashMap<ImageKey, ImageState>,
    ready_order: VecDeque<ImageKey>,
    ready_bytes: usize,
    peak_pixel_bytes: usize,
    evictions: u64,
    reloads: u64,
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
    requests: HashMap<ImageKey, (u64, u32)>,
    request_sinks: HashMap<ImageKey, ImageSink>,
    next_request: u64,
    validity: HashMap<ImageKey, ImageValidity>,
    geometry_managed: bool,
    viewport: Option<gpui::Bounds<gpui::Pixels>>,
    targets: HashMap<ImageKey, ImageTarget>,
    demand: HashMap<ImageKey, demand::Demand>,
    observations: HashMap<ImageKey, (u8, f32)>,
    observation_order: Vec<ImageKey>,
    use_clock: u64,
    sizes: HashMap<ImageKey, (u32, u32)>,
    grids: HashMap<ImageKey, u32>,
    requested_options: HashMap<ImageKey, ImageRenderOptions>,
    retired: HashMap<gpui::ImageId, Arc<gpui::RenderImage>>,
    retired_encoded: Vec<ImageSource>,
}

impl ImageBoard {
    pub(super) fn new(limits: ImageLimits) -> Self {
        Self {
            animations: HashMap::new(),
            settled_animations: HashSet::new(),
            limits,
            render_options: ImageRenderOptions::default(),
            completed_options: HashMap::new(),
            attempted_options: HashMap::new(),
            density_dependent: HashSet::new(),
            static_rasters: HashSet::new(),
            reader: crate::content::images::next_reader_id(),
            generation: 0,
            attached: false,
            states: HashMap::new(),
            ready_order: VecDeque::new(),
            ready_bytes: 0,
            peak_pixel_bytes: 0,
            evictions: 0,
            reloads: 0,
            ready_costs: HashMap::new(),
            pending: VecDeque::new(),
            in_flight: 0,
            blocks: HashMap::new(),
            sink: ImageSink::new(),
            requests: HashMap::new(),
            request_sinks: HashMap::new(),
            next_request: 0,
            validity: HashMap::new(),
            geometry_managed: false,
            viewport: None,
            targets: HashMap::new(),
            demand: HashMap::new(),
            observations: HashMap::new(),
            observation_order: Vec::new(),
            use_clock: 0,
            sizes: HashMap::new(),
            grids: HashMap::new(),
            requested_options: HashMap::new(),
            retired: HashMap::new(),
            retired_encoded: Vec::new(),
        }
    }

    pub(super) fn painted(&mut self, key: &ImageKey) {
        self.use_clock = self.use_clock.wrapping_add(1);
        self.demand.entry(key.clone()).or_default().last_used = self.use_clock;
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
            self.settled_animations.insert(key.clone());
            if let Some(ImageState::Ready(image, ..)) = self.states.get_mut(key) {
                *image = playback.final_image;
            }
            for (_, frame) in playback.frames {
                self.retire_source(frame);
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
    /// Only a `Ready` picture paints. Layout remembers measured geometry for
    /// evicted resources so scrolling stays stable while their pixels reload.
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
        self.retire_all();
        self.cancel_request_sinks();
        self.generation = generation;
        self.states.clear();
        self.requests.clear();
        self.validity.clear();
        self.targets.clear();
        self.demand.clear();
        self.observations.clear();
        self.observation_order.clear();
        self.sizes.clear();
        self.grids.clear();
        self.requested_options.clear();
        self.completed_options.clear();
        self.attempted_options.clear();
        self.density_dependent.clear();
        self.static_rasters.clear();
        self.animations.clear();
        self.settled_animations.clear();
        self.ready_order.clear();
        self.ready_bytes = 0;
        self.peak_pixel_bytes = 0;
        self.evictions = 0;
        self.reloads = 0;
        self.ready_costs.clear();
        self.pending.clear();
        self.in_flight = 0;
        self.blocks.clear();
    }

    /// Records whether a provider is attached, dropping unfinished requests.
    /// Measured geometry survives so reattachment does not shift the document.
    /// Use [`Self::forget_unfinished`] when the provider stays attached.
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
        self.targets.insert(key.clone(), target.clone());
        let blocks = self.blocks.entry(key.clone()).or_default();
        if !blocks.contains(&block) {
            blocks.push(block);
        }
        if let Some(state) = self.states.get(&key) {
            let state = state.clone();
            if !self.geometry_managed
                && self.attached
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
        if self.geometry_managed {
            return ImageState::Loading;
        }
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
        if self.geometry_managed {
            let demand = &self.demand;
            let pending = self.pending.make_contiguous();
            pending.sort_by_key(|request| {
                let priority = demand.get(&request.key).map_or(0, |d| d.priority);
                let refinement = self
                    .states
                    .get(&request.key)
                    .is_some_and(ImageState::is_ready);
                (std::cmp::Reverse(priority), refinement)
            });
        }
        let free = max.saturating_sub(self.in_flight);
        let mut taken = Vec::new();
        while taken.len() < free {
            match self.pending.pop_front() {
                Some(request) => {
                    self.next_request = self.next_request.wrapping_add(1).max(1);
                    self.requests
                        .insert(request.key.clone(), (self.next_request, 0));
                    self.request_sinks.insert(
                        request.key.clone(),
                        self.sink.for_request(self.next_request),
                    );
                    taken.push(request);
                }
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
        self.cancel_request_sinks();
        self.pending.clear();
        self.requests.clear();
        self.in_flight = 0;
        self.blocks.clear();
        let managed = self.geometry_managed;
        self.states.retain(|_, state| {
            state.is_ready() || (managed && matches!(state, ImageState::Released))
        });
        if managed {
            // A new scope permits one fresh attempt without losing measured
            // document positions (including on DPI change and resumption).
            for demand in self.demand.values_mut() {
                demand.priority = 0;
            }
        }
        self.attempted_options = self.completed_options.clone();
        self.ready_order.retain(|key| self.states.contains_key(key));
    }

    /// Whether anything is queued or outstanding. The pump stops when this is
    /// false, so a settled document runs no background timer.
    pub(super) fn active_work(&self) -> bool {
        self.in_flight > 0 || !self.pending.is_empty() || !self.animations.is_empty()
    }
    pub(super) fn work_outstanding(&self) -> bool {
        self.active_work() || self.validity.values().any(|guard| guard.expires.is_some())
    }
    pub(super) fn next_wake(&self) -> std::time::Duration {
        if self.active_work() {
            return std::time::Duration::from_millis(20);
        }
        self.validity
            .values()
            .filter_map(|guard| guard.expires)
            .min()
            .map_or(std::time::Duration::from_millis(20), |at| {
                at.saturating_duration_since(std::time::Instant::now())
                    .max(std::time::Duration::from_millis(1))
            })
    }

    pub(super) fn clear_pending(&mut self) {
        self.cancel_request_sinks();
        self.pending.clear();
        self.requests.clear();
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
            if let Some(ImageState::Ready(image, ..)) = self.states.get(&key) {
                self.retire_source(image.clone());
            }
        }
        let Some(state) = self.states.get_mut(&key) else {
            // The source was dropped by a document/attachment change.
            return Vec::new();
        };
        let upgrading = state.is_ready() || self.settled_animations.contains(&key);
        match completion.result {
            Ok(decoded) => {
                let format = gpui_format(decoded.format);
                let mut bytes = decoded.decoded_bytes();
                self.sizes
                    .insert(key.clone(), (decoded.width, decoded.height));
                let grid = decoded
                    .pixels
                    .as_ref()
                    .map(|p| p.width)
                    .or_else(|| decoded.animation.as_ref().map(|a| a.final_pixels.width))
                    .unwrap_or(decoded.width);
                if decoded.pixels.is_some() && !decoded.source_svg {
                    self.static_rasters.insert(key.clone());
                } else {
                    self.static_rasters.remove(&key);
                }
                self.grids.insert(key.clone(), grid);
                // Header dimensions remain logical layout dimensions. Display
                // grids and finite SVG banks can be rebuilt from original data
                // without changing that geometry or looping GIF/WebP playback.
                let native = (decoded.width as usize)
                    .saturating_mul(decoded.height as usize)
                    .saturating_mul(4);
                if decoded.raster_frames.is_empty()
                    && (decoded.animation.is_some() || bytes != native)
                {
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
                } else if !decoded.raster_frames.is_empty() {
                    let frames = decoded
                        .raster_frames
                        .into_iter()
                        .map(|frame| {
                            let pixels = frame.pixels;
                            let buffer = image::RgbaImage::from_raw(
                                pixels.width,
                                pixels.height,
                                pixels.bgra,
                            )
                            .expect("validated raster frame dimensions");
                            image::Frame::from_parts(
                                buffer,
                                0,
                                0,
                                image::Delay::from_numer_denom_ms(frame.delay.0, frame.delay.1),
                            )
                        })
                        .collect::<Vec<_>>();
                    displayed = ImageSource::Render(Arc::new(gpui::RenderImage::new(frames)));
                } else if let Some(pixels) = decoded.pixels {
                    displayed = decoded_pixels_source(pixels);
                } else {
                    displayed =
                        ImageSource::Image(Arc::new(Image::from_bytes(format, decoded.bytes)));
                }
                *state = ImageState::Ready(displayed, decoded.width, decoded.height);
                self.completed_options.insert(
                    key.clone(),
                    self.requested_options
                        .get(&key)
                        .copied()
                        .unwrap_or(self.render_options),
                );
                self.ready_bytes = self
                    .ready_bytes
                    .saturating_sub(self.ready_costs.get(&key).copied().unwrap_or_default());
                self.ready_costs.insert(key.clone(), bytes);
                self.ready_bytes = self.ready_bytes.saturating_add(bytes);
                self.peak_pixel_bytes = self.peak_pixel_bytes.max(self.ready_bytes);
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
            let candidate = self
                .ready_order
                .iter()
                .enumerate()
                .min_by_key(|(position, key)| {
                    let demand = self.demand.get(*key);
                    (
                        demand.map_or(0, |d| d.priority),
                        demand.map_or(0, |d| d.last_used),
                        *position,
                    )
                })
                .map(|(position, _)| position);
            let Some(key) = candidate.and_then(|position| self.ready_order.remove(position)) else {
                break;
            };
            self.finish_animation(&key);
            let Some(state) = self.states.get_mut(&key) else {
                continue;
            };
            if let ImageState::Ready(image, ..) = state {
                match image.clone() {
                    ImageSource::Render(image) => {
                        self.retired.insert(image.id, image);
                    }
                    source => self.retired_encoded.push(source),
                }
                let bytes = self.ready_costs.remove(&key).unwrap_or_default();
                self.animations.remove(&key);
                self.ready_bytes = self.ready_bytes.saturating_sub(bytes);
                *state = ImageState::Released;
                self.evictions += 1;
                if let Some(sink) = self.request_sinks.remove(&key) {
                    sink.cancel();
                }
                if self.requests.remove(&key).is_some() {
                    self.in_flight = self.in_flight.saturating_sub(1);
                }
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
    pub(super) fn memory_debug(&self) -> (usize, usize, u64, u64) {
        (
            self.ready_bytes,
            self.peak_pixel_bytes,
            self.evictions,
            self.reloads,
        )
    }
    #[cfg(test)]
    pub(super) fn grid_debug(&self, key: &ImageKey) -> Option<u32> {
        self.grids.get(key).copied()
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
            let (display_width, display_ratio) = measurement::dimensions(*width, *height, hints);
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
    measurement::measured(picture, visibility, true)
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
#[path = "image_performance_tests.rs"]
mod performance_tests;
#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
