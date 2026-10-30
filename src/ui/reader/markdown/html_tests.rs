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

/// Records the actual requests handed out by ReaderView; failed completion is
/// intentional so the test exercises identity and refusal without network I/O.
#[derive(Default)]
struct RecordingImages(std::sync::Mutex<Vec<crate::content::images::ImageRequest>>);

impl crate::content::images::ImageProvider for RecordingImages {
    fn start(
        &self,
        request: crate::content::images::ImageRequest,
        sink: crate::content::images::ImageSink,
    ) {
        self.0.lock().unwrap().push(request.clone());
        sink.push(crate::content::images::ImageCompletion {
            scope: request.scope,
            key: request.key,
            result: Err(crate::content::images::ImageFailure::NotFound),
        });
    }

    fn cancel(&self, _scope: crate::content::images::ImageScope) {}
}

#[gpui::test]
fn html_images_use_the_existing_reader_context_and_comments_never_request(
    cx: &mut gpui::TestAppContext,
) {
    use super::super::{DocumentKind, ImageTarget};
    use crate::content::PreparedBody;
    use gpui::VisualTestContext;

    fn frames(cx: &mut VisualTestContext) {
        for _ in 0..4 {
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.background_executor
                .advance_clock(std::time::Duration::from_millis(25));
        }
    }
    let provider = std::sync::Arc::new(RecordingImages::default());
    let (reader, cx) = super::super::tests::open(
        cx,
        PreparedBody::Utf8 {
            text: "<div align=\"center\"><img src=\"images/logo.png\" alt=\"Logo\"></div>\n\n<!-- <img src=\"hidden.png\"> -->\n\nbefore <script>![hidden](secret.png)</script> after".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    reader.update_in(cx, |reader, _, cx| {
        reader.set_image_provider(Some(provider.clone()), cx)
    });
    frames(cx);
    let requests = provider.0.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "comments and scripts must not load images: {requests:?}"
    );
    assert_eq!(
        requests[0].target,
        ImageTarget::Repository {
            owner: fixtures::OWNER.into(),
            name: fixtures::NAME.into(),
            commit: fixtures::COMMIT.into(),
            path: "docs/images/logo.png".into(),
        }
    );
    assert_eq!(
        requests[0].scope,
        reader.read_with(cx, |reader, _| reader.image_scope())
    );

    reader.update_in(cx, |reader, window, cx| {
        reader
            .markdown()
            .unwrap()
            .update(cx, |markdown, cx| markdown.focus(window, cx));
    });
    cx.simulate_keystrokes("secondary-a secondary-c");
    cx.run_until_parked();
    let text = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(text.contains("Logo"), "{text:?}");
    assert!(text.contains("before after"), "{text:?}");
    assert!(!text.contains("hidden"), "{text:?}");
}

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
fn dropped_inline_html_emits_no_images_styles_or_math() {
    for tag in ["script", "style", "template", "iframe", "object"] {
        for source in [
            format!("before <{tag}>![hidden](secret.png) **bold** $math$</{tag}> after"),
            format!("| head |\n|---|\n| before <{tag}>![hidden](secret.png)</{tag}> after |"),
        ] {
            let doc = parse(&source);
            let text = copied(&doc.blocks);
            assert!(text.contains("before after"), "{tag}: {doc:?}");
            assert!(!text.contains("hidden"), "{tag}: {doc:?}");
            assert!(!text.contains("bold"), "{tag}: {doc:?}");
            assert!(!text.contains("math"), "{tag}: {doc:?}");
            assert!(!debug(&doc.blocks).contains("secret.png"), "{tag}: {doc:?}");
        }
    }
}

#[test]
fn inline_suppression_requires_a_real_end_tag() {
    let doc = parse("before <script><!-- </script> --> hidden </scripture> hidden </script> after");
    assert_eq!(copied(&doc.blocks).trim(), "before after");
}

#[test]
fn html_list_starts_cannot_overflow_the_renderers_counter() {
    let doc = parse("<ol start=\"18446744073709551615\"><li>one</li><li>two</li></ol>");
    let (start, items) = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::List { start, items } => Some((*start, items.len())),
            _ => None,
        })
        .expect("list");
    let first = start.expect("an ordered list keeps its start");
    assert_eq!(items, 2);
    assert!(
        first.checked_add(items as u64 + 64).is_some(),
        "start {first} must leave room for the item index"
    );
}

