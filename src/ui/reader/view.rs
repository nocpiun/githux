//! The NSH widget: a prepared document, its native selection, links and
//! pictures, in a bounded viewport or at natural height.
//!
//! * Default/Repository: a bounded viewport that scrolls itself.
//! * Flow: natural height, so a Profile page's own scroll container scrolls the
//!   README (no inner scrollbar and no clipping).
//!
//! Every text block is a `TextView` inside one `selectable` view, so a drag (or
//! Shift+click) selects across blocks and the platform's Ctrl+A/Ctrl+C work on
//! the texts that are mounted. Documents past [`WINDOW_AFTER`] top-level blocks
//! are mounted through NSH's virtual list: only the rows near the viewport
//! exist, which keeps a 2 MiB README usable. A virtualized document's native
//! Ctrl+A therefore covers its mounted rows, not the whole model; this boundary
//! is documented in `docs/nsh-reader.md` rather than silently degraded for
//! normal documents.

use super::{
    LinkTarget, ReaderBody, ReaderDocument,
    images::{ImageBridge, Picture},
    links,
};
use nsh_core::prelude::*;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

/// Above this many top-level Markdown blocks the bounded reader mounts its
/// blocks through a virtual list. Below it the whole document stays in one
/// selectable column, so native Ctrl+A covers the complete model.
pub const WINDOW_AFTER: usize = 512;

/// How a Markdown document is sized vertically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderLayout {
    /// Existing hosts keep the original fixed outer spacing inside a bounded,
    /// self-scrolling viewport.
    Default,
    /// Full-height repository viewport with the reader's own vertical inset.
    /// Still a bounded viewport with its own scrollbar.
    Repository,
    /// Natural height: the document lays out every block in one column and
    /// reports its full height to the parent layout, so the host's own scroll
    /// container scrolls it. Use this when the reader is one section of a
    /// larger scrolling page (a profile README).
    Flow,
}

/// What a user action asks the integration layer to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ReaderAction {
    /// A link was pressed and resolved; the reader did not open it.
    Link(LinkTarget),
    /// The document's natural height changed (a picture landed, text
    /// re-wrapped, the document was replaced). A Flow host re-lays out its own
    /// scroll container from here; a bounded host ignores it.
    LayoutChanged { height: f32 },
    /// The captured account capability no longer allows this document. The
    /// reader has already cleared its body and pictures; the host can remove
    /// it.
    AuthorizationRetired,
}

type ActionHandler = Rc<dyn Fn(ReaderAction)>;

struct Inner {
    document: Signal<Option<Arc<ReaderDocument>>>,
    /// Bumped whenever a picture changes state, so image elements re-render.
    pictures: Signal<u64>,
    bridge: RefCell<ImageBridge>,
    layout: ReaderLayout,
    action: ActionHandler,
    scroll: ElementRef,
    /// The document column's top in the scroll content (full build only).
    document_top: RefCell<f32>,
    /// Measured heading offsets (full build only).
    anchors: RefCell<HashMap<String, f32>>,
    /// Heading anchors to block indexes (virtual list).
    anchor_blocks: RefCell<HashMap<String, usize>>,
    windowed: Cell<bool>,
}

/// Create on the NSH UI thread inside a component's reactive scope. Hold one
/// per page reader.
#[derive(Clone)]
pub struct ReaderHandle(Rc<Inner>);

impl ReaderHandle {
    pub fn new(
        document: Arc<ReaderDocument>,
        bridge: ImageBridge,
        layout: ReaderLayout,
        on_action: impl Fn(ReaderAction) + 'static,
    ) -> Self {
        Self(Rc::new(Inner {
            document: signal(Some(document)),
            pictures: signal(0),
            bridge: RefCell::new(bridge),
            layout,
            action: Rc::new(on_action),
            scroll: element_ref(),
            document_top: RefCell::new(0.0),
            anchors: RefCell::new(HashMap::new()),
            anchor_blocks: RefCell::new(HashMap::new()),
            windowed: Cell::new(false),
        }))
    }

