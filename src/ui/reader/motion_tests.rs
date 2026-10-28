//! Controlled-time events against the real reader targets, including cancellation boundaries.
use super::{
    DocumentKind, ReaderView,
    tests::{context, open},
};
use crate::content::PreparedBody;
use gpui::{
    AppContext, Context, Entity, IntoElement, Modifiers, ParentElement, Render, ScrollDelta,
    ScrollEvent, ScrollSource, Styled, TestAppContext, VisualTestContext, div, point, px,
};

fn body() -> PreparedBody {
    PreparedBody::Utf8 {
        text: (0..80)
            .map(|i| format!("line {i} {}\n\n", "wide ".repeat(45)))
            .collect(),
        has_bom: false,
    }
}

fn pin(reader: &Entity<ReaderView>, ms: u64, cx: &mut VisualTestContext) {
    if ms == 0 {
        cx.update(|_, cx| ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = false));
    }
    reader.update(cx, |reader, cx| {
        reader.pin_scrollbar_clock(ms);
        if let Some(source) = reader.source() {
            source.update(cx, |source, cx| {
                source.pin_scrollbar_clock(ms);
                cx.notify();
            });
        }
        cx.notify();
    });
    cx.run_until_parked();
}

fn offset(reader: &Entity<ReaderView>, cx: &VisualTestContext) -> gpui::Point<gpui::Pixels> {
    reader.read_with(cx, |r, cx| match r.source() {
        Some(source) => source.read(cx).offset(),
        None => r
            .markdown()
            .unwrap()
            .read(cx)
            .list_state()
            .scroll_px_offset_for_scrollbar(),
    })
}

fn moving(reader: &Entity<ReaderView>, cx: &VisualTestContext) -> bool {
    reader.read_with(cx, |r, cx| match r.source() {
        Some(source) => source.read(cx).motion.is_animating(),
        None => r.markdown().unwrap().read(cx).motion().is_animating(),
    })
}

fn wheel(
    cx: &mut VisualTestContext,
    selector: &'static str,
    delta: ScrollDelta,
    shift: bool,
    source: ScrollSource,
) {
    let position = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_move(position, None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position,
        delta,
        modifiers: Modifiers {
            shift,
            ..Modifiers::none()
        },
        source,
        ..Default::default()
    });
}

#[gpui::test]
fn source_and_markdown_discrete_events_progress_and_stop_requesting_frames(
    cx: &mut TestAppContext,
) {
    for kind in [
        DocumentKind::Source { language: None },
        DocumentKind::Markdown,
    ] {
        let (reader, cx) = open(cx, body(), kind);
        pin(&reader, 0, cx);
        let selector = if reader.read_with(cx, |r, _| r.source().is_some()) {
            "reader-source"
        } else {
            "reader-markdown"
        };
        let line_height = cx.update(|w, _| f32::from(w.line_height()));
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -3.0)),
            false,
            ScrollSource::Pointer,
        );
        assert_eq!(
            offset(&reader, cx).y,
            px(0.0),
            "the native list must not jump too"
        );
        assert!(moving(&reader, cx));
        pin(&reader, 40, cx);
        let middle = offset(&reader, cx).y;
        assert!(
            middle < px(0.0) && middle > px(-3.0 * line_height),
            "{middle:?}"
        );
        pin(&reader, 150, cx);
        assert!((f32::from(offset(&reader, cx).y) + 3.0 * line_height).abs() < 0.6);
        assert!(!moving(&reader, cx));
        pin(&reader, 1000, cx);
        // Drain the pending animation notification; a settled reader must schedule no successor.
        for _ in 0..3 {
            cx.update(|w, cx| w.simulate_next_frame(cx));
            cx.run_until_parked();
        }
        assert_eq!(cx.update(|w, cx| w.simulate_next_frame(cx)), 0);
    }
}

#[gpui::test]
fn source_axes_shift_native_x_and_precise_touch_are_independent(cx: &mut TestAppContext) {
    let (reader, cx) = open(cx, body(), DocumentKind::Source { language: None });
    cx.simulate_resize(gpui::size(px(420.0), px(260.0)));
    pin(&reader, 0, cx);
    assert!(reader.read_with(cx, |r, cx| r.source().unwrap().read(cx).reach().x) > px(100.0));
    wheel(
        cx,
        "reader-source",
        ScrollDelta::Lines(point(0.0, -3.0)),
        false,
        ScrollSource::Pointer,
    );
    wheel(
        cx,
        "reader-source",
        ScrollDelta::Lines(point(-4.0, 0.0)),
        true,
        ScrollSource::Pointer,
    );
    assert_eq!(offset(&reader, cx), point(px(0.0), px(0.0)));
    pin(&reader, 50, cx);
    let middle = offset(&reader, cx);
    assert!(middle.x < px(0.0) && middle.y < px(0.0));
    pin(&reader, 150, cx);
    let end = offset(&reader, cx);
    wheel(
        cx,
        "reader-source",
        ScrollDelta::Pixels(point(px(-20.0), px(-10.0))),
        false,
        ScrollSource::Pointer,
    );
    assert_eq!(offset(&reader, cx), end + point(px(-20.0), px(-10.0)));
    let end = offset(&reader, cx);
    wheel(
        cx,
        "reader-source",
        ScrollDelta::Pixels(point(px(0.0), px(-30.0))),
        false,
        ScrollSource::Touch,
    );
    assert_eq!(offset(&reader, cx), end + point(px(0.0), px(-30.0)));
    let end = offset(&reader, cx);
    wheel(
        cx,
        "reader-source",
        ScrollDelta::Pixels(point(px(0.0), px(-25.0))),
        true,
        ScrollSource::Pointer,
    );
    assert_eq!(offset(&reader, cx), end + point(px(-25.0), px(0.0)));
    assert!(!moving(&reader, cx));
}

