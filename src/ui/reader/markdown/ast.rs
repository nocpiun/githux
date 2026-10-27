//! Markdown read into blocks, with inline styles kept as byte ranges.
//!
//! Extracted and rewritten for Githux from Ely's `MarkdownDocument` reader
//! (`ely-documents` @ `30c66fe1`, MIT). The syntax tree is produced by
//! `pulldown-cmark` rather than Ely's mdast so the reader owns the parser too.
//! Only the read-only subset is kept: no editing blocks, tables or history.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

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
    /// Raw HTML, shown as inert text.
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
}

enum Open {
    Style(Style),
    Link(String),
    Image { url: String, start: usize },
}

impl Building {
    fn push_text(&mut self, text: &str) {
        self.inline.text.push_str(text);
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
                    self.inline.text.truncate(start);
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
/// on; editing extensions are off. Raw HTML is kept as inert text and pictures
/// only keep their alt text.
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
    let mut code: Option<(Option<String>, String)> = None;
    let mut html = String::new();
    let mut footnotes: Vec<String> = Vec::new();

    fn finish_inline(inline: &mut Option<(Option<u8>, Building)>, stack: &mut [Container]) {
        let Some((heading, mut building)) = inline.take() else {
            return;
        };
        while !building.open.is_empty() {
            let end = building.inline.text.len();
            building.close(end);
        }
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
        match event {
            Event::Start(Tag::Paragraph) => inline = Some((None, Building::default())),
            Event::End(TagEnd::Paragraph) => finish_inline(&mut inline, &mut stack),
            Event::Start(Tag::Heading { level: l, .. }) => {
                inline = Some((Some(level(l)), Building::default()))
            }
            Event::End(TagEnd::Heading(_)) => finish_inline(&mut inline, &mut stack),
            Event::Start(Tag::Emphasis) => open(&mut inline, Open::Style(Style::Emphasis)),
            Event::Start(Tag::Strong) => open(&mut inline, Open::Style(Style::Strong)),
            Event::Start(Tag::Strikethrough) => open(&mut inline, Open::Style(Style::Strike)),
            Event::Start(Tag::Link { dest_url, .. }) => {
                open(&mut inline, Open::Link(dest_url.to_string()))
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let start = inline.as_ref().map_or(0, |(_, b)| b.inline.text.len());
                open(
                    &mut inline,
                    Open::Image {
                        url: dest_url.to_string(),
                        start,
                    },
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
                        cell.inline.text.push_str(&text);
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
                finish_inline(&mut inline, &mut stack);
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
                finish_inline(&mut inline, &mut stack);
                push_block(&mut stack, Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Container::List { task, .. }) = stack.last_mut() {
                    *task = Some(done);
                }
            }
            Event::Start(Tag::BlockQuote(kind)) => {
                finish_inline(&mut inline, &mut stack);
                stack.push(Container::Blocks {
                    out: Vec::new(),
                    quote: alert(kind),
                });
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                finish_inline(&mut inline, &mut stack);
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
                finish_inline(&mut inline, &mut stack);
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
                finish_inline(&mut inline, &mut stack);
                stack.push(Container::List {
                    start,
                    items: Vec::new(),
                    current: Vec::new(),
                    task: None,
                });
            }
            Event::Start(Tag::Item) => {
                finish_inline(&mut inline, &mut stack);
            }
            Event::End(TagEnd::Item) => {
                finish_inline(&mut inline, &mut stack);
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
                finish_inline(&mut inline, &mut stack);
                if let Some(Container::List { start, items, .. }) = stack.pop() {
                    push_block(&mut stack, Block::List { start, items });
                }
            }
            Event::Start(Tag::Table(aligns)) => {
                finish_inline(&mut inline, &mut stack);
                stack.push(Container::Table {
                    aligns,
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Event::Start(Tag::TableCell) => {
                finish_inline(&mut inline, &mut stack);
                stack.push(Container::Cell(Building::default()));
            }
            Event::End(TagEnd::TableCell) => {
                let Some(Container::Cell(mut building)) = stack.pop() else {
                    continue;
                };
                while !building.open.is_empty() {
                    let end = building.inline.text.len();
                    building.close(end);
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
                    push_block(&mut stack, Block::Table { aligns, head, rows });
                }
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                finish_inline(&mut inline, &mut stack);
                stack.push(Container::Blocks {
                    out: Vec::new(),
                    quote: None,
                });
                footnotes.push(label.to_string());
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                finish_inline(&mut inline, &mut stack);
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
                finish_inline(&mut inline, &mut stack);
                html.clear();
            }
            Event::Html(text) | Event::InlineHtml(text) => match &mut inline {
                Some((_, building)) => building.push_text(&text),
                None => html.push_str(&text),
            },
            Event::End(TagEnd::HtmlBlock) => {
                let text = std::mem::take(&mut html);
                if !text.trim().is_empty() {
                    push_block(&mut stack, Block::Html(text.trim_end().to_string()));
                }
            }
            Event::End(_) => {}
            Event::Start(_) => {}
        }
    }
    finish_inline(&mut inline, &mut stack);
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

fn open(inline: &mut Option<(Option<u8>, Building)>, what: Open) {
    with_inline(inline, |building| {
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

fn push_block(stack: &mut [Container], block: Block) {
    match stack.last_mut() {
        Some(Container::Blocks { out, .. }) => out.push(block),
        Some(Container::List { current, .. }) => current.push(block),
        _ => {}
    }
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
    fn html_is_inert_text() {
        let doc = parse("<script>alert(1)</script>\n\nplain");
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::Html(text) if text.contains("alert(1)"))),
            "{:?}",
            doc.blocks
        );
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
