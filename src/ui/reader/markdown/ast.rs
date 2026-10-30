//! Markdown read into blocks, with inline styles kept as byte ranges.
//!
//! Extracted and rewritten for Githux from Ely's `MarkdownDocument` reader
//! (`ely-documents` @ `30c66fe1`, MIT). The syntax tree is produced by
//! `pulldown-cmark` rather than Ely's mdast so the reader owns the parser too.
//! Only the read-only subset is kept: no editing blocks, tables or history.
//!
//! Raw HTML is normalised by [`super::html`]: comments never reach the model,
//! inline tags become the same styles/links the Markdown syntax produces, and
//! block tags map onto the existing `Block` variants. HTML inside code spans
//! and fenced code is untouched because those arrive as `Code`/`CodeBlock`
//! events, not HTML events.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::html::{self, Element, Node};

/// How a stretch of inline text prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Strong,
    Emphasis,
    Strike,
    /// An inline code span: the reader gives it its own font and padding box.
    Code,
    Link,
    Math,
}

/// Inline text with its styled ranges and link addresses, both in bytes of
/// [`Inline::text`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub styles: Vec<(Range<usize>, Style)>,
    pub links: Vec<(Range<usize>, String)>,
}

impl Inline {
    /// Plain text of `ranges`, for copy or a heading title.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.styles.is_empty()
    }
}

/// Table column alignment.
pub type Align = Alignment;

/// A GitHub alert's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    /// The title GitHub shows.
    pub fn title(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }
}

/// One block of the document.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Inline),
    Heading(u8, Inline),
    Quote {
        kind: Option<AlertKind>,
        body: Vec<Block>,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    List {
        start: Option<u64>,
        /// Each item, and whether it is a checked task.
        items: Vec<(Option<bool>, Vec<Block>)>,
    },
    Table {
        aligns: Vec<Align>,
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
        /// Drawn as a repeated page-like header for a virtualized chunk. It is
        /// presentation only, so copying does not repeat it.
        repeated_head: bool,
    },
    Rule,
    Math {
        inline: bool,
        tex: String,
    },
    /// A picture: never loaded. Its alt text is shown as a note.
    Image {
        alt: String,
        url: String,
    },
    /// Raw HTML kept for compatibility with callers that still match on it.
    ///
    /// The parser no longer produces this variant: comments are dropped and
    /// common HTML is converted to the other block types. It stays so the
    /// renderer in `document.rs` (owned by the image task) keeps compiling
    /// until both tasks agree on removing it.
    Html(String),
    Footnote {
        label: String,
        body: Vec<Block>,
    },
}

/// A heading with the block it starts.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub level: u8,
    pub title: String,
    pub block: usize,
}

/// A parsed document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub headings: Vec<Heading>,
}

fn level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alert(kind: Option<pulldown_cmark::BlockQuoteKind>) -> Option<AlertKind> {
    use pulldown_cmark::BlockQuoteKind as K;
    match kind? {
        K::Note => Some(AlertKind::Note),
        K::Tip => Some(AlertKind::Tip),
        K::Important => Some(AlertKind::Important),
        K::Warning => Some(AlertKind::Warning),
        K::Caution => Some(AlertKind::Caution),
    }
}

/// Inline being built, with the styles and links still open around the end.
#[derive(Default)]
struct Building {
    inline: Inline,
    open: Vec<(usize, Open)>,
    /// Pictures met inside this paragraph, shown after it.
    notes: Vec<Block>,
    /// A task list marker waiting for this item's first block.
    task: Option<bool>,
    /// True while this builder belongs to a table cell.
    in_cell: bool,
    /// Inline HTML elements left open between `InlineHtml` events.
    html: Vec<HtmlOpen>,
    /// A comment-only HTML fragment sat between two text runs; the next run
    /// collapses its leading whitespace so the copy reads like the browser.
    html_gap: bool,
    /// An inline element whose whole content is dropped (`script`, `style`,
    /// ...). Its tags and text arrive as separate events, so suppression must
    /// survive the HTML helper returning.
    suppressed: Option<(String, usize)>,
}

enum Open {
    Style(Style),
    Link(String),
    Image { url: String, start: usize },
}

/// One open inline HTML element and the style or link it contributes.
struct HtmlOpen {
    name: String,
    kind: HtmlKind,
    start: usize,
}

enum HtmlKind {
    Style(Style),
    Link(String),
}

impl Building {
    fn push_text(&mut self, text: &str) {
        if self.suppressed.is_some() {
            return;
        }
        if self.html_gap {
            self.html_gap = false;
            let trimmed = text.trim_start_matches(' ');
            if trimmed.len() != text.len()
                && (self.inline.text.is_empty() || self.inline.text.ends_with(' '))
            {
                self.inline.text.push_str(trimmed);
                return;
            }
        }
        self.inline.text.push_str(text);
    }

    /// Convert an inline HTML fragment into text, styles, links and pictures.
    fn push_html(&mut self, fragment: &str) {
        let nodes = html::parse(fragment);
        let text = self.inline.text.len();
        let notes = self.notes.len();
        let opens = self.html.len();
        for node in &nodes {
            if let Some((name, depth)) = self.suppressed.clone() {
                // Nested same-name elements must close in order, so the span
                // ends only when its outermost copy closes.
                match node {
                    Node::Element(element) if element.name == name && !element.closed => {
                        self.suppressed = Some((name, depth + 1));
                    }
                    Node::Suppressed(open) if *open == name && !html::is_raw_text(&name) => {
                        self.suppressed = Some((name, depth + 1));
                    }
                    Node::End(end) if *end == name => {
                        if depth > 1 {
                            self.suppressed = Some((name, depth - 1));
                        } else {
                            self.suppressed = None;
                        }
                    }
                    // Only a real parsed end tag counts: a `</scripture>` or a
                    // tag inside a comment never ends the span.
                    _ => {}
                }
                continue;
            }
            match node {
                Node::Suppressed(name) => self.suppressed = Some((name.clone(), 1)),
                Node::Element(element)
                    if html::is_dropped(&element.name)
                        && !html::is_void(&element.name)
                        && !element.closed =>
                {
                    self.suppressed = Some((element.name.clone(), 1));
                }
                _ => append_html_inline(
                    std::slice::from_ref(node),
                    &mut self.inline,
                    &mut self.notes,
                    &mut self.html,
                    self.in_cell,
                ),
            }
        }
        // A comment or dropped element glues the surrounding text runs.
        self.html_gap =
            self.inline.text.len() == text && self.notes.len() == notes && self.html.len() == opens;
    }

