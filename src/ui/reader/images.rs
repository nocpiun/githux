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

/// The narrowest displayed width/height ratio. A very tall picture is shown in
/// a box at least this wide relative to its height, letterboxed, so one image
/// cannot dominate the reading column.
const MIN_DISPLAY_RATIO: f32 = 0.25;
/// The widest displayed width/height ratio, for very wide banners.
const MAX_DISPLAY_RATIO: f32 = 12.0;
/// Vertical space reserved while a picture loads or has failed.
const PLACEHOLDER_HEIGHT: f32 = 140.0;
/// Largest height a ready picture occupies; taller pictures are letterboxed.
pub(super) const MAX_DISPLAY_HEIGHT: f32 = 640.0;

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
    /// Top-level block index -> key, for a targeted remeasure on completion.
    blocks: HashMap<usize, ImageKey>,
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
    /// every non-ready picture so the next frame requests it again.
    pub(super) fn set_attached(&mut self, attached: bool) {
        if self.attached == attached {
            return;
        }
        self.attached = attached;
        self.pending.clear();
        self.in_flight = 0;
        self.blocks.clear();
        self.states.retain(|_, state| state.is_ready());
        self.ready_order.retain(|key| self.states.contains_key(key));
    }

    /// Drops unfinished loads and requests while keeping finished pictures, so
    /// a hidden window stops networking without forgetting what is drawn.
    pub(super) fn suspend(&mut self) {
        self.pending.clear();
        self.in_flight = 0;
        self.blocks.clear();
        self.states.retain(|_, state| state.is_ready());
        self.ready_order.retain(|key| self.states.contains_key(key));
    }

    pub(super) fn attached(&self) -> bool {
        self.attached
    }

    /// Called while building a visible image block. Returns the state to draw
    /// and queues the request the first time this source is wanted.
    pub(super) fn want(&mut self, block: usize, target: ImageTarget) -> ImageState {
        let key = target.key();
        self.blocks.insert(block, key.clone());
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
        self.in_flight = self.in_flight.saturating_sub(1);
        if completion.scope != self.scope() {
            return Vec::new();
        }
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
        self.blocks
            .iter()
            .filter(|(_, wanted)| **wanted == key)
            .map(|(block, _)| *block)
            .collect()
    }

    /// Releases the oldest decoded pictures until the budget holds again.
    pub(super) fn evict(&mut self) {
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
            }
        }
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
            // The picture's box keeps the file's own aspect ratio and is
            // constrained by the reading column. Width is the only fixed
            // dimension (the column width, capped by the intrinsic width), so
            // the height derives from the ratio and a very tall picture is
            // bounded by capping the width rather than by a competing height
            // limit. A child image in the flow would contribute its own height
            // and let the box outgrow the ratio, so the picture is painted
            // inside an absolutely positioned layer of that box.
            let ratio = (*width as f32 / (*height as f32).max(1.0))
                .clamp(MIN_DISPLAY_RATIO, MAX_DISPLAY_RATIO);
            let max_width = (*width as f32).min(MAX_DISPLAY_HEIGHT * ratio);
            let mut frame = div()
                .debug_selector(move || frame_selector.clone())
                .relative()
                .w_full()
                .max_w(px(max_width))
                .overflow_hidden()
                .rounded(radius)
                .bg(colors.sunken);
            frame.style().aspect_ratio = Some(ratio);
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