#[test]
fn suppression_search_stays_on_utf8_boundaries() {
    let doc = parse("before <script><!-- </中文> --></script> after\n\nafter script 中文 tail");
    let copied = copied(&doc.blocks);
    assert!(copied.contains("before"), "{copied}");
    assert!(!copied.contains("中文>"), "{copied}");
    assert!(copied.contains("after script 中文 tail"), "{copied}");
}

#[test]
fn raw_text_end_tags_need_a_real_tag_name_boundary() {
    let doc = parse("<script>const example = \"</scripture>\"; secret();</script><p>visible</p>");
    let copied = copied(&doc.blocks);
    assert!(!copied.contains("secret"), "{copied}");
    assert!(copied.contains("visible"), "{copied}");
}

#[test]
fn html_ranges_survive_images_that_remove_alt_text() {
    let doc = parse("<a href=\"guide.md\">before ![alt](p.png) after</a>");
    let paragraph = find_paragraph(&doc.blocks, "before");
    assert_eq!(paragraph.text, "before  after", "{paragraph:?}");
    assert_eq!(
        paragraph.links,
        vec![(0..13, "guide.md".to_string())],
        "the surrounding anchor still covers the text after the image: {paragraph:?}"
    );
}

#[test]
fn many_unclosed_inline_tags_stay_bounded() {
    let mut input = "<b>".repeat(20_000);
    input.push_str("text");
    for _ in 0..20_000 {
        input.push_str("</i>");
    }
    let doc = parse(&input);
    let paragraph = find_paragraph(&doc.blocks, "text");
    assert!(!paragraph.text.is_empty());
}

#[test]
fn html_links_survive_markdown_emphasis_inside_a_cell() {
    let doc = parse("| a |\n|---|\n| <a href=\"guide.md\">read *this*</a> |");
    let rows = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("table");
    let cell = &rows[0][0];
    assert_eq!(cell.text, "read this", "{cell:?}");
    assert_eq!(cell.links, vec![(0..9, "guide.md".to_string())], "{cell:?}");
}

#[test]
fn mixed_markdown_cells_close_html_before_the_trailing_text() {
    let doc = parse(
        "| link | style |\n|---|---|\n| <a href=\"guide.md\">read *this*</a> outside | <b>bold *word*</b> outside |",
    );
    let Block::Table { rows, .. } = &doc.blocks[0] else {
        panic!("expected table: {doc:?}");
    };
    let link = &rows[0][0];
    assert_eq!(link.text, "read this outside");
    assert_eq!(link.links, vec![(0..9, "guide.md".into())]);
    let style = &rows[0][1];
    assert_eq!(style.text, "bold word outside");
    assert!(style.styles.contains(&(0..9, Style::Strong)), "{style:?}");
    assert!(style.styles.contains(&(5..9, Style::Emphasis)), "{style:?}");
}

#[test]
fn inline_title_content_is_dropped_until_its_real_closing_tag() {
    for source in [
        "before <title>hidden</title> after",
        "| heading |\n|---|\n| before <title>hidden</title> after |",
    ] {
        let doc = parse(source);
        let text = copied(&doc.blocks);
        assert!(text.contains("before after"), "{text:?}");
        assert!(!text.contains("hidden"), "{text:?}");
    }
    // An end tag at the exact end of the fragment is still a real close.
    let nodes = html::parse("<title>hidden</title>");
    assert!(
        !nodes
            .iter()
            .any(|node| matches!(node, html::Node::Suppressed(_)))
    );
}

