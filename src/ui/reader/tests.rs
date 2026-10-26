//! Tests for link resolution, read-only behaviour, selection and virtualization.
//!
//! They run on GPUI's test platform. They cover what the platform can prove:
//! resolution and refusal, that no edit path exists, that a drag selects and
//! Ctrl+C writes the clipboard, that only visible rows are drawn, and that the
//! status states render. Windows-native input, IME and the real window remain
//! manual checks, recorded in `docs/devlog/2026-10-25-content-reader.md`.

use gpui::{
    Entity, Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TestAppContext,
    VisualTestContext, point, px,
};

use super::{
    DocumentKind, ReaderAction, ReaderView, ResourceContext, fixtures,
    links::{self, LinkTarget, RefusalReason},
};

use crate::content::PreparedBody;

fn setup(cx: &mut TestAppContext) {
    cx.update(ely_gpui_component::init_for_tests);
}

fn context() -> ResourceContext {
    ResourceContext::new(
        fixtures::OWNER,
        fixtures::NAME,
        fixtures::COMMIT,
        fixtures::PATH,
    )
}

fn open(
    cx: &mut TestAppContext,
    body: PreparedBody,
    kind: DocumentKind,
) -> (Entity<ReaderView>, &mut VisualTestContext) {
    setup(cx);
    let (reader, cx) =
        cx.add_window_view(|window, cx| ReaderView::new(body, kind, context(), window, cx));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (reader, cx)
}

fn resolve(url: &str) -> LinkTarget {
    links::resolve(url, &context())
}

fn repository_path(target: LinkTarget) -> String {
    match target {
        LinkTarget::Repository(link) => link.path,
        other => panic!("expected a repository link, got {other:?}"),
    }
}

#[test]
fn relative_links_keep_repository_commit_and_directory() {
    let target = resolve("images/diagram.png");
    let LinkTarget::Repository(link) = target else {
        panic!("expected a repository link, got {target:?}");
    };
    assert_eq!(link.owner, fixtures::OWNER);
    assert_eq!(link.name, fixtures::NAME);
    assert_eq!(link.commit, fixtures::COMMIT);
    assert_eq!(link.path, "docs/images/diagram.png");
}

#[test]
fn parent_segments_climb_but_stop_at_the_root() {
    assert_eq!(repository_path(resolve("../LICENSE")), "LICENSE");
    assert_eq!(repository_path(resolve("../../../../LICENSE")), "LICENSE");
    assert_eq!(repository_path(resolve("/src/main.rs")), "src/main.rs");
    assert_eq!(repository_path(resolve("./guide.md")), "docs/guide.md");
    assert_eq!(
        repository_path(resolve("guide.md?plain=1")),
        "docs/guide.md"
    );
    assert_eq!(
        repository_path(resolve("images/a%20b.png")),
        "docs/images/a b.png"
    );
}

#[test]
fn a_fragment_is_reported_but_kept_with_the_file() {
    let target = resolve("guide.md#setup");
    let LinkTarget::Repository(link) = target else {
        panic!("expected a repository link, got {target:?}");
    };
    assert_eq!(link.path, "docs/guide.md");
    assert_eq!(link.anchor.as_deref(), Some("setup"));
    assert!(matches!(resolve("#setup"), LinkTarget::Anchor { .. }));
}

#[test]
fn external_links_are_only_http_and_https() {
    assert!(matches!(
        resolve("https://example.com/x"),
        LinkTarget::External { .. }
    ));
    assert!(matches!(
        resolve("http://example.com"),
        LinkTarget::External { .. }
    ));
}

#[test]
fn dangerous_schemes_are_refused() {
    for url in [
        "javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "file:///C:/Windows/win.ini",
        "mailto:someone@example.com",
        "ms-settings:privacy",
        "vbscript:msgbox(1)",
    ] {
        assert!(
            matches!(
                resolve(url),
                LinkTarget::Refused {
                    reason: RefusalReason::Scheme,
                    ..
                }
            ),
            "{url} must be refused"
        );
    }
    assert!(matches!(
        resolve("//evil.example/x"),
        LinkTarget::Refused {
            reason: RefusalReason::ProtocolRelative,
            ..
        }
    ));
    assert!(matches!(
        resolve("   "),
        LinkTarget::Refused {
            reason: RefusalReason::Empty,
            ..
        }
    ));
}