    /// Close any HTML element still open at the end of this builder.
    fn close_html(&mut self) {
        close_html_opens(&mut self.html, &mut self.inline);
    }

    fn close(&mut self, end: usize) {
        if let Some((start, open)) = self.open.pop() {
            match open {
                Open::Style(style) => self.inline.styles.push((start.min(end)..end, style)),
                Open::Link(url) => self.inline.links.push((start.min(end)..end, url)),
                Open::Image { url, .. } => {
                    let start = start.min(end);
                    let alt = self.inline.text[start..end].to_string();
                    self.notes.push(Block::Image { alt, url });
                    if self.in_cell {
                        // A cell has no block list for notes, and images are
                        // never loaded: the description is the only readable
                        // stand-in, so it stays exactly where the image was.
                        self.inline
                            .styles
                            .retain(|(range, _)| range.end <= start || range.start >= end);
                        self.inline
                            .links
                            .retain(|(range, _)| range.end <= start || range.start >= end);
                        return;
                    }
                    self.inline.text.truncate(start);
                    // HTML opened before this image keeps spanning the text
                    // that follows it; only ranges that began inside the
                    // removed alt text are dropped, so no stale byte offset
                    // can split the following UTF-8 characters.
                    self.html.retain(|open| open.start <= start);
                    // The description's formatting belongs to the removed
                    // text. Keeping it would style the following text with
                    // stale ranges, or even cut a multibyte character.
                    self.inline.styles.retain(|(range, _)| range.end <= start);
                    self.inline.links.retain(|(range, _)| range.end <= start);
                }
            }
        }
    }
}

/// An open container collecting blocks.
enum Container {
    Blocks {
        out: Vec<Block>,
        quote: Option<AlertKind>,
    },
    List {
        start: Option<u64>,
        items: Vec<(Option<bool>, Vec<Block>)>,
        current: Vec<Block>,
        /// The task marker seen in this item; nested lists keep their own
        /// container on the stack, so a child cannot clear the parent's.
        task: Option<bool>,
    },
    Table {
        aligns: Vec<Align>,
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
        row: Vec<Inline>,
    },
    /// A table cell holds inline text only.
    Cell(Building),
}

