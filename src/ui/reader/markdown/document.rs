//! A read-only Markdown document: one virtualized block list with a real
//! [`ListState`], plus the reader's thin scrollbar over that same list.
//!
//! Extracted and simplified for Githux from Ely's `MarkdownDocument` Preview
//! (`ely-documents` @ `30c66fe1`, MIT). Editing modes, block opening, editor
//! history, attachments and page viewers are not carried over. What remains is
//! what reading a repository file needs: background parsing, only visible
//! blocks built, selection in prose, real scrolling and link callbacks.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use ely_core::theme::{ActiveTheme, Radius, TextSize};
use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, EmptyView, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, ListAlignment, ListState, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, Render, SharedString, Styled, Task, TextAlign,
    Window, div, list, prelude::*, relative,
};

use super::{
    ast::{self, Align, Block, CellImage, Inline},
    doctext::DocText,
    inline::{LocalLayout, ParagraphView},
};
use crate::ui::reader::images::{self, ImageBoard, ImageHints, ImageState};

/// Texts this long parse at once; longer ones parse in the background.
const AT_ONCE: usize = 128 << 10;

/// Longer texts are not read here; the content layer refuses them earlier.
pub const LARGEST_READ: usize = 2 << 20;

/// What the document tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentEvent {
    /// A new text was read into blocks and headings.
    Parsed,
}

/// Parsed blocks with the source they came from.
#[derive(Default)]
struct Parsed {
    text: String,
    blocks: Vec<Block>,
    headings: Vec<ast::Heading>,
    /// Read-order text and the unit table for document-wide selection.
    doctext: DocText,
}

/// Marker type for a text-selection drag in the reading area.
struct SelectDrag;

/// The drawing context shared by every nested block of one item.
#[derive(Clone, Copy)]
struct NestedStyle<'a> {
    colors: &'a ely_core::theme::Palette,
    body: f32,
    mono: &'a SharedString,
    radius: gpui::Rems,
}

/// One pointer hit inside a paragraph.
struct Hit {
    /// Document offset under the pointer.
    at: usize,
    /// The link under the pointer's glyph, if any.
    link: Option<String>,
    /// Start of the paragraph's document range.
    start: usize,
}

/// One painted table-cell picture hit target.
///
/// Its pixels stand in for the cell's alt text, so a press or drag over it
/// belongs to that picture's document unit instead of being blank space.
struct PictureHit {
    bounds: Bounds<Pixels>,
    range: std::ops::Range<usize>,
    link: Option<String>,
}

/// A link pressed in the document; the owner resolves it.
pub type OnLink = std::rc::Rc<dyn Fn(&str, &mut Window, &mut App) + 'static>;

/// How many children of one container may be built before the container is
/// split across independently virtualized rows. Below this, a nested layout
/// keeps its visual grouping; above it, one large list or table must not build
/// every child in a single item.
const LAZY_CHILDREN: usize = 64;

/// The marker of one list item: a checkbox, an ordered number or a bullet.
fn item_marker(start: Option<u64>, task: Option<bool>, at: usize) -> String {
    match (task, start) {
        (Some(true), _) => "☑".to_string(),
        (Some(false), _) => "☐".to_string(),
        (None, Some(first)) => format!("{}.", first + at as u64),
        (None, None) => "•".to_string(),
    }
}

/// A read-only Markdown reading surface.
pub struct MarkdownView {
    parsed: Arc<Parsed>,
    text: String,
    list: ListState,
    motion: crate::ui::scroll::Motion<ListState>,
    layout: crate::ui::reader::MarkdownLayout,
    focus: FocusHandle,
    on_link: Option<OnLink>,
    /// Bumped for every read, so an older background read lapses.
    generation: u64,
    parsing: Option<Task<()>>,
    /// Scroll to the top once the first read lands.
    pending_top: bool,
    hovered: bool,
    /// The document-wide selection, in document text offsets.
    anchor: usize,
    head: usize,
    dragging: bool,
    /// A link pressed on the last press, opened only on a click without drag.
    pressed_link: Option<String>,
    /// Painted bounds of visible table-cell pictures, by their document unit
    /// index, with the unit's document range and the link wrapping the picture.
    /// A picture has no shaped glyphs, so without this a press on it would look
    /// like blank space and a drag would start from offset 0.
    picture_hits: Rc<RefCell<std::collections::HashMap<usize, PictureHit>>>,
    /// Top-level block the press landed in, so a drag maps hits in order.
    anchor_block: Option<usize>,
    /// Real layouts of visible paragraphs, keyed by their unit index in the
    /// document model (never by document start: two units can share one).
    paragraphs: Rc<RefCell<std::collections::HashMap<usize, Rc<RefCell<LocalLayout>>>>>,
    /// Unit index -> its document range, for pointer mapping.
    paragraph_ranges: Rc<RefCell<std::collections::HashMap<usize, std::ops::Range<usize>>>>,
    /// Painted bounds of visible top-level blocks, by block index.
    block_bounds: Rc<RefCell<std::collections::HashMap<usize, Bounds<Pixels>>>>,
    /// Paragraph units painted in the frame being built. Hit testing uses only
    /// these, so a row scrolled away can never win at its old position.
    visible_units: Rc<RefCell<std::collections::HashSet<usize>>>,
    /// The reader's shared picture state, when a host attached one.
    image_board: Option<Rc<RefCell<ImageBoard>>>,
    /// The fixed repository context relative pictures resolve against.
    resource_context: Option<crate::ui::reader::ResourceContext>,
    _focus_loss: Option<gpui::Subscription>,
}

impl EventEmitter<DocumentEvent> for MarkdownView {}

