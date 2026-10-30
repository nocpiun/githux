//! A small, bounded HTML subset parser for repository Markdown.
//!
//! GitHub READMEs are written in Markdown but commonly contain raw HTML:
//! centred `<div>` wrappers, `<img>` badges, `<table>` layouts, `<br>`,
//! `<details>` blocks and inline `<kbd>`/`<sub>` markup. The Markdown reader
//! must render their content with the existing block and inline types instead
//! of showing the tags as inert text.
//!
//! This module only tokenizes HTML and builds a tolerant element tree. It never
//! executes anything, never keeps raw tag text in the output and never resolves
//! URLs: the caller maps elements onto the reader's own `Block`/`Inline`
//! types, so link and image safety stays in `links::resolve` and the image
//! pipeline. The supported subset and its limits are documented in
//! `docs/content-reader.md`; the rules follow the WHATWG HTML syntax and
//! tokenization sections for the constructs below.
//!
//! Deliberately out of scope: the full HTML tree-construction algorithm,
//! foreign content (SVG/MathML) layout, frames, scripts and CSS.

/// One parsed element with its attributes and children.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// Lower-case tag name, as the HTML tokenizer produces it.
    pub name: String,
    /// Lower-case attribute names with entity-decoded values, in source order.
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    /// True when this fragment contained the matching end tag (or closed the
    /// element implicitly), so an empty element must not stay open.
    pub closed: bool,
}

impl Element {
    /// The value of `name`, if the attribute was present. `name` must already
    /// be lower-case.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Whether a whitespace-separated class token is present.
    pub fn has_class(&self, token: &str) -> bool {
        self.attr("class")
            .is_some_and(|class| class.split_whitespace().any(|item| item == token))
    }
}

/// A parsed HTML node.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    Element(Element),
    /// An end tag with no matching start in this fragment. The caller uses it
    /// to close an element opened by an earlier `InlineHtml` event.
    End(String),
    /// A dropped raw-text element whose end tag is in a later fragment. The
    /// caller keeps suppressing text until that tag arrives.
    Suppressed(String),
}

/// Elements that never have children.
pub fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Elements that start a new block (or are dropped entirely as non-content).
///
/// Anything not listed here is treated as inline and its children are kept.
pub fn is_block(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "base"
            | "blockquote"
            | "body"
            | "caption"
            | "center"
            | "dd"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "embed"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "head"
            | "header"
            | "hgroup"
            | "hr"
            | "html"
            | "iframe"
            | "legend"
            | "li"
            | "link"
            | "main"
            | "meta"
            | "nav"
            | "object"
            | "ol"
            | "p"
            | "pre"
            | "script"
            | "section"
            | "style"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "template"
            | "tfoot"
            | "th"
            | "thead"
            | "title"
            | "tr"
            | "ul"
    )
}

/// Elements whose content is never readable content and must be dropped whole.
pub fn is_dropped(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "embed"
            | "head"
            | "iframe"
            | "link"
            | "meta"
            | "object"
            | "script"
            | "style"
            | "template"
            | "textarea"
            | "title"
    )
}

/// Elements whose text content is raw: tags inside them are literal text.
fn is_raw_text(name: &str) -> bool {
    matches!(name, "script" | "style" | "template" | "title" | "textarea")
}

/// Maximum element nesting this parser builds.
///
/// A repository README can nest thousands of `<div>`s inside the reader's
/// 2 MiB budget. The element tree is walked and dropped recursively, so an
/// unbounded depth would abort the process with a stack overflow even though
/// the input is small. Deeper starts are flattened: their text is kept, the
/// extra tags are dropped, and the end tags are consumed by `suppressed`.
pub(crate) const MAX_DEPTH: usize = 256;