/// Parse `markdown` for reading.
///
/// GitHub's tables, footnotes, strikethrough, task lists, maths and alerts are
/// on; editing extensions are off. HTML is normalized to the existing blocks
/// and inline styles, and pictures retain their URL for the reader pipeline.
pub fn parse(markdown: &str) -> Document {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let mut stack = vec![Container::Blocks {
        out: Vec::new(),
        quote: None,
    }];
    // Paragraph or heading text being collected.
    let mut inline: Option<(Option<u8>, Building)> = None;
    // The dropped element whose closing tag has not arrived yet, with how
    // many same-name copies are still open across fragments.
    let mut dropped: Option<(String, usize)> = None;
    let mut code: Option<(Option<String>, String)> = None;
    let mut html_buffer = String::new();
    let mut footnotes: Vec<String> = Vec::new();
    // Preserve parser bookkeeping for hidden blocks without publishing their
    // empty containers or labels. The destination vector length marks where
    // any output from that hidden block would have been appended.
    let mut hidden_blocks: Vec<(TagEnd, usize)> = Vec::new();

    fn finish_inline(
        inline: &mut Option<(Option<u8>, Building)>,
        stack: &mut [Container],
        dropped: &mut Option<(String, usize)>,
    ) {
        let Some((heading, mut building)) = inline.take() else {
            return;
        };
        // A dropped element opened by inline HTML must keep suppressing after
        // this paragraph ends: the rest of it is ordinary Markdown events.
        if dropped.is_none() {
            *dropped = building.suppressed.clone();
        }
        while !building.open.is_empty() {
            let end = building.inline.text.len();
            building.close(end);
        }
        building.close_html();
        let notes = std::mem::take(&mut building.notes);
        let inline = building.inline;
        // A table cell collects inline text only.
        if let Some(Container::Cell(cell)) = stack.last_mut() {
            let at = cell.inline.text.len();
            if at > 0 {
                cell.inline.text.push(' ');
            }
            cell.inline.text.push_str(&inline.text);
            cell.inline
                .styles
                .extend(inline.styles.into_iter().map(|(range, style)| {
                    let shift = cell.inline.text.len() - inline.text.len();
                    (range.start + shift..range.end + shift, style)
                }));
            cell.inline
                .links
                .extend(inline.links.into_iter().map(|(range, url)| {
                    let shift = cell.inline.text.len() - inline.text.len();
                    (range.start + shift..range.end + shift, url)
                }));
            return;
        }
        let out = match stack.last_mut() {
            Some(Container::Blocks { out, .. }) => out,
            Some(Container::List { current, .. }) => current,
            _ => return,
        };
        if let Some(level) = heading {
            out.push(Block::Heading(level, inline));
        } else if !inline.is_empty() {
            out.push(Block::Paragraph(inline));
        }
        out.extend(notes);
    }

    for event in Parser::new_ext(markdown, options) {
        if dropped.is_some()
            && let Event::Start(tag) = &event
            && matches!(
                tag,
                Tag::CodeBlock(_)
                    | Tag::List(_)
                    | Tag::BlockQuote(_)
                    | Tag::Table(_)
                    | Tag::FootnoteDefinition(_)
            )
        {
            hidden_blocks.push((tag.to_end(), output_len(&stack)));
        }
        // Dropped HTML can span several pulldown-cmark blocks: a blank line
        // ends one `HtmlBlock`, and the elements inside it arrive as ordinary
        // Markdown events. Finalizing that block must not end the suppression,
        // so this state lives here rather than only inside an inline builder.
        // Inline HTML is handled by its builder, which owns the same spans
        // between `InlineHtml` events.
        if dropped.is_some()
            && !matches!(
                &event,
                Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock)
            )
        {
            // Block HTML arrives line by line, so its closing tag must be
            // judged with the whole block: a `</template>` inside a multiline
            // comment is not a real close. Buffer the block and decide at its
            // end. Inline HTML (a closing tag inside a paragraph) is complete
            // by itself.
            match &event {
                Event::Html(text) => {
                    // Accumulate only. Searching the growing prefix on every
                    // line would be quadratic; the block is judged once, at
                    // `End(HtmlBlock)`.
                    html_buffer.push_str(text);
                    continue;
                }
                Event::InlineHtml(text) => {
                    let (name, depth) = dropped.clone().expect("checked above");
                    let span = html::dropped_span(text, &name, depth);
                    if let Some(start) = span.tail_start {
                        if span.depth == 0 {
                            dropped = None;
                        } else {
                            dropped = Some((name, span.depth));
                        }
                        let tail = &text[start..];
                        apply_block_suffix(&mut stack, &mut dropped, tail);
                    } else {
                        dropped = Some((name, span.depth));
                    }
                    continue;
                }
                // Keep every block container balanced, including tables and
                // footnotes. Inline output remains suppressed, but their end
                // events must never pop a container that was never opened.
                Event::Start(
                    Tag::Emphasis
                    | Tag::Strong
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. },
                )
                | Event::End(
                    TagEnd::Emphasis
                    | TagEnd::Strong
                    | TagEnd::Strikethrough
                    | TagEnd::Link
                    | TagEnd::Image,
                ) => continue,
                Event::Start(_) | Event::End(_) => {}
                _ => continue,
            }
        }
        // Inside a table cell an image note has nowhere to go, so the alt
        // text must stay in place instead of being moved after the cell.
        let in_cell = matches!(stack.last(), Some(Container::Cell(_)));
        let suppressed = inline
            .as_ref()
            .is_some_and(|(_, building)| building.suppressed.is_some())
            || matches!(stack.last(), Some(Container::Cell(cell)) if cell.suppressed.is_some());
        // Dropped HTML spans contain Markdown events too. Ignore their entire
        // inline output, including image notes, rather than only their text.
        // HTML events still reach the builder to recognize the closing tag;
        // block boundaries still finalize their owning containers.
        if suppressed
            && matches!(
                &event,
                Event::Text(_)
                    | Event::Code(_)
                    | Event::InlineMath(_)
                    | Event::DisplayMath(_)
                    | Event::SoftBreak
                    | Event::HardBreak
                    | Event::FootnoteReference(_)
                    | Event::TaskListMarker(_)
                    | Event::Start(
                        Tag::Emphasis
                            | Tag::Strong
                            | Tag::Strikethrough
                            | Tag::Link { .. }
                            | Tag::Image { .. }
                    )
                    | Event::End(
                        TagEnd::Emphasis
                            | TagEnd::Strong
                            | TagEnd::Strikethrough
                            | TagEnd::Link
                            | TagEnd::Image
                    )
            )
        {
            continue;
        }
        let closing_event = match &event {
            Event::End(end) => Some(*end),
            _ => None,
        };
        match event {
            Event::Start(Tag::Paragraph) => inline = Some((None, Building::default())),
            Event::End(TagEnd::Paragraph) => finish_inline(&mut inline, &mut stack, &mut dropped),
            Event::Start(Tag::Heading { level: l, .. }) => {
                inline = Some((Some(level(l)), Building::default()))
            }
            Event::End(TagEnd::Heading(_)) => finish_inline(&mut inline, &mut stack, &mut dropped),
            Event::Start(Tag::Emphasis) => open(&mut inline, Open::Style(Style::Emphasis), in_cell),
            Event::Start(Tag::Strong) => open(&mut inline, Open::Style(Style::Strong), in_cell),
            Event::Start(Tag::Strikethrough) => {
                open(&mut inline, Open::Style(Style::Strike), in_cell)
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                open(&mut inline, Open::Link(dest_url.to_string()), in_cell)
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let start = inline.as_ref().map_or(0, |(_, b)| b.inline.text.len());
                open(
                    &mut inline,
                    Open::Image {
                        url: dest_url.to_string(),
                        start,
                    },
                    in_cell,
                );
            }
            Event::End(TagEnd::Emphasis)
            | Event::End(TagEnd::Strong)
            | Event::End(TagEnd::Strikethrough)
            | Event::End(TagEnd::Link)
            | Event::End(TagEnd::Image) => close(&mut inline),
            Event::Text(text) => match (&mut code, &mut inline) {
                (Some((_, raw)), _) => raw.push_str(&text),
                (None, Some((_, building))) => building.push_text(&text),
                (None, None) => {
                    // Tight list items and table cells hold text with no
                    // paragraph of their own.
                    if let Some(Container::Cell(cell)) = stack.last_mut() {
                        cell.push_text(&text);
                    } else {
                        inline = Some((None, Building::default()));
                        if let Some((_, building)) = inline.as_mut() {
                            building.push_text(&text);
                        }
                    }
                }
            },
            Event::Code(text) if inline.is_none() => {
                if let Some(Container::Cell(cell)) = stack.last_mut() {
                    let start = cell.inline.text.len();
                    cell.inline.text.push_str(&text);
                    let end = cell.inline.text.len();
                    cell.inline.styles.push((start..end, Style::Code));
                } else {
                    with_inline(&mut inline, |building| {
                        let start = building.inline.text.len();
                        building.push_text(&text);
                        let end = building.inline.text.len();
                        building.inline.styles.push((start..end, Style::Code));
                    });
                }
            }
            Event::Code(text) => with_inline(&mut inline, |building| {
                let start = building.inline.text.len();
                building.push_text(&text);
                let end = building.inline.text.len();
                building.inline.styles.push((start..end, Style::Code));
            }),
            Event::InlineMath(tex) => {
                with_inline(&mut inline, |building| {
                    let start = building.inline.text.len();
                    building.push_text(&tex);
                    let end = building.inline.text.len();
                    building.inline.styles.push((start..end, Style::Math));
                });
            }
            Event::DisplayMath(tex) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                push_block(
                    &mut stack,
                    Block::Math {
                        inline: false,
                        tex: tex.to_string(),
                    },
                );
            }
            Event::SoftBreak => with_inline(&mut inline, |building| building.push_text(" ")),
            Event::HardBreak => with_inline(&mut inline, |building| building.push_text("\n")),
            Event::FootnoteReference(label) => with_inline(&mut inline, |building| {
                building.push_text(&format!("[{label}]"));
            }),
            Event::Rule => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                push_block(&mut stack, Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Container::List { task, .. }) = stack.last_mut() {
                    *task = Some(done);
                }
            }
            Event::Start(Tag::BlockQuote(kind)) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                stack.push(Container::Blocks {
                    out: Vec::new(),
                    quote: alert(kind),
                });
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                let Some(Container::Blocks { out, quote }) = stack.pop() else {
                    continue;
                };
                push_block(
                    &mut stack,
                    Block::Quote {
                        kind: quote,
                        body: out,
                    },
                );
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let name = info.trim();
                        (!name.is_empty()).then(|| name.to_string())
                    }
                    CodeBlockKind::Indented => None,
                };
                code = Some((language, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, mut text)) = code.take() {
                    while text.ends_with('\n') {
                        text.pop();
                    }
                    push_block(&mut stack, Block::Code { language, text });
                }
            }
            Event::Start(Tag::List(start)) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                stack.push(Container::List {
                    start,
                    items: Vec::new(),
                    current: Vec::new(),
                    task: None,
                });
            }
            Event::Start(Tag::Item) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
            }
            Event::End(TagEnd::Item) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                if let Some(Container::List {
                    items,
                    current,
                    task,
                    ..
                }) = stack.last_mut()
                {
                    items.push((task.take(), std::mem::take(current)));
                }
            }
            Event::End(TagEnd::List(_)) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                if let Some(Container::List { start, items, .. }) = stack.pop() {
                    push_block(&mut stack, Block::List { start, items });
                }
            }
            Event::Start(Tag::Table(aligns)) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                stack.push(Container::Table {
                    aligns,
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Event::Start(Tag::TableCell) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                stack.push(Container::Cell(Building {
                    in_cell: true,
                    ..Building::default()
                }));
                // Keep every inline event in one builder from the first cell
                // byte onward. Starting one only when Markdown emphasis opens
                // splits the HTML stack and loses closing-tag positions.
                inline = Some((
                    None,
                    Building {
                        in_cell: true,
                        ..Building::default()
                    },
                ));
            }
            Event::End(TagEnd::TableCell) => {
                // Collect the single builder used by this cell.
                if let Some(Container::Cell(cell)) = stack.last_mut()
                    && let Some((_, pending)) = inline.take()
                {
                    if dropped.is_none() {
                        dropped = pending.suppressed.clone();
                    }
                    merge_cell_builder(cell, pending);
                }
                let Some(Container::Cell(mut building)) = stack.pop() else {
                    continue;
                };
                building.close_html();
                while !building.open.is_empty() {
                    let end = building.inline.text.len();
                    building.close(end);
                }
                // A cell has no block list to attach image notes to. Images
                // are never loaded, so their alternative text is the only
                // readable content: keep it in the cell instead of dropping
                // it with the note.
                for note in std::mem::take(&mut building.notes) {
                    if let Block::Image { alt, .. } = note
                        && !alt.is_empty()
                    {
                        building.inline.text.push_str(&alt);
                    }
                }
                if let Some(Container::Table { row, .. }) = stack.last_mut() {
                    row.push(building.inline);
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(Container::Table { head, row, .. }) = stack.last_mut() {
                    *head = std::mem::take(row);
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(Container::Table { rows, row, .. }) = stack.last_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(Container::Table {
                    aligns, head, rows, ..
                }) = stack.pop()
                {
                    push_block(
                        &mut stack,
                        Block::Table {
                            aligns,
                            head,
                            rows,
                            repeated_head: false,
                        },
                    );
                }
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                stack.push(Container::Blocks {
                    out: Vec::new(),
                    quote: None,
                });
                footnotes.push(label.to_string());
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                if let Some(Container::Cell(_)) = stack.last() {
                    stack.pop();
                }
                if let Some(Container::Blocks { out, .. }) = stack.pop() {
                    push_block(
                        &mut stack,
                        Block::Footnote {
                            label: footnotes.pop().unwrap_or_default(),
                            body: out,
                        },
                    );
                }
            }
            Event::Start(Tag::HtmlBlock) => {
                finish_inline(&mut inline, &mut stack, &mut dropped);
                html_buffer.clear();
            }
            // Block HTML is buffered until its closing event. Inline HTML
            // belongs to the surrounding paragraph, tight list item or cell,
            // so it must reach a builder even when none exists yet: those
            // contexts never emit `End(HtmlBlock)`.
            Event::Html(text) => match &mut inline {
                Some((_, building)) => building.push_html(&text),
                None => html_buffer.push_str(&text),
            },
            Event::InlineHtml(text) => match &mut inline {
                Some((_, building)) => building.push_html(&text),
                None => {
                    if let Some(Container::Cell(cell)) = stack.last_mut() {
                        cell.push_html(&text);
                    } else {
                        with_inline(&mut inline, |building| building.push_html(&text));
                    }
                }
            },
            Event::End(TagEnd::HtmlBlock) => {
                // Judge the whole block, not its individual line events: a
                // multiline comment or attribute can contain a tag that looks
                // like a dropped opening but is still markup context.
                let text = std::mem::take(&mut html_buffer);
                if let Some((name, depth)) = dropped.clone() {
                    let span = html::dropped_span(&text, &name, depth);
                    if let Some(start) = span.tail_start {
                        if span.depth == 0 {
                            dropped = None;
                        } else {
                            dropped = Some((name, span.depth));
                        }
                        let tail = text[start..].to_string();
                        apply_block_suffix(&mut stack, &mut dropped, &tail);
                    } else {
                        // Still inside the dropped span; the block is content.
                        dropped = Some((name, span.depth));
                    }
                } else if let Some((open, depth)) = unclosed_dropped(&text) {
                    let head_end = html::outstanding_span(&text, &open).head_end;
                    for block in html_blocks(&text[..head_end]) {
                        push_block(&mut stack, block);
                    }
                    dropped = Some((open, depth));
                } else if !text.trim().is_empty() {
                    for block in html_blocks(&text) {
                        push_block(&mut stack, block);
                    }
                }
            }
            Event::End(_) => {}
            Event::Start(_) => {}
        }
        if let Some(end) = closing_event
            && hidden_blocks.last().is_some_and(|(tag, _)| *tag == end)
        {
            let (_, len) = hidden_blocks.pop().expect("matched hidden block");
            // If suppression ended inside this block, retain newly visible
            // content; otherwise discard its purely hidden empty shell.
            if dropped.is_some() {
                truncate_output(&mut stack, len);
            }
        }
    }
    finish_inline(&mut inline, &mut stack, &mut dropped);
    let blocks = match stack.pop() {
        Some(Container::Blocks { out, .. }) => out,
        _ => Vec::new(),
    };
    let mut headings = Vec::new();
    for (ix, block) in blocks.iter().enumerate() {
        if let Block::Heading(level, inline) = block {
            headings.push(Heading {
                level: *level,
                title: inline.text.clone(),
                block: ix,
            });
        }
    }
    Document { blocks, headings }
}

