//! The reader's own Markdown: parsing, read-only block layout, inline code
//! boxes, selection and a virtualized document with a real list scroll source.
//!
//! This module is maintained by Githux. It was extracted from Ely's
//! `ely-documents` reader at revision `30c66fe1d5d23db769a6c834b771d054d7f288cf`
//! (MIT) and reduced to the read-only repository-file path. The per-file
//! mapping, changes and maintenance rules are recorded in
//! `docs/content-reader.md`; upstream updates are reviewed against
//! `docs/content-reader-markdown-patch.md` and never copied over as a whole.

mod ast;
mod doctext;
mod document;
mod inline;

pub use document::MarkdownView;

#[cfg(test)]
pub use ast::{Block, parse};
#[cfg(test)]
pub use inline::{Snapshot, snapshot};