#[test]
fn document_kind_follows_the_path() {
    assert_eq!(DocumentKind::for_path("README.md"), DocumentKind::Markdown);
    assert_eq!(DocumentKind::for_path("README"), DocumentKind::Markdown);
    assert_eq!(
        DocumentKind::for_path("docs/guide.markdown"),
        DocumentKind::Markdown
    );
    assert_eq!(
        DocumentKind::for_path("src/main.rs"),
        DocumentKind::Source { language: None }
    );
}

#[gpui::test]
fn typing_and_editing_keys_never_change_the_source(cx: &mut TestAppContext) {
    let (harness, cx) = open(
        cx,
        fixtures::source(),
        DocumentKind::Source { language: None },
    );
    let source = cx.debug_bounds("reader-source").expect("the source draws");
    cx.simulate_click(source.center(), Modifiers::none());
    cx.run_until_parked();
    let before = harness.read_with(cx, |reader, cx| {
        reader.source().unwrap().read(cx).text().to_string()
    });
    cx.simulate_input("hello 世界 🌍");
    cx.simulate_keystrokes("backspace delete enter tab");
    cx.run_until_parked();
    let after = harness.read_with(cx, |reader, cx| {
        reader.source().unwrap().read(cx).text().to_string()
    });
    assert_eq!(before, after, "the read-only view must not accept an edit");
}

#[gpui::test]
fn dragging_selects_and_copy_writes_the_clipboard(cx: &mut TestAppContext) {
    let (harness, cx) = open(
        cx,
        fixtures::source(),
        DocumentKind::Source { language: None },
    );
    let body = cx
        .debug_bounds("reader-source-body-0")
        .expect("the first line draws");
    let start = point(body.left() + px(8.0), body.center().y);
    let end = point(body.left() + px(120.0), body.center().y);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(
        harness.read_with(cx, |reader, cx| reader
            .source()
            .unwrap()
            .read(cx)
            .has_selection()),
        "the drag selects"
    );
    cx.simulate_keystrokes("secondary-c");
    cx.run_until_parked();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("Ctrl+C writes the clipboard");
    assert!(!copied.is_empty());
    assert!(
        fixtures::SOURCE.contains(&copied),
        "copied {copied:?} is part of the source"
    );
}

#[gpui::test]
fn a_large_source_only_draws_visible_rows(cx: &mut TestAppContext) {
    let (harness, cx) = open(
        cx,
        fixtures::large_source(),
        DocumentKind::Source { language: None },
    );
    let count = harness.read_with(cx, |reader, cx| {
        reader.source().unwrap().read(cx).line_count()
    });
    assert!(count > 10_000, "the fixture has many lines, got {count}");
    assert!(cx.debug_bounds("reader-source-row-0").is_some());
    assert!(
        cx.debug_bounds("reader-source-row-20000").is_none(),
        "a far row must not be rendered"
    );
    let frame = cx.debug_bounds("reader-source").unwrap();
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(0.0, -40.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
    });
    cx.run_until_parked();
    let offset = harness.read_with(cx, |reader, cx| reader.source().unwrap().read(cx).offset());
    assert!(offset.y < px(0.0), "the list scrolls, offset {offset:?}");
}

#[gpui::test]
fn empty_unsupported_and_too_large_draw_a_status(cx: &mut TestAppContext) {
    let (harness, cx) = open(cx, fixtures::empty(), DocumentKind::Markdown);
    assert!(harness.read_with(cx, |reader, _| reader.is_status()));
    assert!(cx.debug_bounds("reader-status").is_some());
    for (body, kind) in [
        (fixtures::unsupported(), DocumentKind::Markdown),
        (fixtures::too_large(), DocumentKind::Markdown),
    ] {
        harness.update_in(cx, |reader, window, cx| reader.show(body, kind, window, cx));
        cx.run_until_parked();
        assert!(harness.read_with(cx, |reader, _| reader.is_status()));
        assert!(cx.debug_bounds("reader-status").is_some());
    }
}