fn open(inline: &mut Option<(Option<u8>, Building)>, what: Open, in_cell: bool) {
    with_inline(inline, |building| {
        building.in_cell |= in_cell;
        let start = building.inline.text.len();
        building.open.push((start, what));
    });
}

fn close(inline: &mut Option<(Option<u8>, Building)>) {
    with_inline(inline, |building| {
        let end = building.inline.text.len();
        building.close(end);
    });
}

fn with_inline(inline: &mut Option<(Option<u8>, Building)>, edit: impl FnOnce(&mut Building)) {
    if inline.is_none() {
        *inline = Some((None, Building::default()));
    }
    if let Some((_, building)) = inline.as_mut() {
        edit(building);
    }
}

/// Merge a cell's pending builder into the cell while re-basing its open
/// Markdown and HTML stacks, so mixed content like `<a href="…">read *this*</a>`
/// keeps its HTML link after the emphasis builder merges back.
fn merge_cell_builder(cell: &mut Building, mut pending: Building) {
    let at = cell.inline.text.len();
    for (start, _open) in &mut pending.open {
        *start += at;
    }
    for open in &mut pending.html {
        open.start += at;
    }
    cell.open.append(&mut pending.open);
    cell.html.append(&mut pending.html);
    cell.inline.text.push_str(&pending.inline.text);
    cell.inline.styles.extend(
        pending
            .inline
            .styles
            .into_iter()
            .map(|(range, style)| (range.start + at..range.end + at, style)),
    );
    cell.inline.links.extend(
        pending
            .inline
            .links
            .into_iter()
            .map(|(range, url)| (range.start + at..range.end + at, url)),
    );
    // A cell keeps a picture's alt text in place; it has no notes to attach.
    if !cell.in_cell {
        cell.notes.append(&mut pending.notes);
    }
}

