//! Controlled clocks exercise actual offsets on laid-out GPUI targets.
use super::*;
use gpui::{
    Axis, Context, IntoElement, Render, ScrollHandle, TestAppContext, VisualTestContext, div,
    point, px,
};
use std::time::{Duration, Instant};

struct Content {
    a: ScrollHandle,
    b: ScrollHandle,
    height: f32,
}
impl Render for Content {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .child(
                div()
                    .id("a")
                    .w(px(100.0))
                    .h(px(100.0))
                    .overflow_scroll()
                    .track_scroll(&self.a)
                    .child(
                        div()
                            .flex_none()
                            .w(px(1000.0))
                            .h(px(self.height))
                            .child("a"),
                    ),
            )
            .child(
                div()
                    .id("b")
                    .w(px(100.0))
                    .h(px(100.0))
                    .overflow_scroll()
                    .track_scroll(&self.b)
                    .child(div().flex_none().w(px(1000.0)).h(px(1000.0)).child("b")),
            )
    }
}
fn content(cx: &mut TestAppContext) -> (gpui::Entity<Content>, &mut VisualTestContext) {
    let (view, cx) = cx.add_window_view(|_, _| Content {
        a: ScrollHandle::new(),
        b: ScrollHandle::new(),
        height: 1000.0,
    });
    cx.run_until_parked();
    (view, cx)
}

#[gpui::test]
fn handles_accumulate_reverse_and_keep_axes_and_regions_independent(cx: &mut TestAppContext) {
    let (view, cx) = content(cx);
    let (a, b) = view.read_with(cx, |v, _| (v.a.clone(), v.b.clone()));
    assert!(a.max_offset().x > px(500.0) && a.max_offset().y > px(500.0));
    let (a_motion, b_motion) = (Motion::new(a.clone()), Motion::new(b.clone()));
    let t = Instant::now();
    a_motion.wheel(t, -200.0, Axis::Horizontal, false, false);
    a_motion.wheel(t, -100.0, Axis::Horizontal, false, false);
    a_motion.wheel(t, -400.0, Axis::Vertical, false, false);
    b_motion.wheel(t, -80.0, Axis::Vertical, true, false);
    assert_eq!(a.offset(), point(px(0.0), px(0.0)));
    assert_eq!(b.offset().y, px(-80.0));
    assert!(a_motion.tick(t + Duration::from_millis(65), false));
    let mid = a.offset();
    assert!(mid.x < px(0.0) && mid.x > px(-300.0));
    assert!(mid.y < px(0.0) && mid.y > px(-400.0));
    a_motion.wheel(
        t + Duration::from_millis(65),
        400.0,
        Axis::Horizontal,
        false,
        false,
    );
    a_motion.tick(t + Duration::from_millis(90), false);
    assert!(a.offset().x > mid.x);
    assert!(!a_motion.tick(t + Duration::from_millis(300), false));
    assert_eq!(a.offset(), point(px(0.0), px(-400.0)));
    assert_eq!(b.offset(), point(px(0.0), px(-80.0)));
    assert!(!a_motion.tick(t + Duration::from_secs(1), false));
    a_motion.wheel(t, -30.0, Axis::Horizontal, false, true);
    assert_eq!(a.offset(), point(px(-30.0), px(-400.0)));
    assert!(!a_motion.is_animating());
}

#[gpui::test]
fn shrink_thumb_takeover_and_programmatic_jump_stop_the_old_goal(cx: &mut TestAppContext) {
    let (view, cx) = content(cx);
    let a = view.read_with(cx, |v, _| v.a.clone());
    let motion = Motion::new(a.clone());
    let t = Instant::now();
    motion.wheel(t, -700.0, Axis::Vertical, false, false);
    motion.tick(t + Duration::from_millis(10), false);
    view.update(cx, |v, cx| {
        v.height = 200.0;
        cx.notify();
    });
    cx.run_until_parked();
    motion.tick(t + Duration::from_millis(200), false);
    assert_eq!(a.offset().y, -a.max_offset().y);
    motion.wheel(t, -800.0, Axis::Horizontal, false, false);
    ScrollTarget::drag_started(&motion, Axis::Horizontal);
    motion.set_axis_offset(Axis::Horizontal, px(-90.0));
    motion.wheel(t, -700.0, Axis::Horizontal, false, false);
    assert_eq!(a.offset().x, px(-90.0));
    ScrollTarget::drag_ended(&motion, Axis::Horizontal);
    assert!(!motion.tick(t + Duration::from_secs(1), false));
    assert_eq!(a.offset().x, px(-90.0));
    motion.wheel(t, -800.0, Axis::Horizontal, false, false);
    motion.set_offset(point(px(-40.0), px(-50.0)));
    assert!(!motion.tick(t + Duration::from_secs(1), false));
    assert_eq!(a.offset(), point(px(-40.0), px(-50.0)));
}