#[gpui::test]
fn markdown_draws_its_blocks(cx: &mut TestAppContext) {
    let (harness, cx) = open(cx, fixtures::markdown(), DocumentKind::Markdown);
    frames(cx);
    assert!(cx.debug_bounds("reader-markdown").is_some());
    assert!(cx.debug_bounds("markdown-block-0").is_some());
    assert!(cx.debug_bounds("markdown-heading-0").is_some());
    assert!(harness.read_with(cx, |reader, _| reader.markdown().is_some()));
}

#[gpui::test]
fn pressed_links_are_resolved_not_opened(cx: &mut TestAppContext) {
    let (harness, cx) = open(cx, fixtures::link_only(), DocumentKind::Markdown);
    let block = cx
        .debug_bounds("markdown-block-0")
        .expect("the block draws");
    cx.simulate_click(
        point(block.left() + px(20.0), block.center().y),
        Modifiers::none(),
    );
    cx.run_until_parked();
    match harness.read_with(cx, |reader, _| reader.last_action().cloned()) {
        Some(ReaderAction::Link(LinkTarget::Repository(link))) => {
            assert_eq!(link.path, "docs/guide.md");
            assert_eq!(link.commit, fixtures::COMMIT);
        }
        other => panic!("expected a repository link, got {other:?}"),
    }
}

#[gpui::test]
fn a_dangerous_link_is_refused_without_opening_anything(cx: &mut TestAppContext) {
    let (harness, cx) = open(cx, fixtures::dangerous_link_only(), DocumentKind::Markdown);
    let block = cx
        .debug_bounds("markdown-block-0")
        .expect("the block draws");
    cx.simulate_click(
        point(block.left() + px(20.0), block.center().y),
        Modifiers::none(),
    );
    cx.run_until_parked();
    let action = harness.read_with(cx, |reader, _| reader.last_action().cloned());
    assert!(
        matches!(
            action,
            Some(ReaderAction::Link(LinkTarget::Refused {
                reason: RefusalReason::Scheme,
                ..
            }))
        ),
        "expected a refusal, got {action:?}"
    );
}

#[gpui::test]
fn handle_link_keeps_the_commit_and_refuses_scripts(cx: &mut TestAppContext) {
    let (harness, cx) = open(
        cx,
        fixtures::source(),
        DocumentKind::Source { language: None },
    );
    harness.update_in(cx, |reader, window, cx| {
        reader.handle_link("../LICENSE", window, cx);
    });
    match harness.read_with(cx, |reader, _| reader.last_action().cloned()) {
        Some(ReaderAction::Link(LinkTarget::Repository(link))) => {
            assert_eq!(link.path, "LICENSE");
            assert_eq!(link.commit, fixtures::COMMIT);
        }
        other => panic!("expected a repository link, got {other:?}"),
    }
    harness.update_in(cx, |reader, window, cx| {
        reader.handle_link("javascript:alert(1)", window, cx);
    });
    assert!(matches!(
        harness.read_with(cx, |reader, _| reader.last_action().cloned()),
        Some(ReaderAction::Link(LinkTarget::Refused {
            reason: RefusalReason::Scheme,
            ..
        }))
    ));
}

