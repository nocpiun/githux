//! Real GPUI windows over the shared scrollbar: no business page is copied, and the
//! bars drive the same handles the content scrolls with.

use std::rc::Rc;

use gpui::{
    Axis, Context, ListAlignment, ListState, Modifiers, MouseButton, Pixels, Render, ScrollHandle,
    Size, TestAppContext, UniformListScrollHandle, VisualTestContext, Window, div, list, point,
    prelude::*, px, size, uniform_list,
};

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region {
    List,
    Wide,
}

/// Two independent regions: a virtualized vertical list, and a plain two-axis scroll box.
struct Probe {
    list: UniformListScrollHandle,
    list_items: usize,
    list_bars: Bars,
    wide: ScrollHandle,
    wide_content: Option<Size<Pixels>>,
    wide_bars: Bars,
    list_hovered: bool,
    wide_hovered: bool,
    clicks: usize,
    clock: std::time::Instant,
    pinned: Option<u64>,
}

impl Probe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = window;
        let _ = cx;
        Self {
            list: UniformListScrollHandle::new(),
            list_items: 200,
            list_bars: Bars::new(),
            wide: ScrollHandle::new(),
            wide_content: Some(size(px(2000.0), px(2000.0))),
            wide_bars: Bars::new(),
            list_hovered: false,
            wide_hovered: false,
            clicks: 0,
            clock: std::time::Instant::now(),
            pinned: None,
        }
    }

    fn now(&self) -> u64 {
        if let Some(pinned) = self.pinned {
            return pinned;
        }
        self.clock.elapsed().as_millis() as u64
    }

    fn list_target(&self) -> ScrollHandle {
        self.list.0.borrow().base_handle.clone()
    }

    fn set_hover(&mut self, region: Region, hovered: bool, cx: &mut Context<Self>) {
        let now = self.now();
        let changed = match region {
            Region::List => {
                self.list_hovered = hovered;
                self.list_bars.hover(hovered, now, false)
            }
            Region::Wide => {
                self.wide_hovered = hovered;
                self.wide_bars.hover(hovered, now, false)
            }
        };
        if changed {
            cx.notify();
        }
    }

    fn set_thumb(&mut self, region: Region, axis: Axis, dragging: bool, cx: &mut Context<Self>) {
        let now = self.now();
        let changed = match region {
            Region::List => {
                let target = self.list_target();
                self.list_bars.drag(&target, axis, dragging, now, false)
            }
            Region::Wide => {
                let target = self.wide.clone();
                self.wide_bars.drag(&target, axis, dragging, now, false)
            }
        };
        if changed {
            cx.notify();
        }
    }

    fn cancel(&mut self, region: Region, cx: &mut Context<Self>) {
        match region {
            Region::List => {
                let target = self.list_target();
                self.list_bars.cancel(&target);
            }
            Region::Wide => {
                let target = self.wide.clone();
                self.wide_bars.cancel(&target);
            }
        }
        cx.notify();
    }

    fn show(&mut self, region: Region, cx: &mut Context<Self>) {
        match region {
            Region::List => self.list_bars.snap(true),
            Region::Wide => self.wide_bars.snap(true),
        }
        cx.notify();
    }

    fn on_drag(&self, region: Region, cx: &mut Context<Self>) -> OnDragging {
        let this = cx.entity().downgrade();
        Rc::new(move |axis, dragging, _window, cx| {
            let _ = this.update(cx, |probe, cx| probe.set_thumb(region, axis, dragging, cx));
        })
    }
}