    /// Host UI drain, before publishing/rendering a snapshot. No I/O: it
    /// validates the captured capability, accepts current completions, and only
    /// queues new provider work. Returns whether the presentation changed.
    pub fn poll(&self) -> bool {
        let changed = self.0.bridge.borrow_mut().poll();
        let current = self.0.bridge.borrow().is_current();
        if !current && self.0.document.get_untracked().is_some() {
            self.0.document.set(None);
            self.0.anchors.borrow_mut().clear();
            self.0.anchor_blocks.borrow_mut().clear();
            (self.0.action)(ReaderAction::AuthorizationRetired);
        }
        if changed {
            self.0.pictures.update(|n| *n += 1);
        }
        changed
    }

    /// Whether any picture is still queued or outstanding.
    pub fn has_pending_images(&self) -> bool {
        self.0.bridge.borrow().has_work()
    }

    /// The document currently shown, if the capability is still valid.
    pub fn document(&self) -> Option<Arc<ReaderDocument>> {
        self.0.document.get_untracked()
    }

    /// The state one picture source has now. Queues the request like a frame
    /// would; useful to hosts and tests that want to check the gate without
    /// mounting the document.
    pub fn picture(&self, raw: &str) -> Picture {
        let Some(document) = self.0.document.get_untracked() else {
            return Picture::Failed(crate::content::images::ImageFailure::StaleContext);
        };
        self.0.bridge.borrow_mut().picture(raw, &document.context)
    }

    /// Already prepared by a background worker. Rebuilding the document drops
    /// the old selection, anchor measurements and picture generation.
    pub fn replace(&self, document: Arc<ReaderDocument>, bridge: ImageBridge) {
        // `RefCell::replace` drops the old bridge, whose `Drop` retires it: the
        // previous account's in-flight pictures are cancelled here.
        self.0.bridge.replace(bridge);
        self.0.anchors.borrow_mut().clear();
        self.0.anchor_blocks.borrow_mut().clear();
        self.0.document.set(Some(document));
        self.0.pictures.update(|n| *n += 1);
        self.0.scroll.scroll_to(point(px(0.0), px(0.0)), false);
    }

    /// Hiding a retained reader keeps completed pictures and stops network work.
    pub fn suspend(&self) {
        self.0.bridge.borrow_mut().suspend();
        self.0.pictures.update(|n| *n += 1);
    }

    /// Permanently invalidates the captured capability and clears the body.
    pub fn retire(&self) {
        self.0.bridge.borrow_mut().retire();
        self.poll();
    }

    /// Resolves and reports one pressed link. Anchors jump inside a bounded
    /// reader; a Flow host receives the target and scrolls its own page.
    pub fn handle_link(&self, url: &str) {
        if !self.0.bridge.borrow().is_current() {
            self.retire();
            return;
        }
        if let Some(document) = self.0.document.get_untracked() {
            let target = links::resolve(url, &document.context);
            if let LinkTarget::Anchor { id } = &target {
                self.jump_to_anchor(id);
            }
            (self.0.action)(ReaderAction::Link(target));
        }
    }

    /// Jumps to a heading anchor inside a bounded reader. Returns whether it
    /// moved. In Flow the host owns the scroll container (see [`ReaderAction`]).
    pub fn jump_to_anchor(&self, id: &str) -> bool {
        if self.0.layout == ReaderLayout::Flow {
            return false;
        }
        if self.windowed() {
            if let Some(index) = self.0.anchor_blocks.borrow().get(id).copied() {
                self.0
                    .scroll
                    .scroll_to_index(index as u32, Percentage(0.0), px(0.0), false);
                return true;
            }
            return false;
        }
        if let Some(y) = self.0.anchors.borrow().get(id) {
            self.0.scroll.scroll_to(
                point(px(0.0), px((*y - *self.0.document_top.borrow()).max(0.0))),
                false,
            );
            true
        } else {
            false
        }
    }

    /// Scrolls a bounded reader to the end. In Flow the host's page scrolls.
    pub fn scroll_to_end(&self) {
        if self.0.layout != ReaderLayout::Flow {
            self.0.scroll.scroll_to_end(false);
        }
    }

    /// Whether the current document is mounted through the virtual list.
    pub fn is_windowed(&self) -> bool {
        self.windowed()
    }

    fn windowed(&self) -> bool {
        self.0.windowed.get()
    }
}

