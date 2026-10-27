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
    ast::{self, Align, Block, Inline},
    doctext::DocText,
    inline::{LocalLayout, ParagraphView},
};

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
            anchor_block: None,
            paragraphs: Default::default(),
            paragraph_ranges: Default::default(),
            block_bounds: Default::default(),
            visible_units: Default::default(),
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

    /// Replaces the text; short texts parse at once, longer ones in the
    /// background pool.
    pub fn reset(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
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

    /// Takes a completed read, unless a newer document replaced it. The
    /// selection is cleared because old offsets belong to the old text.
    fn adopt(&mut self, mut read: Parsed, generation: u64, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        read.blocks = flatten(read.blocks);
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
                        let mut cell = div()
                            .debug_selector(move || format!("markdown-cell-{ix}-{row}-{column}"))
                            .flex_1()
                            .min_w_0()
                            .px_3()
                            .py_2()
                            .child(if repeated && header {
                                // A repeated chunk header is presentation only:
                                // its document range is empty, so it must not
                                // take part in offset hit testing. It still
                                // consumes its unit to keep the cursor in step.
                                let _ = this.next_unit(ix, &unit_cursor);
                                div().child(inline.text.clone()).into_any_element()
                            } else {
                                this.paragraph_unit(
                                    gpui::ElementId::Name(
                                        format!("reader-cell-{ix}-{row}-{column}").into(),
                                    ),
                                    inline,
                                    this.next_unit(ix, &unit_cursor),
                                )
                                .align(table_align(aligns, column))
                                .into_any_element()
                            });
                        if header {
                            cell = cell
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors.fg_muted);
                        }
                        cell
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
            Block::Image { alt, url } => div()
                .debug_selector(move || format!("markdown-picture-note-{ix}"))
                .px_3()
                .py_2()
                .rounded(radius)
                .bg(colors.sunken)
                .text_size(gpui::px(sm))
                .text_color(colors.fg_muted)
                .child(if alt.is_empty() {
                    format!("picture not loaded: {url}")
                } else {
                    format!("{alt} — picture not loaded")
                })
                .into_any_element(),
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
        div()
            .debug_selector(move || format!("markdown-block-{ix}"))
            .px_3()
            .py_1()
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
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_3()
                            .py_2()
                            .child(if repeated && header {
                                let _ = this.next_unit(ix, units);
                                div().child(inline.text.clone()).into_any_element()
                            } else {
                                this.paragraph_unit(
                                    gpui::ElementId::Name(
                                        format!("reader-nested-cell-{}-{row}-{column}", id.1)
                                            .into(),
                                    ),
                                    inline,
                                    this.next_unit(ix, units),
                                )
                                .align(table_align(aligns, column))
                                .into_any_element()
                            })
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
            other => div()
                .text_size(gpui::px(body))
                .text_color(colors.fg_muted)
                .child(match other {
                    Block::Image { alt, .. } => format!("{alt} — picture not loaded"),
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
        ParagraphView::new(key, inline.clone(), doc, self.selection(), layout)
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

impl Render for MarkdownView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        // A new frame: only paragraphs laid out below are hit-testable.
        self.visible_units.borrow_mut().clear();
        let list = list(
            self.list.clone(),
            cx.processor(|view: &mut Self, ix: usize, window: &mut Window, cx| {
                view.item(ix, window, cx)
            }),
        )
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
            .px_2()
            .py_3()
            .child(list)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
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