impl Render for Probe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now();
        if !cx.has_active_drag() {
            if self.list_bars.dragging().is_some() {
                let target = self.list.0.borrow().base_handle.clone();
                self.list_bars.settle(&target, now, false);
            }
            if self.wide_bars.dragging().is_some() {
                let target = self.wide.clone();
                self.wide_bars.settle(&target, now, false);
            }
        }
        let list_opacity = self.list_bars.opacity(now);
        let wide_opacity = self.wide_bars.opacity(now);
        let owner = cx.entity_id();
        let list_target = self.list_target();
        let wide_target = self.wide.clone();
        let list_on_drag = self.on_drag(Region::List, cx);
        let wide_on_drag = self.on_drag(Region::Wide, cx);
        let vertical = Placement::overlay(4.0, 8.0);
        let horizontal = Placement::overlay(4.0, 8.0).clear_of(&vertical, 2.0);
        let vertical = vertical.clear_of(&horizontal, 2.0);

        let list_bar = Bar {
            target: &list_target,
            id: BarId {
                owner,
                name: "probe-list-v",
                axis: Axis::Vertical,
            },
            placement: vertical,
            style: Style::DEFAULT,
            opacity: list_opacity,
            on_drag: list_on_drag,
        }
        .render(cx);
        let wide_v = Bar {
            target: &wide_target,
            id: BarId {
                owner,
                name: "probe-wide-v",
                axis: Axis::Vertical,
            },
            placement: vertical,
            style: Style::DEFAULT,
            opacity: wide_opacity,
            on_drag: wide_on_drag.clone(),
        }
        .render(cx);
        let wide_h = Bar {
            target: &wide_target,
            id: BarId {
                owner,
                name: "probe-wide-h",
                axis: Axis::Horizontal,
            },
            placement: horizontal,
            style: Style::DEFAULT,
            opacity: wide_opacity,
            on_drag: wide_on_drag,
        }
        .render(cx);

        let list_items = self.list_items;
        let list = uniform_list("probe-list", list_items, move |range, _window, _cx| {
            range
                .map(|ix| {
                    div()
                        .debug_selector(move || format!("probe-list-row-{ix}"))
                        .h(px(24.0))
                        .child(format!("row {ix}"))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.list)
        .size_full();

        let content = div()
            .id("probe-wide-content")
            .debug_selector(|| "probe-wide-content".to_string())
            .on_click(cx.listener(|probe, _, _, cx| {
                probe.clicks += 1;
                cx.notify();
            }))
            .map(|content| match self.wide_content {
                Some(size) => content.w(size.width).h(size.height),
                None => content.size_full(),
            });

        div()
            .debug_selector(|| "probe".to_string())
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .id("probe-list-region")
                    .debug_selector(|| "probe-list".to_string())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .on_hover(cx.listener(|probe, hovered, _, cx| {
                        probe.set_hover(Region::List, *hovered, cx);
                    }))
                    .child(list)
                    .child(list_bar),
            )
            .child(
                div()
                    .id("probe-wide-region")
                    .debug_selector(|| "probe-wide".to_string())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .on_hover(cx.listener(|probe, hovered, _, cx| {
                        probe.set_hover(Region::Wide, *hovered, cx);
                    }))
                    .child(
                        div()
                            .id("probe-wide-scroll")
                            .debug_selector(|| "probe-wide-scroll".to_string())
                            .size_full()
                            .overflow_x_scroll()
                            .overflow_y_scroll()
                            .track_scroll(&self.wide)
                            .child(content),
                    )
                    .child(wide_v)
                    .child(wide_h),
            )
    }
}

fn probe(cx: &mut TestAppContext) -> (gpui::Entity<Probe>, &mut VisualTestContext) {
    cx.update(ely_core::init_for_tests);
    cx.add_window_view(Probe::new)
}

fn frames(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
}

fn bounds(selector: &'static str, cx: &mut VisualTestContext) -> gpui::Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} was not drawn"))
}