/// Largest `start` accepted from an HTML ordered list.
///
/// The renderer adds an item's index to the list's first number, so an
/// arbitrary `u64` from `<ol start="…">` could overflow. This leaves room for
/// far more items than a 2 MiB document can hold.
pub(crate) const MAX_LIST_START: u64 = u64::MAX - (1 << 32);
/// Parse an HTML fragment into a tolerant element tree.
///
/// Unmatched end tags are ignored, unmatched start tags stay open until the
/// fragment ends, comments/declarations/processing instructions are dropped,
/// and a tag that never closes is dropped rather than shown as text.
pub fn parse(fragment: &str) -> Vec<Node> {
    let bytes = fragment.as_bytes();
    let len = bytes.len();
    let mut roots = Vec::new();
    let mut stack: Vec<Element> = Vec::new();
    // Start tags past `MAX_DEPTH` that were not pushed, so their end tags must
    // be ignored instead of closing a real ancestor.
    let mut suppressed = 0usize;
    let mut i = 0;
    while i < len {
        if bytes[i] != b'<' {
            let start = i;
            while i < len && bytes[i] != b'<' {
                i += 1;
            }
            let text = decode_entities(&fragment[start..i]);
            push_text(&mut stack, &mut roots, text);
            continue;
        }
        let Some((tag, next)) = scan_tag(fragment, i) else {
            // A lone `<` is text, exactly like the HTML tokenizer's
            // "data state" fallback.
            push_text(&mut stack, &mut roots, "<".to_string());
            i += 1;
            continue;
        };
        match tag {
            Tag::Comment | Tag::Declaration => {}
            Tag::Start {
                name,
                attrs,
                self_closing,
            } => {
                if is_raw_text(&name) {
                    let (content, next, closed) = raw_content(fragment, next, &name);
                    if is_dropped(&name) && !closed {
                        attach(&mut stack, &mut roots, Node::Suppressed(name));
                    } else if !is_dropped(&name) {
                        let element = Element {
                            name,
                            attrs,
                            children: vec![Node::Text(decode_entities(&content))],
                            closed,
                        };
                        attach(&mut stack, &mut roots, Node::Element(element));
                    }
                    i = next;
                    continue;
                }
                let element = Element {
                    name,
                    attrs,
                    children: Vec::new(),
                    closed: false,
                };
                if self_closing || is_void(&element.name) {
                    attach(&mut stack, &mut roots, Node::Element(element));
                } else if stack.len() >= MAX_DEPTH {
                    suppressed += 1;
                } else {
                    stack.push(element);
                }
            }
            Tag::End { name } => {
                if suppressed > 0 {
                    suppressed -= 1;
                } else if let Some(position) = stack
                    .iter()
                    .rposition(|open| open.name.eq_ignore_ascii_case(&name))
                {
                    // Unmatched inner elements are implicitly closed; each
                    // finished element is attached to its parent here.
                    while stack.len() > position {
                        let mut element = stack.pop().expect("a parsed element");
                        element.closed = true;
                        attach(&mut stack, &mut roots, Node::Element(element));
                    }
                } else {
                    attach(&mut stack, &mut roots, Node::End(name));
                }
                if let Some(name) = stack.last().map(|element| element.name.clone())
                    && is_raw_text(&name)
                {
                    // An unclosed raw-text element runs to the fragment end;
                    // its content must not become readable text.
                    while let Some(element) = stack.pop() {
                        attach(&mut stack, &mut roots, Node::Element(element));
                    }
                    break;
                }
            }
        }
        i = next;
    }
    while let Some(element) = stack.pop() {
        attach(&mut stack, &mut roots, Node::Element(element));
    }
    roots
}

/// Concatenate the readable text of `nodes`, keeping line breaks from `<br>`.
///
/// Used for `<pre>`/`<code>` blocks where whitespace is significant.
pub fn raw_text(nodes: &[Node]) -> String {
    let mut out = String::new();
    collect_raw_text(nodes, &mut out);
    out
}

fn collect_raw_text(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::End(_) | Node::Suppressed(_) => {}
            Node::Element(element) => match element.name.as_str() {
                "br" => out.push('\n'),
                name if is_dropped(name) => {}
                _ => collect_raw_text(&element.children, out),
            },
        }
    }
}

/// Collapse HTML whitespace runs to one space, keeping one leading/trailing
/// space when the input had whitespace there.
///
/// Only HTML's ASCII whitespace collapses. `&nbsp;` and other Unicode spaces
/// are content: they keep the author's spacing and prevent wrapping.
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending = false;
    for ch in text.chars() {
        if is_html_whitespace(ch) {
            pending = true;
        } else {
            if pending {
                out.push(' ');
                pending = false;
            }
            out.push(ch);
        }
    }
    if pending {
        out.push(' ');
    }
    out
}

/// The ASCII whitespace the HTML parser itself treats as collapsible.
pub fn is_html_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{0c}')
}