#[test]
fn html_ranges_never_point_into_removed_image_alt_text() {
    let doc = parse("![ab<b>x](a)中文</b>");
    let paragraph = find_paragraph(&doc.blocks, "中文");
    let ranges: Vec<_> = paragraph
        .styles
        .iter()
        .map(|(range, _)| range.clone())
        .chain(paragraph.links.iter().map(|(range, _)| range.clone()))
        .collect();
    for range in ranges {
        assert!(
            paragraph.text.is_char_boundary(range.start)
                && paragraph.text.is_char_boundary(range.end)
                && range.end <= paragraph.text.len(),
            "stale range {range:?} in {:?}",
            paragraph.text
        );
    }
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

#[test]
fn dropped_elements_stay_dropped_across_html_block_boundaries() {
    for tag in ["template", "iframe", "object", "style", "script"] {
        let source = format!("<{tag}>\n\n![hidden](secret.png)\n\n</{tag}>\n\nafter");
        let doc = parse(&source);
        assert!(!debug(&doc.blocks).contains("secret.png"), "{tag}: {doc:?}");
        assert_eq!(copied(&doc.blocks).trim(), "after", "{tag}: {doc:?}");
    }
    // A comment or a longer name must not resume the dropped span.
    let doc = parse(
        "<template>\n\n![hidden](secret.png)\n\n<!-- </template> -->\n\n</templates>\n\nstill hidden\n\n</template>\n\nafter",
    );
    let text = copied(&doc.blocks);
    assert!(!text.contains("still hidden"), "{text:?}");
    assert_eq!(text.trim(), "after", "{text:?}");
}

#[test]
fn html_entities_keep_non_breaking_spaces() {
    let doc = parse("<p>a&nbsp;&nbsp;b</p>\n\n<p>c   d</p>");
    let text = find_paragraph(&doc.blocks, "a\u{a0}");
    assert_eq!(text.text, "a\u{a0}\u{a0}b");
    assert_eq!(
        find_paragraph(&doc.blocks, "c").text,
        "c d",
        "ordinary HTML ASCII runs keep collapsing"
    );
}

#[test]
fn a_dropped_span_keeps_the_content_before_and_after_it() {
    let doc = parse("<p>before</p><template>\n\nhidden\n\n</template>\n\nafter");
    let text = copied(&doc.blocks);
    assert!(text.contains("before"), "{text:?}");
    assert!(!text.contains("hidden"), "{text:?}");
    assert!(text.contains("after"), "{text:?}");
}

#[test]
fn a_list_that_opens_the_dropped_span_still_closes_normally() {
    let doc = parse("- <template>\n\nhidden\n\n</template>\n\nafter");
    let text = copied(&doc.blocks);
    assert!(!text.contains("hidden"), "{text:?}");
    assert!(text.contains("after"), "{text:?}");
    assert!(
        doc.blocks.iter().any(
            |block| matches!(block, Block::Paragraph(inline) if inline.text.contains("after"))
        ),
        "the trailing paragraph lands in the root, not a stale list: {doc:?}"
    );
}

#[test]
fn inline_closing_tags_end_block_suppression() {
    let doc = parse("<template>\n\nhidden </template>\n\nafter");
    let text = copied(&doc.blocks);
    assert!(!text.contains("hidden"), "{text:?}");
    assert!(text.contains("after"), "{text:?}");
}

#[test]
fn textarea_content_is_dropped_whole() {
    for source in [
        "<textarea>hidden</textarea>",
        "<textarea>\n\nhidden\n\n</textarea>\n\nafter",
        "<p>a</p><textarea>hidden</textarea><p>b</p>",
    ] {
        let doc = parse(source);
        let text = copied(&doc.blocks);
        assert!(!text.contains("hidden"), "{source:?} -> {doc:?}");
    }
}

#[test]
fn void_elements_and_comments_never_start_suppression() {
    for source in [
        "<meta charset=\"utf-8\">\n\n# Visible",
        "<link rel=\"stylesheet\" href=\"a.css\">\n\n# Visible",
        "<!--\n<script>\n-->\n\nVisible",
        "<div data-tag=\"<script>\">\n\nVisible",
    ] {
        let doc = parse(source);
        let text = copied(&doc.blocks);
        assert!(text.contains("Visible"), "{source:?} -> {doc:?}");
    }
}

#[test]
fn an_inline_opening_keeps_suppressing_after_its_paragraph_ends() {
    let doc = parse("before <template>\n\n![hidden](secret.png)\n\n</template>\n\nafter");
    let text = copied(&doc.blocks);
    assert!(text.contains("before"), "{text:?}");
    assert!(!text.contains("secret.png"), "{doc:?}");
    assert!(text.contains("after"), "{text:?}");
}

#[test]
fn a_multiline_comment_cannot_close_a_dropped_span() {
    let doc = parse(
        "<template>\n\n<!--\n</template>\n-->\n\n![hidden](secret.png)\n\n</template>\n\nafter",
    );
    let text = copied(&doc.blocks);
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    assert!(!text.contains("hidden"), "{text:?}");
    assert!(text.contains("after"), "{text:?}");
}

#[test]
fn a_new_dropped_opening_after_the_close_is_suppressed_too() {
    let doc =
        parse("<template>\n\n</template><iframe>\n\n![hidden](secret.png)\n\n</iframe>\n\nafter");
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    let text = copied(&doc.blocks);
    assert!(!text.contains("hidden"), "{text:?}");
    assert!(text.contains("after"), "{text:?}");
}

#[test]
fn nested_templates_drop_their_whole_subtree() {
    let doc =
        parse("<template><template>inner</template><img src=\"secret.png\" alt=\"x\"></template>");
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    // The outer element closes here, so the trailing text stays readable.
    let doc = parse(
        "<template><template>inner</template><img src=\"secret.png\">\n\n</template>\n\nafter",
    );
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    assert!(copied(&doc.blocks).contains("after"), "{doc:?}");
}

#[test]
fn a_long_suppressed_block_is_judged_once_per_block() {
    let mut input = String::from("<template>\n\n<div>\n");
    for _ in 0..20_000 {
        input.push_str("x\n");
    }
    input.push_str("</template>\n\nafter");
    let started = std::time::Instant::now();
    let doc = parse(&input);
    assert!(copied(&doc.blocks).contains("after"), "{doc:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "suppressed block parsing stayed linear: {:?}",
        started.elapsed()
    );
}

#[test]
fn template_nesting_is_tracked_across_blocks() {
    let doc = parse(
        "<template>\n\n<template>\n\n</template>\n\n![hidden](secret.png)\n\n</template>\n\nafter",
    );
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    let text = copied(&doc.blocks);
    assert!(!text.contains("hidden"), "{text:?}");
    assert!(text.contains("after"), "{text:?}");
}

#[test]
fn tables_balance_when_suppression_ends_inside_a_cell() {
    let doc = parse("<template>\n\n| header |\n| --- |\n| </template> visible |\n\nafter");
    let text = copied(&doc.blocks);
    assert!(text.contains("visible"), "{doc:?}");
    assert!(text.contains("after"), "{doc:?}");
    assert!(!text.contains("header"), "{doc:?}");
}

#[test]
fn completed_templates_do_not_hide_the_prefix_of_an_unclosed_one() {
    let doc = parse(
        "<template>closed</template><p>visible</p><template>\n\nhidden\n\n</template>\n\nafter",
    );
    let text = copied(&doc.blocks);
    assert!(text.contains("visible"), "{doc:?}");
    assert!(text.contains("after"), "{doc:?}");
    assert!(!text.contains("hidden"), "{doc:?}");
    assert!(!text.contains("closed"), "{doc:?}");
}

#[test]
fn cell_suppression_continues_after_the_table() {
    let doc = parse(
        "| h |\n|---|\n| before <template> |\n\n![hidden](secret.png)\n\n</template>\n\nafter",
    );
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    let text = copied(&doc.blocks);
    assert!(text.contains("before"), "{doc:?}");
    assert!(text.contains("after"), "{doc:?}");
}

#[test]
fn dropped_subtrees_stay_hidden_beyond_the_depth_limit() {
    for tag in ["template", "iframe", "object"] {
        let source = format!(
            "{}<{tag}><img src=\"secret.png\"></{tag}>{}",
            "<div>".repeat(256),
            "</div>".repeat(256)
        );
        let doc = parse(&source);
        assert!(!debug(&doc.blocks).contains("secret.png"), "{tag}: {doc:?}");
    }
}

#[test]
fn raw_text_spans_keep_their_own_open_state() {
    for tag in ["script", "style", "title", "textarea"] {
        let open = format!("prefix<{tag}>body");
        let span = html::outstanding_span(&open, tag);
        assert_eq!(span.head_end, 6, "{tag}: {span:?}");
        assert_eq!(span.depth, 1, "{tag}: {span:?}");
        assert_eq!(span.tail_start, None, "{tag}: {span:?}");
        let complete = format!("<{tag}>body</{tag}>suffix");
        let span = html::dropped_span(&complete, tag, 0);
        assert_eq!(&complete[span.tail_start.unwrap()..], "suffix");
    }
}

#[test]
fn raw_text_tag_strings_do_not_increment_inline_nesting() {
    for tag in ["script", "style", "title", "textarea"] {
        let doc = parse(&format!(
            "before <{tag}>const example = \"<{tag}>\";</{tag}> after\n\ntail"
        ));
        let text = copied(&doc.blocks);
        assert!(text.contains("before after"), "{tag}: {doc:?}");
        assert!(text.contains("tail"), "{tag}: {doc:?}");
    }
}

#[test]
fn hidden_blocks_emit_no_empty_shells_or_labels() {
    for hidden in [
        "```hidden-language\nsecret\n```",
        "- secret\n- hidden",
        "> [!WARNING]\n> secret",
        "| secret |\n|---|\n| hidden |",
    ] {
        let doc = parse(&format!("<template>\n\n{hidden}\n\n</template>\n\nvisible"));
        assert_eq!(doc.blocks.len(), 1, "{hidden}: {doc:?}");
        assert_eq!(copied(&doc.blocks).trim(), "visible", "{doc:?}");
    }
}

#[test]
fn nested_raw_text_does_not_close_inline_suppression() {
    let doc = parse(
        "before <template><script>const s = \"</template>\";</script>![hidden](secret.png)</template> after",
    );
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    let text = copied(&doc.blocks);
    assert!(text.contains("before after"), "{doc:?}");
    assert!(!text.contains("const s"), "{doc:?}");
}

#[test]
fn nested_raw_text_survives_inline_to_document_transfer() {
    let doc = parse(
        "before <template><script>\n\n</template>\n\n![hidden](secret.png)\n\n</script></template>\n\nafter",
    );
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    let text = copied(&doc.blocks);
    assert!(text.contains("before"), "{doc:?}");
    assert!(text.contains("after"), "{doc:?}");
}

#[test]
fn raw_text_context_transfers_from_cells_and_block_openings() {
    for source in [
        "| h |\n|---|\n| before <template><script> |\n\n![hidden](secret.png)\n\n</script></template>\n\nafter",
        "<template><script>\n\n</template>\n\n![hidden](secret.png)\n\n</script></template>\n\nafter",
    ] {
        let doc = parse(source);
        assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
        assert!(copied(&doc.blocks).contains("after"), "{doc:?}");
    }
}

#[test]
fn an_active_raw_text_span_does_not_nest_in_later_blocks() {
    for tag in ["script", "style", "title", "textarea"] {
        let doc = parse(&format!("before <{tag}>\n\n<{tag}>\n</{tag}>\n\nafter"));
        let text = copied(&doc.blocks);
        assert!(text.contains("before"), "{tag}: {doc:?}");
        assert!(text.contains("after"), "{tag}: {doc:?}");
        let span = html::scan_dropped_span(&format!("<{tag}>\n</{tag}>"), tag, 1, &mut None);
        assert_eq!(span.depth, 0, "{tag}: {span:?}");
    }
}

#[test]
fn inline_spans_opened_before_suppression_close_at_their_real_end() {
    let doc = parse("*before <script>hidden*</script> after");
    let text = find_paragraph(&doc.blocks, "before");
    assert_eq!(text.text, "before after");
    assert_eq!(text.styles, vec![(0..7, Style::Emphasis)]);
    let doc = parse("[before <script>hidden](guide.md)</script> after");
    let text = find_paragraph(&doc.blocks, "before");
    assert_eq!(text.text, "before after");
    assert_eq!(text.links, vec![(0..7, "guide.md".into())]);
}

#[test]
fn hidden_inline_spans_close_after_suppression_without_popping_visible_links() {
    let doc = parse("[visible <template>*hidden</template> tail* rest](guide.md)");
    let text = find_paragraph(&doc.blocks, "visible");
    assert_eq!(text.text, "visible tail rest");
    assert_eq!(text.links, vec![(0..17, "guide.md".into())]);
    assert!(text.styles.is_empty(), "{text:?}");

    // A visible span nested inside the formerly hidden span still owns its
    // own end. Later suppression must not inherit stale hidden bookkeeping.
    let doc = parse(
        "[visible <template>*hidden</template> **bold** tail* rest](guide.md)\n\n*before <script>hidden*</script> after",
    );
    let text = find_paragraph(&doc.blocks, "visible");
    assert_eq!(text.text, "visible bold tail rest");
    assert_eq!(text.links, vec![(0..22, "guide.md".into())]);
    assert_eq!(text.styles, vec![(8..12, Style::Strong)]);
    let later = find_paragraph(&doc.blocks, "before");
    assert_eq!(later.styles, vec![(0..7, Style::Emphasis)]);
}

#[test]
fn existing_html_spans_close_inside_a_dropped_element() {
    let doc = parse("<a href=\"guide.md\">before <template></a></template> after");
    let text = find_paragraph(&doc.blocks, "before");
    assert_eq!(text.text, "before after");
    assert_eq!(text.links, vec![(0..7, "guide.md".into())]);
    let doc = parse("<b>before <template></b></template> after");
    let text = find_paragraph(&doc.blocks, "before");
    assert_eq!(text.styles, vec![(0..7, Style::Strong)]);
}

#[test]
fn a_script_string_cannot_close_a_dropped_template() {
    let doc = parse(
        "<template>\n\n<script>const example = \"</template>\";</script>\n\n![hidden](secret.png)\n\n</template>\n\nafter",
    );
    assert!(!debug(&doc.blocks).contains("secret.png"), "{doc:?}");
    let text = copied(&doc.blocks);
    assert!(!text.contains("hidden"), "{text:?}");
    assert!(text.contains("after"), "{text:?}");
}
