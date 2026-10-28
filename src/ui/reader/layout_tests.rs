//! Repository-only Markdown content inset uses the real ListState geometry.
use super::tests::open;
use super::*;
use crate::content::PreparedBody;
use gpui::{Modifiers, TestAppContext, VisualTestContext, px};
fn frames(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|w, cx| w.simulate_next_frame(cx));
    }
}
#[gpui::test]
fn repository_inset_scrolls_inside_full_viewport_without_replacing_document(
    cx: &mut TestAppContext,
) {
    let text = (0..80)
        .map(|i| format!("Paragraph {i}: hello 中文 **bold** and `inline`.\n\n"))
        .collect::<String>();
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(800.), px(500.)));
    frames(cx);
    let document = reader.read_with(cx, |r, _| r.markdown().unwrap().clone());
    let before = document.read_with(cx, |d, _| d.list_state().viewport_bounds());
    let root = cx.debug_bounds("reader-markdown").unwrap();
    assert!(
        before.origin.y > root.origin.y,
        "default host keeps outer padding"
    );
    reader.update(cx, |r, cx| {
        r.set_markdown_layout(MarkdownLayout::Repository, cx)
    });
    frames(cx);
    let viewport = document.read_with(cx, |d, _| d.list_state().viewport_bounds());
    let root = cx.debug_bounds("reader-markdown").unwrap();
    assert_eq!(viewport.origin.y, root.origin.y);
    assert_eq!(viewport.bottom(), root.bottom());
    assert!(viewport.origin.x > before.origin.x);
    assert_eq!(
        document.entity_id(),
        reader.read_with(cx, |r, _| r.markdown().unwrap().entity_id())
    );
    let count = document.read_with(cx, |d, _| d.block_count());
    assert_eq!(count, 80, "no synthetic list items");
    let first = cx.debug_bounds("markdown-paragraph-0").unwrap();
    assert!(first.origin.y > viewport.origin.y);
    document.update(cx, |d, cx| d.select_all(cx));
    let selected = document.read_with(cx, |d, _| d.selected_text());
    reader.update(cx, |r, cx| {
        r.set_markdown_layout(MarkdownLayout::Repository, cx)
    });
    assert_eq!(selected, document.read_with(cx, |d, _| d.selected_text()));
    // Blank scrolling inset participates in the document's existing selection hit testing.
    cx.simulate_click(
        gpui::point(viewport.center().x, viewport.origin.y + px(2.)),
        Modifiers::none(),
    );
    assert!(document.read_with(cx, |d, _| d.selected_text().is_empty()));
    document.update(cx, |d, cx| {
        d.list_state().scroll_to_end();
        cx.notify();
    });
    frames(cx);
    let end = cx.debug_bounds("markdown-paragraph-79").unwrap_or_else(|| {
        panic!(
            "top={:?}, reach={:?}, offset={:?}",
            document.read_with(cx, |d, _| d.list_state().logical_scroll_top()),
            document.read_with(cx, |d, _| d.list_state().max_offset_for_scrollbar()),
            document.read_with(cx, |d, _| d.list_state().scroll_px_offset_for_scrollbar())
        )
    });
    assert!(
        end.bottom() < viewport.bottom(),
        "last content retains scrolling bottom inset"
    );
    document.update(cx, |d, cx| d.select_all(cx));
    cx.simulate_click(
        gpui::point(viewport.center().x, viewport.bottom() - px(2.)),
        Modifiers::none(),
    );
    assert!(document.read_with(cx, |d, _| d.selected_text().is_empty()));
}
#[gpui::test]
fn short_repository_markdown_bottom_whitespace_clears_selection(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "# Short\n\nBody\n".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    reader.update(cx, |r, cx| {
        r.set_markdown_layout(MarkdownLayout::Repository, cx)
    });
    frames(cx);
    let doc = reader.read_with(cx, |r, _| r.markdown().unwrap().clone());
    doc.update(cx, |d, cx| d.select_all(cx));
    let viewport = doc.read_with(cx, |d, _| d.list_state().viewport_bounds());
    cx.simulate_click(
        gpui::point(viewport.center().x, viewport.bottom() - px(5.)),
        Modifiers::none(),
    );
    assert!(doc.read_with(cx, |d, _| d.selected_text().is_empty()));
    assert_eq!(doc.read_with(cx, |d, _| d.block_count()), 2);
}

