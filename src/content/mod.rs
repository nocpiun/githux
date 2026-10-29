//! Bounded, UI-independent preparation for a future read-only content view.
//! No Markdown parser, syntax highlighting, image downloads or disk cache.

// The service is delivered before its reader/data integration.
#![allow(dead_code)]

pub mod images;

use crate::domain::blob::{BlobBody, BlobIdentity, BlobRead};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedContent {
    NonUtf8,
    BinaryControlCharacters,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PreparedBody {
    Empty,
    /// The original UTF-8 bytes, including any BOM and original line endings.
    Utf8 {
        text: String,
        has_bom: bool,
    },
    Unsupported(UnsupportedContent),
    TooLarge {
        size: u64,
        limit: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct PreparedContent {
    pub identity: BlobIdentity,
    pub body: PreparedBody,
}

/// Consumes the bytes so accepted UTF-8 text reuses the same allocation.
/// `max_bytes` provides a separate preparation/result budget, even if the
/// network caller used a larger configurable read limit.
pub fn prepare_content(blob: BlobRead, max_bytes: usize) -> PreparedContent {
    let (identity, body) = blob.into_parts();
    let body = match body {
        BlobBody::TooLarge { size, limit } => PreparedBody::TooLarge { size, limit },
        BlobBody::Bytes(bytes) if bytes.len() > max_bytes => PreparedBody::TooLarge {
            size: bytes.len() as u64,
            limit: max_bytes,
        },
        BlobBody::Bytes(bytes) if bytes.is_empty() => PreparedBody::Empty,
        BlobBody::Bytes(bytes) => match String::from_utf8(bytes) {
            Err(_) => PreparedBody::Unsupported(UnsupportedContent::NonUtf8),
            Ok(text) => {
                // Conservative text policy: UTF-8 alone does not prove a file
                // is text. Allow whitespace controls, reject NUL/other C0/C1.
                if text
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\t' | '\r' | '\n'))
                {
                    PreparedBody::Unsupported(UnsupportedContent::BinaryControlCharacters)
                } else {
                    let has_bom = text.starts_with('\u{feff}');
                    PreparedBody::Utf8 { text, has_bom }
                }
            }
        },
    };
    PreparedContent { identity, body }
}