/// Mount once in a page. The host calls `handle.poll()` on its UI drain and
/// retires it on invalidation.
pub fn reader(handle: ReaderHandle) -> AnyElement {
    let cleanup_handle = handle.clone();
    cleanup(move || cleanup_handle.0.bridge.borrow_mut().retire());
    let content_handle = handle.clone();
    let content = dynamic(move || {
        let h = content_handle.clone();
        match h.0.document.get() {
            None => view()
                .test_id("reader-retired")
                .p(px(12.0))
                .child(text("Content is no longer available."))
                .into_element(),
            Some(document) => render_document(h, document),
        }
    });
    if handle.0.layout == ReaderLayout::Flow {
        content
    } else {
        view().size_full().child(content).into_element()
    }
}

/// NSH's default tokens with the application's font families. Fonts are
/// `.font()`-registered by the host; a fixture root may call this with real,
/// bundled files. The host owns the final palette and typography.
pub fn dark_tokens() -> nsh_core::element::Tokens {
    use nsh_core::element::Tokens;
    let mut tokens = Tokens::default();
    tokens.set_font(nsh_core::element::FONT_SANS, "Githux UI");
    tokens.set_font(nsh_core::element::FONT_MONO, "Google Sans Code");
    tokens
}

fn padding(layout: ReaderLayout) -> f32 {
    match layout {
        ReaderLayout::Default => 8.0,
        ReaderLayout::Repository | ReaderLayout::Flow => 12.0,
    }
}

fn render_document(handle: ReaderHandle, document: Arc<ReaderDocument>) -> AnyElement {
    match &document.body {
        ReaderBody::Status(label) => view()
            .test_id("reader-status")
            .p(px(12.0))
            .child(text(*label))
            .into_element(),
        ReaderBody::Source {
            text: source,
            language,
        } => view()
            .w_full()
            .h_full()
            .p(px(12.0))
            .test_id("reader-source")
            .child(
                nsh_base::editor()
                    .default_value(source.clone())
                    .language(language.clone().map(SharedString::from))
                    .read_only(true)
                    .line_numbers(true)
                    .folding(false)
                    .active_line(false)
                    .indent_guides(false)
                    .scroll_beyond_last_line(nsh_base::EditorScrollBeyond::from(false))
                    .font_family("Google Sans Code")
                    .font_size(px(14.0))
                    .line_height(px(22.4))
                    .w_full()
                    .h_full(),
            )
            .into_element(),
        ReaderBody::Markdown(document_model) => {
            // Anchors: always map heading slugs to their block index (the
            // virtual list needs it); the full build additionally measures
            // pixel offsets so a bounded reader can scroll precisely.
            {
                let mut index = handle.0.anchor_blocks.borrow_mut();
                index.clear();
                for (at, block) in document_model.blocks.iter().enumerate() {
                    if let nsh_base::text_view::Block::Heading {
                        anchor: Some(anchor),
                        ..
                    } = block
                    {
                        index.insert(anchor.clone(), at);
                    }
                }
            }
            let windowed =
                handle.0.layout != ReaderLayout::Flow && document_model.blocks.len() > WINDOW_AFTER;
            handle.0.windowed.set(windowed);
            if windowed {
                return windowed_document(handle, document_model.blocks.len());
            }
            let origin_handle = handle.clone();
            let height = Rc::new(RefCell::new(-1.0f32));
            let inset = padding(handle.0.layout);
            let mut column = view()
                .flex_col()
                .gap(px(12.0))
                .w_full()
                .py(px(12.0))
                .px(px(inset))
                .selectable(true)
                .test_id("reader-document")
                .on_layout(move |bounds: &Bounds<Points>| {
                    *origin_handle.0.document_top.borrow_mut() = bounds.origin.y.0;
                    if *height.borrow() != bounds.size.height.0 {
                        *height.borrow_mut() = bounds.size.height.0;
                        (origin_handle.0.action)(ReaderAction::LayoutChanged {
                            height: bounds.size.height.0,
                        });
                    }
                });
            for (index, block) in document_model.blocks.iter().enumerate() {
                column = column.child(block_row(
                    handle.clone(),
                    document.clone(),
                    index,
                    block.clone(),
                    true,
                ));
            }
            if handle.0.layout == ReaderLayout::Flow {
                column.into_element()
            } else {
                scroll()
                    .element_ref(&handle.0.scroll)
                    .size_full()
                    .test_id("reader-scroll")
                    .child(column)
                    .into_element()
            }
        }
    }
}

