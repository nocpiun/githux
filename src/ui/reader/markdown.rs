//! Repository-safe input preparation around the public Ely document API.
//!
//! Ely's document does not accept a picture resolver. Strip image and HTML
//! nodes before handing it repository content, so an untrusted path can never
//! reach a filesystem/data image loader. Parsing runs on the background pool;
//! the document entity is replaced only if this request is still current.

use ely_documents::{markdown_tree, mdast};

/// A placeholder made only of letters and spaces. It cannot contribute the
/// punctuation that opens image, link, code or HTML syntax, and no untouched
/// source outside the removed node is changed.
const OMITTED: &str = "picture or HTML omitted; ";

fn removed(node: &mdast::Node, spans: &mut Vec<std::ops::Range<usize>>) {
    if matches!(
        node,
        mdast::Node::Image(_) | mdast::Node::ImageReference(_) | mdast::Node::Html(_)
    ) {
        spans.push(node.position().clone());
        return;
    }
    if let Some(children) = node.children() {
        for child in children {
            removed(child, spans);
        }
    }
}

fn strip(current: &str) -> String {
    let tree = markdown_tree(current);
    let mut spans = Vec::new();
    removed(&tree, &mut spans);
    spans.sort_unstable_by_key(|r| r.start);
    let mut out = String::with_capacity(current.len());
    let mut at = 0;
    for range in spans {
        if range.start < at {
            continue;
        }
        out.push_str(&current[at..range.start]);
        out.push_str(OMITTED);
        // Block removal still leaves the surrounding source on its own lines.
        out.extend(current[range.clone()].chars().filter(|c| *c == '\n'));
        at = range.end;
    }
    out.push_str(&current[at..]);
    out
}

/// Replace picture/HTML syntax with inert letters, then re-read. Inserting a
/// bare word next to source punctuation can still complete a construct the
/// first parse saw as separate nodes, so the result is verified rather than
/// assumed safe. Every pass only edits node spans; unrelated links and code
/// remain byte-for-byte intact.
pub(super) fn safe_markdown(text: &str) -> String {
    let mut current = text.to_string();
    for _ in 0..8 {
        let mut spans = Vec::new();
        removed(&markdown_tree(&current), &mut spans);
        if spans.is_empty() {
            return current;
        }
        current = strip(&current);
    }
    // Residual syntax that keeps re-forming cannot be made safe by dropping
    // only its node span (the opener may live in an adjacent node), so omit
    // the whole source rather than ever handing it to an image loader.
    String::new()
}

#[cfg(test)]
mod tests {
    use super::safe_markdown;

    fn counts(text: &str) -> (usize, usize, usize) {
        fn walk(n: &ely_documents::mdast::Node, c: &mut (usize, usize, usize)) {
            match n {
                ely_documents::mdast::Node::Image(_)
                | ely_documents::mdast::Node::ImageReference(_)
                | ely_documents::mdast::Node::Html(_) => c.0 += 1,
                ely_documents::mdast::Node::Link(_) => c.1 += 1,
                ely_documents::mdast::Node::Code(_) => c.2 += 1,
                _ => {}
            }
            if let Some(children) = n.children() {
                for child in children {
                    walk(child, c);
                }
            }
        }
        let mut c = (0, 0, 0);
        walk(&ely_documents::markdown_tree(text), &mut c);
        c
    }

    #[test]
    fn removal_is_inert_and_unrelated_markdown_survives() {
        let input = "![badge](file://attacker/share/a.png)\n\n[Guide](guide.md)\n\n```rust\nfn main() {}\n```\n\ninline `code`";
        let safe = safe_markdown(input);
        assert_eq!(counts(&safe).0, 0, "{safe}");
        assert_eq!(counts(&safe).1, 1, "{safe}");
        assert_eq!(counts(&safe).2, 1, "{safe}");
        assert!(safe.contains("fn main()"), "{safe}");
        assert!(safe.contains("inline `code`"), "{safe}");
    }

    #[test]
    fn replacements_cannot_reform_an_image_or_html() {
        for input in [
            "!<i>(file://attacker.example/share/image.png)",
            "!<!--x-->(file:///C:/secret.png)",
            "<img src=\"file:///C:/secret.png\">",
            "text `![x](data:image/png;base64,AAAA)` tail",
        ] {
            let safe = safe_markdown(input);
            assert_eq!(counts(&safe).0, 0, "{input:?} -> {safe:?}");
        }
    }
}
