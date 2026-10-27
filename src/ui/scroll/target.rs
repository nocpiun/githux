//! Real scrolling targets. Offsets are negative, maximum offsets are nonnegative.
use gpui::{Axis, Bounds, ListState, Pixels, Point, ScrollHandle, Window};

/// Minimal scrollbar boundary. Future motion controllers cancel their animation in
/// `drag_started` and `set_axis_offset`; the scrollbar itself never consumes wheel input.
pub trait ScrollTarget {
    fn viewport_bounds(&self) -> Bounds<Pixels>;
    fn offset(&self) -> Point<Pixels>;
    fn max_offset(&self) -> Point<Pixels>;
    /// Must preserve the other axis. Values are negative and already clamped.
    fn set_axis_offset(&self, axis: Axis, offset: Pixels);
    fn drag_started(&self, _axis: Axis) {}
    fn drag_ended(&self, _axis: Axis) {}
    fn redraw(&self, window: &mut Window) {
        window.refresh();
    }
}

impl ScrollTarget for ScrollHandle {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.bounds()
    }
    fn offset(&self) -> Point<Pixels> {
        self.offset()
    }
    fn max_offset(&self) -> Point<Pixels> {
        self.max_offset()
    }
    fn set_axis_offset(&self, axis: Axis, offset: Pixels) {
        let mut current = self.offset();
        match axis {
            Axis::Horizontal => current.x = offset,
            Axis::Vertical => current.y = offset,
        }
        self.set_offset(current);
    }
}

/// GPUI's native non-uniform-list adapter: lazy measurement stays in `ListState`, and
/// drag hooks freeze its measured content height rather than being simulated by Githux.
impl ScrollTarget for ListState {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.viewport_bounds()
    }
    fn offset(&self) -> Point<Pixels> {
        self.scroll_px_offset_for_scrollbar()
    }
    fn max_offset(&self) -> Point<Pixels> {
        self.max_offset_for_scrollbar()
    }
    fn set_axis_offset(&self, axis: Axis, offset: Pixels) {
        debug_assert_eq!(
            axis,
            Axis::Vertical,
            "GPUI ListState only supports vertical scrolling"
        );
        if axis == Axis::Vertical {
            self.set_offset_from_scrollbar(Point::new(Pixels::ZERO, offset));
        }
    }
    fn drag_started(&self, axis: Axis) {
        debug_assert_eq!(axis, Axis::Vertical);
        if axis == Axis::Vertical {
            self.scrollbar_drag_started();
        }
    }
    fn drag_ended(&self, axis: Axis) {
        debug_assert_eq!(axis, Axis::Vertical);
        if axis == Axis::Vertical {
            self.scrollbar_drag_ended();
        }
    }
}