#[gpui::test]
fn markdown_does_not_convert_horizontal_input_and_reduced_motion_is_direct(
    cx: &mut TestAppContext,
) {
    let (reader, cx) = open(cx, body(), DocumentKind::Markdown);
    pin(&reader, 0, cx);
    wheel(
        cx,
        "reader-markdown",
        ScrollDelta::Pixels(point(px(-40.0), px(0.0))),
        false,
        ScrollSource::Pointer,
    );
    wheel(
        cx,
        "reader-markdown",
        ScrollDelta::Lines(point(0.0, -4.0)),
        true,
        ScrollSource::Pointer,
    );
    assert_eq!(offset(&reader, cx).y, px(0.0));
    cx.update(|_, cx| ely_core::theme::Theme::update(cx, |theme| theme.reduced_motion = true));
    wheel(
        cx,
        "reader-markdown",
        ScrollDelta::Lines(point(0.0, -4.0)),
        false,
        ScrollSource::Pointer,
    );
    assert!(offset(&reader, cx).y < px(0.0));
    assert!(!moving(&reader, cx));
}

#[gpui::test]
fn wheel_is_cancelled_by_reveal_replacement_escape_and_deactivation(cx: &mut TestAppContext) {
    for kind in [
        DocumentKind::Source { language: None },
        DocumentKind::Markdown,
    ] {
        let (reader, cx) = open(cx, body(), kind.clone());
        let selector = if reader.read_with(cx, |r, _| r.source().is_some()) {
            "reader-source"
        } else {
            "reader-markdown"
        };
        pin(&reader, 0, cx);
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -8.0)),
            false,
            ScrollSource::Pointer,
        );
        pin(&reader, 30, cx);
        reader.update(cx, |r, cx| {
            match r.source() {
                Some(s) => s
                    .read(cx)
                    .motion
                    .set_axis_offset(gpui::Axis::Vertical, px(-20.0)),
                None => r
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .motion()
                    .set_axis_offset(gpui::Axis::Vertical, px(-20.0)),
            }
            cx.notify();
        });
        pin(&reader, 200, cx);
        assert!((f32::from(offset(&reader, cx).y) + 20.0).abs() < 0.6);
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -8.0)),
            false,
            ScrollSource::Pointer,
        );
        assert!(moving(&reader, cx));
        cx.simulate_keystrokes("escape");
        assert!(!moving(&reader, cx));
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -8.0)),
            false,
            ScrollSource::Pointer,
        );
        assert!(moving(&reader, cx));
        cx.deactivate_window();
        assert!(!moving(&reader, cx));
        cx.update(|w, _| w.activate_window());
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -8.0)),
            false,
            ScrollSource::Pointer,
        );
        cx.update(|window, cx| {
            reader.update(cx, |r, cx| {
                r.show(
                    PreparedBody::Utf8 {
                        text: "short".into(),
                        has_bom: false,
                    },
                    kind,
                    window,
                    cx,
                )
            })
        });
        pin(&reader, 1000, cx);
        assert_eq!(offset(&reader, cx).y, px(0.0));
        assert!(!moving(&reader, cx));
    }
}

#[gpui::test]
fn selection_press_and_context_invalidation_cancel_pending_motion(cx: &mut TestAppContext) {
    for kind in [
        DocumentKind::Source { language: None },
        DocumentKind::Markdown,
    ] {
        let (reader, cx) = open(cx, body(), kind);
        pin(&reader, 0, cx);
        let source = reader.read_with(cx, |r, _| r.source().is_some());
        let selector = if source {
            "reader-source"
        } else {
            "reader-markdown"
        };
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -5.0)),
            false,
            ScrollSource::Pointer,
        );
        let hit = cx
            .debug_bounds(if source {
                "reader-source-body-0"
            } else {
                "markdown-paragraph-0"
            })
            .unwrap();
        cx.simulate_mouse_down(
            hit.origin + point(px(5.0), px(5.0)),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
        assert!(!moving(&reader, cx));
        let end = offset(&reader, cx);
        pin(&reader, 200, cx);
        assert_eq!(offset(&reader, cx), end);
        cx.simulate_mouse_up(hit.center(), gpui::MouseButton::Left, Modifiers::none());
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -5.0)),
            false,
            ScrollSource::Pointer,
        );
        assert!(moving(&reader, cx));
        reader.update(cx, |r, cx| r.cancel_interactions(cx));
        assert!(!moving(&reader, cx));
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -5.0)),
            false,
            ScrollSource::Pointer,
        );
        cx.simulate_visibility_change(gpui::WindowVisibility::Hidden);
        cx.run_until_parked();
        assert!(!moving(&reader, cx));
    }
}

