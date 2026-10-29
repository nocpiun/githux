//! Regression tests for the HTML normalisation the reader applies to
//! repository Markdown.
//!
//! The fixture lives in `reader::fixtures::HTML` so the native preview and the
//! parser tests share one document. These tests run without a window: they
//! check the parsed model and the read-order text that copy uses, not pixels.

use super::super::fixtures;
use super::ast::{Align, Block, Inline, Style, parse};
use super::doctext::DocText;
use super::html;

fn debug(blocks: &[Block]) -> String {
    format!("{blocks:?}")
}

fn find_paragraph(blocks: &[Block], needle: &str) -> Inline {
    blocks
        .iter()
        .find_map(|block| match block {
            Block::Paragraph(inline) if inline.text.contains(needle) => Some(inline.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no paragraph containing {needle:?} in {blocks:?}"))
}

fn copied(blocks: &[Block]) -> String {
    DocText::build(blocks).text().to_string()
}

#[test]
fn the_user_fixture_never_shows_html_outside_code() {
    let doc = parse(fixtures::HTML);
    assert!(
        !doc.blocks
            .iter()
            .any(|block| matches!(block, Block::Html(_))),
        "raw HTML must not survive as inert text: {:?}",
        doc.blocks
    );
    let copied = copied(&doc.blocks);
    for forbidden in ["<strong>", "<table", "<img", "hidden cell", "never runs"] {
        assert!(
            !copied.contains(forbidden),
            "{forbidden:?} leaked: {copied}"
        );
    }
    // The only comment left is the one inside the fenced HTML sample.
    assert_eq!(copied.matches("<!--").count(), 1, "{copied}");
    assert!(copied.contains("项目徽标"), "{copied}");
    assert!(copied.contains("尾部文本"), "{copied}");
}

#[test]
fn containers_emphasis_links_and_pictures_keep_their_semantics() {
    let doc = parse(fixtures::HTML);
    assert!(matches!(doc.blocks[0], Block::Heading(1, _)));

    let text = find_paragraph(&doc.blocks, "普通段落");
    assert_eq!(
        text.text,
        "普通段落，带 加粗、斜体、inline <tag> 与 相对链接。"
    );
    assert!(text.styles.contains(&(19..25, Style::Strong)), "{text:?}");
    assert!(text.styles.contains(&(28..34, Style::Emphasis)), "{text:?}");
    assert!(text.styles.contains(&(37..49, Style::Code)), "{text:?}");
    assert_eq!(text.links, vec![(54..66, "guide.md#setup".to_string())]);

    let picture = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Image { alt, url } => Some((alt.clone(), url.clone())),
            _ => None,
        })
        .expect("the centred image is a picture block");
    assert_eq!(picture.0, "项目徽标");
    assert_eq!(picture.1, "https://example.com/logo.png");

    let summary = find_paragraph(&doc.blocks, "展开说明");
    assert_eq!(summary.text, "展开说明");
    assert!(
        summary.styles.contains(&(0..12, Style::Strong)),
        "{summary:?}"
    );
    let body = find_paragraph(&doc.blocks, "内部段落");
    assert!(body.styles.contains(&(16..20, Style::Code)), "{body:?}");
}

#[test]
fn html_tables_map_onto_the_markdown_table_block() {
    let doc = parse(fixtures::HTML);
    let (aligns, head, rows) = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Table {
                aligns, head, rows, ..
            } => Some((aligns.clone(), head.clone(), rows.clone())),
            _ => None,
        })
        .expect("the HTML table is a table block");
    assert_eq!(head[0].text, "列");
    assert_eq!(head[1].text, "含义");
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0][0].text, "空文件");
    assert_eq!(rows[1][1].text, "非 UTF-8");
    assert_eq!(aligns[0], Align::Left);
    assert_eq!(aligns[1], Align::Right);
}

#[test]
fn html_lists_become_real_lists() {
    let doc = parse(fixtures::HTML);
    let items = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::List { items, .. } => Some(items.clone()),
            _ => None,
        })
        .expect("the HTML list is a list block");
    assert_eq!(items.len(), 2);
    assert!(debug(&items[0].1).contains("第一项"));
    let second = debug(&items[1].1);
    assert!(second.contains("第二项"), "{second}");
    assert!(
        second.contains("换行"),
        "the <br> becomes a line break: {second}"
    );
}

#[test]
fn html_inside_code_stays_verbatim() {
    let doc = parse(fixtures::HTML);
    let code = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Code { language, text } if language.as_deref() == Some("html") => {
                Some(text.clone())
            }
            _ => None,
        })
        .expect("the fenced HTML sample");
    assert!(code.contains("<!-- 代码块中的注释保留 -->"), "{code}");
    assert!(code.contains("<div align=\"center\">raw</div>"), "{code}");

    let copied = copied(&doc.blocks);
    assert!(copied.contains("<div align=\"center\">"), "{copied}");
    assert!(copied.contains("<span>x</span>"), "{copied}");
}

#[test]
fn multiline_and_inline_comments_never_reach_the_model() {
    let doc = parse("before <!-- one\ntwo --> after\n\n<!-- whole\nblock comment -->\n\nend");
    let copied = copied(&doc.blocks);
    assert_eq!(copied.trim(), "before after\n\nend", "{copied}");
    assert!(!debug(&doc.blocks).contains("<!--"), "{:?}", doc.blocks);
}

