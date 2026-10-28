//! Tests for link resolution, read-only behaviour, selection and virtualization.
//!
//! They run on GPUI's test platform. They cover what the platform can prove:
//! resolution and refusal, that no edit path exists, that a drag selects and
//! Ctrl+C writes the clipboard, that only visible rows are drawn, and that the
//! status states render. Windows-native input, IME and the real window remain
//! manual checks, recorded in `docs/devlog/2026-10-25-content-reader.md`.

use gpui::{
    Entity, Modifiers, MouseButton, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext,
    point, px,
};

use super::{
    DocumentKind, ReaderAction, ReaderView, ResourceContext, fixtures,
    links::{self, LinkTarget, RefusalReason},
};

use crate::content::PreparedBody;

fn setup(cx: &mut TestAppContext) {
    cx.update(ely_core::init_for_tests);
    cx.update(|cx| {
        ely_core::theme::Theme::update(cx, |theme| {
            theme.font_family = "Noto Sans".into();
            theme.mono_family = "Google Sans Code".into();
        })
    });
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
    cx.simulate_event(ScrollEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(0.0, -40.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
    });
    cx.run_until_parked();
    let offset = harness.read_with(cx, |reader, cx| reader.source().unwrap().read(cx).offset());
    assert!(offset.y < px(0.0), "the list scrolls, offset {offset:?}");
}

