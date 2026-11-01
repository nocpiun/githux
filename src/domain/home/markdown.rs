//! Plain-text extraction and compact excerpt generation for pull request bodies.
//!
//! Pull request bodies are Markdown. To display a readable excerpt without raw
//! syntax (`#`, `**`, `*`, ````, `[...](...)`, `<!-- ... -->`, HTML tags), this
//! parses the text with `pulldown_cmark` and collects inline text and code spans,
//! separating blocks by space and dropping all HTML comments, raw HTML tags, and images.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Default character ceiling for compact PR body previews (~3-4 lines of card text).
pub const DEFAULT_BODY_EXCERPT_CHARS: usize = 200;

/// Compact plain-text preview of a pull request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyPreview {
    /// Plain text excerpt, with Markdown formatting, HTML tags and comments stripped.
    pub excerpt: String,
    /// `true` if the original plain text exceeded the excerpt limit and was truncated.
    pub is_truncated: bool,
}

/// Extract a plain-text preview from a Markdown body using [`DEFAULT_BODY_EXCERPT_CHARS`].
pub fn extract_body_preview(markdown: &str) -> Option<BodyPreview> {
    extract_body_preview_with_limit(markdown, DEFAULT_BODY_EXCERPT_CHARS)
}

/// Extract a plain-text preview with an explicit character budget.
///
/// Returns `None` if the input is empty or contains only whitespace, HTML tags,
/// comments or images with no usable plain text.
pub fn extract_body_preview_with_limit(markdown: &str, max_chars: usize) -> Option<BodyPreview> {
    let markdown = markdown.trim();
    if markdown.is_empty() || max_chars == 0 {
        return None;
    }

    let mut words: Vec<String> = Vec::new();
    let mut current_word = String::new();

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Text(t) | Event::Code(t) => {
                for ch in t.chars() {
                    if ch.is_whitespace() {
                        if !current_word.is_empty() {
                            words.push(std::mem::take(&mut current_word));
                        }
                    } else {
                        current_word.push(ch);
                    }
                }
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                let stripped = strip_html_tags_and_comments(&h);
                for ch in stripped.chars() {
                    if ch.is_whitespace() {
                        if !current_word.is_empty() {
                            words.push(std::mem::take(&mut current_word));
                        }
                    } else {
                        current_word.push(ch);
                    }
                }
            }
            Event::Start(Tag::Paragraph)
            | Event::End(TagEnd::Paragraph)
            | Event::Start(Tag::Heading { .. })
            | Event::End(TagEnd::Heading(_))
            | Event::Start(Tag::Item)
            | Event::End(TagEnd::Item)
            | Event::Start(Tag::BlockQuote(_))
            | Event::End(TagEnd::BlockQuote(_))
            | Event::HardBreak
            | Event::SoftBreak
                if !current_word.is_empty() =>
            {
                words.push(std::mem::take(&mut current_word));
            }
            _ => {}
        }
    }
    if !current_word.is_empty() {
        words.push(current_word);
    }

    if words.is_empty() {
        return None;
    }

    let joined = words.join(" ");
    let char_count = joined.chars().count();
    if char_count <= max_chars {
        Some(BodyPreview {
            excerpt: joined,
            is_truncated: false,
        })
    } else {
        // Collect up to `max_chars` unicode characters without breaking across multi-byte boundaries.
        let target_chars: String = joined.chars().take(max_chars).collect();
        let mut truncated = target_chars.as_str();
        // Break cleanly at the last space within the last 40 characters if available.
        if let Some(last_space) = truncated.rfind(' ')
            && last_space >= target_chars.len().saturating_sub(40)
        {
            truncated = &truncated[..last_space];
        }
        let truncated = truncated.trim_end_matches([' ', '.', ',', ';', ':', '!', '?', '-', '—']);
        Some(BodyPreview {
            excerpt: format!("{truncated}…"),
            is_truncated: true,
        })
    }
}

/// Strips HTML tags (`<...>`) and comments (`<!-- ... -->`) while keeping
/// the inner text content between tags.
fn strip_html_tags_and_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut in_comment = false;
    let mut in_tag = false;
    let mut i = 0;
    while i < chars.len() {
        if !in_comment
            && !in_tag
            && i + 3 < chars.len()
            && chars[i] == '<'
            && chars[i + 1] == '!'
            && chars[i + 2] == '-'
            && chars[i + 3] == '-'
        {
            in_comment = true;
            i += 4;
            continue;
        }
        if in_comment {
            if i + 2 < chars.len() && chars[i] == '-' && chars[i + 1] == '-' && chars[i + 2] == '>'
            {
                in_comment = false;
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }
        if chars[i] == '<' {
            in_tag = true;
            out.push(' ');
            i += 1;
            continue;
        }
        if in_tag {
            if chars[i] == '>' {
                in_tag = false;
                out.push(' ');
            }
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_blank_markdown_yields_none() {
        assert_eq!(extract_body_preview(""), None);
        assert_eq!(extract_body_preview("   \n\t  "), None);
        assert_eq!(extract_body_preview("<!-- html comment only -->"), None);
        assert_eq!(extract_body_preview("<p><!-- comment --></p>"), None);
    }

    #[test]
    fn short_markdown_extracts_clean_plain_text_without_truncation() {
        let input = "Fixes the **critical** issue in `parser` for [Issue #42](https://github.com).";
        let preview = extract_body_preview(input).expect("preview");
        assert_eq!(
            preview.excerpt,
            "Fixes the critical issue in parser for Issue #42."
        );
        assert!(!preview.is_truncated);
    }

    #[test]
    fn html_tags_and_headers_are_stripped() {
        let input = "## Summary\n<details><summary>Details</summary>Extra info</details>\n\nParagraph text.";
        let preview = extract_body_preview(input).expect("preview");
        assert_eq!(
            preview.excerpt,
            "Summary Details Extra info Paragraph text."
        );
        assert!(!preview.is_truncated);
    }

    #[test]
    fn long_markdown_truncates_at_word_boundary_with_ellipsis() {
        let input = "WordOne WordTwo WordThree WordFour WordFive WordSix WordSeven WordEight WordNine WordTen";
        let preview = extract_body_preview_with_limit(input, 35).expect("preview");
        assert!(preview.is_truncated);
        assert!(preview.excerpt.ends_with('…'));
        assert!(preview.excerpt.chars().count() <= 36);
        // Should not break in the middle of a word if space is nearby.
        assert!(!preview.excerpt.contains("WordFive"));
    }

    #[test]
    fn unicode_and_cjk_are_counted_safely() {
        let input = "修复了在特定情况下的内存泄漏问题，并对相关逻辑进行了单元测试覆盖。另外优化了异步任务调度。";
        let preview = extract_body_preview_with_limit(input, 20).expect("preview");
        assert!(preview.is_truncated);
        assert!(preview.excerpt.ends_with('…'));
        assert_eq!(preview.excerpt.chars().count(), 21); // 20 chars + 1 ellipsis
    }
}