#[gpui::test]
fn wheel_and_thumb_reach_true_tail_inset_for_all_final_block_kinds(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "short".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    reader.update(cx, |r, cx| {
        r.set_markdown_layout(MarkdownLayout::Repository, cx)
    });
    cx.simulate_resize(gpui::size(px(700.), px(450.)));
    for ending in [
        "Final paragraph.
",
        "- final one
- final two
",
        "```rust
let final_line = 42;
```
",
        "| Last | Value |
| --- | --- |
| end | 42 |
",
    ] {
        let mut text = String::new();
        for i in 0..80 {
            text.push_str(&format!(
                "Paragraph {i}: some content 中文.

"
            ));
        }
        text.push_str(ending);
        reader.update_in(cx, |r, w, cx| {
            r.show(
                PreparedBody::Utf8 {
                    text,
                    has_bom: false,
                },
                DocumentKind::Markdown,
                w,
                cx,
            )
        });
        frames(cx);
        let document = reader.read_with(cx, |r, _| r.markdown().unwrap().clone());
        let count = document.read_with(cx, |d, _| d.block_count());
        let viewport = document.read_with(cx, |d, _| d.list_state().viewport_bounds());
        // Thumb writes use exactly the same Motion/ListState target as the real Bar.
        for _ in 0..6 {
            document.update(cx, |d, cx| {
                let motion = d.motion();
                use crate::ui::scroll::ScrollTarget;
                motion.set_axis_offset(gpui::Axis::Vertical, -motion.max_offset().y);
                cx.notify();
            });
            frames(cx);
        }
        let last = document
            .read_with(cx, |d, _| d.list_state().bounds_for_item(count - 1))
            .expect("last actual block painted");
        assert!(last.bottom() <= viewport.bottom() + px(1.));
        let content = document
            .read_with(cx, |d, _| d.painted_content_bounds(count - 1))
            .unwrap();
        assert!(
            viewport.bottom() - content.bottom() >= px(12.0),
            "thumb target preserves content inset for {ending}: {content:?} / {viewport:?}"
        );
        // Its content, excluding outer block padding, is recorded by item prepaint.
        document.update(cx, |d, cx| d.select_all(cx));
        let copied = document.read_with(cx, |d, _| d.selected_text());
        assert!(copied.contains("Paragraph 79"));
        cx.simulate_click(
            gpui::point(viewport.center().x, viewport.bottom() - px(2.)),
            Modifiers::none(),
        );
        assert!(document.read_with(cx, |d, _| d.selected_text().is_empty()));
        document.update(cx, |d, cx| {
            d.list_state().scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        frames(cx);
        for _ in 0..4 {
            cx.simulate_mouse_move(viewport.center(), None, Modifiers::none());
            cx.simulate_event(gpui::ScrollEvent {
                position: viewport.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-10000.))),
                modifiers: Modifiers::none(),
                touch_phase: Default::default(),
                source: gpui::ScrollSource::Pointer,
            });
            frames(cx);
        }
        assert!(
            document.read_with(cx, |d, _| d
                .list_state()
                .bounds_for_item(count - 1)
                .is_some()),
            "wheel reaches tail for {ending}"
        );
        let content = document
            .read_with(cx, |d, _| d.painted_content_bounds(count - 1))
            .unwrap();
        assert!(
            viewport.bottom() - content.bottom() >= px(12.0),
            "wheel content inset for {ending}"
        );
        // Drive the actual shared thumb event path after the lazy tail has been measured.
        document.update(cx, |d, cx| {
            d.list_state().scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        reader.update(cx, |r, cx| {
            r.show_scrollbar();
            cx.notify();
        });
        frames(cx);
        let thumb = cx.debug_bounds("reader-markdown-scrollbar-thumb").unwrap();
        let start = thumb.center();
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::none());
        for step in 1..=12 {
            cx.simulate_mouse_move(
                gpui::point(
                    start.x,
                    start.y + (viewport.size.height + px(120.)) * step as f32 / 12.,
                ),
                gpui::MouseButton::Left,
                Modifiers::none(),
            );
            frames(cx);
        }
        cx.simulate_mouse_up(
            gpui::point(start.x, viewport.bottom() + px(120.)),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
        frames(cx);
        let content = document
            .read_with(cx, |d, _| d.painted_content_bounds(count - 1))
            .unwrap();
        assert!(
            document.read_with(cx, |d, _| d
                .list_state()
                .bounds_for_item(count - 1)
                .is_some()),
            "actual thumb reaches tail"
        );
        assert!(
            viewport.bottom() - content.bottom() >= px(12.),
            "actual thumb retains content inset"
        );
    }
}

#[gpui::test]
fn repository_option_does_not_rebuild_or_repad_source(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "fn main() {}
"
            .into(),
            has_bom: false,
        },
        DocumentKind::Source { language: None },
    );
    frames(cx);
    let source = reader.read_with(cx, |r, _| r.source().unwrap().entity_id());
    let gutter = cx.debug_bounds("reader-source-gutter-0");
    reader.update(cx, |r, cx| {
        r.set_markdown_layout(MarkdownLayout::Repository, cx)
    });
    frames(cx);
    assert_eq!(
        source,
        reader.read_with(cx, |r, _| r.source().unwrap().entity_id())
    );
    assert_eq!(gutter, cx.debug_bounds("reader-source-gutter-0"));
}
