//! Real scrolling targets. Offsets are negative, maximum offsets are nonnegative.
use gpui::{Axis, Bounds, Pixels, Point, ScrollHandle, Window};

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