/// The bounded viewport for a long document: only the rows near the viewport
/// are mounted. Each cell's content is a `dynamic` that reads both the
/// recycled position and the document signal, so a replaced document never
/// leaves a recycled row showing the previous body.
fn windowed_document(handle: ReaderHandle, count: usize) -> AnyElement {
    let cell_handle = handle.clone();
    let cell_document = handle.0.document;
    let inset = padding(handle.0.layout);
    virtual_list(
        move || count,
        move |at| {
            let h = cell_handle.clone();
            let document = cell_document;
            dynamic(move || {
                let index = at.index();
                let Some(prepared) = document.get() else {
                    return view().into_element();
                };
                let Some(block) = prepared.markdown_blocks().get(index) else {
                    return view().into_element();
                };
                block_row(h.clone(), prepared.clone(), index, block.clone(), false)
            })
        },
    )
    .estimated_item_size(px(64.0))
    .content_padding_y(px(12.0))
    .content_padding_x(px(inset))
    .row_gap(px(12.0))
    .selectable(true)
    .element_ref(&handle.0.scroll)
    .size_full()
    .test_id("reader-scroll")
    .into_element()
}

/// One top-level block in its own `TextView`: shared selection, link reports
/// and provider-gated pictures. In the full build a heading also records its
/// measured offset for anchor jumps.
fn block_row(
    handle: ReaderHandle,
    document: Arc<ReaderDocument>,
    index: usize,
    block: nsh_base::text_view::Block,
    measure_anchor: bool,
) -> AnyElement {
    let image_handle = handle.clone();
    let image_context = document.context.clone();
    let link_handle = handle.clone();
    let block_view = nsh_base::text_view()
        .document(nsh_base::text_view::Document::new(vec![block.clone()]))
        .selectable(true)
        .links(nsh_base::TextViewLinks::Report)
        .on_link_press(move |event: &nsh_base::TextViewLinkPressEvent| {
            link_handle.handle_link(&event.url)
        })
        .render_image(move |image: &nsh_base::text_view::Image| {
            let h = image_handle.clone();
            let context = image_context.clone();
            let raw = image.src.clone();
            let alt = image.alt.clone();
            dynamic(move || {
                h.0.pictures.get();
                let picture = h.0.bridge.borrow_mut().picture(&raw, &context);
                picture_element(picture, &alt)
            })
        });
    let mut row = view()
        .test_id(&format!("reader-block-{index}"))
        .child(block_view);
    if measure_anchor
        && let nsh_base::text_view::Block::Heading {
            anchor: Some(anchor),
            ..
        } = block
    {
        let anchor = anchor.clone();
        let h = handle.clone();
        row = row.on_layout(move |bounds: &Bounds<Points>| {
            h.0.anchors
                .borrow_mut()
                .insert(anchor.clone(), bounds.origin.y.0);
        });
    }
    row.into_element()
}

/// Draws one picture in its current state, always keeping the provider's
/// already-validated bytes in memory (never a README address).
fn picture_element(picture: Picture, alt: &str) -> AnyElement {
    match picture {
        Picture::Ready {
            source,
            width,
            height,
        } => {
            let ratio = width.max(1) as f32 / height.max(1) as f32;
            let display_width = (width as f32).min(640.0 * ratio);
            view()
                .flex_col()
                .max_w(px(display_width))
                .w_full()
                .child(
                    nsh_core::element::image(source)
                        .w_full()
                        .aspect_ratio(ratio)
                        .object_fit(nsh_core::ResizeMode::Contain)
                        .label(alt.to_owned()),
                )
                .child(text(alt.to_owned()).select_text())
                .into_element()
        }
        Picture::Pending => view()
            .h(px(140.0))
            .child(text(alt.to_owned()).select_text())
            .child(text("loading picture").select_none())
            .into_element(),
        Picture::Released => view()
            .h(px(140.0))
            .child(text(alt.to_owned()).select_text())
            .child(text("picture released").select_none())
            .into_element(),
        Picture::Failed(error) => view()
            .h(px(140.0))
            .child(text(alt.to_owned()).select_text())
            .child(text(error.label()).select_none())
            .into_element(),
    }
}