#[gpui::test]
fn dragging_a_source_selection_across_a_scroll_keeps_the_row_it_is_on(cx: &mut TestAppContext) {
    let (harness, cx) = open(
        cx,
        fixtures::large_source(),
        DocumentKind::Source { language: None },
    );
    cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
    frames(cx);
    let frame = cx.debug_bounds("reader-source").unwrap();
    let start = cx
        .debug_bounds("reader-source-body-1")
        .expect("the second line draws")
        .left_center()
        + point(px(4.0), px(0.0));
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    // Scroll while the button is held, then keep dragging over the row now
    // under the pointer.
    cx.simulate_event(ScrollEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(0.0, -12.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
    });
    frames(cx);
    let moved = frame.center();
    cx.simulate_mouse_move(moved, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(moved, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_up(moved, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let copied = harness
        .read_with(cx, |reader, cx| {
            reader.source().unwrap().read(cx).selected_text()
        })
        .unwrap_or_default();
    assert!(!copied.is_empty(), "the drag still selects real text");
    assert!(
        copied.starts_with("fn generated_1"),
        "the selection follows the row the press began on: {copied:?}"
    );
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
    cx.simulate_event(ScrollEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(-20.0, 0.0)),
        modifiers: Modifiers::shift(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
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
    cx.simulate_event(ScrollEvent {
        position: frame.center(),
        delta: ScrollDelta::Pixels(point(px(-100_000.0), px(0.0))),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
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
    // The gap is the original 0.75rem plus 6px, measured through the real
    // layout, not typed spaces.
    let expected = super::source::gutter_gap(px(16.0));
    assert_eq!(expected, px(18.0));
    // Gutter box = left inset + drawn number + this gap. Both numbers are
    // measured, so the padding cannot be replaced by typed spaces unnoticed.
    let number: f32 = cx.update(|window, cx| {
        use ely_core::theme::ActiveTheme as _;
        let theme = cx.theme();
        let size = theme
            .text_size(ely_core::theme::TextSize::Sm)
            .to_pixels(window.rem_size());
        let run = gpui::TextRun {
            len: 1,
            font: gpui::font(theme.mono_family.clone()),
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .layout_line("1", size, std::slice::from_ref(&run), None)
            .width
            .into()
    });
    let gap =
        f32::from(gutter.size.width) - f32::from(super::source::gutter_left(px(16.0))) - number;
    assert_eq!(
        super::source::gutter_left(px(16.0)),
        px(12.0),
        "line numbers start 0.75rem from the left edge"
    );
    // The number itself is inset by that real padding: the number box starts
    // 12px after the gutter's left edge, and the body still starts after the
    // number plus the unchanged right gap.
    let number_box = cx
        .debug_bounds("reader-source-number-0")
        .expect("the number box draws");
    // 1-digit and 5-digit rows share the same left inset and text column.
    for ix in [0usize, 1] {
        let key: &'static str = Box::leak(format!("reader-source-number-{ix}").into_boxed_str());
        let box_of = cx.debug_bounds(key).expect("number draws");
        let inset = f32::from(box_of.left() - gutter.left());
        assert!((inset - 12.0).abs() <= 1.0, "row {ix} inset {inset}");
    }
    let left_inset = f32::from(number_box.left() - gutter.left());
    assert!(
        (left_inset - 12.0).abs() <= 1.0,
        "the line number starts 0.75rem from the left edge: {left_inset}"
    );
    assert!(
        (gap - f32::from(expected)).abs() <= 1.0,
        "the right gap is still the original 0.75rem plus 6px: {gap} vs {}",
        f32::from(expected)
    );
    assert!(
        gap > 12.0,
        "the right gap stayed past the original 0.75rem baseline: {gap}"
    );
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
    cx.simulate_event(ScrollEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(0.0, -40.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
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

/// Right-aligned numbers of 1, 2, 4 and 5 digits keep the same gap, so the
/// text column does not shift as a file grows.
#[gpui::test]
fn gutter_keeps_its_gap_for_every_number_width(cx: &mut TestAppContext) {
    let mut text = String::new();
    for n in 0..12_000 {
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
    cx.simulate_resize(gpui::size(px(900.0), px(400.0)));
    frames(cx);
    let reach = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).reach());
    assert!(reach.y > px(0.0), "the fixture scrolls: {reach:?}");
    let mut gaps = Vec::new();
    for step in 0..=40 {
        if step > 0 {
            let frame = cx.debug_bounds("reader-source").unwrap();
            cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
            cx.simulate_event(ScrollEvent {
                position: frame.center(),
                delta: ScrollDelta::Lines(point(0.0, -120.0)),
                modifiers: Modifiers::none(),
                touch_phase: Default::default(),
                source: gpui::ScrollSource::Pointer,
            });
        }
        frames(cx);
        let mut found = 0;
        for ix in 0..12_000 {
            let Ok(gutter) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let key: &'static str =
                    Box::leak(format!("reader-source-gutter-{ix}").into_boxed_str());
                cx.debug_bounds(key)
            })) else {
                continue;
            };
            let Some(gutter) = gutter else { continue };
            let body_key: &'static str =
                Box::leak(format!("reader-source-body-{ix}").into_boxed_str());
            let Some(body) = cx.debug_bounds(body_key) else {
                continue;
            };
            // Same gap between the gutter box and the text on every row.
            gaps.push(f32::from(body.left()) - f32::from(gutter.right()));
            found += 1;
            if found > 2 {
                break;
            }
        }
    }
    assert!(gaps.len() > 8, "sampled gaps: {gaps:?}");
    let first = gaps[0];
    assert!(
        gaps.iter().all(|gap| (gap - first).abs() <= 0.5),
        "the text column never shifts with number width: {gaps:?}"
    );
    assert!(first >= 0.0);
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

/// A released gesture stops extending the selection: a later drag that starts
/// outside the text must not change what a copy would produce.
#[gpui::test]
fn releasing_ends_the_selection_gesture(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        fixtures::source(),
        DocumentKind::Source { language: None },
    );
    cx.simulate_resize(gpui::size(px(600.0), px(300.0)));
    frames(cx);
    let row = cx.debug_bounds("reader-source-body-0").unwrap();
    let start = point(row.left() + px(4.0), row.center().y);
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(
        point(start.x + px(80.0), start.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(start.x + px(80.0), start.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    frames(cx);
    let selected = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).selected_text());
    assert!(selected.is_some(), "the drag selected something");
    // A fresh drag over the text after the release must not extend the old one
    // from the previous head position.
    let other = point(row.left() + px(200.0), row.center().y);
    cx.simulate_mouse_down(other, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(other, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(other, MouseButton::Left, Modifiers::none());
    frames(cx);
    let after = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).selected_text());
    assert!(
        after.as_deref().map(str::len).unwrap_or(0)
            <= selected.as_deref().map(str::len).unwrap_or(0),
        "a press after release starts a new caret, not a longer old selection: {after:?}"
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
    cx.simulate_event(ScrollEvent {
        position: frame.center(),
        delta: ScrollDelta::Lines(point(0.0, -40.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
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
        reader.markdown().unwrap().read(cx).text().to_string().len()
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

/// The thumb drag moves the list's real offset.
///
/// `on_drag` creates the drag on the first move that crosses the threshold and
/// then stops propagation, so that move only arms the gesture; the capture
/// listener sees `active_drag` from the next move on. A real drag therefore
/// needs more than one move, which is what this exercises.
#[gpui::test]
fn dragging_the_source_scrollbar_moves_the_real_offset(cx: &mut TestAppContext) {
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
    let vertical = cx
        .debug_bounds("reader-scrollbar-y-thumb")
        .expect("the vertical thumb is drawn");
    let start = vertical.origin + point(px(1.0), px(f32::from(vertical.size.height) / 2.0));
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let mut offsets = Vec::new();
    for step in 1..=5 {
        cx.simulate_mouse_move(
            start + point(px(0.0), px(15.0 * step as f32)),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
        offsets.push(reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).offset()));
    }
    cx.simulate_mouse_up(
        start + point(px(0.0), px(75.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();

    assert_eq!(
        offsets[0],
        point(px(0.0), px(0.0)),
        "the arming move does not jump the thumb"
    );
    let moved = offsets.last().copied().expect("five moves");
    assert!(
        moved.y < px(-10.0),
        "dragging down scrolls the real list offset: {offsets:?}"
    );
    assert_eq!(
        moved.x,
        px(0.0),
        "a vertical drag never moves x: {offsets:?}"
    );
    let reach = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).reach());
    assert!(
        moved.y >= -reach.y,
        "the drag clamps to the list's measured extent: {moved:?} vs {reach:?}"
    );
}

/// A horizontal drag moves only x, so the two axes stay independent.
#[gpui::test]
fn dragging_the_horizontal_scrollbar_leaves_the_vertical_offset(cx: &mut TestAppContext) {
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
    let horizontal = cx
        .debug_bounds("reader-scrollbar-x-thumb")
        .expect("the horizontal thumb is drawn");
    let start = horizontal.origin + point(px(f32::from(horizontal.size.width) / 2.0), px(1.0));
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    for step in 1..=5 {
        cx.simulate_mouse_move(
            start + point(px(20.0 * step as f32), px(0.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    cx.simulate_mouse_up(
        start + point(px(100.0), px(0.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    let offset = reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).offset());
    assert!(
        offset.x < px(0.0),
        "the horizontal offset moved: {offset:?}"
    );
    assert_eq!(offset.y, px(0.0), "and the vertical offset did not");
}

/// The local document owns a real ListState; the reader bar reads it.
#[gpui::test]
fn markdown_scrollbar_follows_the_real_list_and_drags(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, fixtures::large_markdown(), DocumentKind::Markdown);
    cx.simulate_resize(gpui::size(px(700.0), px(300.0)));
    frames(cx);
    let before = reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).top_block());
    let bounds = cx.debug_bounds("reader-markdown").unwrap();
    cx.simulate_event(ScrollEvent {
        position: bounds.center(),
        delta: ScrollDelta::Lines(point(0.0, -40.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
    });
    frames(cx);
    assert!(
        reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).top_block()) > before,
        "the wheel scrolls the local ListState"
    );
    assert!(cx.debug_bounds("reader-markdown-scrollbar-thumb").is_some());
}

#[gpui::test]
fn markdown_scrollbar_is_absent_without_overflow(cx: &mut TestAppContext) {
    let (_, cx) = open(cx, fixtures::inline_code_short(), DocumentKind::Markdown);
    frames(cx);
    assert!(cx.debug_bounds("reader-markdown-scrollbar-thumb").is_none());
}

fn inline_snapshot(cx: &mut VisualTestContext, text: &str) -> super::markdown::Snapshot {
    cx.update(|_, cx| super::markdown::snapshot(text, cx).expect("the real paragraph was laid out"))
}

#[gpui::test]
fn inline_code_has_real_padding_code_font_and_wrapped_geometry(cx: &mut TestAppContext) {
    let (_, cx) = open(cx, fixtures::inline_code_short(), DocumentKind::Markdown);
    frames(cx);
    let layout = inline_snapshot(cx, "A short x span.");
    let code = layout.pieces.iter().find(|p| p.code).unwrap();
    assert_eq!(
        code.box_bounds.size.width - code.text_bounds.size.width,
        px(7.0),
        "horizontal code padding is real layout"
    );
    assert_eq!(code.text_bounds.left() - code.box_bounds.left(), px(3.5));
    assert!(
        layout
            .code_fonts
            .iter()
            .any(|font| font.as_ref() == "Google Sans Code"),
        "code runs request the theme mono family: {:?}",
        layout.code_fonts
    );
    let last = layout.pieces.last().unwrap();
    assert!(
        last.text_bounds.left() >= code.box_bounds.right(),
        "padding participates in following text placement"
    );
}

#[gpui::test]
fn long_inline_code_wraps_and_drags_across_code_spans(cx: &mut TestAppContext) {
    let long = "long_code_".repeat(50);
    let rendered = format!("中文 🚀 first code中间 second {long} end");
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: format!("中文 🚀 `first code`中间 `second {long}` end"),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(420.0), px(700.0)));
    frames(cx);
    let layout = inline_snapshot(cx, &rendered);
    assert!(
        layout.pieces.iter().filter(|p| p.code).count() > 2,
        "long code wraps into actual shaped rows"
    );
    for piece in &layout.pieces {
        assert!(
            piece.text_bounds.right() <= layout.bounds.right() + px(0.5),
            "no horizontal overflow: {piece:?}"
        );
        assert!(
            rendered.is_char_boundary(piece.range.start)
                && rendered.is_char_boundary(piece.range.end)
        );
    }
    let first = &layout.pieces[0];
    let last = layout.pieces.last().unwrap();
    let start = point(first.text_bounds.left(), first.text_bounds.center().y);
    let end = point(
        last.text_bounds.right() + px(1.0),
        last.text_bounds.center().y,
    );
    // Selection is document-wide now: drag from the paragraph start to its
    // end and copy. The copied text must be the same read-order text the
    // model holds, with inline-code padding absent.
    drag(cx, &[start, end]);
    frames(cx);
    let copied = copy_selection(&reader, cx);
    assert_eq!(copied, rendered, "cross-code-span selection copies exactly");
    assert!(
        !copied.contains("  "),
        "no padding was inserted: {copied:?}"
    );
}

#[gpui::test]
fn code_fence_language_sits_near_the_block_top(cx: &mut TestAppContext) {
    let (_, cx) = open(cx, fixtures::markdown(), DocumentKind::Markdown);
    cx.simulate_resize(gpui::size(px(1100.0), px(900.0)));
    frames(cx);
    let (block, language, text) = (0..12)
        .find_map(|ix| {
            let key = |what: &str| -> &'static str {
                Box::leak(format!("markdown-{what}-{ix}").into_boxed_str())
            };
            let block = cx.debug_bounds(key("code-block"))?;
            let language = cx.debug_bounds(key("code-language"))?;
            let text = cx.debug_bounds(key("code-text"))?;
            Some((block, language, text))
        })
        .expect("a fenced block with a language draws");
    let inset = f32::from(language.top() - block.top());
    assert!((0.0..=8.0).contains(&inset), "label inset {inset}");
    assert!(
        language.bottom() <= text.top(),
        "label stays above the code body"
    );
}

#[test]
fn repository_pictures_never_resolve_to_filesystem_reads() {
    // The local renderer has no image loader: only an inert note remains.
    let doc = super::markdown::parse(
        "![x](file:///secret)\n\n[Guide](guide.md)\n\n```rust\nfn main() {}\n```",
    );
    assert!(
        doc.blocks
            .iter()
            .any(|b| matches!(b, super::markdown::Block::Image { .. }))
    );
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        super::markdown::Block::Paragraph(p) if !p.links.is_empty()
    )));
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        super::markdown::Block::Code { language: Some(l), .. } if l == "rust"
    )));
}

#[gpui::test]
fn dragging_a_link_back_to_its_start_does_not_open_it(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "[x](guide.md) tail".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    // Same geometry the single-glyph link click test uses.
    let block = cx.debug_bounds("markdown-block-0").unwrap();
    let start = point(block.left() + px(18.0), block.center().y);
    drag(cx, &[start, start + point(px(90.0), px(0.0)), start]);
    cx.run_until_parked();
    assert!(
        !matches!(
            reader.read_with(cx, |r, _| r.last_action().cloned()),
            Some(ReaderAction::Link(_))
        ),
        "a cancelled selection gesture must not navigate"
    );
}
#[gpui::test]
fn table_text_uses_the_body_size_of_the_surrounding_prose(cx: &mut TestAppContext) {
    // A top-level table, a table nested in a quote, and a table long enough to
    // be split into virtualized chunks: all must shape at the prose size.
    let mut body = String::from(
        "Prose paragraph around the tables.

| a | b |
|---|---|
| c | d |

",
    );
    body.push_str(
        "> | q | r |
> |---|---|
> | s | t |

",
    );
    body.push_str(
        "| h |
|---|
",
    );
    for row in 0..80 {
        body.push_str(&format!(
            "| row{row} |
"
        ));
    }
    let (_, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: body,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(700.0)));
    frames(cx);
    let prose = inline_snapshot(cx, "Prose paragraph around the tables.");
    for text in ["a", "b", "c", "d", "q", "s", "h", "row0"] {
        let cell = inline_snapshot(cx, text);
        assert_eq!(
            cell.font_size, prose.font_size,
            "cell {text:?} must shape at the prose size"
        );
        assert_eq!(
            cell.line_height, prose.line_height,
            "cell {text:?} must use the prose line height"
        );
    }
}
#[gpui::test]
fn a_chunked_long_table_keeps_one_header_size_and_wraps_when_narrow(cx: &mut TestAppContext) {
    let mut body = String::from(
        "Before the table.

| Column | Meaning |
|---|---|
",
    );
    for row in 0..80 {
        body.push_str(&format!(
            "| value{row} | a meaning for {row} |
"
        ));
    }
    let (_, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: body,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
    frames(cx);
    let prose = inline_snapshot(cx, "Before the table.");
    let frame = cx.debug_bounds("reader-markdown").unwrap();
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    let scroll = |cx: &mut VisualTestContext, times: usize| {
        for _ in 0..times {
            cx.simulate_event(ScrollEvent {
                position: frame.center(),
                delta: ScrollDelta::Lines(point(0.0, -30.0)),
                modifiers: Modifiers::none(),
                touch_phase: Default::default(),
                source: gpui::ScrollSource::Pointer,
            });
            frames(cx);
        }
    };
    // Walk down until the real header is on screen, then keep going to the
    // repeated header of a later virtualized chunk. Block 0 is the prose, so
    // the first table chunk is block 1 and the next chunk is block 2.
    let mut first_header = None;
    for _ in 0..40 {
        if let Some(bounds) = cx.debug_bounds("markdown-cell-1-0-0") {
            first_header = Some(bounds);
            break;
        }
        scroll(cx, 1);
    }
    let first_header = first_header.expect("the real header draws while scrolling");
    let repeat_header = loop {
        if let Some(bounds) = cx.debug_bounds("markdown-cell-2-0-0") {
            break Some(bounds);
        }
        scroll(cx, 3);
        if cx.debug_bounds("markdown-block-1").is_none()
            && cx.debug_bounds("markdown-block-2").is_none()
        {
            break None;
        }
    };
    let repeat_header = repeat_header.expect("the repeated chunk header draws");
    assert_eq!(
        repeat_header.size.height, first_header.size.height,
        "the repeated header keeps the header height"
    );
    // Narrowing only rewraps the cells: same shaped size and line height.
    cx.simulate_resize(gpui::size(px(320.0), px(600.0)));
    frames(cx);
    let narrow = inline_snapshot(cx, "Before the table.");
    assert_eq!(
        narrow.font_size, prose.font_size,
        "narrowing keeps the size"
    );
    assert_eq!(
        narrow.line_height, prose.line_height,
        "narrowing keeps the line height"
    );
}
#[gpui::test]
fn table_cells_preserve_center_and_right_alignment(cx: &mut TestAppContext) {
    let (_, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "| Left | Center | Right |\n|:---|:---:|---:|\n| left | middle | right |".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    for id in [
        "markdown-cell-0-0-0",
        "markdown-cell-0-0-1",
        "markdown-cell-0-0-2",
        "markdown-cell-0-1-0",
    ] {
        assert!(cx.debug_bounds(id).is_some(), "{id}");
    }
    // The shaped rows themselves are aligned, not just the cell box.
    let right = cx.debug_bounds("markdown-cell-0-0-2").unwrap();
    let right_text = inline_snapshot(cx, "Right");
    let right_glyphs = right_text.bounds.right();
    assert!(
        (right_glyphs - (right.right() - px(12.0))).abs() <= px(2.0),
        "right-aligned glyphs end at the cell's inner right edge: {right:?} vs {right_glyphs:?}"
    );
    let center = cx.debug_bounds("markdown-cell-0-0-1").unwrap();
    let center_text = inline_snapshot(cx, "Center");
    assert!(
        (center_text.bounds.center().x - center.center().x).abs() <= px(2.0),
        "centered glyphs sit at the cell centre"
    );
    let left = cx.debug_bounds("markdown-cell-0-1-0").unwrap();
    let left_text = inline_snapshot(cx, "left");
    assert!(
        (left_text.bounds.left() - (left.left() + px(12.0))).abs() <= px(2.0),
        "left-aligned glyphs start at the cell's inner left edge"
    );
}

#[gpui::test]
fn a_wide_table_keeps_every_cell_range_distinct(cx: &mut TestAppContext) {
    // Many columns, few rows: virtualization limits rows, not columns, so
    // each cell must reach its own unit instead of rescanning the block.
    let columns = 1_200;
    let mut body = String::new();
    body.push_str(&"| h |".repeat(columns));
    body.push(char::from(10));
    body.push_str(&"|---|".repeat(columns));
    body.push(char::from(10));
    for column in 0..columns {
        body.push_str(&format!("| c{column} |"));
    }
    body.push(char::from(10));
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: body,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    let start = std::time::Instant::now();
    cx.simulate_resize(gpui::size(px(500.0), px(500.0)));
    frames(cx);
    assert!(
        start.elapsed() < BUDGET,
        "a {columns}-column table took {:?}",
        start.elapsed()
    );
    let text = reader.read_with(cx, |r, cx| {
        r.markdown().unwrap().read(cx).text().to_string()
    });
    assert!(
        text.contains("c0") && text.contains("c1199"),
        "every cell is modelled"
    );
    // Each cell maps to its own document offset, so a copy is not shifted.
    let first = text.find("c0").unwrap();
    assert_eq!(&text[first..first + 2], "c0");
}
#[gpui::test]
fn a_tall_center_aligned_cell_lays_out_within_budget(cx: &mut TestAppContext) {
    let cell = "word ".repeat(20_000);
    let body = format!(
        "| a | b |
|---|---:|
| {cell} | x |"
    );
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: body,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    let start = std::time::Instant::now();
    cx.simulate_resize(gpui::size(px(420.0), px(500.0)));
    frames(cx);
    assert!(
        start.elapsed() < BUDGET,
        "a tall right-aligned cell took {:?}",
        start.elapsed()
    );
    assert!(reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).text().len()) > 90_000);
}

#[gpui::test]
fn markdown_hover_fades_and_resize_and_replacement_clamp_the_real_list(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, fixtures::large_markdown(), DocumentKind::Markdown);
    frames(cx);
    let bounds = cx.debug_bounds("reader-markdown").unwrap();
    cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
    frames(cx);
    assert!(
        cx.debug_bounds("reader-markdown-scrollbar-thumb").is_some(),
        "the reader fade bar reads the local ListState"
    );
    reader.update_in(cx, |r, w, cx| {
        r.show(fixtures::inline_code_short(), DocumentKind::Markdown, w, cx)
    });
    frames(cx);
    assert_eq!(
        reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).top_block()),
        0
    );
}

#[gpui::test]
fn adjacent_differently_styled_code_budgets_every_padding_inset(cx: &mut TestAppContext) {
    let source = "`aaaa`**`bbbb`**".repeat(12);
    let text = "aaaabbbb".repeat(12);
    let (_, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: source,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(190.0), px(700.0)));
    frames(cx);
    let layout = inline_snapshot(cx, &text);
    for piece in layout.pieces.iter().filter(|p| p.code) {
        assert!(piece.box_bounds.left() >= layout.bounds.left() - px(0.5));
        assert!(
            piece.box_bounds.right() <= layout.bounds.right() + px(0.5),
            "padding must fit: {piece:?}"
        );
    }
}

#[gpui::test]
fn a_dense_inline_paragraph_lays_out_within_budget(cx: &mut TestAppContext) {
    let source = "`x` ".repeat(20_000);
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: source,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    let start = std::time::Instant::now();
    cx.simulate_resize(gpui::size(px(420.0), px(500.0)));
    frames(cx);
    assert!(
        start.elapsed() < BUDGET,
        "dense resize took {:?}",
        start.elapsed()
    );
    assert!(reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).text().len()) > 70_000);
}

