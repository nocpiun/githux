//! Versioned link resolution for the read-only reader.
//!
//! The reader never opens an address itself. It resolves each address against
//! the fixed repository/commit/path context of the content it is showing and
//! reports a structured action, so the integration layer decides whether to
//! open an external browser, move to another file at the same commit, or do
//! nothing. Dangerous schemes (`javascript:`, `data:`, `file:`, custom
//! protocols) are refused here and never reach the system opener.

use super::ResourceContext;

/// Why an address could not be turned into a safe target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// The address is empty.
    Empty,
    /// A scheme the reader does not hand to the system.
    Scheme,
    /// A protocol-relative address (`//host/path`) with no repository meaning.
    ProtocolRelative,
    /// The path is empty, holds a backslash, a control character or escapes
    /// the repository root in a way that has no file.
    InvalidPath,
}

/// A link that stays inside the same repository at the same commit.
///
/// The repository, commit and path are carried together so the integration
/// layer can open the exact version the reader is showing. It must not fall
/// back to the branch's latest commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryLink {
    pub owner: String,
    pub name: String,
    pub commit: String,
    pub path: String,
    /// The `#fragment`, if the address carried one.
    pub anchor: Option<String>,
}

/// The structured target of a pressed link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Another file in the same repository at the same commit.
    Repository(RepositoryLink),
    /// An absolute `http`/`https` address for the integration layer to open.
    External { url: String },
    /// A fragment on the current document. Anchor scrolling is not implemented
    /// this round; the target is reported so the integration layer can act.
    Anchor { id: String },
    /// An address the reader refuses to turn into an action.
    Refused { url: String, reason: RefusalReason },
}

/// The scheme of `url`, when it has one that a browser would read as such.
///
/// A scheme starts with an ASCII letter, continues with letters, digits, `+`,
/// `-` or `.`, and is followed by `:`. Anything before the first `/`, `?` or
/// `#` that does not match is a relative path, not a scheme.
fn scheme(url: &str) -> Option<&str> {
    let colon = url.find(':')?;
    let (head, _) = url.split_at(colon);
    if head.is_empty() {
        return None;
    }
    let mut chars = head.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')) {
        return None;
    }
    Some(head)
}

/// `url` split at its first `#`, fragment decoded as written.
fn split_anchor(url: &str) -> (&str, Option<String>) {
    match url.split_once('#') {
        Some((before, after)) => (before, Some(after.to_string())),
        None => (url, None),
    }
}

/// `%XX` decoded; a broken escape is refused rather than kept.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => {
                let pair = bytes.get(at + 1..at + 3)?;
                let hex = std::str::from_utf8(pair).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                at += 3;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// `target` resolved against the directory holding `current`.
///
/// `..` climbs towards the root and stops there, as a browser does; a segment
/// holding a backslash or a control character makes the whole path invalid.
fn resolve_path(current: &str, target: &str) -> Option<String> {
    let mut parts: Vec<String> = if target.starts_with('/') {
        Vec::new()
    } else {
        match current.rsplit_once('/') {
            Some((dir, _)) => dir.split('/').map(str::to_string).collect(),
            None => Vec::new(),
        }
    };
    for segment in target.trim_start_matches('/').split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            segment if segment.contains('\\') || segment.chars().any(char::is_control) => {
                return None;
            }
            segment => parts.push(segment.to_string()),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Resolves `raw` against `context` without any I/O.
pub fn resolve(raw: &str, context: &ResourceContext) -> LinkTarget {
    let url = raw.trim();
    if url.is_empty() {
        return LinkTarget::Refused {
            url: raw.to_string(),
            reason: RefusalReason::Empty,
        };
    }
    if let Some(id) = url.strip_prefix('#') {
        return LinkTarget::Anchor { id: id.to_string() };
    }
    if url.starts_with("//") {
        return LinkTarget::Refused {
            url: url.to_string(),
            reason: RefusalReason::ProtocolRelative,
        };
    }
    if let Some(scheme) = scheme(url) {
        return match scheme.to_ascii_lowercase().as_str() {
            "http" | "https" => LinkTarget::External {
                url: url.to_string(),
            },
            _ => LinkTarget::Refused {
                url: url.to_string(),
                reason: RefusalReason::Scheme,
            },
        };
    }
    let (path, anchor) = split_anchor(url);
    // A query never names a repository object; drop it as a browser drops it
    // for a static file address.
    let path = path.split('?').next().unwrap_or_default();
    let path = match percent_decode(path) {
        Some(path) => path,
        None => {
            return LinkTarget::Refused {
                url: url.to_string(),
                reason: RefusalReason::InvalidPath,
            };
        }
    };
    if path.is_empty() {
        return match anchor {
            Some(id) => LinkTarget::Anchor { id },
            None => LinkTarget::Refused {
                url: url.to_string(),
                reason: RefusalReason::Empty,
            },
        };
    }
    match resolve_path(&context.path, &path) {
        Some(path) => LinkTarget::Repository(RepositoryLink {
            owner: context.owner.clone(),
            name: context.name.clone(),
            commit: context.commit.clone(),
            path,
            anchor,
        }),
        None => LinkTarget::Refused {
            url: url.to_string(),
            reason: RefusalReason::InvalidPath,
        },
    }
}