/// Decode the HTML character references this reader supports.
///
/// Numeric references and the common named references are decoded; an unknown
/// or malformed reference is left exactly as written, which is safe because it
/// is only ever emitted as text.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
        if ch == '&'
            && let Some((decoded, next)) = decode_at(text, i)
        {
            out.push(decoded);
            i = next;
            continue;
        }
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn decode_at(text: &str, i: usize) -> Option<(char, usize)> {
    let rest = &text[i + 1..];
    // Only a short window can form a character reference; scanning the whole
    // remaining string for `;` would make a README full of bare `&` quadratic.
    let semi = rest
        .char_indices()
        .take(33)
        .find(|(_, ch)| *ch == ';')
        .map(|(index, _)| index)?;
    let body = &rest[..semi];
    let decoded = if let Some(number) = body.strip_prefix('#') {
        let code = if let Some(hex) = number.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            number.parse::<u32>().ok()
        };
        code.and_then(char::from_u32)
    } else {
        named_entity(body)
    }?;
    Some((decoded, i + 1 + semi + 1))
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "deg" => '\u{b0}',
        "plusmn" => '\u{b1}',
        "times" => '\u{d7}',
        "divide" => '\u{f7}',
        "laquo" => '\u{ab}',
        "raquo" => '\u{bb}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "hellip" => '\u{2026}',
        "middot" => '\u{b7}',
        "bull" => '\u{2022}',
        "dagger" => '\u{2020}',
        "Dagger" => '\u{2021}',
        "permil" => '\u{2030}',
        "euro" => '\u{20ac}',
        "pound" => '\u{a3}',
        "yen" => '\u{a5}',
        "cent" => '\u{a2}',
        "sect" => '\u{a7}',
        "para" => '\u{b6}',
        "micro" => '\u{b5}',
        "alpha" => '\u{3b1}',
        "beta" => '\u{3b2}',
        "gamma" => '\u{3b3}',
        "delta" => '\u{3b4}',
        "lambda" => '\u{3bb}',
        "mu" => '\u{3bc}',
        "pi" => '\u{3c0}',
        "sigma" => '\u{3c3}',
        "omega" => '\u{3c9}',
        "Delta" => '\u{394}',
        "Sigma" => '\u{3a3}',
        "Omega" => '\u{3a9}',
        "forall" => '\u{2200}',
        "exist" => '\u{2203}',
        "empty" => '\u{2205}',
        "isin" => '\u{2208}',
        "notin" => '\u{2209}',
        "le" => '\u{2264}',
        "ge" => '\u{2265}',
        "ne" => '\u{2260}',
        "equiv" => '\u{2261}',
        "infin" => '\u{221e}',
        "sum" => '\u{2211}',
        "prod" => '\u{220f}',
        "radic" => '\u{221a}',
        "int" => '\u{222b}',
        "rarr" => '\u{2192}',
        "larr" => '\u{2190}',
        "uarr" => '\u{2191}',
        "darr" => '\u{2193}',
        "harr" => '\u{2194}',
        "spades" => '\u{2660}',
        "clubs" => '\u{2663}',
        "hearts" => '\u{2665}',
        "diams" => '\u{2666}',
        _ => return None,
    })
}

enum Tag {
    Comment,
    Declaration,
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    End {
        name: String,
    },
}

/// Scan the markup starting at `i` (which points at `<`).
///
/// Returns the token and the index just past it, or `None` when the byte at
/// `i` cannot start markup (a lone `<`, or an incomplete tag that the HTML
/// tokenizer would drop).
fn scan_tag(fragment: &str, i: usize) -> Option<(Tag, usize)> {
    let bytes = fragment.as_bytes();
    let len = bytes.len();
    debug_assert_eq!(bytes.get(i), Some(&b'<'));
    let rest = &fragment[i + 1..];
    if rest.starts_with("!--") {
        let end = fragment[i + 4..]
            .find("-->")
            .map(|offset| i + 4 + offset + 3)
            .unwrap_or(len);
        return Some((Tag::Comment, end));
    }
    if rest.starts_with('!') || rest.starts_with('?') {
        // An unterminated declaration consumes the rest of the fragment;
        // returning `None` would make the caller rescan the same suffix.
        let end = fragment[i..]
            .find('>')
            .map(|offset| i + offset + 1)
            .unwrap_or(len);
        return Some((Tag::Declaration, end));
    }
    if let Some(name) = rest.strip_prefix('/') {
        let name_len = name.bytes().take_while(|byte| is_name_byte(*byte)).count();
        if name_len == 0 {
            return None;
        }
        let name = name[..name_len].to_ascii_lowercase();
        let end = fragment[i..]
            .find('>')
            .map(|offset| i + offset + 1)
            .unwrap_or(len);
        return Some((Tag::End { name }, end));
    }
    if !rest.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
        return None;
    }
    let name_len = rest.bytes().take_while(|byte| is_name_byte(*byte)).count();
    let name = rest[..name_len].to_ascii_lowercase();
    let mut cursor = i + 1 + name_len;
    let mut attrs = Vec::new();
    let mut self_closing = false;
    loop {
        while cursor < len && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= len {
            // An incomplete tag is dropped instead of shown as text.
            return Some((
                Tag::Start {
                    name,
                    attrs,
                    self_closing,
                },
                len,
            ));
        }
        match bytes[cursor] {
            b'>' => {
                cursor += 1;
                break;
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'>') => {
                self_closing = true;
                cursor += 2;
                break;
            }
            _ => {}
        }
        let attr_start = cursor;
        while cursor < len
            && !bytes[cursor].is_ascii_whitespace()
            && !matches!(bytes[cursor], b'=' | b'/' | b'>')
        {
            cursor += 1;
        }
        if cursor == attr_start {
            // A stray character such as `=`; skip it and keep scanning.
            cursor += 1;
            continue;
        }
        let attr_name = fragment[attr_start..cursor].to_ascii_lowercase();
        while cursor < len && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let mut value = String::new();
        if bytes.get(cursor) == Some(&b'=') {
            cursor += 1;
            while cursor < len && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if let Some(quote) = bytes
                .get(cursor)
                .copied()
                .filter(|b| *b == b'"' || *b == b'\'')
            {
                cursor += 1;
                let value_start = cursor;
                while cursor < len && bytes[cursor] != quote {
                    cursor += 1;
                }
                value = decode_entities(&fragment[value_start..cursor.min(len)]);
                if cursor < len {
                    cursor += 1;
                }
            } else {
                let value_start = cursor;
                while cursor < len && !bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'>'
                {
                    cursor += 1;
                }
                value = decode_entities(&fragment[value_start..cursor]);
            }
        }
        attrs.push((attr_name, value));
    }
    Some((
        Tag::Start {
            name,
            attrs,
            self_closing,
        },
        cursor,
    ))
}

