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
    let tree = markdown_tree(text);
    let mut spans = Vec::new();
    collect(&tree, &mut spans);
    spans.sort_unstable_by_key(|r| r.start);
    let mut output = String::with_capacity(text.len());
    let mut at = 0;
    for range in spans {
        if range.start < at {
            continue;
        }
        output.push_str(&text[at..range.start]);
        output.push_str("[picture/HTML omitted]");
        // Preserve paragraph/line separation without preserving loadable URLs.
        output.extend(text[range.clone()].chars().filter(|c| *c == '\n'));
        at = range.end;
    }
    output.push_str(&text[at..]);
    output
}