#[gpui::test]
fn wheel_then_real_thumb_drag_outside_release_has_no_old_goal(cx: &mut TestAppContext) {
    for kind in [
        DocumentKind::Source { language: None },
        DocumentKind::Markdown,
    ] {
        let (reader, cx) = open(cx, body(), kind);
        cx.simulate_resize(gpui::size(px(600.0), px(260.0)));
        pin(&reader, 0, cx);
        let source = reader.read_with(cx, |r, _| r.source().is_some());
        let selector = if source {
            "reader-source"
        } else {
            "reader-markdown"
        };
        wheel(
            cx,
            selector,
            ScrollDelta::Lines(point(0.0, -12.0)),
            false,
            ScrollSource::Pointer,
        );
        pin(&reader, 30, cx);
        reader.update(cx, |r, cx| {
            r.show_scrollbar();
            if let Some(s) = r.source() {
                s.update(cx, |s, cx| {
                    s.show_scrollbar();
                    cx.notify();
                });
            }
            cx.notify();
        });
        cx.run_until_parked();
        let thumb = cx
            .debug_bounds(if source {
                "reader-scrollbar-y-thumb"
            } else {
                "reader-markdown-scrollbar-thumb"
            })
            .unwrap();
        let start = thumb.center();
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::none());
        for step in 1..=5 {
            cx.simulate_mouse_move(
                start + point(px(0.0), px(10.0 * step as f32)),
                gpui::MouseButton::Left,
                Modifiers::none(),
            );
        }
        assert!(!moving(&reader, cx));
        cx.simulate_mouse_up(
            point(px(700.0), px(400.0)),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
        let end = offset(&reader, cx);
        assert!(end.y < px(0.0));
        pin(&reader, 500, cx);
        assert!((f32::from(offset(&reader, cx).y - end.y)).abs() < 0.6);
        reader.read_with(cx, |r, cx| {
            if let Some(s) = r.source() {
                assert!(s.read(cx).bars.dragging().is_none());
            } else {
                assert!(r.bars.dragging().is_none());
                assert!(
                    !r.markdown()
                        .unwrap()
                        .read(cx)
                        .list_state()
                        .is_scrollbar_dragging()
                );
            }
        });
    }
}

#[gpui::test]
fn two_readers_and_retained_hidden_reader_have_separate_motion(cx: &mut TestAppContext) {
    super::tests::setup(cx);
    struct Host {
        a: Entity<ReaderView>,
        b: Entity<ReaderView>,
        show_a: bool,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .w(px(300.0))
                        .h_full()
                        .children(self.show_a.then(|| self.a.clone())),
                )
                .child(div().w(px(300.0)).h_full().child(self.b.clone()))
        }
    }
    let (host, cx) = cx.add_window_view(|w, cx| Host {
        a: cx.new(|cx| {
            ReaderView::new(
                body(),
                DocumentKind::Source { language: None },
                context(),
                w,
                cx,
            )
        }),
        b: cx.new(|cx| ReaderView::new(body(), DocumentKind::Markdown, context(), w, cx)),
        show_a: true,
    });
    cx.update(|w, _| w.activate_window());
    cx.run_until_parked();
    let (a, b) = host.read_with(cx, |h, _| (h.a.clone(), h.b.clone()));
    pin(&a, 0, cx);
    pin(&b, 0, cx);
    wheel(
        cx,
        "reader-source",
        ScrollDelta::Lines(point(0.0, -5.0)),
        false,
        ScrollSource::Pointer,
    );
    pin(&a, 40, cx);
    pin(&b, 40, cx);
    assert!(offset(&a, cx).y < px(0.0));
    assert_eq!(offset(&b, cx).y, px(0.0));
    host.update(cx, |h, cx| {
        h.show_a = false;
        cx.notify();
    });
    cx.run_until_parked();
    // Keyed state is retired after its last rendered frame.
    for _ in 0..3 {
        cx.update(|w, _| w.refresh());
        cx.run_until_parked();
    }
    assert!(!moving(&a, cx));
    wheel(
        cx,
        "reader-markdown",
        ScrollDelta::Lines(point(0.0, -3.0)),
        false,
        ScrollSource::Pointer,
    );
    pin(&b, 200, cx);
    assert!(offset(&b, cx).y < px(0.0));
}