fn raw_content(fragment: &str, start: usize, name: &str) -> (String, usize, bool) {
    let needle = format!("</{name}");
    let mut search = start;
    while let Some(offset) = find_ci(&fragment[search..], &needle) {
        let after = search + offset + needle.len();
        // `</scripture>` is text inside a script, not its end tag: the name
        // must end at whitespace, `/`, or `>`.
        let boundary = fragment.as_bytes().get(after).copied();
        if boundary.is_none_or(|byte| byte == b'>' || byte == b'/' || byte.is_ascii_whitespace()) {
            let content_end = search + offset;
            if let Some(gt) = fragment[content_end..].find('>') {
                return (
                    fragment[start..content_end].to_string(),
                    content_end + gt + 1,
                    true,
                );
            }
            break;
        }
        search = after;
    }
    (fragment[start..].to_string(), fragment.len(), false)
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .find(|offset| haystack[*offset..*offset + needle.len()].eq_ignore_ascii_case(needle))
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
}

fn attach(stack: &mut [Element], roots: &mut Vec<Node>, node: Node) {
    if let Some(parent) = stack.last_mut() {
        attach_to(&mut parent.children, node);
    } else {
        attach_to(roots, node);
    }
}

/// Append a node, merging adjacent text so runs stay one string.
fn attach_to(children: &mut Vec<Node>, node: Node) {
    if let Node::Text(text) = &node
        && let Some(Node::Text(previous)) = children.last_mut()
    {
        previous.push_str(text);
        return;
    }
    children.push(node);
}

fn push_text(stack: &mut [Element], roots: &mut Vec<Node>, text: String) {
    if !text.is_empty() {
        attach(stack, roots, Node::Text(text));
    }
}

/// Byte bounds of the span occupied by element `name` in `fragment`.
///
/// Returns `(head_end, tail_start)`. `head_end` is where readable content ends:
/// the opening tag when the element opens here, or the start of the closing tag
/// when the fragment only closes a span opened in an earlier fragment.
/// `tail_start` is `Some` only when this fragment contains the real end tag, and
/// marks where readable content can resume.
///
/// Only the tokenizer's own view counts, so a `</template>` inside a comment or
/// a longer name such as `</templates>` never closes the span.
pub fn dropped_bounds(fragment: &str, name: &str) -> (usize, Option<usize>) {
    let bytes = fragment.as_bytes();
    let len = bytes.len();
    let mut depth = 0usize;
    let mut head_end = len;
    let mut i = 0;
    while i < len {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let Some((tag, next)) = scan_tag(fragment, i) else {
            i += 1;
            continue;
        };
        match tag {
            Tag::Start {
                name: tag_name,
                self_closing,
                ..
            } if tag_name == name => {
                if depth == 0 {
                    head_end = head_end.min(i);
                }
                if is_raw_text(&tag_name) {
                    let (_, after, closed) = raw_content(fragment, next, &tag_name);
                    if closed && depth == 0 {
                        return (head_end, Some(after));
                    }
                    i = after;
                    continue;
                }
                if !self_closing && !is_void(&tag_name) {
                    depth += 1;
                }
            }
            Tag::End { name: tag_name } if tag_name == name => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        return (head_end, Some(next));
                    }
                } else {
                    // This fragment only closes a span opened earlier.
                    return (i, Some(next));
                }
            }
            _ => {}
        }
        i = next;
    }
    (head_end, None)
}
