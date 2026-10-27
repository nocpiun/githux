//! Repository-safe input preparation around the public Ely document API.
//!
//! Ely's document does not accept a picture resolver. Strip image and HTML
//! nodes before handing it repository content, so an untrusted path can never
//! reach a filesystem/data image loader. Parsing runs on the background pool;
//! the document entity is replaced only if this request is still current.
use ely_documents::{markdown_tree, mdast};

pub(super) fn safe_markdown(text: &str) -> String {
    fn collect(node: &mdast::Node, spans: &mut Vec<std::ops::Range<usize>>) {
        if matches!(
            node,
            mdast::Node::Image(_) | mdast::Node::ImageReference(_) | mdast::Node::Html(_)
        ) {
            spans.push(node.position().clone());
            return;
        }
        if let Some(children) = node.children() {
            for child in children {
                collect(child, spans);
            }
        }
    }
    // Escape inert text after the removal and parse again until no picture or
    // HTML node remains. A single insertion such as `!<i>(url)` can otherwise
    // become `![replacement](url)` at the second parse boundary.
    let mut current = text.to_string();
    for _ in 0..8 {
        let tree = markdown_tree(&current);
        let mut spans = Vec::new();
        collect(&tree, &mut spans);
        if spans.is_empty() {
            return current;
        }
        spans.sort_unstable_by_key(|r| r.start);
        let mut out = String::with_capacity(current.len());
        let mut at = 0;
        for range in spans {
            if range.start < at {
                continue;
            }
            out.push_str(&current[at..range.start]);
            out.push_str("[picture or HTML omitted]");
            out.extend(current[range.clone()].chars().filter(|c| *c == '\n'));
            at = range.end;
        }
        out.push_str(&current[at..]);
        // Brackets/parentheses/backticks and a leading `!` are inert in prose;
        // they cannot reopen link or image syntax across the replacement.
        current = out
            .replace('[', "\\[")
            .replace(']', "\\]")
            .replace('(', "\\(")
            .replace(')', "\\)")
            .replace('`', "\\`")
            .replace('!', "\\!");
    }
    // The conservative last resort has no syntax at all.
    current.replace(
        [
            '<', '>', '[', ']', '(', ')', '`', '!', '*', '_', '~', '|', ':', '/', '\\',
        ],
        "",
    )
}