fn output_len(stack: &[Container]) -> usize {
    match stack.last() {
        Some(Container::Blocks { out, .. }) => out.len(),
        Some(Container::List { current, .. }) => current.len(),
        _ => 0,
    }
}

fn truncate_output(stack: &mut [Container], len: usize) {
    match stack.last_mut() {
        Some(Container::Blocks { out, .. }) => out.truncate(len),
        Some(Container::List { current, .. }) => current.truncate(len),
        _ => {}
    }
}

fn push_block(stack: &mut [Container], block: Block) {
    match stack.last_mut() {
        Some(Container::Blocks { out, .. }) => out.push(block),
        Some(Container::List { current, .. }) => current.push(block),
        _ => {}
    }
}

/// Convert a buffered block-HTML fragment into the reader's own blocks.
fn html_blocks(fragment: &str) -> Vec<Block> {
    let nodes = html::parse(fragment);
    let mut out = Vec::new();
    blocks_from_nodes(&nodes, &mut out);
    out
}

/// The dropped element a fragment opens without closing, if any, with the
/// number of same-name copies it leaves open.
///
/// A block-HTML fragment can end at a blank line while the element is still
/// open; its content continues as ordinary Markdown, so the caller keeps
/// suppressing until the matching end tag arrives in a later fragment.
fn unclosed_dropped(fragment: &str) -> Option<(String, usize)> {
    let nodes = html::parse(fragment);
    unclosed_node(&nodes, fragment)
}

fn unclosed_node(nodes: &[Node], fragment: &str) -> Option<(String, usize)> {
    for node in nodes.iter().rev() {
        match node {
            Node::Suppressed(name) => {
                return Some((name.clone(), html::outstanding_span(fragment, name).depth));
            }
            Node::Element(element)
                if html::is_dropped(&element.name)
                    && !html::is_void(&element.name)
                    && !element.closed =>
            {
                return Some((
                    element.name.clone(),
                    html::outstanding_span(fragment, &element.name).depth,
                ));
            }
            Node::Element(element) => {
                if let Some(open) = unclosed_node(&element.children, fragment) {
                    return Some(open);
                }
            }
            _ => {}
        }
    }
    None
}

/// Convert the readable text after a dropped close, then resume suppression if
/// that suffix itself opens another dropped element.
fn apply_block_suffix(stack: &mut [Container], dropped: &mut Option<(String, usize)>, tail: &str) {
    if let Some((open, depth)) = unclosed_dropped(tail) {
        let head_end = html::outstanding_span(tail, &open).head_end;
        for block in html_blocks(&tail[..head_end]) {
            push_block(stack, block);
        }
        *dropped = Some((open, depth));
    } else if !tail.trim().is_empty() {
        for block in html_blocks(tail) {
            push_block(stack, block);
        }
    }
}

/// Walk `nodes` as block content, turning inline runs into paragraphs.
fn blocks_from_nodes(nodes: &[Node], out: &mut Vec<Block>) {
    let mut run = Inline::default();
    let mut notes = Vec::new();
    let mut opens = Vec::new();
    for node in nodes {
        match node {
            Node::Text(text) => push_html_text(&mut run, text),
            Node::End(_) | Node::Suppressed(_) => {}
            Node::Element(element) if html::is_block(&element.name) => {
                flush_html_run(&mut run, &mut notes, &mut opens, out);
                block_from_element(element, out);
            }
            Node::Element(element) => {
                append_html_element(element, &mut run, &mut notes, &mut opens, false);
            }
        }
    }
    flush_html_run(&mut run, &mut notes, &mut opens, out);
}

fn flush_html_run(
    run: &mut Inline,
    notes: &mut Vec<Block>,
    opens: &mut Vec<HtmlOpen>,
    out: &mut Vec<Block>,
) {
    close_html_opens(opens, run);
    trim_inline_end(run);
    if !run.is_empty() {
        out.push(Block::Paragraph(std::mem::take(run)));
    }
    out.append(notes);
}

/// Build the inline content of a heading or table cell.
///
/// `in_cell` keeps a picture's alternative text where the picture was; a
/// heading returns the pictures as notes for the caller to place after it.
fn inline_of_nodes(nodes: &[Node], in_cell: bool) -> (Inline, Vec<Block>) {
    let mut inline = Inline::default();
    let mut notes = Vec::new();
    let mut opens = Vec::new();
    append_html_inline(nodes, &mut inline, &mut notes, &mut opens, in_cell);
    close_html_opens(&mut opens, &mut inline);
    trim_inline_end(&mut inline);
    (inline, notes)
}