fn drag(cx: &mut VisualTestContext, start: gpui::Point<Pixels>, delta: gpui::Point<Pixels>) {
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    for step in 1..=5 {
        let at = point(
            start.x + delta.x * step as f32,
            start.y + delta.y * step as f32,
        );
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    }
    let end = point(start.x + delta.x * 5.0, start.y + delta.y * 5.0);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn no_overflow_paints_nothing_and_leaves_the_content_clickable(cx: &mut TestAppContext) {
    let (probe, cx) = probe(cx);
    probe.update(cx, |probe, _| {
        probe.list_items = 0;
        probe.wide_content = None;
    });
    cx.simulate_resize(size(px(600.0), px(400.0)));
    cx.run_until_parked();
    probe.update(cx, |probe, cx| {
        probe.show(Region::List, cx);
        probe.show(Region::Wide, cx);
    });
    frames(cx);

    for selector in ["probe-list-v", "probe-wide-v", "probe-wide-h"] {
        assert!(
            cx.debug_bounds(selector).is_none(),
            "{selector} must not be painted"
        );
    }
    let region = bounds("probe-wide", cx);
    cx.simulate_click(region.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.clicks),
        1,
        "a hidden bar must not block input"
    );
}

#[gpui::test]
fn each_axis_and_region_moves_only_itself(cx: &mut TestAppContext) {
    let (probe, cx) = probe(cx);
    cx.simulate_resize(size(px(600.0), px(400.0)));
    cx.run_until_parked();
    probe.update(cx, |probe, cx| {
        probe.show(Region::List, cx);
        probe.show(Region::Wide, cx);
    });
    frames(cx);

    // The list's own vertical drag moves only the list.
    let thumb = bounds("probe-list-v-thumb", cx);
    drag(cx, thumb.center(), point(px(0.0), px(20.0)));
    frames(cx);
    let list_offset = probe.read_with(cx, |probe, _| probe.list_target().offset());
    assert!(
        list_offset.y < px(0.0),
        "the list scrolled: {list_offset:?}"
    );
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.wide.offset()),
        point(px(0.0), px(0.0)),
        "another region must not move"
    );

    // A vertical drag changes y only.
    let thumb = bounds("probe-wide-v-thumb", cx);
    drag(cx, thumb.center(), point(px(0.0), px(30.0)));
    frames(cx);
    let after_v = probe.read_with(cx, |probe, _| probe.wide.offset());
    assert!(
        after_v.y < px(0.0),
        "the vertical drag scrolled y: {after_v:?}"
    );
    assert_eq!(after_v.x, px(0.0), "a vertical drag leaves x: {after_v:?}");

    // A horizontal drag changes x only and keeps the vertical offset.
    let thumb = bounds("probe-wide-h-thumb", cx);
    drag(cx, thumb.center(), point(px(30.0), px(0.0)));
    frames(cx);
    let after_h = probe.read_with(cx, |probe, _| probe.wide.offset());
    assert!(
        after_h.x < px(0.0),
        "the horizontal drag scrolled x: {after_h:?}"
    );
    assert_eq!(
        after_h.y, after_v.y,
        "a horizontal drag leaves y: {after_h:?}"
    );
}

#[gpui::test]
fn the_two_tracks_do_not_meet_in_the_corner(cx: &mut TestAppContext) {
    let (probe, cx) = probe(cx);
    cx.simulate_resize(size(px(600.0), px(400.0)));
    cx.run_until_parked();
    probe.update(cx, |probe, cx| probe.show(Region::Wide, cx));
    frames(cx);

    let vertical = bounds("probe-wide-v", cx);
    let horizontal = bounds("probe-wide-h", cx);
    assert!(
        vertical.bottom() <= horizontal.top(),
        "the vertical track ends above the horizontal one: {vertical:?} vs {horizontal:?}"
    );
    let region = bounds("probe-wide", cx);
    assert_eq!(vertical.bottom(), region.bottom() - px(4.0 + 14.0));
}