#[gpui::test]
fn uniform_and_native_lists_keep_their_real_offsets_and_native_drag_hooks(cx: &mut TestAppContext) {
    use gpui::{ListAlignment, ListState, UniformListScrollHandle, list, uniform_list};
    struct Lists {
        uniform: UniformListScrollHandle,
        native: ListState,
    }
    impl Render for Lists {
        fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .child(
                    uniform_list("motion-uniform", 200, |range, _, _| {
                        range
                            .map(|i| div().h(px(24.0)).child(i.to_string()).into_any_element())
                            .collect()
                    })
                    .track_scroll(&self.uniform)
                    .w(px(120.0))
                    .h(px(200.0)),
                )
                .child(
                    list(self.native.clone(), |i, _, _| {
                        div().h(px(24.0)).child(i.to_string()).into_any_element()
                    })
                    .w(px(120.0))
                    .h(px(200.0)),
                )
        }
    }
    let (view, cx) = cx.add_window_view(|_, _| Lists {
        uniform: UniformListScrollHandle::new(),
        native: ListState::new(200, ListAlignment::Top, px(40.0)),
    });
    cx.run_until_parked();
    let (uniform, native) = view.read_with(cx, |v, _| {
        (v.uniform.0.borrow().base_handle.clone(), v.native.clone())
    });
    let um = Motion::new(uniform.clone());
    let nm = Motion::new(native.clone());
    let t = Instant::now();
    assert!(um.wheel(t, -150.0, Axis::Vertical, false, false));
    assert!(nm.wheel(t, -200.0, Axis::Vertical, false, false));
    assert!(!um.tick(t + Duration::from_millis(200), false));
    assert!(!nm.tick(t + Duration::from_millis(200), false));
    assert_eq!(uniform.offset().y, px(-150.0));
    let expected = (-200.0f32).max(-f32::from(native.max_offset().y));
    assert!((native.offset().y - px(expected)).abs() <= px(1.0));
    assert!(!nm.wheel(t, -200.0, Axis::Horizontal, false, false));
    nm.drag_started(Axis::Horizontal);
    assert!(!native.is_scrollbar_dragging());
    nm.drag_ended(Axis::Horizontal);
    nm.drag_started(Axis::Vertical);
    assert!(native.is_scrollbar_dragging());
    nm.set_axis_offset(Axis::Vertical, px(-300.0));
    nm.drag_ended(Axis::Vertical);
    assert!(!native.is_scrollbar_dragging());
    assert!(!nm.tick(t + Duration::from_secs(1), false));
    assert!(native.offset().y <= px(0.0) && native.offset().y >= -native.max_offset().y);
}

#[test]
fn stored_goal_shrinks_before_reversal_and_reduced_motion_stops_frames() {
    let mut state = AxisMotion::default();
    let t = Instant::now();
    state.wheel(t, 0.0, 1000.0, -600.0, false, false);
    let early = state
        .tick(t + Duration::from_millis(10), 0.0, 1000.0, false)
        .unwrap();
    let smaller = state
        .tick(t + Duration::from_millis(20), early, 100.0, false)
        .unwrap();
    assert!((-100.0..0.0).contains(&smaller));
    state.wheel(
        t + Duration::from_millis(20),
        smaller,
        120.0,
        300.0,
        false,
        false,
    );
    assert_eq!(
        state.tick(t + Duration::from_millis(200), smaller, 120.0, false),
        Some(0.0)
    );
    assert!(!state.animating);
    state.wheel(t, 0.0, 1000.0, -600.0, false, false);
    assert_eq!(
        state.tick(t + Duration::from_millis(20), 0.0, 80.0, true),
        Some(-80.0)
    );
    assert!(!state.animating);
}

#[gpui::test]
fn nested_motion_owns_one_wheel_and_keeps_child_clicks_working(cx: &mut TestAppContext) {
    cx.update(ely_core::init_for_tests);
    use gpui::{Modifiers, ScrollDelta, ScrollEvent, ScrollSource};
    struct Nested {
        outer: ScrollHandle,
        inner: ScrollHandle,
        outer_motion: Motion<ScrollHandle>,
        inner_motion: Motion<ScrollHandle>,
        clicks: usize,
    }
    impl Render for Nested {
        fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .w(px(240.0))
                .h(px(200.0))
                .child(
                    div()
                        .id("outer")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.outer)
                        .child(
                            div().h(px(1000.0)).child(
                                div()
                                    .relative()
                                    .w(px(120.0))
                                    .h(px(100.0))
                                    .child(
                                        div()
                                            .id("inner")
                                            .debug_selector(|| "motion-inner".into())
                                            .size_full()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.inner)
                                            .child(
                                                div().h(px(1000.0)).child(
                                                    div()
                                                        .id("click-child")
                                                        .debug_selector(|| "motion-child".into())
                                                        .h(px(25.0))
                                                        .on_click(cx.listener(|s, _, _, cx| {
                                                            s.clicks += 1;
                                                            cx.notify();
                                                        }))
                                                        .child("click"),
                                                ),
                                            ),
                                    )
                                    .child(wheel_overlay(
                                        "inner",
                                        Axis::Vertical,
                                        self.inner_motion.clone(),
                                    )),
                            ),
                        ),
                )
                .child(wheel_overlay(
                    "outer",
                    Axis::Vertical,
                    self.outer_motion.clone(),
                ))
        }
    }
    let (view, cx) = cx.add_window_view(|_, _| {
        let outer = ScrollHandle::new();
        let inner = ScrollHandle::new();
        Nested {
            outer_motion: Motion::new(outer.clone()),
            inner_motion: Motion::new(inner.clone()),
            outer,
            inner,
            clicks: 0,
        }
    });
    cx.run_until_parked();
    let child = cx.debug_bounds("motion-child").unwrap().center();
    cx.simulate_click(child, Modifiers::none());
    assert_eq!(view.read_with(cx, |s, _| s.clicks), 1);
    let inner = cx.debug_bounds("motion-inner").unwrap().center();
    cx.simulate_mouse_move(inner, None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: inner,
        delta: ScrollDelta::Pixels(point(px(0.0), px(-50.0))),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: ScrollSource::Pointer,
    });
    assert_eq!(view.read_with(cx, |s, _| s.inner.offset().y), px(-50.0));
    assert_eq!(view.read_with(cx, |s, _| s.outer.offset().y), px(0.0));
}