fn block_from_element(element: &Element, out: &mut Vec<Block>) {
    let name = element.name.as_str();
    if html::is_dropped(name) {
        return;
    }
    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = name.as_bytes()[1] - b'0';
            let (inline, notes) = inline_of_nodes(&element.children, false);
            if !inline.is_empty() {
                out.push(Block::Heading(level, inline));
            }
            out.extend(notes);
        }
        "hr" => out.push(Block::Rule),
        "img" => push_html_image(element, out),
        "br" => {}
        "pre" => push_html_code(element, out),
        "blockquote" => {
            let mut body = Vec::new();
            blocks_from_nodes(&element.children, &mut body);
            out.push(Block::Quote { kind: None, body });
        }
        "ul" | "ol" => push_html_list(element, out),
        "table" => {
            if let Some(table) = html_table(element) {
                out.push(table);
            } else {
                // A malformed or over-budget table still reads as text instead
                // of allocating an enormous padded grid.
                blocks_from_nodes(&element.children, out);
            }
        }
        "summary" => {
            // `<details><summary>` reads as a bold line before the body.
            let (mut inline, notes) = inline_of_nodes(&element.children, false);
            if !inline.text.is_empty() {
                inline.styles.push((0..inline.text.len(), Style::Strong));
            }
            if !inline.is_empty() {
                out.push(Block::Paragraph(inline));
            }
            out.extend(notes);
        }
        _ => blocks_from_nodes(&element.children, out),
    }
}

fn push_html_image(element: &Element, out: &mut Vec<Block>) {
    let alt = element.attr("alt").unwrap_or_default();
    let url = element.attr("src").unwrap_or_default().trim();
    if url.is_empty() {
        if !alt.trim().is_empty() {
            out.push(Block::Paragraph(Inline {
                text: alt.to_string(),
                ..Inline::default()
            }));
        }
        return;
    }
    out.push(Block::Image {
        alt: alt.to_string(),
        url: url.to_string(),
    });
}

fn push_html_code(element: &Element, out: &mut Vec<Block>) {
    let language = find_code_language(&element.children);
    let mut text = html::raw_text(&element.children);
    if let Some(stripped) = text.strip_prefix('\n') {
        text = stripped.to_string();
    }
    while text.ends_with('\n') {
        text.pop();
    }
    out.push(Block::Code { language, text });
}

fn find_code_language(nodes: &[Node]) -> Option<String> {
    for node in nodes {
        let Node::Element(element) = node else {
            continue;
        };
        if element.name == "code" {
            let class = element.attr("class")?;
            for token in class.split_whitespace() {
                if let Some(language) = token
                    .strip_prefix("language-")
                    .or_else(|| token.strip_prefix("lang-"))
                {
                    return Some(language.to_string());
                }
            }
        }
        if let Some(language) = find_code_language(&element.children) {
            return Some(language);
        }
    }
    None
}

fn push_html_list(element: &Element, out: &mut Vec<Block>) {
    let start = if element.name == "ol" {
        Some(
            element
                .attr("start")
                .and_then(|value| value.trim().parse::<u64>().ok())
                // The renderer adds the item index to this number, and a list
                // may hold as many items as the content budget allows. Cap it
                // so `first + index` can never overflow.
                .map(|first| first.min(html::MAX_LIST_START))
                .unwrap_or(1),
        )
    } else {
        None
    };
    let mut items = Vec::new();
    for child in &element.children {
        let Node::Element(item) = child else {
            continue;
        };
        if item.name != "li" {
            continue;
        }
        let mut body = Vec::new();
        blocks_from_nodes(&item.children, &mut body);
        items.push((None, body));
    }
    if !items.is_empty() {
        out.push(Block::List { start, items });
    }
}

/// Upper bound on padded table cells (rows plus header times widest row).
///
/// One wide row followed by thousands of narrow rows would otherwise expand a
/// few hundred kilobytes of HTML into gigabytes of `Inline` values.
const MAX_TABLE_CELLS: usize = 65_536;

fn html_table(element: &Element) -> Option<Block> {
    let mut head = Vec::new();
    let mut rows = Vec::new();
    let mut aligns = Vec::new();
    collect_table_rows(element, &mut head, &mut rows, &mut aligns);
    if head.is_empty() && !rows.is_empty() {
        head = rows.remove(0);
    }
    if head.is_empty() {
        return None;
    }
    let width = head
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(aligns.len());
    if width.saturating_mul(rows.len().saturating_add(1)) > MAX_TABLE_CELLS {
        return None;
    }
    aligns.resize(width, Align::None);
    head.resize(width, Inline::default());
    for row in rows.iter_mut() {
        row.resize(width, Inline::default());
    }
    Some(Block::Table {
        aligns,
        head,
        rows,
        repeated_head: false,
    })
}

fn collect_table_rows(
    element: &Element,
    head: &mut Vec<Inline>,
    rows: &mut Vec<Vec<Inline>>,
    aligns: &mut Vec<Align>,
) {
    for child in &element.children {
        let Node::Element(node) = child else {
            continue;
        };
        match node.name.as_str() {
            "tr" => {
                let mut cells = Vec::new();
                let mut row_aligns = Vec::new();
                let mut has_th = false;
                for cell in &node.children {
                    let Node::Element(cell) = cell else {
                        continue;
                    };
                    if cell.name != "td" && cell.name != "th" {
                        continue;
                    }
                    has_th |= cell.name == "th";
                    let (inline, _) = inline_of_nodes(&cell.children, true);
                    cells.push(inline);
                    row_aligns.push(cell_align(cell));
                }
                if cells.is_empty() {
                    continue;
                }
                if head.is_empty() && rows.is_empty() && has_th {
                    *head = cells;
                } else {
                    rows.push(cells);
                }
                for (column, align) in row_aligns.into_iter().enumerate() {
                    if align == Align::None {
                        continue;
                    }
                    if aligns.len() <= column {
                        aligns.resize(column + 1, Align::None);
                    }
                    if aligns[column] == Align::None {
                        aligns[column] = align;
                    }
                }
            }
            "thead" | "tbody" | "tfoot" => collect_table_rows(node, head, rows, aligns),
            _ => {}
        }
    }
}

