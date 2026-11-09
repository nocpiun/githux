//! Pure preparation. Call `ReaderDocument::prepare` on the host's background executor.
use super::{
    ResourceContext,
    parser::ast::{self, Block, Inline, Style},
};
use crate::content::{PreparedBody, PreparedContent};
use nsh_base::text_view::{self as native, Block as N, Inline as I};
use std::sync::Arc;

pub const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentKind {
    Markdown,
    Source { language: Option<String> },
}
impl DocumentKind {
    pub fn for_path(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        if lower.rsplit('/').next() == Some("readme")
            || [".md", ".markdown", ".mdx"]
                .iter()
                .any(|s| lower.ends_with(s))
        {
            Self::Markdown
        } else {
            Self::Source {
                language: path
                    .rsplit('.')
                    .next()
                    .filter(|_| path.contains('.'))
                    .map(str::to_owned),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReaderBody {
    Markdown(native::Document),
    Source {
        text: String,
        language: Option<String>,
    },
    Status(&'static str),
}
#[derive(Debug, Clone, PartialEq)]
pub struct ReaderDocument {
    pub context: ResourceContext,
    pub body: ReaderBody,
}
impl ReaderDocument {
    pub fn from_prepared(content: PreparedContent, kind: DocumentKind) -> Arc<Self> {
        let context = ResourceContext::from_identity(&content.identity);
        Self::prepare(context, content.body, kind)
    }
    pub fn prepare(context: ResourceContext, body: PreparedBody, kind: DocumentKind) -> Arc<Self> {
        let body = match body {
            PreparedBody::Empty => ReaderBody::Status("文件为空"),
            PreparedBody::Unsupported(_) => ReaderBody::Status("无法作为文本显示"),
            PreparedBody::TooLarge { .. } => ReaderBody::Status("超出读取上限"),
            PreparedBody::Utf8 { text, .. } if text.len() > MAX_TEXT_BYTES => {
                ReaderBody::Status("超出读取上限")
            }
            PreparedBody::Utf8 { text, .. } if text.is_empty() => ReaderBody::Status("文件为空"),
            PreparedBody::Utf8 { text, .. } => match kind {
                DocumentKind::Markdown => {
                    ReaderBody::Markdown(native::Document::new(blocks(ast::parse(&text).blocks)))
                }
                DocumentKind::Source { language } => ReaderBody::Source { text, language },
            },
        };
        Arc::new(Self { context, body })
    }
}

fn inline(value: Inline) -> Vec<I> {
    let mut boundaries = vec![0, value.text.len()];
    for (r, _) in &value.styles {
        boundaries.extend([r.start, r.end]);
    }
    for (r, _) in &value.links {
        boundaries.extend([r.start, r.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
        .windows(2)
        .filter_map(|pair| {
            let (start, end) = (pair[0], pair[1]);
            let text = value.text.get(start..end)?;
            if text.is_empty() {
                return None;
            }
            let mut node = I::text(text);
            for (_, style) in value
                .styles
                .iter()
                .filter(|(r, _)| r.start <= start && r.end >= end)
            {
                node = match style {
                    Style::Strong => I::Strong {
                        content: vec![node],
                    },
                    Style::Emphasis => I::Emphasis {
                        content: vec![node],
                    },
                    Style::Strike => I::Strikethrough {
                        content: vec![node],
                    },
                    Style::Code => I::Code {
                        code: text.to_owned(),
                    },
                    Style::Math => I::Math {
                        tex: text.to_owned(),
                    },
                    Style::Link => node,
                };
            }
            if let Some((_, url)) = value
                .links
                .iter()
                .find(|(r, _)| r.start <= start && r.end >= end)
            {
                node = I::Link {
                    url: url.clone(),
                    title: None,
                    content: vec![node],
                };
            }
            Some(node)
        })
        .collect()
}
fn blocks(values: Vec<Block>) -> Vec<N> {
    values
        .into_iter()
        .map(|block| match block {
            Block::Paragraph(v) => N::paragraph(inline(v)),
            Block::Heading(level, v) => N::Heading {
                level,
                anchor: Some(slug(&v.text)),
                content: inline(v),
            },
            Block::Quote { kind, body } => N::Blockquote {
                children: blocks(body),
                alert: kind.map(|kind| match kind {
                    ast::AlertKind::Note => native::Alert::Note,
                    ast::AlertKind::Tip => native::Alert::Tip,
                    ast::AlertKind::Important => native::Alert::Important,
                    ast::AlertKind::Warning => native::Alert::Warning,
                    ast::AlertKind::Caution => native::Alert::Caution,
                }),
            },
            Block::Code { language, text } => N::CodeBlock(native::CodeBlock::new(language, text)),
            Block::List { start, items } => N::List {
                ordered: start.is_some(),
                start: start.unwrap_or(1).min(u32::MAX as u64) as u32,
                loose: true,
                items: items
                    .into_iter()
                    .map(|(checked, children)| {
                        let mut item = native::ListItem::new(blocks(children));
                        item.checked = checked;
                        item
                    })
                    .collect(),
            },
            Block::Table {
                aligns, head, rows, ..
            } => {
                let mut table = native::Table::default();
                table.align = aligns
                    .into_iter()
                    .map(|a| match a {
                        pulldown_cmark::Alignment::None => native::ColumnAlign::None,
                        pulldown_cmark::Alignment::Left => native::ColumnAlign::Left,
                        pulldown_cmark::Alignment::Center => native::ColumnAlign::Center,
                        pulldown_cmark::Alignment::Right => native::ColumnAlign::Right,
                    })
                    .collect();
                table.head = head
                    .into_iter()
                    .map(|c| native::TableCell::new(inline(c)))
                    .collect();
                table.rows = rows
                    .into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(|c| native::TableCell::new(inline(c)))
                            .collect()
                    })
                    .collect();
                N::Table(table)
            }
            Block::Rule => N::Rule,
            Block::Math { inline: _, tex } => N::Math { tex },
            Block::Image { alt, url } => {
                let mut image = native::Image::new(url);
                image.alt = alt;
                N::paragraph(vec![I::Image(image)])
            }
            Block::Html(text) => N::paragraph(vec![I::text(text)]),
            Block::Footnote { label, body } => N::Blockquote {
                children: std::iter::once(N::paragraph(vec![I::text(label)]))
                    .chain(blocks(body))
                    .collect(),
                alert: None,
            },
        })
        .collect()
}

pub(crate) fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}
