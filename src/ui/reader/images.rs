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

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui::{
    AnyElement, Image, ImageFormat as GpuiImageFormat, ImageSource, IntoElement, ObjectFit,
    ParentElement, Styled, div, img, prelude::*, px,
};

use crate::content::images::{
    ImageCompletion, ImageFailure, ImageFormat, ImageKey, ImageLimits, ImageRequest, ImageScope,
    ImageSink, ImageTarget,
};

use super::{LinkTarget, ResourceContext, links};

/// Vertical space reserved while a picture loads or has failed.
const PLACEHOLDER_HEIGHT: f32 = 140.0;

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
    Ready(Arc<Image>, u32, u32),
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
pub(crate) struct ImageBoard {
    limits: ImageLimits,
    /// This reader's own identity, so a shared provider never mixes two
    /// readers' work even when their document generations coincide.
    reader: u64,
    generation: u64,
    attached: bool,
    states: HashMap<ImageKey, ImageState>,
    ready_order: VecDeque<ImageKey>,
    ready_bytes: usize,
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
            limits,
            reader: crate::content::images::next_reader_id(),
            generation: 0,
            attached: false,
            states: HashMap::new(),
            ready_order: VecDeque::new(),
            ready_bytes: 0,
            pending: VecDeque::new(),
            in_flight: 0,
            blocks: HashMap::new(),
            sink: ImageSink::new(),
        }
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
        self.forget_unfinished();
    }

    /// Starts a new document generation, dropping every picture of the old one.
    pub(super) fn begin(&mut self, generation: u64) {
        self.generation = generation;
        self.states.clear();
        self.ready_order.clear();
        self.ready_bytes = 0;
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
        self.forget_unfinished();
    }

    pub(super) fn attached(&self) -> bool {
        self.attached
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
            return state.clone();
        }
        if !self.attached {
            return ImageState::Unavailable;
        }
        self.states.insert(key.clone(), ImageState::Loading);
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
        self.pending.clear();
        self.in_flight = 0;
        self.blocks.clear();
        self.states.retain(|_, state| state.is_ready());
        self.ready_order.retain(|key| self.states.contains_key(key));
    }

    /// Whether anything is queued or outstanding. The pump stops when this is
    /// false, so a settled document runs no background timer.
    pub(super) fn work_outstanding(&self) -> bool {
        self.in_flight > 0 || !self.pending.is_empty()
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
        let Some(state) = self.states.get_mut(&key) else {
            // The source was dropped by a document/attachment change.
            return Vec::new();
        };
        match completion.result {
            Ok(decoded) => {
                let format = gpui_format(decoded.format);
                let bytes = decoded.decoded_bytes();
                let image = Arc::new(Image::from_bytes(format, decoded.bytes));
                *state = ImageState::Ready(image, decoded.width, decoded.height);
                self.ready_bytes = self.ready_bytes.saturating_add(bytes);
                self.ready_order.retain(|known| known != &key);
                self.ready_order.push_back(key.clone());
            }
            Err(failure) => *state = ImageState::Failed(failure),
        }
        self.blocks.get(&key).cloned().unwrap_or_default()
    }

    /// Releases the oldest decoded pictures until the budget holds again.
    /// Returns the top-level blocks whose picture shrank to a placeholder, so
    /// their rows are measured at the new height instead of keeping the old one.
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
            if let ImageState::Ready(_, width, height) = state {
                let bytes = (*width as usize)
                    .saturating_mul(*height as usize)
                    .saturating_mul(4);
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

/// The theme-derived styling one picture block needs.
#[derive(Clone, Copy)]
pub(super) struct PictureStyle<'a> {
    pub colors: &'a ely_core::theme::Palette,
    pub small_text: f32,
    pub radius: gpui::Rems,
}

/// Draws one picture block in its current state, always keeping the alt text.
pub(super) fn image_element(
    id: gpui::ElementId,
    selector: String,
    alt: &str,
    state: &ImageState,
    hints: ImageHints,
    style: PictureStyle<'_>,
    frame_selector: String,
) -> AnyElement {
    let PictureStyle {
        colors,
        small_text: sm,
        radius,
    } = style;
    let note = |text: String| {
        div()
            .debug_selector({
                let selector = selector.clone();
                move || selector.clone()
            })
            .px_3()
            .py_2()
            .rounded(radius)
            .bg(colors.sunken)
            .text_size(gpui::px(sm))
            .text_color(colors.fg_muted)
            .child(text)
    };
    let caption = |alt: &str| {
        let alt = alt.to_string();
        (!alt.is_empty()).then(|| {
            div()
                .text_size(gpui::px(sm))
                .text_color(colors.fg_subtle)
                .child(alt)
        })
    };
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
            let picture = img(ImageSource::Image(image.clone()))
                .id(id)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Contain);
            // Left-aligned like GitHub's README rendering: the picture, then
            // its alt text as a caption in the same column.
            div()
                .debug_selector(move || selector.clone())
                .w_full()
                .flex()
                .flex_col()
                .items_start()
                .gap_1()
                .child(frame.child(picture))
                .children(caption(alt))
                .into_any_element()
        }
        ImageState::Loading => {
            let text = if alt.is_empty() {
                "Loading picture…".to_string()
            } else {
                format!("{alt} — loading picture…")
            };
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .h(px(PLACEHOLDER_HEIGHT))
                        .rounded(radius)
                        .bg(colors.sunken)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(gpui::px(sm))
                        .text_color(colors.fg_subtle)
                        .child("…"),
                )
                .child(note(text))
                .into_any_element()
        }
        ImageState::Unavailable => note(if alt.is_empty() {
            "Picture not loaded — no image service is attached.".to_string()
        } else {
            format!("{alt} — picture not loaded (no image service is attached)")
        })
        .into_any_element(),
        ImageState::Released => note(if alt.is_empty() {
            "Picture released to keep memory bounded.".to_string()
        } else {
            format!("{alt} — picture released to keep memory bounded")
        })
        .into_any_element(),
        ImageState::Failed(failure) => note(if alt.is_empty() {
            format!("Picture not loaded — {}.", failure.label())
        } else {
            format!("{alt} — picture not loaded ({})", failure.label())
        })
        .into_any_element(),
    }
}

/// A debug selector for a picture block, stable per top-level block.
pub(super) fn image_selector(ix: usize) -> String {
    format!("markdown-picture-note-{ix}")
}

/// The selector of a ready picture's frame, for layout checks.
pub(super) fn frame_selector(ix: usize) -> String {
    format!("markdown-picture-frame-{ix}")
}

#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