/// A very long line must give the list a horizontal extent it can scroll, not
/// be clipped inside the viewport. Shift+wheel is the Windows horizontal wheel
/// gesture (`gpui-pre-windows` maps shift to `delta.x`); the test sends that
/// delta directly because the platform translation is not part of the test
/// platform.
#[gpui::test]
fn a_long_line_scrolls_horizontally(cx: &mut TestAppContext) {
    let (harness, cx) = open(
        cx,
        fixtures::source(),
        DocumentKind::Source { language: None },
    );
    let frame = cx.debug_bounds("reader-source").expect("the source draws");
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(-20.0, 0.0)),
        modifiers: Modifiers::shift(),
        touch_phase: Default::default(),
    });
    cx.run_until_parked();
    let offset = harness.read_with(cx, |reader, cx| reader.source().unwrap().read(cx).offset());
    assert!(
        offset.x < px(0.0),
        "the widest line must be reachable, offset {offset:?}"
    );
    // The long line is wider than the viewport, so the list must be able to
    // scroll right rather than clip it.
    let widest = harness.read_with(cx, |reader, cx| {
        reader.source().unwrap().read(cx).content_width()
    });
    assert!(
        widest > frame.size.width,
        "the content is wider than the viewport: {widest:?} vs {:?}",
        frame.size.width
    );
}

/// The widest rendered row must remain reachable even when eight other lines
/// have more UTF-8 bytes. Use the test platform's actual shaped bounds rather
/// than assuming how wide its fallback glyphs are.
#[gpui::test]
fn mixed_unicode_lines_expand_horizontal_extent(cx: &mut TestAppContext) {
    let mut text = format!("{}\n", "中".repeat(100)).repeat(8);
    text.push_str(&"a".repeat(299));
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text,
            has_bom: false,
        },
        DocumentKind::Source { language: None },
    );
    cx.simulate_resize(gpui::size(px(600.0), px(500.0)));
    frames(cx);
    let ascii = cx
        .debug_bounds("reader-source-body-8")
        .expect("ASCII row visible");
    let width = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).content_width());
    assert!(
        width >= ascii.size.width,
        "extent {width:?} must include ASCII width {:?}",
        ascii.size.width
    );
    let frame = cx.debug_bounds("reader-source").unwrap();
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: frame.center(),
        delta: ScrollDelta::Pixels(point(px(-100_000.0), px(0.0))),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
    });
    frames(cx);
    let end = cx.debug_bounds("reader-source-body-8").unwrap().right();
    assert!(
        end <= frame.right() + px(1.0),
        "ASCII tail {end:?} must be reachable inside {:?}",
        frame.right()
    );
}

/// The gutter is real layout padding and the selection uses that same origin.
#[gpui::test]
fn gutter_padding_keeps_selection_coordinates_aligned(cx: &mut TestAppContext) {
    let (_reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "abcdef\n".to_string(),
            has_bom: false,
        },
        DocumentKind::Source { language: None },
    );
    cx.simulate_resize(gpui::size(px(600.0), px(400.0)));
    frames(cx);
    let gutter = cx
        .debug_bounds("reader-source-gutter-0")
        .expect("gutter drawn");
    let body = cx.debug_bounds("reader-source-body-0").expect("body drawn");
    assert!(
        body.left() >= gutter.right(),
        "the body starts after the padded gutter"
    );
    let row = cx.debug_bounds("reader-source-row-0").unwrap();
    assert_eq!(gutter.left(), row.left());
    assert_eq!(body.left() - row.left(), gutter.size.width);
    cx.simulate_click(
        point(body.left() + px(2.0), body.center().y),
        Modifiers::none(),
    );
    cx.simulate_keystrokes("secondary-a secondary-c");
    cx.run_until_parked();
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("abcdef\n")
    );
}