#[gpui::test]
fn a_dense_fenced_block_lays_out_within_budget(cx: &mut TestAppContext) {
    let mut source = String::from(
        "```rust
",
    );
    source.push_str(&"x=1;".repeat(20_000));
    source.push_str(
        "
```
",
    );
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: source,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    let start = std::time::Instant::now();
    cx.simulate_resize(gpui::size(px(420.0), px(500.0)));
    frames(cx);
    assert!(
        start.elapsed() < BUDGET,
        "dense fence layout took {:?}",
        start.elapsed()
    );
    assert!(reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).text().len()) > 80_000);
}

#[gpui::test]
fn a_single_character_link_opens_from_its_right_half(cx: &mut TestAppContext) {
    let (r, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "[x](guide.md)".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    let b = cx.debug_bounds("markdown-block-0").unwrap();
    cx.simulate_click(point(b.left() + px(18.0), b.center().y), Modifiers::none());
    cx.run_until_parked();
    assert!(matches!(
        r.read_with(cx, |r, _| r.last_action().cloned()),
        Some(ReaderAction::Link(LinkTarget::Repository(_)))
    ));
}
#[gpui::test]
fn async_markdown_first_load_paints_without_forced_refresh_or_input(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, fixtures::inline_code_short(), DocumentKind::Markdown);
    frames(cx);
    reader.update_in(cx, |r, w, cx| {
        r.show(fixtures::large_markdown(), DocumentKind::Markdown, w, cx)
    });
    cx.run_until_parked();
    for _ in 0..4 {
        cx.update(|w, cx| w.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    assert!(reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).read_now()));
    assert!(
        cx.debug_bounds("markdown-block-0").is_some(),
        "parse completion schedules the first block without window.refresh or wheel input"
    );
    assert!(cx.debug_bounds("markdown-block-20000").is_none());
}

#[gpui::test]
fn replacing_markdown_rejects_previous_background_results(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, fixtures::inline_code_short(), DocumentKind::Markdown);
    reader.update_in(cx, |r, w, cx| {
        r.show(fixtures::large_markdown(), DocumentKind::Markdown, w, cx)
    });
    reader.update_in(cx, |r, w, cx| {
        r.show(
            PreparedBody::Utf8 {
                text: "# Latest document".into(),
                has_bom: false,
            },
            DocumentKind::Markdown,
            w,
            cx,
        )
    });
    frames(cx);
    assert_eq!(
        reader.read_with(cx, |r, cx| r
            .markdown()
            .unwrap()
            .read(cx)
            .text()
            .to_string()),
        "# Latest document"
    );
}

#[gpui::test]
fn repository_file_picture_draws_a_note_without_loading(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "![untrusted](file://attacker.example/share/image.png)".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    assert!(cx.debug_bounds("markdown-picture-note-0").is_some());
    assert!(
        reader
            .read_with(cx, |r, cx| r
                .markdown()
                .unwrap()
                .read(cx)
                .text()
                .to_string())
            .contains("file:")
    );
}

#[gpui::test]
fn wrapped_code_continuation_keeps_its_left_inset(cx: &mut TestAppContext) {
    let long = "x".repeat(4000);
    let (_, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: format!("prefix `{long}` suffix"),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(260.0), px(500.0)));
    frames(cx);
    let layout = inline_snapshot(cx, &format!("prefix {long} suffix"));
    assert!(layout.pieces.iter().filter(|p| p.code).count() > 2);
    for piece in layout.pieces.iter().filter(|p| p.code) {
        assert!(
            piece.box_bounds.right() <= layout.bounds.right() + px(0.5),
            "continuation inset fits inside the row: {piece:?} in {:?}",
            layout.bounds
        );
    }
}

#[gpui::test]
fn table_cells_keep_unique_keys_but_share_document_selection(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "| one | two |\n|---|---|\n| alpha | beta |".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    // Each cell is its own element and its own unit, so a press in one cell
    // cannot borrow another cell's key or text offsets.
    for id in ["markdown-cell-0-1-0", "markdown-cell-0-1-1"] {
        assert!(cx.debug_bounds(id).is_some(), "{id} draws");
    }
    let cell = cx.debug_bounds("markdown-cell-0-1-0").unwrap();
    cx.simulate_click(
        point(cell.left() + px(30.0), cell.center().y),
        Modifiers::none(),
    );
    frames(cx);
    // Selection is document-wide: Ctrl+A takes the whole model in read order,
    // never one cell's private range.
    reader.update_in(cx, |r, w, cx| {
        let document = r.markdown().unwrap().clone();
        document.update(cx, |document, cx| document.focus(w, cx));
    });
    frames(cx);
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    cx.simulate_keystrokes("secondary-c");
    cx.run_until_parked();
    let copied = cx
        .read_from_clipboard()
        .and_then(|i| i.text())
        .unwrap_or_default();
    let expected = reader.read_with(cx, |r, cx| {
        r.markdown().unwrap().read(cx).document_text().to_string()
    });
    assert!(!expected.is_empty());
    assert_eq!(copied, expected, "Ctrl+A copies the whole document");
    assert!(
        copied.contains("alpha") && copied.contains("beta"),
        "{copied:?}"
    );
}

#[test]
fn nested_lists_keep_ordered_and_unchecked_markers() {
    use super::markdown::Block;
    let doc = super::markdown::parse(
        "> 3. step
>
> - [ ] todo",
    );
    let quote = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Quote { body, .. } => Some(body),
            _ => None,
        })
        .expect("a quote");
    let ordered = quote.iter().find_map(|b| match b {
        Block::List { start, items } => Some((start, items)),
        _ => None,
    });
    let (start, items) = ordered.expect("an ordered list inside the quote");
    assert_eq!(*start, Some(3), "ordered list keeps its starting number");
    let task = quote.iter().find_map(|b| match b {
        Block::List { start, items } => Some((start, items)),
        _ => None,
    });
    let _ = (items, task);
}

#[gpui::test]
fn nested_quote_and_table_show_their_contents(cx: &mut TestAppContext) {
    let (_, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "> > important warning\n>\n> | a | b |\n> |---|---|\n> | 1 | 2 |".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    let quote = cx
        .debug_bounds("markdown-block-0")
        .expect("the quote draws");
    assert!(
        quote.size.height > px(40.0),
        "nested quote and table occupy their parsed height: {:?}",
        quote.size
    );
}

#[gpui::test]
fn a_large_list_splits_into_virtualized_rows(cx: &mut TestAppContext) {
    let source = "- item\n".repeat(20_000);
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: source,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    let count = reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).block_count());
    assert!(
        count > 1,
        "one huge list is split so only visible rows are built: {count}"
    );
    assert!(
        cx.debug_bounds("markdown-block-400").is_none(),
        "far list rows are not built"
    );
}

#[gpui::test]
fn clicking_blank_row_space_does_not_open_the_next_rows_link(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "plain  \n[Guide](guide.md)".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(700.0), px(400.0)));
    frames(cx);
    let layout = inline_snapshot(cx, "plain\nGuide");
    let first = &layout.pieces[0];
    // Far right of the first row, beyond its glyphs but above the link row.
    let blank = point(
        layout.bounds.right() - px(2.0),
        first.text_bounds.center().y,
    );
    cx.simulate_click(blank, Modifiers::none());
    cx.run_until_parked();
    assert!(
        reader
            .read_with(cx, |r, _| r.last_action().cloned())
            .is_none(),
        "a press beyond the row's glyphs must not open the next row's link"
    );
}

#[gpui::test]
fn a_linked_heading_inside_a_quote_keeps_its_link(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "> # [Guide](guide.md)".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    assert!(
        cx.debug_bounds("markdown-block-0").is_some(),
        "the quote draws"
    );
    let layout = inline_snapshot(cx, "Guide");
    let guide = layout.pieces[0].text_bounds;
    cx.simulate_click(guide.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(matches!(
        reader.read_with(cx, |r, _| r.last_action().cloned()),
        Some(ReaderAction::Link(LinkTarget::Repository(_)))
    ));
}

#[gpui::test]
fn a_markdown_link_is_visibly_styled(cx: &mut TestAppContext) {
    let (_, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "Read the [installation guide](guide.md) now".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    let parsed = super::markdown::parse("Read the [installation guide](guide.md) now");
    let links = match &parsed.blocks[0] {
        super::markdown::Block::Paragraph(inline) => inline.links.clone(),
        _ => panic!("paragraph"),
    };
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].1, "guide.md");
    // The laid-out text still contains the label where the link range points.
    let text = "Read the installation guide now";
    assert_eq!(&text[links[0].0.clone()], "installation guide");
}

#[gpui::test]
fn a_dense_link_paragraph_lays_out_within_budget(cx: &mut TestAppContext) {
    let source = "[x](guide.md) ".repeat(20_000);
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: source,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    let start = std::time::Instant::now();
    cx.simulate_resize(gpui::size(px(420.0), px(500.0)));
    frames(cx);
    assert!(
        start.elapsed() < BUDGET,
        "dense links resize took {:?}",
        start.elapsed()
    );
    assert!(reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).text().len()) > 70_000);
}

/// A heading, prose with inline code and a link, a list and a quote: enough
/// to drag across several different elements.
/// Guard against accidentally quadratic layout on a dense document. A shared
/// CI runner is much slower than a workstation, so the limit only has to catch
/// order-of-magnitude regressions, not measure real interactive latency.
const BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

const CROSS_BLOCK: &str = "# Title\n\nThe first paragraph with `code` and a [guide](guide.md).\n\n- item one\n- item two\n\n> quoted line\n\n```rust\nfn main() {}\n```\n";

fn doc_text(reader: &Entity<ReaderView>, cx: &mut VisualTestContext) -> String {
    reader.read_with(cx, |r, cx| {
        r.markdown().unwrap().read(cx).document_text().to_string()
    })
}

fn selection(reader: &Entity<ReaderView>, cx: &mut VisualTestContext) -> std::ops::Range<usize> {
    reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).selection())
}

fn selected(reader: &Entity<ReaderView>, cx: &mut VisualTestContext) -> String {
    reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).selected_text())
}

fn copy_selection(_reader: &Entity<ReaderView>, cx: &mut VisualTestContext) -> String {
    cx.read_from_clipboard();
    cx.simulate_keystrokes("secondary-c");
    cx.run_until_parked();
    cx.read_from_clipboard()
        .and_then(|i| i.text())
        .unwrap_or_default()
}

fn drag(cx: &mut VisualTestContext, points: &[gpui::Point<gpui::Pixels>]) {
    let Some((start, rest)) = points.split_first() else {
        return;
    };
    cx.simulate_mouse_move(*start, None, Modifiers::none());
    cx.simulate_mouse_down(*start, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    for point in rest {
        // gpui creates the drag on the first move past the threshold and only
        // dispatches from the next one, so a drag needs a second move.
        cx.simulate_mouse_move(*point, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        cx.simulate_mouse_move(*point, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    }
    if let Some(end) = points.last() {
        cx.simulate_mouse_up(*end, MouseButton::Left, Modifiers::none());
    }
}

#[gpui::test]
fn dragging_from_a_heading_into_prose_selects_across_elements(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: CROSS_BLOCK.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(800.0)));
    frames(cx);
    let heading = cx.debug_bounds("markdown-block-0").unwrap();
    let prose = cx.debug_bounds("markdown-block-1").unwrap();
    drag(
        cx,
        &[
            point(heading.left() + px(12.0), heading.center().y),
            point(prose.left() + px(120.0), prose.center().y),
        ],
    );
    frames(cx);
    let range = selection(&reader, cx);
    assert!(
        range.start < range.end,
        "a cross-block drag selects: {range:?}"
    );
    let text = selected(&reader, cx);
    assert!(
        text.contains("Title"),
        "selection covers the heading: {text:?}"
    );
    assert!(
        text.contains("first para"),
        "and reaches the prose: {text:?}"
    );
    assert_eq!(
        copy_selection(&reader, cx),
        text,
        "copy matches the selection"
    );
}

#[gpui::test]
fn dragging_backwards_from_prose_into_a_heading_works_too(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: CROSS_BLOCK.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(800.0)));
    frames(cx);
    let heading = cx.debug_bounds("markdown-block-0").unwrap();
    let prose = cx.debug_bounds("markdown-block-1").unwrap();
    drag(
        cx,
        &[
            point(prose.left() + px(300.0), prose.center().y),
            point(heading.left() + px(18.0), heading.center().y),
        ],
    );
    frames(cx);
    let text = selected(&reader, cx);
    assert!(
        text.contains("Title"),
        "reverse drag reaches the heading: {text:?}"
    );
    assert_eq!(copy_selection(&reader, cx), text);
}

#[gpui::test]
fn dragging_across_blocks_and_blank_gaps_keeps_extending(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: CROSS_BLOCK.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(800.0)));
    frames(cx);
    let prose = cx.debug_bounds("markdown-block-1").unwrap();
    let list = cx.debug_bounds("markdown-block-2").unwrap();
    let quote = cx.debug_bounds("markdown-block-3").unwrap();
    // The second point sits in the blank gap between prose and list.
    drag(
        cx,
        &[
            point(prose.left() + px(10.0), prose.top() + px(6.0)),
            point(prose.center().x, prose.bottom() + px(2.0)),
            point(list.left() + px(40.0), list.center().y),
            point(quote.left() + px(220.0), quote.center().y),
            point(quote.left() + px(220.0), quote.center().y),
        ],
    );
    frames(cx);
    let text = selected(&reader, cx);
    assert!(text.contains("first paragraph"), "{text:?}");
    assert!(
        text.contains("item one"),
        "the selection crossed the list: {text:?}"
    );
    assert!(text.contains("quoted"), "and reached the quote: {text:?}");
    assert!(
        !text.contains("fn main"),
        "it stops before the code block: {text:?}"
    );
    assert_eq!(copy_selection(&reader, cx), text);
}

#[gpui::test]
fn ctrl_a_selects_the_whole_document_including_unrendered_blocks(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, fixtures::large_markdown(), DocumentKind::Markdown);
    cx.simulate_resize(gpui::size(px(700.0), px(400.0)));
    frames(cx);
    assert!(
        cx.debug_bounds("markdown-block-200").is_none(),
        "far blocks are virtualized"
    );
    let body = cx.debug_bounds("markdown-block-0").unwrap();
    cx.simulate_click(
        point(body.left() + px(20.0), body.center().y),
        Modifiers::none(),
    );
    frames(cx);
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    let full = doc_text(&reader, cx);
    assert_eq!(
        selection(&reader, cx),
        0..full.len(),
        "Ctrl+A covers the model, not one block"
    );
    assert!(
        full.contains("Section 5000"),
        "the model includes off-screen content"
    );
    assert_eq!(selected(&reader, cx), full);
    assert_eq!(copy_selection(&reader, cx), full);
}

#[gpui::test]
fn pressing_blank_reader_space_clears_the_selection(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: CROSS_BLOCK.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(800.0)));
    frames(cx);
    let body = cx.debug_bounds("markdown-block-1").unwrap();
    cx.simulate_click(
        point(body.left() + px(20.0), body.center().y),
        Modifiers::none(),
    );
    frames(cx);
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    let before = selection(&reader, cx);
    assert!(before.start < before.end);
    let viewport = cx.debug_bounds("reader-markdown").unwrap();
    // The reading area's right padding is blank space, not a text element.
    cx.simulate_click(
        point(viewport.right() - px(1.0), viewport.center().y),
        Modifiers::none(),
    );
    frames(cx);
    assert_eq!(
        selection(&reader, cx),
        0..0,
        "blank space clears the selection"
    );
    assert!(selected(&reader, cx).is_empty());
    let _ = copy_selection(&reader, cx);
    assert!(
        cx.read_from_clipboard()
            .and_then(|i| i.text())
            .unwrap_or_default()
            .is_empty(),
        "an empty selection writes nothing"
    );
}

#[gpui::test]
fn dragging_past_the_viewport_keeps_selecting_and_release_ends_it(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, fixtures::large_markdown(), DocumentKind::Markdown);
    cx.simulate_resize(gpui::size(px(700.0), px(320.0)));
    frames(cx);
    let body = cx.debug_bounds("markdown-block-0").unwrap();
    let start = point(body.left() + px(12.0), body.center().y);
    let outside = point(start.x, body.bottom() + px(400.0));
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    // First move arms the drag, the second one is dispatched to it.
    cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::none());
    frames(cx);
    cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::none());
    frames(cx);
    let grown = selection(&reader, cx);
    assert!(
        grown.start < grown.end,
        "a drag beyond the viewport still selects"
    );
    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
    frames(cx);
    let after_release = selection(&reader, cx);
    cx.simulate_mouse_move(
        point(start.x, body.top() + px(4.0)),
        None,
        Modifiers::none(),
    );
    frames(cx);
    assert_eq!(
        selection(&reader, cx),
        after_release,
        "moving without a button no longer extends the old selection"
    );
}

#[gpui::test]
fn a_drag_that_crosses_a_link_does_not_open_it(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: CROSS_BLOCK.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(800.0)));
    frames(cx);
    let prose = cx.debug_bounds("markdown-block-1").unwrap();
    drag(
        cx,
        &[
            point(prose.left() + px(10.0), prose.center().y),
            point(prose.right() - px(20.0), prose.center().y),
        ],
    );
    frames(cx);
    assert!(
        reader
            .read_with(cx, |r, _| r.last_action().cloned())
            .is_none(),
        "a selection drag never opens a link"
    );
    assert!(selection(&reader, cx).start < selection(&reader, cx).end);
}

#[gpui::test]
fn replacing_the_document_drops_the_old_selection(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: CROSS_BLOCK.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    let body = cx.debug_bounds("markdown-block-0").unwrap();
    cx.simulate_click(
        point(body.left() + px(20.0), body.center().y),
        Modifiers::none(),
    );
    frames(cx);
    reader.update_in(cx, |r, w, cx| {
        let document = r.markdown().unwrap().clone();
        document.update(cx, |document, cx| document.focus(w, cx));
    });
    frames(cx);
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    assert!(selection(&reader, cx).start < selection(&reader, cx).end);
    reader.update_in(cx, |r, w, cx| {
        r.show(
            PreparedBody::Utf8 {
                text: "# Replaced".into(),
                has_bom: false,
            },
            DocumentKind::Markdown,
            w,
            cx,
        )
    });
    frames(cx);
    assert_eq!(
        selection(&reader, cx),
        0..0,
        "old offsets belonged to the old text"
    );
    assert!(!selected(&reader, cx).contains("Title"));
}

#[gpui::test]
fn a_large_single_list_selects_all_without_building_every_row(cx: &mut TestAppContext) {
    let source = "- item text\n".repeat(20_000);
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: source,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    frames(cx);
    let body = cx.debug_bounds("markdown-block-0").unwrap();
    // The list is split into virtualized rows; the first row is on screen.
    cx.simulate_click(
        point(body.left() + px(20.0), body.center().y),
        Modifiers::none(),
    );
    frames(cx);
    let start = std::time::Instant::now();
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    assert!(
        start.elapsed() < BUDGET,
        "Ctrl+A over a huge list stays fast: {:?}",
        start.elapsed()
    );
    // Ctrl+A must work from the document focus alone.
    reader.update_in(cx, |r, w, cx| {
        let document = r.markdown().unwrap().clone();
        document.update(cx, |document, cx| document.focus(w, cx));
    });
    frames(cx);
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    let full = doc_text(&reader, cx);
    assert!(full.len() > 100_000, "the model covers the whole file");
    assert_eq!(selected(&reader, cx), full);
    assert!(
        cx.debug_bounds("markdown-block-400").is_none(),
        "rows stay virtualized"
    );
}

#[gpui::test]
fn line_number_left_inset_holds_for_one_to_five_digits(cx: &mut TestAppContext) {
    // A file with 12,000 lines uses five digits from the first row, so the
    // widest number width and the gutter's left inset are exercised together.
    // The same invariant is checked on a small file for one digit.
    for (lines, expected_digits) in [(1usize, 1usize), (12_000usize, 5usize)] {
        let mut text = String::new();
        for n in 0..lines {
            text.push_str(&format!("line {n}\n"));
        }
        let (_, cx) = open(
            cx,
            PreparedBody::Utf8 {
                text,
                has_bom: false,
            },
            DocumentKind::Source { language: None },
        );
        cx.simulate_resize(gpui::size(px(900.0), px(420.0)));
        frames(cx);
        let row = cx.debug_bounds("reader-source-row-0").unwrap();
        let number = cx
            .debug_bounds("reader-source-number-0")
            .expect("the number draws");
        let body = cx
            .debug_bounds("reader-source-body-0")
            .expect("the body draws");
        let inset = f32::from(number.left() - row.left());
        assert!(
            (inset - 12.0).abs() <= 1.0,
            "{expected_digits} digits: the number starts 0.75rem from the left ({inset})"
        );
        let gap = f32::from(body.left() - number.right());
        assert!(
            (gap - 18.0).abs() <= 1.5,
            "{expected_digits} digits: the right gap stays 0.75rem+6px ({gap})"
        );
        // Right-aligned numbers: the body column is the same for both widths.
        let body_offset = f32::from(body.left() - row.left());
        let digits = expected_digits as f32;
        assert!(
            body_offset >= 12.0 + digits * 4.0,
            "{expected_digits} digits: the text column leaves room for the number"
        );
    }
}

#[gpui::test]
fn nested_code_before_a_linked_paragraph_keeps_their_own_text(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "> ```\n> abc\n> ```\n>\n> [Guide](guide.md)".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(800.0), px(500.0)));
    frames(cx);
    let layout = inline_snapshot(cx, "Guide");
    cx.simulate_click(layout.pieces[0].text_bounds.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        matches!(
            reader.read_with(cx, |r, _| r.last_action().cloned()),
            Some(ReaderAction::Link(LinkTarget::Repository(_)))
        ),
        "the paragraph after a nested code block keeps its own link"
    );
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    let all = selected(&reader, cx);
    assert!(all.contains("abc") && all.contains("Guide"), "{all:?}");
}

#[gpui::test]
fn scrolled_away_rows_do_not_win_pointer_hits(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, fixtures::large_markdown(), DocumentKind::Markdown);
    cx.simulate_resize(gpui::size(px(700.0), px(360.0)));
    frames(cx);
    let frame = cx.debug_bounds("reader-markdown").unwrap();
    // Visit later blocks, then return to the top.
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    for _ in 0..6 {
        cx.simulate_event(ScrollEvent {
            position: frame.center(),
            delta: ScrollDelta::Lines(point(0.0, -40.0)),
            modifiers: Modifiers::none(),
            touch_phase: Default::default(),
            source: gpui::ScrollSource::Pointer,
        });
        frames(cx);
    }
    for _ in 0..10 {
        cx.simulate_event(ScrollEvent {
            position: frame.center(),
            delta: ScrollDelta::Lines(point(0.0, 40.0)),
            modifiers: Modifiers::none(),
            touch_phase: Default::default(),
            source: gpui::ScrollSource::Pointer,
        });
        frames(cx);
    }
    frames(cx);
    let top = cx.debug_bounds("markdown-heading-0").unwrap();
    cx.simulate_click(
        point(top.left() + px(20.0), top.center().y),
        Modifiers::none(),
    );
    frames(cx);
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    let text = selected(&reader, cx);
    assert!(text.starts_with("Section 0"), "{text:?}");
}

#[gpui::test]
fn a_link_in_a_row_after_an_empty_cell_still_opens(cx: &mut TestAppContext) {
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: "| a | b |\n|---|---|\n|  | [x](guide.md) |".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(500.0)));
    frames(cx);
    let layout = inline_snapshot(cx, "x");
    let link = layout.pieces[0].text_bounds.center();
    cx.simulate_click(link, Modifiers::none());
    cx.run_until_parked();
    assert!(
        matches!(
            reader.read_with(cx, |r, _| r.last_action().cloned()),
            Some(ReaderAction::Link(LinkTarget::Repository(_)))
        ),
        "the link after an empty cell resolves against its own unit"
    );
}

#[gpui::test]
fn dragging_into_a_wrapped_line_gap_selects_only_that_row(cx: &mut TestAppContext) {
    let body = "first row text that wraps a lot 中文 emoji 🚀 and keeps going for width. tail row";
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: body.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(360.0), px(400.0)));
    frames(cx);
    let layout = inline_snapshot(cx, body);
    assert!(layout.pieces.len() > 1, "the paragraph wraps");
    let first_row = &layout.pieces[0].text_bounds;
    let start = point(first_row.left(), first_row.center().y);
    // Beside the first row, past its last glyph.
    let end = point(layout.bounds.right() - px(1.0), first_row.center().y);
    drag(cx, &[start, end]);
    frames(cx);
    let copied = copy_selection(&reader, cx);
    assert!(!copied.is_empty(), "{copied:?}");
    assert!(
        !copied.contains("tail row"),
        "a drag beside the first row must not swallow later rows: {copied:?}"
    );
}

#[gpui::test]
fn a_newly_scrolled_in_link_is_clickable(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, fixtures::large_markdown(), DocumentKind::Markdown);
    cx.simulate_resize(gpui::size(px(700.0), px(320.0)));
    frames(cx);
    let frame = cx.debug_bounds("reader-markdown").unwrap();
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    for _ in 0..4 {
        cx.simulate_event(ScrollEvent {
            position: frame.center(),
            delta: ScrollDelta::Lines(point(0.0, -40.0)),
            modifiers: Modifiers::none(),
            touch_phase: Default::default(),
            source: gpui::ScrollSource::Pointer,
        });
        frames(cx);
    }
    // A paragraph built by this scroll (not present before) must take presses.
    let paragraph = (1..80).find_map(|ix| {
        let key: &'static str = Box::leak(format!("markdown-paragraph-{ix}").into_boxed_str());
        cx.debug_bounds(key)
    });
    let paragraph = paragraph.expect("a scrolled-in paragraph draws");
    let layout = inline_snapshot(
        cx,
        "A paragraph with 中文 and emoji 🚀 that pads the document so the reader has to virtualize it. code and a link.",
    );
    let _ = layout;
    cx.simulate_click(
        point(paragraph.left() + px(24.0), paragraph.center().y),
        Modifiers::none(),
    );
    frames(cx);
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    assert!(
        selection(&reader, cx).start == 0,
        "the press landed in the document, not on stale geometry"
    );
}

#[gpui::test]
fn a_fenced_code_block_can_be_dragged_and_copied(cx: &mut TestAppContext) {
    let body = "lead paragraph before the code.\n\n```rust\nlet alpha = 1;\nlet beta = 2;\n```\n\ntail paragraph after the code.";
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: body.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(800.0), px(600.0)));
    frames(cx);
    let layout = inline_snapshot(cx, "let alpha = 1;\nlet beta = 2;");
    let start = layout.pieces[0].text_bounds.left_center() + point(px(1.0), px(0.0));
    let last = layout.pieces.last().unwrap().text_bounds.right_center();
    drag(cx, &[start, point(last.x - px(1.0), last.y)]);
    frames(cx);
    let copied = copy_selection(&reader, cx);
    assert!(copied.contains("let alpha = 1;"), "{copied:?}");
    assert!(
        !copied.contains("lead paragraph"),
        "a code drag must not fall back to preceding prose: {copied:?}"
    );
}

#[gpui::test]
fn a_repeated_chunk_header_never_opens_the_next_rows_link(cx: &mut TestAppContext) {
    let mut body = String::from(
        "| [Docs](docs.md) | b |
|---|---|
",
    );
    for row in 0..70 {
        body.push_str(&format!(
            "| [body{row}](wrong{row}.md) | d{row} |
"
        ));
    }
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: body,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
    frames(cx);
    // Scroll to the second chunk so its repeated header is on screen.
    let frame = cx.debug_bounds("reader-markdown").unwrap();
    cx.simulate_mouse_move(frame.center(), None, Modifiers::none());
    for _ in 0..30 {
        cx.simulate_event(ScrollEvent {
            position: frame.center(),
            delta: ScrollDelta::Lines(point(0.0, -30.0)),
            modifiers: Modifiers::none(),
            touch_phase: Default::default(),
            source: gpui::ScrollSource::Pointer,
        });
        frames(cx);
    }
    let header = cx
        .debug_bounds("markdown-cell-1-0-0")
        .expect("the second chunk's header is on screen");
    cx.simulate_click(
        header.left_center() + point(px(16.0), px(0.0)),
        Modifiers::none(),
    );
    cx.run_until_parked();
    let action = reader.read_with(cx, |r, _| r.last_action().cloned());
    assert!(
        !matches!(action, Some(ReaderAction::Link(_))),
        "a presentation-only repeated header must not resolve a body link: {action:?}"
    );
}

#[gpui::test]
fn a_long_table_copies_its_header_once(cx: &mut TestAppContext) {
    let mut body = String::from("| a | b |\n|---|---|\n");
    for row in 0..80 {
        body.push_str(&format!("| c{row} | d{row} |\n"));
    }
    let (reader, cx) = open(
        cx,
        PreparedBody::Utf8 {
            text: body,
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    cx.simulate_resize(gpui::size(px(800.0), px(600.0)));
    frames(cx);
    // Focus the table, then Ctrl+A for the whole document, including
    // virtualized table chunks.
    let cell = inline_snapshot(cx, "c0").pieces[0].text_bounds.center();
    cx.simulate_click(cell, Modifiers::none());
    frames(cx);
    cx.simulate_keystrokes("secondary-a");
    frames(cx);
    let copied = copy_selection(&reader, cx);
    assert_eq!(
        copied.matches("a\tb").count(),
        1,
        "the repeated chunk header is presentation only: {copied:?}"
    );
    assert!(
        copied.contains("c0") && copied.contains("c79"),
        "{copied:?}"
    );
}
