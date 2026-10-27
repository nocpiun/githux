//! A read-only Markdown document: one virtualized block list with a real
//! [`ListState`], plus the reader's thin scrollbar over that same list.
//!
//! Extracted and simplified for Githux from Ely's `MarkdownDocument` Preview
//! (`ely-documents` @ `30c66fe1`, MIT). Editing modes, block opening, editor
//! history, attachments and page viewers are not carried over. What remains is
//! what reading a repository file needs: background parsing, only visible
//! blocks built, selection in prose, real scrolling and link callbacks.

use std::sync::Arc;

use ely_core::theme::{ActiveTheme, Radius, TextSize};
use gpui::{
    AnyElement, App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, IntoElement,
    ListAlignment, ListState, Render, SharedString, Task, Window, div, list, prelude::*, relative,
};

use super::{
    ast::{self, Block, Inline},
    inline::ParagraphView,
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
}

/// A link pressed in the document; the owner resolves it.
pub type OnLink = std::rc::Rc<dyn Fn(&str, &mut Window, &mut App) + 'static>;

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
        let view = Self {
            parsed: Arc::new(Parsed::default()),
            text: String::new(),
            list,
            focus,
            on_link: None,
            generation: 0,
            parsing: None,
            pending_top: false,
            hovered: false,
        };
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
                    },
                    generation,
                    cx,
                );
            });
        }));
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

    pub fn on_link(&mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) {
        self.on_link = Some(std::rc::Rc::new(handler));
    }

    fn adopt(&mut self, read: Parsed, generation: u64, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
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

    fn paragraph(&self, ix: usize, inline: &Inline) -> ParagraphView {
        self.paragraph_keyed(
            gpui::ElementId::Name(format!("reader-inline-{ix}").into()),
            inline,
        )
    }

    /// A paragraph with a key unique within the document. Selection and focus
    /// live in the keyed state, so every selectable paragraph needs its own:
    /// sharing a key between table cells would apply one cell's byte offsets
    /// to another cell's text.
    fn paragraph_keyed(&self, key: gpui::ElementId, inline: &Inline) -> ParagraphView {
        let on_link = self.on_link.clone();
        let view = ParagraphView::new(key, inline.clone());
        match on_link {
            Some(on_link) => view.on_link(on_link),
            None => view,
        }
    }

    fn item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(block) = self.parsed.blocks.get(ix) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let mono = theme.mono_family.clone();
        let body = f32::from(theme.text_size(TextSize::Base).to_pixels(window.rem_size()));
        let sm = f32::from(theme.text_size(TextSize::Sm).to_pixels(window.rem_size()));
        let xs = f32::from(theme.text_size(TextSize::Xs).to_pixels(window.rem_size()));
        let radius = theme.radius(Radius::Md);
        let element = match block {
            Block::Paragraph(inline) => div()
                .debug_selector(move || format!("markdown-paragraph-{ix}"))
                .text_size(gpui::px(body))
                .line_height(gpui::px(body * 1.6))
                .text_color(colors.fg)
                .child(self.paragraph(ix, inline))
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
                    .child(self.paragraph(ix, inline))
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
                                        ("quote", ix, at),
                                        block,
                                        &colors,
                                        body,
                                        &mono,
                                        radius,
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
                let code = gpui::StyledText::new(text.clone()).with_highlights(highlights);
                // The label is a real element above the code body. Shifting it
                // up fixes the old centred gap without moving the code rows.
                div()
                    .debug_selector(move || format!("markdown-code-block-{ix}"))
                    .flex()
                    .flex_col()
                    .rounded(radius)
                    .bg(colors.sunken)
                    .px_3()
                    .pb_3()
                    .pt_2()
                    .font_family(mono)
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
                            .child(code),
                    )
                    .into_any_element()
            }
            Block::List { start, items } => {
                let mut list = div().flex().flex_col().gap_2();
                for (at, (task, blocks)) in items.iter().enumerate() {
                    let marker = match (start, task) {
                        (_, Some(true)) => "☑".to_string(),
                        (_, Some(false)) => "☐".to_string(),
                        (Some(first), None) => format!("{}.", first + at as u64),
                        (None, None) => "•".to_string(),
                    };
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
                                                ("list", ix, at * 16 + sub),
                                                block,
                                                &colors,
                                                body,
                                                &mono,
                                                radius,
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                            ),
                    );
                }
                list.into_any_element()
            }
            Block::Table { aligns, head, rows } => {
                let cell =
                    |this: &Self, inline: &Inline, row: usize, column: usize, header: bool| {
                        let mut cell = div()
                            .debug_selector(move || format!("markdown-cell-{ix}-{row}-{column}"))
                            .flex_1()
                            .min_w_0()
                            .px_3()
                            .py_2()
                            .child(this.paragraph_keyed(
                                gpui::ElementId::Name(
                                    format!("reader-cell-{ix}-{row}-{column}").into(),
                                ),
                                inline,
                            ));
                        if aligns.get(column) == Some(&ast::Align::Right) {
                            cell = cell.text_right();
                        } else if aligns.get(column) == Some(&ast::Align::Center) {
                            cell = cell.text_center();
                        }
                        if header {
                            cell = cell
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors.fg_muted);
                        }
                        cell
                    };
                let mut table = div()
                    .flex()
                    .flex_col()
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
                                    ("footnote", ix, at),
                                    block,
                                    &colors,
                                    body,
                                    &mono,
                                    radius,
                                )
                            })
                            .collect::<Vec<_>>(),
                    ),
                )
                .into_any_element(),
        };
        div()
            .debug_selector(move || format!("markdown-block-{ix}"))
            .px_3()
            .py_1()
            .child(element)
            .into_any_element()
    }

    /// A block nested inside a quote, list item or footnote.
    fn nested(
        &self,
        id: (&'static str, usize, usize),
        block: &Block,
        colors: &ely_core::theme::Palette,
        body: f32,
        mono: &SharedString,
        radius: gpui::Rems,
    ) -> AnyElement {
        let key = (id.0, id.1 * 1024 + id.2);
        match block {
            Block::Paragraph(inline) => {
                let on_link = self.on_link.clone();
                let mut view = ParagraphView::new(key, inline.clone());
                if let Some(on_link) = on_link {
                    view = view.on_link(on_link);
                }
                div()
                    .text_size(gpui::px(body))
                    .line_height(gpui::px(body * 1.6))
                    .text_color(colors.fg)
                    .child(view)
                    .into_any_element()
            }
            Block::Code { text, .. } => div()
                .px_3()
                .py_2()
                .rounded(radius)
                .bg(colors.sunken)
                .font_family(mono.clone())
                .child(text.clone())
                .into_any_element(),
            Block::List { items, .. } => div()
                .flex()
                .flex_col()
                .gap_1()
                .children(
                    items
                        .iter()
                        .enumerate()
                        .map(|(at, (task, blocks))| {
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(colors.fg_muted)
                                        .child(if task == &Some(true) { "☑" } else { "•" }),
                                )
                                .child(
                                    div().flex_1().min_w_0().children(
                                        blocks
                                            .iter()
                                            .map(|block| {
                                                self.nested(
                                                    (id.0, id.1, id.2 + at + 1),
                                                    block,
                                                    colors,
                                                    body,
                                                    mono,
                                                    radius,
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
            other => div()
                .text_size(gpui::px(body))
                .text_color(colors.fg_muted)
                .child(match other {
                    Block::Image { alt, .. } => format!("{alt} — picture not loaded"),
                    Block::Html(text) => text.clone(),
                    Block::Math { tex, .. } => tex.clone(),
                    Block::Heading(_, inline) => inline.text.clone(),
                    Block::Quote { .. } | Block::Table { .. } | Block::Footnote { .. } => {
                        String::new()
                    }
                    Block::Paragraph(_) | Block::Code { .. } | Block::List { .. } | Block::Rule => {
                        String::new()
                    }
                })
                .into_any_element(),
        }
    }
}

impl Render for MarkdownView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let list = list(
            self.list.clone(),
            cx.processor(|view: &mut Self, ix: usize, window: &mut Window, cx| {
                view.item(ix, window, cx)
            }),
        )
        .size_full();
        div()
            .id("reader-markdown")
            .debug_selector(|| "reader-markdown".to_string())
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .min_h_0()
            .bg(colors.surface)
            .px_2()
            .py_3()
            .child(list)
            .into_any_element()
    }
}