impl Focusable for MarkdownView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MarkdownView {
    /// An empty document; [`Self::reset`] supplies the text.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let overdraw = window.rem_size() * 40.0;
        let list = ListState::new(0, ListAlignment::Top, overdraw);
        let focus = cx.focus_handle();
        let mut view = Self {
            parsed: Arc::new(Parsed::default()),
            text: String::new(),
            motion: crate::ui::scroll::Motion::new(list.clone()),
            layout: crate::ui::reader::MarkdownLayout::Default,
            list,
            focus,
            on_link: None,
            generation: 0,
            parsing: None,
            pending_top: false,
            hovered: false,
            anchor: 0,
            head: 0,
            dragging: false,
            pressed_link: None,
            picture_hits: Default::default(),
            anchor_block: None,
            paragraphs: Default::default(),
            paragraph_ranges: Default::default(),
            block_bounds: Default::default(),
            visible_units: Default::default(),
            image_board: None,
            resource_context: None,
            _focus_loss: None,
        };
        // Losing focus ends the gesture: a later move must not keep extending
        // an old selection.
        let focus = view.focus.clone();
        view._focus_loss = Some(cx.on_blur(&focus, window, |view, _, cx| view.end_drag(cx)));
        // The list owns its offset; the overlay scrollbar lives outside it and
        // needs a frame whenever the list scrolls.
        let this = cx.entity().downgrade();
        view.list.set_scroll_handler(move |_, _, cx| {
            if let Some(view) = this.upgrade() {
                view.update(cx, |_, cx| cx.notify());
            }
        });
        view
    }

    pub(crate) fn set_layout(
        &mut self,
        layout: crate::ui::reader::MarkdownLayout,
        cx: &mut Context<Self>,
    ) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.motion.cancel();
        self.list.remeasure();
        cx.notify();
    }

    /// Attaches the reader's shared picture state and the fixed context its
    /// relative picture addresses resolve against.
    pub(crate) fn set_images(
        &mut self,
        board: Rc<RefCell<ImageBoard>>,
        context: crate::ui::reader::ResourceContext,
    ) {
        self.image_board = Some(board);
        self.resource_context = Some(context);
    }

    /// Re-measures one top-level block after a picture changed size. The list
    /// keeps its scroll anchor, so a picture landing below the viewport does
    /// not move the text being read.
    pub(crate) fn remeasure_block(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.layout.is_flow() {
            // Flow mode lays every block out in one column on each frame, so the
            // picture's new height reaches the parent layout by re-rendering.
            cx.notify();
            return;
        }
        if ix < self.list.item_count() {
            self.list.remeasure_items(ix..ix + 1);
            cx.notify();
        }
    }

    /// Replaces the text; short texts parse at once, longer ones in the
    /// background pool.
    pub fn reset(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.motion.cancel();
        self.generation += 1;
        self.text = text.into();
        self.pending_top = true;
        // Old offsets belong to the old document.
        self.clear_selection(cx);
        self.paragraphs.borrow_mut().clear();
        let generation = self.generation;
        let text = self.text.clone();
        if text.len() > LARGEST_READ {
            self.parsing = None;
            self.adopt(
                Parsed {
                    text,
                    ..Parsed::default()
                },
                generation,
                cx,
            );
            return;
        }
        if text.len() <= AT_ONCE {
            self.parsing = None;
            let read = ast::parse(&text);
            self.adopt(
                Parsed {
                    text,
                    blocks: read.blocks,
                    headings: read.headings,
                    doctext: DocText::default(),
                },
                generation,
                cx,
            );
            return;
        }
        self.parsing = Some(cx.spawn(async move |view, cx| {
            let read = cx.background_spawn(async move { ast::parse(&text) }).await;
            let _ = view.update(cx, |view, cx| {
                let text = view.text.clone();
                view.adopt(
                    Parsed {
                        text,
                        blocks: read.blocks,
                        headings: read.headings,
                        doctext: DocText::default(),
                    },
                    generation,
                    cx,
                );
            });
        }));
    }

    /// Gives the document keyboard focus, as a press in the reading area does.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The text as last set.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the shown blocks read the current text.
    pub fn read_now(&self) -> bool {
        self.parsed.text == self.text
    }

    /// The block list's own scroll state: the real source for a scrollbar.
    pub fn list_state(&self) -> &ListState {
        &self.list
    }

    pub(crate) fn motion(&self) -> &crate::ui::scroll::Motion<ListState> {
        &self.motion
    }

    /// Outline as last read: level, title, block index.
    pub fn outline(&self) -> Vec<(u8, String, usize)> {
        self.parsed
            .headings
            .iter()
            .map(|heading| (heading.level, heading.title.clone(), heading.block))
            .collect()
    }

    /// The block at the top of the viewport.
    pub fn top_block(&self) -> usize {
        self.list.logical_scroll_top().item_ix
    }

    /// How many blocks the list holds (only visible ones are built).
    pub fn block_count(&self) -> usize {
        self.list.item_count()
    }

    #[cfg(test)]
    pub(crate) fn painted_content_bounds(&self, index: usize) -> Option<Bounds<Pixels>> {
        self.block_bounds.borrow().get(&index).copied()
    }

    /// Takes a completed read, unless a newer document replaced it. The
    /// selection is cleared because old offsets belong to the old text.
    fn adopt(&mut self, mut read: Parsed, generation: u64, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.motion.cancel();
        read.blocks = flatten(read.blocks);
        read.headings = ast::headings(&read.blocks);
        read.doctext = DocText::build(&read.blocks);
        #[cfg(test)]
        if std::env::var_os("GITHUX_MD_DEBUG").is_some() {
            for ix in 0..read.blocks.len() {
                eprintln!(
                    "block {ix}: units {:?}",
                    read.doctext
                        .units_of(ix)
                        .map(|u| u.at.clone())
                        .collect::<Vec<_>>()
                );
            }
        }
        self.clear_selection(cx);
        self.paragraphs.borrow_mut().clear();
        self.paragraph_ranges.borrow_mut().clear();
        self.block_bounds.borrow_mut().clear();
        self.picture_hits.borrow_mut().clear();
        let old = self.parsed.blocks.len();
        let new = read.blocks.len();
        if old == 0 && new > 0 {
            // A list measured while empty keeps that measuring state after a
            // bare splice; a reset lets the first real block lay out.
            self.list.reset(new);
        } else {
            self.list.splice(0..old, new);
        }
        self.parsed = Arc::new(read);
        if std::mem::take(&mut self.pending_top) {
            self.list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: gpui::px(0.0),
            });
        }
        cx.emit(DocumentEvent::Parsed);
        cx.notify();
    }

    pub fn on_link(&mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) {
        self.on_link = Some(std::rc::Rc::new(handler));
    }

    /// The document-wide selection, in offsets of [`Self::selected_text`]'s
    /// source. Empty when nothing is selected.
    pub fn selection(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    /// The text a copy takes: the model in reading order, never the visible
    /// blocks only.
    pub fn selected_text(&self) -> String {
        let range = self.selection();
        self.parsed.doctext.slice(range).to_string()
    }

    /// The whole document's selectable text, for Ctrl+A and tests.
    pub fn document_text(&self) -> &str {
        self.parsed.doctext.text()
    }

    /// Selects the whole document, including blocks outside the viewport.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.anchor = 0;
        self.head = self.parsed.doctext.range().end;
        self.dragging = false;
        self.pressed_link = None;
        cx.notify();
    }

    /// Drops the selection and any drag, without touching scroll position.
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.anchor != self.head || self.dragging || self.pressed_link.is_some() {
            self.anchor = 0;
            self.head = 0;
            self.dragging = false;
            self.pressed_link = None;
            cx.notify();
        }
    }

    /// Ends transient gestures while preserving the completed selection for later copy.
    pub(crate) fn cancel_gesture(&mut self, cx: &mut Context<Self>) {
        self.pressed_link = None;
        self.end_drag(cx);
    }

    /// Copies the selection; false when there is nothing to copy.
    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.selected_text();
        if text.is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// A press started a drag at document offset `at`.
    fn press(
        &mut self,
        at: usize,
        shift: bool,
        link: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion.cancel();
        window.focus(&self.focus, cx);
        if !shift {
            self.anchor = at;
        }
        self.head = at;
        self.dragging = true;
        self.pressed_link = link;
        cx.notify();
    }

    /// A drag moved to document offset `at`.
    fn extend(&mut self, at: usize, cx: &mut Context<Self>) {
        // Moving the pointer at all makes this a selection gesture, even when
        // it later returns to the press position: a link must not open.
        if self.dragging {
            self.pressed_link = None;
        }
        if self.dragging && self.head != at {
            self.head = at;
            cx.notify();
        }
    }

    /// Ends a drag, including one released outside the viewport.
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging {
            self.dragging = false;
            cx.notify();
        }
    }

    /// Opens the link recorded by the press, if the gesture stayed a click.
    fn click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.anchor != self.head {
            return;
        }
        if let Some(url) = self.pressed_link.take()
            && let Some(on_link) = &self.on_link
        {
            on_link(&url, window, cx);
        }
    }

    /// The document offset and link under a pointer, through the real layout
    /// of whichever visible paragraph contains it.
    /// A hit against the live layout maps, read at event time.
    fn hit_at_live(&self, position: Point<Pixels>) -> Option<Hit> {
        self.hit_in(
            position,
            &self.paragraphs.borrow(),
            &self.visible_units.borrow(),
        )
    }

    /// A hit against explicit maps (used by tests and the drag fallback).
    fn hit_at(
        &self,
        position: Point<Pixels>,
        layouts: &std::collections::HashMap<usize, Rc<RefCell<LocalLayout>>>,
        visible: &std::collections::HashSet<usize>,
    ) -> Option<Hit> {
        self.hit_in(position, layouts, visible)
    }

    fn hit_in(
        &self,
        position: Point<Pixels>,
        layouts: &std::collections::HashMap<usize, Rc<RefCell<LocalLayout>>>,
        visible: &std::collections::HashSet<usize>,
    ) -> Option<Hit> {
        let ranges = self.paragraph_ranges.borrow();
        let mut order: Vec<usize> = layouts
            .keys()
            .copied()
            .filter(|index| visible.contains(index))
            .collect();
        if order.is_empty() {
            // Layout has not run yet (first frame): fall back to all known.
            order = layouts.keys().copied().collect();
        }
        order.sort_unstable();
        let mut best: Option<(usize, usize, Option<String>)> = None;
        for index in order {
            let Some(layout) = layouts.get(&index) else {
                continue;
            };
            let layout = layout.borrow();
            let Some(local) = layout.index_at(position) else {
                continue;
            };
            let Some(doc) = ranges.get(&index) else {
                continue;
            };
            let at = (doc.start + local).min(doc.end);
            // Table cells and tight list items can share painted rows. In a
            // forward drag keep the furthest hit, in a reverse drag the
            // earliest, so the selection head keeps moving the way the
            // pointer is travelling.
            let forward = self.head >= self.anchor;
            let replace = match &best {
                None => true,
                Some((known, _, _)) => {
                    if forward {
                        at > *known
                    } else {
                        at < *known
                    }
                }
            };
            if replace {
                let link = layout
                    .glyph_at(position)
                    .and_then(|glyph| self.parsed.doctext.link_at(doc.start + glyph));
                best = Some((at, at, link));
            }
        }
        // A table-cell picture has no shaped glyphs, so its unit has no
        // paragraph layout. Its painted box is the hit target: without it a
        // drag starting on the picture would look like blank space and select
        // from the beginning of the document.
        {
            let pictures = self.picture_hits.borrow();
            let forward = self.head >= self.anchor;
            for hit in pictures.values() {
                if !hit.bounds.contains(&position) {
                    continue;
                }
                let at = if forward {
                    hit.range.end
                } else {
                    hit.range.start
                };
                let replace = match &best {
                    None => true,
                    Some((known, _, _)) => {
                        if forward {
                            at > *known
                        } else {
                            at < *known
                        }
                    }
                };
                if replace {
                    best = Some((at, at, hit.link.clone()));
                }
            }
        }
        best.map(|(_, at, link)| Hit {
            at,
            link,
            start: at,
        })
    }

    /// The top-level block whose document units contain `start`.
    fn block_of(&self, start: usize) -> Option<usize> {
        (0..self.parsed.blocks.len()).find(|ix| {
            self.parsed
                .doctext
                .units_of(*ix)
                .any(|unit| unit.at.start <= start && start <= unit.at.end)
        })
    }

    /// A document offset for a pointer outside any paragraph's glyphs.
    ///
    /// If the pointer is inside a visible block, its padding or its row gaps
    /// still belong to that block: forward drags take the block's end and
    /// backward drags its start. Outside every block, the same direction rule
    /// extends to the last block above (forward) or first block below
    /// (backward), which also covers dragging past the viewport.
    fn edge_offset(&self, position: Point<Pixels>) -> usize {
        // Beside a visible paragraph's rows: use its nearest row end/start, so
        // dragging into the trailing whitespace of a wrapped line does not
        // swallow the rest of the block.
        {
            let layouts = self.paragraphs.borrow();
            let visible = self.visible_units.borrow();
            let ranges = self.paragraph_ranges.borrow();
            let mut best: Option<(f32, usize)> = None;
            for (index, layout) in layouts.iter() {
                if !visible.contains(index) {
                    continue;
                }
                let layout = layout.borrow();
                let Some(doc) = ranges.get(index) else {
                    continue;
                };
                if position.y < layout.bounds().top() || position.y > layout.bounds().bottom() {
                    continue;
                }
                let Some(local) = layout.nearest_index(position) else {
                    continue;
                };
                let distance = f32::from(position.x - layout.bounds().left()).abs();
                let at = (doc.start + local).min(doc.end);
                if best.is_none_or(|(known, _)| distance < known) {
                    best = Some((distance, at));
                }
            }
            if let Some((_, at)) = best {
                return at;
            }
        }
        let bounds = self.block_bounds.borrow();
        let mut blocks: Vec<(usize, Bounds<Pixels>)> =
            bounds.iter().map(|(ix, b)| (*ix, *b)).collect();
        blocks.sort_by_key(|(ix, _)| *ix);
        if blocks.is_empty() {
            return self.head;
        }
        let forward = self.head >= self.anchor;
        if let Some((ix, _)) = blocks
            .iter()
            .find(|(_, block)| position.y >= block.top() && position.y <= block.bottom())
        {
            return if forward {
                self.block_end(*ix)
            } else {
                self.block_start(*ix)
            };
        }
        if forward {
            let mut chosen = blocks[0].0;
            for (ix, block) in &blocks {
                if block.top() <= position.y {
                    chosen = *ix;
                }
            }
            self.block_end(chosen)
        } else {
            let mut chosen = blocks[blocks.len() - 1].0;
            for (ix, block) in blocks.iter().rev() {
                if block.bottom() >= position.y {
                    chosen = *ix;
                }
            }
            self.block_start(chosen)
        }
    }

    /// The document offset of the first selectable unit of block `ix`.
    fn block_start(&self, ix: usize) -> usize {
        self.parsed
            .doctext
            .units_of(ix)
            .next()
            .map_or(self.head, |unit| unit.at.start)
    }

    /// The document offset of the last selectable unit end of block `ix`.
    fn block_end(&self, ix: usize) -> usize {
        self.parsed
            .doctext
            .units_of(ix)
            .last()
            .map_or(self.head, |unit| unit.at.end)
    }

    fn item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(block) = self.parsed.blocks.get(ix) else {
            return div().into_any_element();
        };
        let centered = matches!(block, Block::Centered(_));
        // Centering must preserve the normal heading/prose renderer, including
        // its typography and spacing. `flatten` leaves one child per wrapper.
        let block = match block {
            Block::Centered(body) if body.len() == 1 => &body[0],
            _ => block,
        };
        // Consumed in exactly the order `DocText` registered units, so nested
        // containers map to the right document offsets without path matching.
        let unit_cursor = std::cell::Cell::new(0usize);
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let mono = theme.mono_family.clone();
        let body = f32::from(theme.text_size(TextSize::Base).to_pixels(window.rem_size()));
        let sm = f32::from(theme.text_size(TextSize::Sm).to_pixels(window.rem_size()));
        let xs = f32::from(theme.text_size(TextSize::Xs).to_pixels(window.rem_size()));
        let radius = theme.radius(Radius::Md);
        let nested_style = NestedStyle {
            colors: &colors,
            body,
            mono: &mono,
            radius,
        };
        if let Block::Image { alt, .. } = block
            && !alt.is_empty()
        {
            let _ = self.next_unit(ix, &unit_cursor);
        }
        let element = match block {
            Block::Paragraph(inline) => div()
                .debug_selector(move || format!("markdown-paragraph-{ix}"))
                .text_size(gpui::px(body))
                .line_height(gpui::px(body * 1.6))
                .text_color(colors.fg)
                .child(self.paragraph_unit(
                    gpui::ElementId::Name(format!("reader-inline-{ix}").into()),
                    inline,
                    self.next_unit(ix, &unit_cursor),
                ))
                .into_any_element(),
            Block::Heading(level, inline) => {
                let scale = match level {
                    1 => 2.25,
                    2 => 1.75,
                    3 => 1.5,
                    4 => 1.25,
                    _ => 1.0,
                };
                div()
                    .debug_selector(move || format!("markdown-heading-{ix}"))
                    .pt_2()
                    .text_size(gpui::px(body * scale))
                    .line_height(relative(1.3))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(if *level == 6 {
                        colors.fg_muted
                    } else {
                        colors.fg
                    })
                    .child(self.paragraph_unit(
                        gpui::ElementId::Name(format!("reader-inline-{ix}").into()),
                        inline,
                        self.next_unit(ix, &unit_cursor),
                    ))
                    .into_any_element()
            }
            Block::Quote { kind, body: inner } => {
                let mut quoted = div().flex().flex_col().gap_2();
                if let Some(kind) = kind {
                    quoted = quoted.child(
                        div()
                            .text_size(gpui::px(sm))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(colors.fg_muted)
                            .child(kind.title()),
                    );
                }
                div()
                    .pl_4()
                    .border_l_2()
                    .border_color(colors.border_strong)
                    .text_color(colors.fg_muted)
                    .child(
                        quoted.children(
                            inner
                                .iter()
                                .enumerate()
                                .map(|(at, block)| {
                                    self.nested(
                                        ix,
                                        &unit_cursor,
                                        ("quote", format!("{ix}-{at}")),
                                        block,
                                        nested_style,
                                    )
                                })
                                .collect::<Vec<_>>(),
                        ),
                    )
                    .into_any_element()
            }
            Block::Code { language, text } => {
                let styles = match language.as_deref() {
                    Some(name) => ely_core::forms::syntax_fenced(name)
                        .map_or_else(Vec::new, |syntax| {
                            ely_core::forms::code_highlights_with(text, syntax, cx)
                        }),
                    None => ely_core::forms::code_highlights(text, cx),
                };
                let highlights = styles.into_iter().map(|(range, highlight)| {
                    let mut style = gpui::HighlightStyle {
                        color: Some(highlight.color),
                        font_weight: highlight.weight,
                        font_style: highlight.italic.then_some(gpui::FontStyle::Italic),
                        background_color: highlight.background,
                        ..gpui::HighlightStyle::default()
                    };
                    if highlight.strike {
                        style.strikethrough = Some(gpui::StrikethroughStyle {
                            thickness: gpui::px(1.0),
                            color: Some(highlight.color),
                        });
                    }
                    (range, style)
                });
                // A code block is selectable like prose: its own unit range,
                // real glyph layout, and the document's selection drawn over
                // it. No padding is inserted into the copied bytes.
                let unit = self.next_unit(ix, &unit_cursor);
                let code_inline = Inline {
                    text: text.clone(),
                    styles: Vec::new(),
                    links: Vec::new(),
                    cell_images: Vec::new(),
                };
                // The language label is a real element above the code body.
                // Shifting it up fixes the old centred gap without moving the
                // code rows.
                div()
                    .debug_selector(move || format!("markdown-code-block-{ix}"))
                    .flex()
                    .flex_col()
                    .rounded(radius)
                    .bg(colors.sunken)
                    .px_3()
                    .pb_3()
                    .pt_2()
                    .font_family(mono.clone())
                    .text_size(gpui::px(sm))
                    .text_color(colors.syntax.variable)
                    .when_some(language.clone(), |block, language| {
                        block.child(
                            div()
                                .debug_selector(move || format!("markdown-code-language-{ix}"))
                                .mb_1()
                                .relative()
                                .top(gpui::px(-6.0))
                                .text_size(gpui::px(xs))
                                .text_color(colors.fg_subtle)
                                .child(language),
                        )
                    })
                    .child(
                        div()
                            .debug_selector(move || format!("markdown-code-text-{ix}"))
                            .child(
                                // Syntax colours come from the same run
                                // builder as prose, so selection coordinates
                                // and glyph positions stay shared.
                                self.paragraph_unit(
                                    gpui::ElementId::Name(format!("reader-code-{ix}").into()),
                                    &code_inline,
                                    unit,
                                )
                                .code(highlights.collect()),
                            ),
                    )
                    .into_any_element()
            }
            Block::List { start, items } => {
                let mut list = div().flex().flex_col().gap_2();
                for (at, (task, blocks)) in items.iter().enumerate() {
                    let marker = item_marker(*start, *task, at);
                    list = list.child(
                        div()
                            .flex()
                            .gap_2()
                            .w_full()
                            .child(div().flex_none().text_color(colors.fg_muted).child(marker))
                            .child(
                                div().flex_1().min_w_0().flex().flex_col().gap_1().children(
                                    blocks
                                        .iter()
                                        .enumerate()
                                        .map(|(sub, block)| {
                                            self.nested(
                                                ix,
                                                &unit_cursor,
                                                ("list", format!("{ix}-{at}-{sub}")),
                                                block,
                                                nested_style,
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                            ),
                    );
                }
                list.into_any_element()
            }
            Block::Table {
                aligns,
                head,
                rows,
                repeated_head,
            } => {
                let repeated = *repeated_head;
                let cell =
                    |this: &Self, inline: &Inline, row: usize, column: usize, header: bool| {
                        this.table_cell(
                            ix,
                            &unit_cursor,
                            inline,
                            column,
                            aligns,
                            repeated,
                            header,
                            &format!("cell-{ix}-{row}-{column}"),
                            format!("markdown-cell-{ix}-{row}-{column}"),
                            colors.fg_muted,
                        )
                    };
                // Table text uses the same body size and line height as prose:
                // cells inherit it, so the shaped runs, selection, links and
                // wrapped heights all agree with the surrounding document.
                let mut table = div()
                    .flex()
                    .flex_col()
                    .text_size(gpui::px(body))
                    .line_height(gpui::px(body * 1.6))
                    .rounded(radius)
                    .border_1()
                    .border_color(colors.border)
                    .overflow_hidden()
                    .child(
                        div().flex().bg(colors.hover).children(
                            head.iter()
                                .enumerate()
                                .map(|(column, inline)| cell(self, inline, 0, column, true))
                                .collect::<Vec<_>>(),
                        ),
                    );
                for (row, cells) in rows.iter().enumerate() {
                    table = table.child(
                        div()
                            .flex()
                            .border_t_1()
                            .border_color(colors.border)
                            .children(
                                cells
                                    .iter()
                                    .enumerate()
                                    .map(|(column, inline)| {
                                        cell(self, inline, row + 1, column, false)
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                    );
                }
                table.into_any_element()
            }
            Block::Rule => div()
                .debug_selector(move || format!("markdown-rule-{ix}"))
                .my_2()
                .border_t_2()
                .border_color(colors.border_strong)
                .into_any_element(),
            Block::Math { tex, .. } => div()
                .debug_selector(move || format!("markdown-math-{ix}"))
                .px_3()
                .py_2()
                .rounded(radius)
                .bg(colors.sunken)
                .font_family(mono)
                .text_size(gpui::px(sm))
                .child(tex.clone())
                .into_any_element(),
            Block::ImageRow(pictures) => self.picture_row(ix, &unit_cursor, pictures),
            Block::Centered(blocks) => div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .children(blocks.iter().enumerate().map(|(at, block)| {
                    self.nested(
                        ix,
                        &unit_cursor,
                        ("center", format!("{ix}-{at}")),
                        block,
                        nested_style,
                    )
                }))
                .into_any_element(),
            Block::Image { url, hints, .. } => self
                .picture_unit(ix, url, *hints)
                .unwrap_or_else(|| self.picture_probe(url, *hints)),
            Block::Html(text) => div()
                .debug_selector(move || format!("markdown-html-{ix}"))
                .font_family(mono)
                .text_size(gpui::px(sm))
                .text_color(colors.fg_subtle)
                .child(text.clone())
                .into_any_element(),
            Block::Footnote { label, body: inner } => div()
                .flex()
                .gap_2()
                .text_size(gpui::px(sm))
                .text_color(colors.fg_muted)
                .child(div().flex_none().child(format!("[{label}]")))
                .child(
                    div().flex_1().flex().flex_col().children(
                        inner
                            .iter()
                            .enumerate()
                            .map(|(at, block)| {
                                self.nested(
                                    ix,
                                    &unit_cursor,
                                    ("footnote", format!("{ix}-{at}")),
                                    block,
                                    nested_style,
                                )
                            })
                            .collect::<Vec<_>>(),
                    ),
                )
                .into_any_element(),
        };
        // Record the painted block bounds so a drag can locate blocks even
        // over blank space or outside the viewport.
        let bounds = self.block_bounds.clone();
        let inset = self.layout == crate::ui::reader::MarkdownLayout::Repository;
        // Flow mode uses the same real first/last block inset; the host scrolls
        // it, so the document does not need list-level padding tricks.
        let vertical = window.rem_size() * 0.75;
        let hidden_images = self.only_hidden_images(block);
        div()
            .debug_selector(move || format!("markdown-block-{ix}"))
            .when(centered, |block| {
                block.w_full().flex().flex_col().items_center()
            })
            .px_3()
            .when(!hidden_images, |block| block.py_1())
            .when(hidden_images, |block| {
                block.h(gpui::px(0.0)).overflow_hidden()
            })
            // Include real content insets in first/last block measurements.
            // ListState's scrollbar reach excludes list-level padding, so using
            // list.py here would let Motion/thumb clamp away the bottom inset.
            .when(inset && ix == 0, |block| {
                block.pt(window.rem_size() * 0.25 + vertical)
            })
            .when(inset && ix + 1 == self.parsed.blocks.len(), |block| {
                block.pb(window.rem_size() * 0.25 + vertical)
            })
            .on_children_prepainted(move |painted, _, _| {
                if let Some(painted) = painted.first().copied() {
                    bounds.borrow_mut().insert(ix, painted);
                }
            })
            .child(element)
            .into_any_element()
    }

    /// A block nested inside a quote, list item or footnote.
    ///
    /// `ix` and `units` are the owning top-level block and its render-order
    /// unit cursor, so this paragraph maps to the matching document range.
    fn nested(
        &self,
        ix: usize,
        units: &std::cell::Cell<usize>,
        id: (&'static str, String),
        block: &Block,
        style: NestedStyle<'_>,
    ) -> AnyElement {
        self.nested_keyed(ix, units, id, block, style)
    }

    fn nested_keyed(
        &self,
        ix: usize,
        units: &std::cell::Cell<usize>,
        id: (&'static str, String),
        block: &Block,
        style: NestedStyle<'_>,
    ) -> AnyElement {
        let NestedStyle {
            colors,
            body,
            mono,
            radius,
        } = style;
        let key = gpui::ElementId::Name(format!("reader-{}-{}", id.0, id.1).into());
        if let Block::Image { alt, .. } = block
            && !alt.is_empty()
        {
            let _ = self.next_unit(ix, units);
        }
        match block {
            Block::Paragraph(inline) => div()
                .text_size(gpui::px(body))
                .line_height(gpui::px(body * 1.6))
                .text_color(colors.fg)
                .child(self.paragraph_unit(key, inline, self.next_unit(ix, units)))
                .into_any_element(),
            Block::Code { text, .. } => {
                // The model registered this code block as a unit; consume it
                // so the next paragraph maps to its own range.
                let _ = self.next_unit(ix, units);
                div()
                    .px_3()
                    .py_2()
                    .rounded(radius)
                    .bg(colors.sunken)
                    .font_family(mono.clone())
                    .child(text.clone())
                    .into_any_element()
            }
            Block::List { start, items } => div()
                .flex()
                .flex_col()
                .gap_1()
                .children(
                    items
                        .iter()
                        .enumerate()
                        .map(|(at, (task, blocks))| {
                            let marker = item_marker(*start, *task, at);
                            div()
                                .flex()
                                .gap_2()
                                .child(div().flex_none().text_color(colors.fg_muted).child(marker))
                                .child(
                                    div().flex_1().min_w_0().children(
                                        blocks
                                            .iter()
                                            .enumerate()
                                            .map(|(sub, block)| {
                                                self.nested_keyed(
                                                    ix,
                                                    units,
                                                    (id.0, format!("{}-{at}-{sub}", id.1)),
                                                    block,
                                                    style,
                                                )
                                            })
                                            .collect::<Vec<_>>(),
                                    ),
                                )
                        })
                        .collect::<Vec<_>>(),
                )
                .into_any_element(),
            Block::Rule => div()
                .my_1()
                .border_t_1()
                .border_color(colors.border_strong)
                .into_any_element(),
            Block::Quote { kind, body: inner } => {
                let mut quote = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pl_4()
                    .border_l_2()
                    .border_color(colors.border_strong);
                if let Some(kind) = kind {
                    quote = quote.child(
                        div()
                            .text_size(gpui::px(body))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(colors.fg_muted)
                            .child(kind.title()),
                    );
                }
                quote
                    .children(
                        inner
                            .iter()
                            .enumerate()
                            .map(|(at, block)| {
                                self.nested_keyed(
                                    ix,
                                    units,
                                    (id.0, format!("{}-quote-{at}", id.1)),
                                    block,
                                    style,
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                    .into_any_element()
            }
            Block::Table {
                aligns,
                head,
                rows,
                repeated_head,
            } => {
                let repeated = *repeated_head;
                let cell =
                    |this: &Self, inline: &Inline, row: usize, column: usize, header: bool| {
                        this.table_cell(
                            ix,
                            units,
                            inline,
                            column,
                            aligns,
                            repeated,
                            header,
                            &format!("nested-{}-{row}-{column}", id.1),
                            format!("markdown-nested-cell-{}-{row}-{column}", id.1),
                            colors.fg_muted,
                        )
                    };
                // Same body size and line height as prose, so a nested table
                // is not larger than the text around it.
                let mut table = div()
                    .flex()
                    .flex_col()
                    .text_size(gpui::px(body))
                    .line_height(gpui::px(body * 1.6))
                    .rounded(radius)
                    .border_1()
                    .border_color(colors.border)
                    .overflow_hidden();
                table = table.child(
                    div().flex().children(
                        head.iter()
                            .enumerate()
                            .map(|(column, inline)| cell(self, inline, 0, column, true))
                            .collect::<Vec<_>>(),
                    ),
                );
                for (row, cells) in rows.iter().enumerate() {
                    table = table.child(
                        div()
                            .flex()
                            .border_t_1()
                            .border_color(colors.border)
                            .children(
                                cells
                                    .iter()
                                    .enumerate()
                                    .map(|(column, inline)| {
                                        cell(self, inline, row + 1, column, false)
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                    );
                }
                table.into_any_element()
            }
            Block::Footnote { label, body: inner } => div()
                .flex()
                .gap_2()
                .text_size(gpui::px(body))
                .text_color(colors.fg_muted)
                .child(div().flex_none().child(format!("[{label}]")))
                .child(
                    div().flex_1().flex().flex_col().children(
                        inner
                            .iter()
                            .enumerate()
                            .map(|(at, block)| {
                                self.nested_keyed(
                                    ix,
                                    units,
                                    (id.0, format!("{}-fn-{at}", id.1)),
                                    block,
                                    style,
                                )
                            })
                            .collect::<Vec<_>>(),
                    ),
                )
                .into_any_element(),
            Block::Heading(level, inline) => {
                // A nested heading keeps its selectable formatting and link
                // ranges; only its size follows the heading level.
                let view = self.paragraph_unit(
                    gpui::ElementId::Name(format!("reader-nested-{}-{}", id.0, id.1).into()),
                    inline,
                    self.next_unit(ix, units),
                );
                div()
                    .text_size(gpui::px(body * (1.0 + f32::from(6 - level) * 0.15)))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.fg)
                    .child(view)
                    .into_any_element()
            }
            Block::ImageRow(pictures) => self.picture_row(ix, units, pictures),
            Block::Centered(blocks) => div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .children(blocks.iter().enumerate().map(|(at, block)| {
                    self.nested(ix, units, ("center", format!("{ix}-{at}")), block, style)
                }))
                .into_any_element(),
            Block::Image { url, hints, .. } => self
                .picture_unit(ix, url, *hints)
                .unwrap_or_else(|| self.picture_probe(url, *hints)),
            other => div()
                .text_size(gpui::px(body))
                .text_color(colors.fg_muted)
                .child(match other {
                    Block::Html(text) => text.clone(),
                    Block::Math { tex, .. } => tex.clone(),
                    Block::Paragraph(inline) => inline.text.clone(),
                    _ => String::new(),
                })
                .into_any_element(),
        }
    }
}

impl MarkdownView {
    /// One table cell: its text runs and any pictures it keeps inline.
    ///
    /// A cell registers one document unit per piece (see [`ast::cell_parts`]),
    /// exactly as `DocText` registered them, so selection, links and copy keep
    /// working through a cell that also shows images. `presentation` marks the
    /// repeated header of a virtualized chunk: it paints the same pieces but
    /// takes no part in offset hit testing.
    #[allow(clippy::too_many_arguments)]
    fn table_cell(
        &self,
        ix: usize,
        units: &std::cell::Cell<usize>,
        cell: &Inline,
        column: usize,
        aligns: &[Align],
        repeated: bool,
        header: bool,
        key: &str,
        selector: String,
        muted: gpui::Hsla,
    ) -> AnyElement {
        let align = table_align(aligns, column);
        let presentation = repeated && header;
        let content = if cell.cell_images.is_empty() {
            // A cell without pictures draws exactly as it always has: one
            // shaped paragraph, aligned per column.
            if presentation {
                let _ = self.next_unit(ix, units);
                div().child(cell.text.clone()).into_any_element()
            } else {
                self.paragraph_unit(
                    gpui::ElementId::Name(format!("reader-{key}-0").into()),
                    cell,
                    self.next_unit(ix, units),
                )
                .align(align)
                .into_any_element()
            }
        } else {
            // A cell with pictures stacks them with its text runs in reading
            // order. Each piece keeps the cell's full inner width, so a
            // picture shrinks to a narrow cell and keeps its aspect ratio
            // instead of being clipped or stretching the column.
            let mut content = div().flex().flex_col().gap_1().w_full().min_w_0();
            content = match align {
                TextAlign::Center => content.items_center(),
                TextAlign::Right => content.items_end(),
                TextAlign::Left => content.items_start(),
            };
            let mut pending = Vec::new();
            let mut visible_children = 0usize;
            let parts = ast::cell_parts(cell);
            for (at, part) in parts.iter().enumerate() {
                match part.image {
                    Some(image) => {
                        // `DocText` registered one unit per piece, in this
                        // order, so the cursor advances once per piece either
                        // way; a picture's unit is its alt text, so copying
                        // keeps the description while the cell paints pixels.
                        let unit = self.next_unit(ix, units);
                        let link = image.link.clone();
                        let Some(picture) =
                            self.cell_picture(ix, image, format!("{key}-picture-{at}"))
                        else {
                            // Pending demand stays invisible and adds no cell
                            // row; the board still has the request and remeasures
                            // this block when the picture lands.
                            if let Some(demand) =
                                self.pending_picture(&image.url, image.hints, visible_children)
                            {
                                pending.push(demand);
                            }
                            continue;
                        };
                        let drawable = self
                            .picture_visibility(&image.url)
                            .is_some_and(|(board, key)| board.borrow().is_ready(&key));
                        let mut frame = div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .when(align == TextAlign::Center, |row| row.justify_center())
                            .when(align == TextAlign::Right, |row| row.justify_end())
                            .child(picture);
                        if !presentation && drawable {
                            // The painted picture is the hit target for its own
                            // document unit, so a selection drag starts here
                            // instead of at document offset 0.
                            let hits = self.picture_hits.clone();
                            let range = unit.1.clone();
                            let link = link.clone();
                            frame = frame.on_children_prepainted(move |bounds, _, _| {
                                if let Some(bounds) = bounds.first().copied() {
                                    hits.borrow_mut().insert(
                                        unit.0,
                                        PictureHit {
                                            bounds,
                                            range: range.clone(),
                                            link: link.clone(),
                                        },
                                    );
                                }
                            });
                        }
                        if link.is_some() && drawable {
                            frame = frame.cursor(gpui::CursorStyle::PointingHand);
                        }
                        content = content.child(frame.into_any_element());
                    }
                    None => {
                        // One unit per text run, in the same order `DocText`
                        // registered them. A repeated chunk header is
                        // presentation only: it consumes its unit to keep the
                        // cursor in step but stays out of offset hit testing.
                        let unit = self.next_unit(ix, units);
                        let piece = cell.slice(part.range.clone());
                        content = content.child(div().w_full().min_w_0().child(if presentation {
                            div().child(piece.text).into_any_element()
                        } else {
                            self.paragraph_unit(
                                gpui::ElementId::Name(format!("reader-{key}-{at}").into()),
                                &piece,
                                unit,
                            )
                            .align(align)
                            .into_any_element()
                        }));
                    }
                }
                visible_children += 1;
            }
            images::pending_container(content, self.image_board.clone(), pending)
        };
        let mut cell = div()
            .debug_selector(move || selector)
            .flex_1()
            .min_w_0()
            .px_3()
            .py_2()
            .child(content);
        if header {
            cell = cell.font_weight(gpui::FontWeight::MEDIUM).text_color(muted);
        }
        cell.into_any_element()
    }

    /// One picture inside a table cell, sized within the cell.
    ///
    /// It draws through the same picture presenter as every other picture.
    /// `None` means nothing is drawn: still loading, failed/refused,
    /// unavailable or released after eviction. Alt text stays in the copy
    /// model; ready sizing and measured geometry survive resource eviction.
    fn cell_picture(&self, ix: usize, picture: &CellImage, key: String) -> Option<AnyElement> {
        let state = match (&self.image_board, &self.resource_context) {
            (Some(board), Some(context)) => match images::resolve_target(&picture.url, context) {
                Ok(target) => board.borrow_mut().want(ix, target),
                Err(failure) => ImageState::Failed(failure),
            },
            _ => ImageState::Unavailable,
        };
        if !state.is_ready() {
            return None;
        }
        let element = images::image_element(
            gpui::ElementId::Name(format!("reader-{key}").into()),
            format!("markdown-{key}"),
            &state,
            picture.hints,
            format!("markdown-{key}-frame"),
        );
        Some(images::painted_when_visible(
            element,
            self.picture_visibility(&picture.url),
        ))
    }

    /// One picture row. Alt text remains in the document/copy model while
    /// non-ready picture UI stays silent.
    fn picture_row(
        &self,
        ix: usize,
        units: &std::cell::Cell<usize>,
        pictures: &[(String, String, ImageHints)],
    ) -> AnyElement {
        let mut pending = Vec::new();
        let mut visible_children = 0usize;
        let content = div()
            .debug_selector(move || format!("markdown-picture-row-{ix}"))
            .max_w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .when(
                self.parsed
                    .blocks
                    .get(ix)
                    .is_some_and(|block| matches!(block, Block::Centered(_))),
                |row| row.justify_center(),
            )
            .gap_1()
            .children(
                pictures
                    .iter()
                    .enumerate()
                    .filter_map(|(at, (alt, url, hints))| {
                        if !alt.is_empty() {
                            let _ = self.next_unit(ix, units);
                        }
                        // Every non-ready picture is dropped from the row, so it
                        // cannot claim its own gap or keep an empty box. The
                        // shared presenter already reports only pixels.
                        let Some(picture) = self.picture_unit(ix, url, *hints) else {
                            if let Some(demand) =
                                self.pending_picture(url, *hints, visible_children)
                            {
                                pending.push(demand);
                            }
                            return None;
                        };
                        visible_children += 1;
                        // Different pictures in one paragraph must keep independent element IDs.
                        Some(
                            div()
                                .debug_selector(move || {
                                    format!("markdown-inline-picture-{ix}-{at}")
                                })
                                .id(gpui::ElementId::Name(
                                    format!("picture-row-{ix}-{at}").into(),
                                ))
                                .child(picture),
                        )
                    }),
            );
        images::pending_container(content, self.image_board.clone(), pending)
    }

    fn pending_picture(
        &self,
        url: &str,
        hints: ImageHints,
        position: usize,
    ) -> Option<images::PendingDemand> {
        let (board, key) = self.picture_visibility(url)?;
        if board.borrow().has_layout_geometry(&key) {
            return None;
        }
        Some((key, hints, position))
    }
    fn picture_probe(&self, url: &str, hints: ImageHints) -> AnyElement {
        let pending = self.pending_picture(url, hints, 0).into_iter().collect();
        images::pending_container(div().w_full(), self.image_board.clone(), pending)
    }

    /// The board and source key a picture's animation visibility belongs to.
    fn picture_visibility(
        &self,
        url: &str,
    ) -> Option<(Rc<RefCell<ImageBoard>>, crate::content::images::ImageKey)> {
        self.image_board.clone().zip(
            self.resource_context
                .as_ref()
                .and_then(|context| images::resolve_target(url, context).ok())
                .map(|target| target.key()),
        )
    }

    /// Whether a picture source is on screen right now.
    ///
    /// Only `Ready` paints. Loading/unrequested, failed/refused, unavailable
    /// and released pictures all render nothing, so a block made only of them
    /// must not keep its image padding or an empty row around them.
    fn picture_has_geometry(&self, url: &str) -> bool {
        let Some(board) = self.image_board.as_ref() else {
            // Without a board no picture can paint.
            return false;
        };
        let Some(context) = self.resource_context.as_ref() else {
            return false;
        };
        match images::resolve_target(url, context) {
            Ok(target) => board.borrow().has_layout_geometry(&target.key()),
            Err(_) => false,
        }
    }

    /// A block whose pictures are all invisible contributes no image row.
    fn only_hidden_images(&self, block: &Block) -> bool {
        let hidden = |url: &str| !self.picture_has_geometry(url);
        match block {
            Block::Image { url, .. } => hidden(url),
            Block::ImageRow(pictures) => {
                !pictures.is_empty() && pictures.iter().all(|(_, url, _)| hidden(url))
            }
            Block::Centered(body) => {
                !body.is_empty() && body.iter().all(|block| self.only_hidden_images(block))
            }
            _ => false,
        }
    }

    /// One picture block: real pixels when ready, nothing at all otherwise.
    ///
    /// `None` means the picture contributes no element of its own: still
    /// loading/unrequested, failed/refused, unavailable or released. Demand is
    /// still registered through `want`, so the state machine, budgets,
    /// cancellation and remeasurement are unchanged; only the visuals are
    /// omitted, and alt text stays in the copy model.
    fn picture_unit(&self, ix: usize, url: &str, hints: ImageHints) -> Option<AnyElement> {
        let state = match (&self.image_board, &self.resource_context) {
            (Some(board), Some(context)) => match images::resolve_target(url, context) {
                Ok(target) => board.borrow_mut().want(ix, target),
                Err(failure) => ImageState::Failed(failure),
            },
            _ => ImageState::Unavailable,
        };
        if !state.is_ready() {
            return None;
        }
        let picture = images::image_element(
            gpui::ElementId::Name(format!("reader-picture-{ix}-{url}").into()),
            images::image_selector(ix),
            &state,
            hints,
            images::frame_selector(ix),
        );
        Some(images::painted_when_visible(
            picture,
            self.picture_visibility(url),
        ))
    }

    /// A paragraph drawn from one unit of the document model.
    ///
    /// The paragraph paints the document's selection, registers its real
    /// layout for pointer mapping, and keeps a key unique per nesting path.
    fn paragraph_unit(
        &self,
        key: gpui::ElementId,
        inline: &Inline,
        unit: (usize, std::ops::Range<usize>),
    ) -> ParagraphView {
        let (index, doc) = unit;
        // Laid out this frame: it may take part in hit testing.
        self.visible_units.borrow_mut().insert(index);
        self.paragraph_ranges
            .borrow_mut()
            .insert(index, doc.clone());
        let layout = self
            .paragraphs
            .borrow_mut()
            .entry(index)
            .or_insert_with(|| Rc::new(RefCell::new(LocalLayout::default())))
            .clone();
        let centered = self
            .parsed
            .doctext
            .unit(index)
            .and_then(|unit| self.parsed.blocks.get(unit.block))
            .is_some_and(|block| matches!(block, Block::Centered(_)));
        ParagraphView::new(key, inline.clone(), doc, self.selection(), layout).align(if centered {
            TextAlign::Center
        } else {
            TextAlign::Left
        })
    }

    /// The next unit of block `ix`: its global unit index and document range.
    fn next_unit(
        &self,
        ix: usize,
        cursor: &std::cell::Cell<usize>,
    ) -> (usize, std::ops::Range<usize>) {
        let at = cursor.get();
        cursor.set(at + 1);
        // The global index already addresses the unit directly, so a wide
        // table does not rescan every preceding cell.
        let global = self.parsed.doctext.unit_index(ix, at);
        let range = global
            .and_then(|index| self.parsed.doctext.unit(index))
            .map(|unit| unit.at.clone())
            .unwrap_or(0..0);
        (global.unwrap_or(usize::MAX), range)
    }
}

/// Split only over-large containers into several virtualized blocks. A list
/// keeps its own numbering by carrying the original first number and index.
/// The text alignment a table column asks for.
fn table_align(aligns: &[Align], column: usize) -> TextAlign {
    match aligns.get(column) {
        Some(Align::Right) => TextAlign::Right,
        Some(Align::Center) => TextAlign::Center,
        _ => TextAlign::Left,
    }
}

fn flatten(blocks: Vec<Block>) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Block::Centered(body) => {
                // Centering is presentation, not one unbounded virtual-list row.
                for child in flatten(body) {
                    if matches!(child, Block::Centered(_)) {
                        out.push(child);
                    } else {
                        out.push(Block::Centered(vec![child]));
                    }
                }
            }
            Block::List { start, items } if items.len() > LAZY_CHILDREN => {
                for (chunk, group) in items.chunks(LAZY_CHILDREN).enumerate() {
                    let first = chunk * LAZY_CHILDREN;
                    out.push(Block::List {
                        // Numbering continues across the split rows.
                        start: start.map(|first_number| first_number + first as u64),
                        items: group.to_vec(),
                    });
                }
            }
            Block::Table {
                aligns, head, rows, ..
            } if rows.len() > LAZY_CHILDREN => {
                for (chunk, group) in rows.chunks(LAZY_CHILDREN).enumerate() {
                    out.push(Block::Table {
                        aligns: aligns.clone(),
                        // The header repeats on each virtual row, as a table
                        // header does on a long printed page. Only the first
                        // chunk owns the real header for copying.
                        head: head.clone(),
                        rows: group.to_vec(),
                        repeated_head: chunk > 0,
                    });
                }
            }
            other => out.push(other),
        }
    }
    out
}

impl MarkdownView {
    /// Natural height: every block in one column, so the element reports the
    /// document's full height and the host's scroll container scrolls it.
    ///
    /// Nothing about the document changes: the same `item()` builds each block,
    /// so selection units, links, code, tables and pictures behave exactly as in
    /// the virtualized viewport. Only the sizing differs.
    fn render_flow(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let horizontal = window.rem_size() * 0.5;
        self.visible_units.borrow_mut().clear();
        self.picture_hits.borrow_mut().clear();
        let blocks = self.parsed.blocks.len();
        let mut column = div().flex().flex_col().w_full().min_w_0();
        for ix in 0..blocks {
            column = column.child(self.item(ix, window, cx));
        }
        div()
            .id("reader-markdown")
            .debug_selector(|| "reader-markdown".to_string())
            .track_focus(&self.focus)
            .key_context("ReaderMarkdown")
            .relative()
            .w_full()
            .min_w_0()
            // Flow is a section of the host's page: the host owns the background, so the
            // document paints none and the surrounding section shows through. Code fences,
            // inline code and quotes keep their own semantic fills.
            .px(horizontal)
            .py_3()
            .child(column)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.motion.cancel();
                    window.focus(&view.focus, cx);
                    match view.hit_at_live(event.position) {
                        Some(hit) => {
                            if !event.modifiers.shift {
                                view.anchor_block = view.block_of(hit.start);
                            }
                            view.press(hit.at, event.modifiers.shift, hit.link, window, cx)
                        }
                        None => {
                            view.anchor = 0;
                            view.head = 0;
                            view.dragging = true;
                            view.pressed_link = None;
                            cx.notify();
                        }
                    }
                }),
            )
            .on_drag(SelectDrag, |_, _, _, cx| cx.new(|_| EmptyView))
            .on_drag_move(cx.listener(
                move |view, event: &gpui::DragMoveEvent<SelectDrag>, _, cx| {
                    if !view.dragging {
                        return;
                    }
                    let position = event.event.position;
                    if let Some(hit) = view.hit_at_live(position) {
                        view.extend(hit.at, cx);
                    } else {
                        view.extend(view.edge_offset(position), cx);
                    }
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    view.end_drag(cx);
                    view.click(window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.end_drag(cx)),
            )
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                let keystroke = &event.keystroke;
                if !keystroke.modifiers.secondary(cx) {
                    return;
                }
                match keystroke.key.as_str() {
                    "a" => {
                        view.select_all(cx);
                        cx.stop_propagation();
                    }
                    "c" if view.copy(cx) => cx.stop_propagation(),
                    _ => {}
                }
            }))
            .into_any_element()
    }
}

impl Render for MarkdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.layout.is_flow() {
            return self.render_flow(window, cx);
        }
        let colors = cx.theme().colors.clone();
        let repository_layout = self.layout == crate::ui::reader::MarkdownLayout::Repository;
        let horizontal = window.rem_size() * 0.5
            + if repository_layout {
                gpui::px(4.0)
            } else {
                gpui::px(0.0)
            };
        // A new frame: only paragraphs and pictures laid out below are
        // hit-testable.
        self.visible_units.borrow_mut().clear();
        self.picture_hits.borrow_mut().clear();
        let list = list(
            self.list.clone(),
            cx.processor(|view: &mut Self, ix: usize, window: &mut Window, cx| {
                view.item(ix, window, cx)
            }),
        )
        // Content insets belong to real first/last item measurements (see item).
        .size_full();
        // Pointer positions inside a visible paragraph map through that
        // paragraph's real layout to document offsets; positions on blank
        // reader space (padding, gaps, after the last block) clear selection.
        // Hit testing reads the live maps inside each event: the list's layout
        // callback can register new paragraphs after `render()` builds these
        // closures, and those units must be clickable immediately.
        div()
            .id("reader-markdown")
            .debug_selector(|| "reader-markdown".to_string())
            .track_focus(&self.focus)
            .key_context("ReaderMarkdown")
            .relative()
            .size_full()
            .min_h_0()
            .bg(colors.surface)
            .px(horizontal)
            .when(!repository_layout, |root| root.py_3())
            .child(list)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.motion.cancel();
                    window.focus(&view.focus, cx);
                    let hit = view.hit_at_live(event.position);
                    match hit {
                        Some(hit) => {
                            if !event.modifiers.shift {
                                view.anchor_block = view.block_of(hit.start);
                            }
                            view.press(hit.at, event.modifiers.shift, hit.link, window, cx)
                        }
                        // Blank space anywhere in the reading area starts a
                        // fresh gesture and drops the previous selection.
                        None => {
                            view.anchor = 0;
                            view.head = 0;
                            view.dragging = true;
                            view.pressed_link = None;
                            cx.notify();
                        }
                    }
                }),
            )
            // A drag started in the reading area keeps receiving moves even
            // when the pointer leaves this element, so selection can cross
            // blocks and the viewport. `on_drag_move` is the capture-phase
            // channel gpui provides for exactly this (see the scrollbar).
            .on_drag(SelectDrag, |_, _, _, cx| cx.new(|_| EmptyView))
            .on_drag_move(cx.listener(
                move |view, event: &gpui::DragMoveEvent<SelectDrag>, _, cx| {
                    if !view.dragging {
                        return;
                    }
                    let position = event.event.position;
                    if let Some(hit) = view.hit_at_live(position) {
                        view.extend(hit.at, cx);
                    } else {
                        view.extend(view.edge_offset(position), cx);
                    }
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    view.end_drag(cx);
                    view.click(window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.end_drag(cx)),
            )
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                let keystroke = &event.keystroke;
                if !keystroke.modifiers.secondary(cx) {
                    return;
                }
                match keystroke.key.as_str() {
                    "a" => {
                        view.select_all(cx);
                        cx.stop_propagation();
                    }
                    "c" if view.copy(cx) => cx.stop_propagation(),
                    _ => {}
                }
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod review_tests {
    use super::item_marker;

    #[test]
    fn every_nesting_level_uses_the_same_item_marker() {
        assert_eq!(item_marker(Some(3), None, 0), "3.");
        assert_eq!(item_marker(Some(3), None, 2), "5.");
        assert_eq!(item_marker(None, Some(false), 0), "☐");
        assert_eq!(item_marker(None, Some(true), 0), "☑");
        assert_eq!(item_marker(None, None, 0), "•");
    }
}
