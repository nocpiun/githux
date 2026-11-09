//! The reader's own Markdown and HTML model.
//!
//! Extracted and rewritten for Githux from Ely's `MarkdownDocument` reader
//! (`ely-documents` @ `30c66fe1`, MIT) and reduced to the read-only repository
//! path. `ast` turns GFM into blocks with inline byte ranges, `html` normalises
//! the raw HTML subset GitHub READMEs use, and `doctext` is the read-order text
//! model the parser tests copy from. The Native Shell reader maps these blocks
//! onto `nsh_base::text_view::Document` in [`super::model`]; no GPUI or Ely
//! rendering type remains here.
//!
//! Upstream updates are reviewed against `docs/content-reader-markdown-patch.md`
//! and never copied over as a whole.

// The copied parser keeps the full HTML/AST surface; the NSH consumer uses a
// subset, so a few kept-for-compatibility variants are intentionally not
// exercised here (scoped allowance, removed once the consumer covers them).
#![allow(dead_code)]

pub(super) mod ast;
#[cfg(test)]
pub(super) mod doctext;
pub(super) mod html;
#[cfg(test)]
mod html_tests;