fn cell_align(cell: &Element) -> Align {
    if let Some(align) = cell.attr("align") {
        return match align.to_ascii_lowercase().as_str() {
            "center" => Align::Center,
            "right" => Align::Right,
            "left" => Align::Left,
            _ => Align::None,
        };
    }
    if let Some(style) = cell.attr("style") {
        let style = style.to_ascii_lowercase();
        if style.contains("text-align:center") || style.contains("text-align: center") {
            return Align::Center;
        }
        if style.contains("text-align:right") || style.contains("text-align: right") {
            return Align::Right;
        }
        if style.contains("text-align:left") || style.contains("text-align: left") {
            return Align::Left;
        }
    }
    Align::None
}

/// Append the readable content of parsed HTML nodes to an inline run.
fn append_html_inline(
    nodes: &[Node],
    inline: &mut Inline,
    notes: &mut Vec<Block>,
    opens: &mut Vec<HtmlOpen>,
    in_cell: bool,
) {
    for node in nodes {
        match node {
            Node::Text(text) => push_html_text(inline, text),
            Node::End(name) => close_html(opens, name, inline),
            Node::Suppressed(_) => {}
            Node::Element(element) => append_html_element(element, inline, notes, opens, in_cell),
        }
    }
}

fn append_html_element(
    element: &Element,
    inline: &mut Inline,
    notes: &mut Vec<Block>,
    opens: &mut Vec<HtmlOpen>,
    in_cell: bool,
) {
    let name = element.name.as_str();
    if html::is_dropped(name) {
        return;
    }
    let style = match name {
        "code" | "kbd" | "samp" => Some(HtmlKind::Style(Style::Code)),
        "strong" | "b" => Some(HtmlKind::Style(Style::Strong)),
        "em" | "i" => Some(HtmlKind::Style(Style::Emphasis)),
        "del" | "s" | "strike" => Some(HtmlKind::Style(Style::Strike)),
        _ => None,
    };
    if let Some(kind) = style {
        return open_html(element, kind, inline, notes, opens, in_cell);
    }
    match name {
        "br" => inline.text.push('\n'),
        "wbr" | "hr" => {}
        "img" => {
            let alt = element.attr("alt").unwrap_or_default();
            let url = element.attr("src").unwrap_or_default().trim();
            if url.is_empty() || in_cell {
                push_html_text(inline, alt);
            } else {
                notes.push(Block::Image {
                    alt: alt.to_string(),
                    url: url.to_string(),
                });
            }
        }
        "a" => match element.attr("href").filter(|href| !href.is_empty()) {
            Some(href) => open_html(
                element,
                HtmlKind::Link(href.to_string()),
                inline,
                notes,
                opens,
                in_cell,
            ),
            None => append_html_inline(&element.children, inline, notes, opens, in_cell),
        },
        name if html::is_block(name) => {
            // A block inside inline content (for example a `<p>` in a table
            // cell) still reads as one run, separated by a space.
            push_html_text(inline, " ");
            append_html_inline(&element.children, inline, notes, opens, in_cell);
            push_html_text(inline, " ");
        }
        _ => append_html_inline(&element.children, inline, notes, opens, in_cell),
    }
}

fn open_html(
    element: &Element,
    kind: HtmlKind,
    inline: &mut Inline,
    notes: &mut Vec<Block>,
    opens: &mut Vec<HtmlOpen>,
    in_cell: bool,
) {
    let start = inline.text.len();
    // The HTML parser bounds one fragment's tree, but this stack survives
    // across `InlineHtml` events. Bound it too, so an unmatched closing tag
    // never scans an unbounded history of openings.
    if opens.len() >= html::MAX_DEPTH {
        return;
    }
    opens.push(HtmlOpen {
        name: element.name.clone(),
        kind,
        start,
    });
    // A start tag delivered on its own stays open across the following text
    // and end-tag events; a complete element (including an empty one) closes
    // here.
    if !element.children.is_empty() {
        append_html_inline(&element.children, inline, notes, opens, in_cell);
        close_html(opens, &element.name, inline);
    } else if element.closed {
        close_html(opens, &element.name, inline);
    }
}

fn close_html(opens: &mut Vec<HtmlOpen>, name: &str, inline: &mut Inline) {
    let Some(position) = opens.iter().rposition(|open| open.name == name) else {
        return;
    };
    close_html_to(opens, position, inline);
}

fn close_html_opens(opens: &mut Vec<HtmlOpen>, inline: &mut Inline) {
    close_html_to(opens, 0, inline);
}

fn close_html_to(opens: &mut Vec<HtmlOpen>, position: usize, inline: &mut Inline) {
    while opens.len() > position {
        let open = opens.pop().expect("an open HTML element");
        let end = inline.text.len();
        let start = open.start.min(end);
        if start == end {
            continue;
        }
        match open.kind {
            HtmlKind::Style(style) => inline.styles.push((start..end, style)),
            HtmlKind::Link(url) => inline.links.push((start..end, url)),
        }
    }
}

/// Append collapsed HTML text, avoiding doubled or leading spaces.
fn push_html_text(inline: &mut Inline, text: &str) {
    let collapsed = html::collapse_whitespace(text);
    if collapsed.is_empty() {
        return;
    }
    if inline.text.is_empty() {
        let trimmed = collapsed.trim_start_matches(' ');
        if trimmed.is_empty() {
            return;
        }
        inline.text.push_str(trimmed);
    } else if inline.text.ends_with(' ')
        && let Some(rest) = collapsed.strip_prefix(' ')
    {
        inline.text.push_str(rest);
    } else {
        inline.text.push_str(&collapsed);
    }
}