#[test]
fn malformed_markup_is_bounded() {
    // An unterminated comment runs to the end of its fragment and is dropped.
    let doc = parse("<!-- never closed\n\nmore text");
    let visible = debug(&doc.blocks);
    assert!(!visible.contains("never closed"), "{visible}");

    // An unclosed inline element still styles the text that follows it.
    let bold = parse("<b>bold without end");
    let text = find_paragraph(&bold.blocks, "bold without end");
    assert!(text.styles.contains(&(0..16, Style::Strong)), "{text:?}");

    // Text that only looks like a tag stays literal text rather than markup.
    let literal = parse("a < b and c > d");
    assert_eq!(
        find_paragraph(&literal.blocks, "a < b").text,
        "a < b and c > d"
    );
}

#[test]
fn scripts_handlers_and_frames_are_not_visible_or_active() {
    let doc = parse(
        "<button onclick=\"steal()\">go</button>\n\n<iframe src=\"https://example.com\">fallback</iframe>\n\n<img src=\"a.png\" alt=\"pic\" onerror=\"steal()\">",
    );
    let visible = debug(&doc.blocks);
    assert!(visible.contains("go"), "{visible}");
    assert!(!visible.contains("onclick"), "{visible}");
    assert!(!visible.contains("steal"), "{visible}");
    assert!(!visible.contains("fallback"), "{visible}");
    assert!(
        doc.blocks
            .iter()
            .any(|block| matches!(block, Block::Image { alt, .. } if alt == "pic"))
    );
}

#[test]
fn a_block_inside_a_cell_is_flattened_without_losing_text() {
    let doc = parse("<table><tr><td><p>first</p><p>second</p></td><td><b>x</b></td></tr></table>");
    let (head, rows) = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Table { head, rows, .. } => Some((head.clone(), rows.clone())),
            _ => None,
        })
        .expect("table");
    // A header-less HTML table uses its first row as the Markdown header row.
    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(head[0].text, "first second", "{:?}", head[0]);
    assert_eq!(head[1].text, "x", "{:?}", head[1]);
    assert!(
        head[1].styles.contains(&(0..1, Style::Strong)),
        "{:?}",
        head[1]
    );
}

#[test]
fn entities_and_attributes_are_decoded_once() {
    let doc = parse("<p title=\"a &amp; b\">&copy; 2026 &mdash; &lt;ok&gt; &#128640;</p>");
    let text = find_paragraph(&doc.blocks, "2026");
    assert_eq!(text.text, "© 2026 — <ok> 🚀", "{text:?}");
}

#[test]
fn headings_from_html_are_registered_for_navigation() {
    let doc = parse("<h2 id=\"setup\">Setup</h2>\n\nbody");
    assert!(matches!(doc.blocks[0], Block::Heading(2, _)));
    assert_eq!(doc.headings[0].title, "Setup");
    assert_eq!(doc.headings[0].level, 2);
}

#[test]
fn deeply_nested_html_is_flattened_instead_of_overflowing() {
    // This is small enough to fit the reader budget but deep enough that an
    // unbounded recursive tree would abort while parsing, walking or dropping.
    let depth = 60_000;
    let input = format!("{}text{}", "<div>".repeat(depth), "</div>".repeat(depth));
    let doc = parse(&input);
    let copied = copied(&doc.blocks);
    assert!(copied.contains("text"), "{copied}");
}

#[test]
fn entity_decoding_does_not_rescan_the_rest_of_the_document() {
    let bare = "&".repeat(100_000);
    assert_eq!(html::decode_entities(&bare).len(), bare.len());
    assert_eq!(html::decode_entities("a &amp; b &#128640;"), "a & b 🚀");
}

#[test]
fn dropped_inline_elements_suppress_their_content_across_events() {
    let doc = parse(
        "before <script>alert(1)</script> after\n\n<style>x { color: red }</style>\n\n<iframe>fallback</iframe>",
    );
    let copied = copied(&doc.blocks);
    assert_eq!(copied.trim(), "before after", "{copied}");
}

#[test]
fn unterminated_end_tags_consume_the_fragment_in_linear_time() {
    let mut input = String::from("<div>");
    for _ in 0..100_000 {
        input.push_str("</a");
    }
    input.push_str("</div>");
    let doc = parse(&input);
    assert!(!copied(&doc.blocks).contains("</a"));
}

#[test]
fn over_wide_tables_fall_back_to_text_without_quadratic_padding() {
    let mut input = String::from("<table><tr>");
    for _ in 0..2_000 {
        input.push_str("<td>x</td>");
    }
    input.push_str("</tr>");
    for _ in 0..2_000 {
        input.push_str("<tr><td>y</td></tr>");
    }
    input.push_str("</table>");
    let doc = parse(&input);
    assert!(
        !doc.blocks
            .iter()
            .any(|block| matches!(block, Block::Table { .. })),
        "an over-budget table must not become a padded grid"
    );
    assert!(copied(&doc.blocks).contains("x"), "the text is preserved");
}

#[test]
fn html_inline_style_ranges_stay_on_char_boundaries() {
    let doc = parse("<b>中文 bold</b> tail");
    let text = find_paragraph(&doc.blocks, "中文");
    assert_eq!(text.text, "中文 bold tail");
    assert!(text.styles.contains(&(0..11, Style::Strong)), "{text:?}");
    for (range, _) in &text.styles {
        assert!(
            text.text.is_char_boundary(range.start) && text.text.is_char_boundary(range.end),
            "{range:?} in {:?}",
            text.text
        );
    }
}