#[gpui::test]
fn a_shrinking_range_keeps_the_drag_inside_the_track(cx: &mut TestAppContext) {
    let (probe, cx) = probe(cx);
    cx.simulate_resize(size(px(600.0), px(400.0)));
    cx.run_until_parked();
    probe.update(cx, |probe, cx| probe.show(Region::Wide, cx));
    frames(cx);

    let thumb = bounds("probe-wide-v-thumb", cx);
    cx.simulate_mouse_move(thumb.center(), None, Modifiers::none());
    cx.simulate_mouse_down(thumb.center(), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    for step in 1..=5 {
        cx.simulate_mouse_move(
            point(thumb.center().x, thumb.center().y + px(30.0 * step as f32)),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    // The content shrinks mid-drag; the next move must stay clamped.
    probe.update(cx, |probe, _| {
        probe.wide_content = Some(size(px(2000.0), px(300.0)))
    });
    frames(cx);
    for step in 6..=10 {
        cx.simulate_mouse_move(
            point(thumb.center().x, thumb.center().y + px(30.0 * step as f32)),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    cx.simulate_mouse_up(
        point(thumb.center().x, thumb.center().y + px(300.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    let (offset, max) = probe.read_with(cx, |probe, _| {
        (probe.wide.offset(), probe.wide.max_offset())
    });
    assert!(
        offset.y <= px(0.0) && offset.y >= -max.y,
        "{offset:?} of {max:?}"
    );
}

#[gpui::test]
fn cancelling_drops_the_drag_and_the_visibility(cx: &mut TestAppContext) {
    let (probe, cx) = probe(cx);
    cx.simulate_resize(size(px(600.0), px(400.0)));
    cx.run_until_parked();
    probe.update(cx, |probe, cx| probe.show(Region::List, cx));
    frames(cx);

    let thumb = bounds("probe-list-v-thumb", cx);
    cx.simulate_mouse_move(thumb.center(), None, Modifiers::none());
    cx.simulate_mouse_down(thumb.center(), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        thumb.center() + point(px(0.0), px(20.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    assert!(
        probe
            .read_with(cx, |probe, _| probe.list_bars.dragging())
            .is_some(),
        "the thumb drag is live"
    );
    probe.update(cx, |probe, cx| probe.cancel(Region::List, cx));
    frames(cx);
    assert!(
        probe
            .read_with(cx, |probe, _| probe.list_bars.dragging())
            .is_none()
    );
    assert!(!probe.read_with(cx, |probe, _| probe.list_bars.is_animating()));
    assert!(cx.debug_bounds("probe-list-v").is_none());
}

#[gpui::test]
fn the_list_bar_binds_the_real_virtualized_handle(cx: &mut TestAppContext) {
    let (probe, cx) = probe(cx);
    cx.simulate_resize(size(px(600.0), px(400.0)));
    cx.run_until_parked();
    probe.update(cx, |probe, cx| probe.show(Region::List, cx));
    frames(cx);
    let reach = probe.read_with(cx, |probe, _| probe.list_target().max_offset());
    assert!(reach.y > px(0.0), "the list overflows: {reach:?}");
    let thumb = bounds("probe-list-v-line", cx);
    assert_eq!(thumb.size.width, px(THICKNESS), "the thumb is a hairline");
    drag(cx, thumb.center(), point(px(0.0), px(40.0)));
    frames(cx);
    let offset = probe.read_with(cx, |probe, _| probe.list_target().offset());
    assert!(
        offset.y < px(0.0),
        "the drag moved the real list handle: {offset:?}"
    );
    assert!(
        offset.y >= -reach.y,
        "and stayed inside the range: {offset:?}"
    );
}

#[gpui::test]
fn only_the_region_that_was_released_loses_its_drag(cx: &mut TestAppContext) {
    let (probe, cx) = probe(cx);
    cx.simulate_resize(size(px(600.0), px(400.0)));
    cx.run_until_parked();
    probe.update(cx, |probe, cx| {
        probe.show(Region::List, cx);
        probe.show(Region::Wide, cx);
    });
    frames(cx);
    // Start a real thumb drag on the wide region, then release outside every region.
    let thumb = bounds("probe-wide-v-thumb", cx);
    cx.simulate_mouse_move(thumb.center(), None, Modifiers::none());
    cx.simulate_mouse_down(thumb.center(), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        thumb.center() + point(px(0.0), px(30.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.wide_bars.dragging()),
        Some(Axis::Vertical)
    );
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.list_bars.dragging()),
        None
    );
    cx.simulate_mouse_up(
        point(px(2.0), px(2.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    frames(cx);
    assert!(
        probe
            .read_with(cx, |probe, _| probe.wide_bars.dragging())
            .is_none(),
        "the platform drag ended"
    );
}

#[gpui::test]
fn style_and_placement_are_separate_from_the_band(cx: &mut TestAppContext) {
    // The tab band reserves 5px but paints a 3px line at its start, so the shared
    // placement must not squeeze the tabs into a reader-sized track box.
    assert_eq!(crate::ui::geometry::TAB_SCROLLBAR_BAND, THICKNESS + 2.0);
    let band = Placement::BAND;
    assert_eq!(band.inset, 0.0);
    assert_eq!(band.hit, THICKNESS);
    assert_eq!(band.edge, Edge::Start);
    let _ = cx;
}

/// A target that records the drag hooks, so the contract a future motion controller
/// relies on (start/end per axis, another axis untouched) is proven without a window.
#[derive(Clone, Default)]
struct Recorded {
    bounds: gpui::Bounds<Pixels>,
    offset: std::rc::Rc<std::cell::Cell<gpui::Point<Pixels>>>,
    max: gpui::Point<Pixels>,
    hooks: std::rc::Rc<std::cell::RefCell<Vec<(Axis, bool)>>>,
}

impl ScrollTarget for Recorded {
    fn viewport_bounds(&self) -> gpui::Bounds<Pixels> {
        self.bounds
    }
    fn offset(&self) -> gpui::Point<Pixels> {
        self.offset.get()
    }
    fn max_offset(&self) -> gpui::Point<Pixels> {
        self.max
    }
    fn set_axis_offset(&self, axis: Axis, offset: Pixels) {
        let mut current = self.offset.get();
        match axis {
            Axis::Horizontal => current.x = offset,
            Axis::Vertical => current.y = offset,
        }
        self.offset.set(current);
    }
    fn drag_started(&self, axis: Axis) {
        self.hooks.borrow_mut().push((axis, true));
    }
    fn drag_ended(&self, axis: Axis) {
        self.hooks.borrow_mut().push((axis, false));
    }
}

#[test]
fn bars_report_drag_hooks_once_per_axis_and_cancel_clears_everything() {
    let target = Recorded {
        bounds: gpui::Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(100.0))),
        offset: Default::default(),
        max: point(px(50.0), px(400.0)),
        hooks: Default::default(),
    };
    let mut bars = Bars::new();
    assert!(bars.drag(&target, Axis::Vertical, true, 0, false));
    assert!(
        !bars.drag(&target, Axis::Horizontal, true, 0, false),
        "one drag at a time"
    );
    assert_eq!(bars.dragging(), Some(Axis::Vertical));
    assert!(bars.drag(&target, Axis::Vertical, false, 10, false));
    assert_eq!(
        *target.hooks.borrow(),
        vec![(Axis::Vertical, true), (Axis::Vertical, false)]
    );

    bars.hover(true, 20, false);
    assert!(bars.is_animating());
    bars.drag(&target, Axis::Horizontal, true, 30, false);
    bars.hover(false, 40, false);
    bars.cancel(&target);
    assert_eq!(bars.dragging(), None);
    assert!(!bars.is_animating());
    assert_eq!(bars.opacity(1000), 0.0);
    assert_eq!(
        *target.hooks.borrow(),
        vec![
            (Axis::Vertical, true),
            (Axis::Vertical, false),
            (Axis::Horizontal, true),
            (Axis::Horizontal, false)
        ]
    );
    // The other axis keeps its offset when only one is written.
    target.set_axis_offset(Axis::Vertical, px(-30.0));
    target.set_axis_offset(Axis::Horizontal, px(-10.0));
    assert_eq!(target.offset(), point(px(-10.0), px(-30.0)));
}

mod native_list_tests {
    use super::*;

    struct NativeList {
        state: ListState,
        bars: Bars,
    }

    impl NativeList {
        fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
            Self {
                state: ListState::new(200, ListAlignment::Top, px(40.0)),
                bars: Bars::new(),
            }
        }
    }

    impl Render for NativeList {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.bars.dragging().is_some() && !cx.has_active_drag() {
                let state = self.state.clone();
                self.bars.settle(&state, 0, false);
            }
            let state = self.state.clone();
            let drag_state = state.clone();
            let this = cx.entity().downgrade();
            let on_drag: OnDragging = Rc::new(move |axis, dragging, _window, cx| {
                let state = drag_state.clone();
                let _ = this.update(cx, |view, cx| {
                    if view.bars.drag(&state, axis, dragging, 0, false) {
                        cx.notify();
                    }
                });
            });
            let bar = Bar {
                target: &state,
                id: BarId {
                    owner: cx.entity_id(),
                    name: "native-list-v",
                    axis: Axis::Vertical,
                },
                placement: Placement::overlay(4.0, 8.0),
                style: Style::DEFAULT,
                opacity: self.bars.opacity(0),
                on_drag,
            }
            .render(cx);
            div()
                .relative()
                .size_full()
                .child(
                    list(state, |ix, _window, _cx| {
                        div()
                            .h(px(24.0))
                            .child(format!("native row {ix}"))
                            .into_any_element()
                    })
                    .size_full(),
                )
                .child(bar)
        }
    }

    #[gpui::test]
    fn native_list_state_drags_and_uses_its_drag_hooks(cx: &mut TestAppContext) {
        cx.update(ely_core::init_for_tests);
        let (view, cx) = cx.add_window_view(NativeList::new);
        cx.simulate_resize(size(px(300.0), px(240.0)));
        cx.run_until_parked();
        view.update(cx, |view, _| view.bars.snap(true));
        frames(cx);
        assert!(view.read_with(cx, |view, _| view.state.max_offset_for_scrollbar().y) > px(0.0));
        let thumb = bounds("native-list-v-thumb", cx).center();
        drag(cx, thumb, point(px(0.0), px(20.0)));
        frames(cx);
        let offset = view.read_with(cx, |view, _| view.state.scroll_px_offset_for_scrollbar());
        assert!(offset.y < px(0.0), "native ListState moved: {offset:?}");
        assert_eq!(offset.x, Pixels::ZERO);
        assert!(!view.read_with(cx, |view, _| view.state.is_scrollbar_dragging()));
    }
}

#[test]
fn a_list_state_ignores_the_axis_it_cannot_scroll() {
    let state = ListState::new(10, ListAlignment::Top, px(20.0));
    // Nothing measured yet: the calls must be no-ops, not panics.
    state.set_axis_offset(Axis::Horizontal, px(-40.0));
    state.drag_started(Axis::Horizontal);
    assert!(
        !state.is_scrollbar_dragging(),
        "a horizontal drag opens no list drag"
    );
    state.drag_started(Axis::Vertical);
    assert!(state.is_scrollbar_dragging(), "the vertical drag opens it");
    state.drag_ended(Axis::Horizontal);
    assert!(
        state.is_scrollbar_dragging(),
        "the wrong axis does not close it"
    );
    state.drag_ended(Axis::Vertical);
    assert!(!state.is_scrollbar_dragging());
    assert_eq!(state.scroll_px_offset_for_scrollbar().x, Pixels::ZERO);
}
