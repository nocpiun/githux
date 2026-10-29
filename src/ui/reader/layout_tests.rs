//! Repository-only Markdown content inset uses the real ListState geometry.
use super::tests::{context, open, setup};
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

// ---- natural-height (Flow) layout ----

/// A host that puts a Flow reader inside its own scroll container, the way a
/// profile page scrolls a README section.
struct FlowHost {
    reader: Entity<crate::ui::reader::ReaderView>,
    scrolled: Rc<std::cell::Cell<f32>>,
}

impl gpui::Render for FlowHost {
    fn render(&mut self, _w: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        gpui::div()
            .id("flow-host")
            .debug_selector(|| "flow-host".to_string())
            .size_full()
            .overflow_y_scroll()
            .child(self.reader.clone())
            .child(gpui::div().h(gpui::px(1200.0)).child("page tail"))
    }
}

fn flow_document(blocks: usize) -> crate::content::PreparedBody {
    let mut text = String::new();
    for index in 0..blocks {
        text.push_str(&format!(
            "## Section {index}\n\nA paragraph of prose with 中文 and emoji 🚀 that gives the document real height.\n\n"
        ));
    }
    crate::content::PreparedBody::Utf8 {
        text,
        has_bom: false,
    }
}

#[gpui::test]
fn flow_layout_reports_natural_height_and_has_no_inner_scroll_region(
    cx: &mut gpui::TestAppContext,
) {
    setup(cx);
    let body = flow_document(12);
    let (host, cx) = cx.add_window_view(|window, cx| {
        let reader = cx.new(|cx| {
            let mut reader = crate::ui::reader::ReaderView::new(
                body,
                DocumentKind::Markdown,
                context(),
                window,
                cx,
            );
            reader.set_markdown_layout(MarkdownLayout::Flow, cx);
            reader
        });
        FlowHost {
            reader,
            scrolled: Rc::new(std::cell::Cell::new(0.0)),
        }
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(gpui::size(gpui::px(700.0), gpui::px(400.0)));
    frames(cx);

    let reader = host.read_with(cx, |host, _| host.reader.clone());
    let document_bounds = cx.debug_bounds("reader-markdown").expect("document");
    let host_bounds = cx.debug_bounds("flow-host").expect("host");
    eprintln!(
        "document {:?} host {:?}",
        document_bounds.size, host_bounds.size
    );
    // The document is taller than the host's viewport: it expanded to its own
    // height instead of being clipped to a bounded reader viewport.
    assert!(
        document_bounds.size.height > host_bounds.size.height,
        "a flow document expands past the viewport: {document_bounds:?} vs {host_bounds:?}"
    );
    // The last block paints inside the document (nothing is clipped away).
    let blocks = reader.read_with(cx, |reader, cx| {
        reader
            .markdown()
            .map(|document| document.read(cx).block_count())
    });
    assert_eq!(blocks, Some(24), "no blocks are dropped");
    assert!(reader.read_with(cx, |reader, _| reader.is_flow()));
}

#[gpui::test]
fn flow_layout_keeps_selection_and_copy_working(cx: &mut gpui::TestAppContext) {
    setup(cx);
    let body = crate::content::PreparedBody::Utf8 {
        text: "# Title\n\nThe first paragraph.\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```\n"
            .to_string(),
        has_bom: false,
    };
    let (reader, cx) = cx.add_window_view(|window, cx| {
        let mut reader =
            crate::ui::reader::ReaderView::new(body, DocumentKind::Markdown, context(), window, cx);
        reader.set_markdown_layout(MarkdownLayout::Flow, cx);
        reader
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(gpui::size(gpui::px(700.0), gpui::px(400.0)));
    frames(cx);
    reader.update(cx, |reader, cx| {
        reader
            .markdown()
            .expect("markdown")
            .update(cx, |document, cx| {
                document.select_all(cx);
                assert!(document.copy(cx));
            });
    });
    let document = reader.read_with(cx, |reader, cx| {
        reader
            .markdown()
            .expect("markdown")
            .read(cx)
            .selected_text()
    });
    assert!(document.contains("The first paragraph."), "{document}");
    assert!(document.contains("item two"), "{document}");
    assert!(document.contains("fn main"), "{document}");
}

/// A Flow reader tells its host when its natural height changes, so a picture
/// landing or a re-wrap re-lays out the host's own scroll container.
#[gpui::test]
fn flow_layout_reports_height_changes_to_its_host(cx: &mut gpui::TestAppContext) {
    setup(cx);
    let reports = Rc::new(std::cell::Cell::new(0usize));
    let seen = reports.clone();
    let body = crate::content::PreparedBody::Utf8 {
        text: "# Title\n\nA short paragraph with 中文.\n".to_string(),
        has_bom: false,
    };
    let (reader, cx) = cx.add_window_view(|window, cx| {
        let mut reader =
            crate::ui::reader::ReaderView::new(body, DocumentKind::Markdown, context(), window, cx);
        reader.set_markdown_layout(MarkdownLayout::Flow, cx);
        reader.on_layout_changed(Rc::new(move |_, _cx| {
            seen.set(seen.get() + 1);
        }));
        reader
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(gpui::size(gpui::px(700.0), gpui::px(400.0)));
    frames(cx);
    frames(cx);
    let first = reports.get();
    assert!(first > 0, "the host is told about the first layout");

    // Replacing the document with a much taller one reports a new height.
    let long = flow_document(10);
    reader.update_in(cx, |reader, window, cx| {
        reader.show(long, DocumentKind::Markdown, window, cx);
    });
    frames(cx);
    frames(cx);

    assert!(
        reports.get() > first,
        "a taller document reports a new natural height"
    );

    // A stable document does not keep reporting.
    let settled = reports.get();
    frames(cx);
    frames(cx);
    assert_eq!(
        reports.get(),
        settled,
        "an unchanged document does not keep re-laying out its host"
    );
}