/// Drop trailing spaces and clamp the ranges that pointed past them.
fn trim_inline_end(inline: &mut Inline) {
    let len = inline.text.trim_end_matches(' ').len();
    if len == inline.text.len() {
        return;
    }
    inline.text.truncate(len);
    for (range, _) in inline.styles.iter_mut() {
        range.end = range.end.min(len);
    }
    for (range, _) in inline.links.iter_mut() {
        range.end = range.end.min(len);
    }
    inline.styles.retain(|(range, _)| range.start < range.end);
    inline.links.retain(|(range, _)| range.start < range.end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inline(text: &str) -> Inline {
        let doc = parse(text);
        match &doc.blocks[0] {
            Block::Paragraph(inline) => inline.clone(),
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn inline_styles_and_links_use_byte_ranges() {
        let parsed = inline("a **b** `c` [d](guide.md) ~~e~~ 中文");
        assert_eq!(parsed.text, "a b c d e 中文");
        assert!(parsed.styles.contains(&(2..3, Style::Strong)));
        assert!(parsed.styles.contains(&(4..5, Style::Code)));
        assert!(parsed.styles.contains(&(8..9, Style::Strike)));
        assert_eq!(parsed.links, vec![(6..7, "guide.md".to_string())]);
    }

    #[test]
    fn blocks_cover_the_syntax_the_reader_draws() {
        let doc = parse(
            "# Title\n\n- a\n- b\n\n1. c\n\n> quote\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|:--|--:|\n| 1 | 2 |\n\n---\n\n$m$\n\n[^1]\n\n[^1]: note\n",
        );
        assert!(matches!(doc.blocks[0], Block::Heading(1, _)));
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::List { start: None, .. }))
        );
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::List { start: Some(1), .. }))
        );
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Quote { .. })));
        assert!(doc.blocks.iter().any(|b| matches!(
            b,
            Block::Code { language: Some(lang), text } if lang == "rust" && text.contains("fn main")
        )));
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Table { .. })));
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Rule)));
        assert_eq!(doc.headings[0].title, "Title");
    }

    #[test]
    fn table_cells_keep_their_text() {
        let doc = parse(
            "| a | b |
|:--|--:|
| 1 | 2 |",
        );
        let table = doc.blocks.iter().find_map(|b| match b {
            Block::Table { head, rows, .. } => Some((head, rows)),
            _ => None,
        });
        let (head, rows) = table.expect("a table");
        assert_eq!(head.len(), 2, "{head:?}");
        assert!(
            head.iter().all(|cell| !cell.text.trim().is_empty()),
            "{head:?}"
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(
            rows[0].iter().all(|cell| !cell.text.trim().is_empty()),
            "{rows:?}"
        );
    }

    #[test]
    fn pictures_keep_only_alt_text_and_never_load() {
        let doc = parse("![alt 图](file://attacker/share/a.png)\n\ntext");
        assert!(
            matches!(&doc.blocks[0], Block::Image { alt, url } if alt == "alt 图" && url.contains("file:"))
        );
        assert!(matches!(&doc.blocks[1], Block::Paragraph(p) if p.text == "text"));
    }

    #[test]
    fn script_content_is_dropped_instead_of_shown_as_html() {
        let doc = parse("<script>alert(1)</script>\n\nplain");
        let visible = format!("{:?}", doc.blocks);
        assert!(!visible.contains("alert"), "{visible}");
        assert!(!visible.contains("<script"), "{visible}");
        assert!(visible.contains("plain"), "{visible}");
    }

    #[test]
    fn an_image_in_a_table_cell_keeps_its_alt_text() {
        let doc = parse("| a | b |\n|---|---|\n| ![Windows](w.svg) or ![Linux](l.svg) | x |");
        let cells = match &doc.blocks[0] {
            Block::Table { rows, .. } => &rows[0],
            other => panic!("{other:?}"),
        };
        assert_eq!(
            cells[0].text, "Windows or Linux",
            "the alt text stays where the image was"
        );
    }

    #[test]
    fn inline_html_becomes_text_styles_in_a_tight_item() {
        let doc = parse(
            "- <kbd>Ctrl</kbd>
- plain

| <b>h</b> |
|---|
| <i>x</i> |",
        );
        let text: String = doc
            .blocks
            .iter()
            .map(|block| format!("{block:?}"))
            .collect();
        assert!(!text.contains("<kbd>"), "{text}");
        assert!(!text.contains("<b>"), "{text}");
        assert!(!text.contains("<i>"), "{text}");
        assert!(text.contains("Ctrl"), "{text}");
        assert!(text.contains("h"), "{text}");
        assert!(text.contains("x"), "{text}");
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn nested_lists_keep_their_parent_task_marker() {
        let doc = parse(
            "- [x] parent
  - child
- [ ] second",
        );
        let list = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::List { items, .. } => Some(items),
                _ => None,
            })
            .expect("a list");
        assert_eq!(
            list[0].0,
            Some(true),
            "parent checkbox survives nested item"
        );
        assert_eq!(
            list[1].0,
            Some(false),
            "sibling keeps its own unchecked state"
        );
        let nested = list[0]
            .1
            .iter()
            .find_map(|b| match b {
                Block::List { items, .. } => Some(items),
                _ => None,
            })
            .expect("nested list");
        assert_eq!(nested[0].0, None, "child is an ordinary bullet");
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;

    #[test]
    fn a_table_cell_keeps_emphasis_and_links() {
        let doc = parse(
            "| a | b |
|---|---|
| [Guide](guide.md) | **bold** tail |",
        );
        let (head, rows) = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table { head, rows, .. } => Some((head, rows)),
                _ => None,
            })
            .expect("table");
        let _ = head;
        assert_eq!(rows[0][0].text, "Guide");
        assert_eq!(rows[0][0].links, vec![(0..5, "guide.md".to_string())]);
        assert_eq!(rows[0][1].text, "bold tail");
        assert!(rows[0][1].styles.contains(&(0..4, Style::Strong)));
    }

    #[test]
    fn a_formatted_image_description_cannot_style_or_split_following_text() {
        let doc = parse("![**alt**](img.png)plain é");
        let text = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Paragraph(inline) => Some(inline.clone()),
                _ => None,
            })
            .expect("paragraph");
        assert_eq!(text.text, "plain é");
        assert!(text.styles.is_empty(), "{:?}", text.styles);
        for (range, _) in &text.styles {
            assert!(
                text.text.is_char_boundary(range.start) && text.text.is_char_boundary(range.end)
            );
        }
    }
}

#[cfg(test)]
mod quote_heading_test {
    use super::*;
    #[test]
    fn a_heading_inside_a_quote_keeps_its_link_ranges() {
        let doc = parse("> # [Guide](guide.md)");
        let quote = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Quote { body, .. } => Some(body),
                _ => None,
            })
            .expect("quote");
        let heading = quote
            .iter()
            .find_map(|b| match b {
                Block::Heading(1, inline) => Some(inline),
                _ => None,
            })
            .expect("heading");
        assert_eq!(heading.text, "Guide");
        assert_eq!(heading.links, vec![(0..5, "guide.md".to_string())]);
    }
}