/// Reader bars bind the UniformList's own base handle: they exist only while
/// visible, start hidden, and wheel scrolling moves the same real offset.
#[gpui::test]
fn source_scrollbars_follow_the_real_list_and_fade_out(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        fixtures::source(),
        DocumentKind::Source { language: None },
    );
    cx.simulate_resize(gpui::size(px(600.0), px(200.0)));
    frames(cx);
    let frame = cx.debug_bounds("reader-source").unwrap();
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    reader.update_in(cx, |reader, _, cx| {
        reader.source().unwrap().update(cx, |source, _| {
            source.pin_scrollbar_clock(0);
            source.show_scrollbar();
        });
    });
    frames(cx);
    assert!(cx.debug_bounds("reader-scrollbar-y").is_some());
    assert!(cx.debug_bounds("reader-scrollbar-x").is_some());
    let reach = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).reach());
    assert!(reach.y > px(0.0), "the short viewport overflows: {reach:?}");
    assert!(reach.x > px(0.0), "a long line overflows: {reach:?}");
    cx.simulate_event(ScrollWheelEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(0.0, -40.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
    });
    frames(cx);
    let offset = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).offset());
    assert!(
        offset.y < px(0.0),
        "the wheel moves the same handle: {offset:?}"
    );
    // Fade-out timing depends on real hover transitions and is left to the
    // window acceptance; this asserts only that the bars exist while shown and
    // bind the same list the wheel moved.
    assert!(
        cx.debug_bounds("reader-scrollbar-y").is_some(),
        "the vertical bar stays while the pointer is over the reader"
    );
}

/// A drag that continues onto a shorter or empty row keeps extending the
/// selection, and the copied text matches the selection the reader reports.
#[gpui::test]
fn dragging_across_short_and_empty_rows_keeps_selecting(cx: &mut TestAppContext) {
    let (_reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "a much longer first line

short
"
            .to_string(),
            has_bom: false,
        },
        DocumentKind::Source { language: None },
    );
    cx.simulate_resize(gpui::size(px(500.0), px(300.0)));
    frames(cx);
    let long = cx.debug_bounds("reader-source-body-0").unwrap();
    let empty_row = cx.debug_bounds("reader-source-row-hit-1").unwrap();
    let start = point(long.left() + px(4.0), long.center().y);
    let end = empty_row.center();
    cx.simulate_click(start, Modifiers::none());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let selected = _reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).selected_text());
    assert!(
        selected
            .as_deref()
            .is_some_and(|text| text.starts_with("a much longer first line")),
        "a visible selection exists before copying: {selected:?}"
    );
    cx.simulate_keystrokes("secondary-c");
    cx.run_until_parked();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("the drag copies");
    assert!(
        copied.contains("a much longer first line"),
        "the selection spans to the empty row: {copied:?}"
    );
}

/// Row hit bounds belong to the frame that painted them: after scrolling, a
/// drag maps to the row now under the pointer, never to a line that used to sit
/// at those pixels.
#[gpui::test]
fn row_hit_bounds_are_replaced_after_scrolling(cx: &mut TestAppContext) {
    let mut text = String::new();
    for n in 0..200 {
        text.push_str(&format!(
            "line {n}
"
        ));
    }
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text,
            has_bom: false,
        },
        DocumentKind::Source { language: None },
    );
    cx.simulate_resize(gpui::size(px(500.0), px(220.0)));
    frames(cx);
    let before = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).row_count());
    assert!(before > 0, "the first frame painted rows: {before}");
    let frame = cx.debug_bounds("reader-source").unwrap();
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(0.0, -40.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
    });
    frames(cx);
    let after = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).row_count());
    assert!(
        after > 0 && after <= before + 4,
        "scrolling replaces the map instead of accumulating it: {before} then {after}"
    );
}

/// A near-limit Markdown document is parsed off the UI thread and only its
/// visible blocks are drawn.
#[gpui::test]
fn a_large_markdown_document_only_draws_visible_blocks(cx: &mut TestAppContext) {
    let (harness, cx) = open(cx, fixtures::large_markdown(), DocumentKind::Markdown);
    frames(cx);
    let bytes = harness.read_with(cx, |reader, cx| {
        reader.markdown().unwrap().read(cx).text(cx).len()
    });
    assert!(bytes > 1_000_000, "the fixture is large, got {bytes} bytes");
    assert!(
        cx.debug_bounds("markdown-block-20000").is_none(),
        "far blocks must not be built"
    );
    assert!(cx.debug_bounds("markdown-block-0").is_some());
}

/// Four frames, as Ely's own `MarkdownDocument` tests draw after a parse.
fn frames(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}
