//! Scrollable spacing with unchanged business indices and a full viewport clip.
use super::ScrollTarget;
use gpui::{
    AnyElement, App, AvailableSpace, Axis, Bounds, IntoElement, ListState, ParentElement, Pixels,
    Point, Styled, Window, div, point, px,
};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContentInsets {
    pub top: Pixels,
    pub bottom: Pixels,
}

#[derive(Clone, Default)]
pub struct ListInsets(Rc<Cell<ContentInsets>>, Rc<Cell<Option<(Pixels, usize)>>>);
impl ListInsets {
    pub fn get(&self) -> ContentInsets {
        self.0.get()
    }

    pub fn invalidate_hint(&self) {
        self.1.set(None);
    }
}

impl ContentInsets {
    pub fn new(top: Pixels, bottom: Pixels) -> Self {
        Self { top, bottom }
    }

    /// GPUI adds style padding.top at every logical anchor. Keep the leading
    /// space in the first item's envelope instead; the actual row is unchanged.
    pub fn item(self, index: usize, count: usize, row: AnyElement) -> AnyElement {
        if index != 0 || count == 0 || self.top == px(0.0) {
            return row;
        }
        div()
            .w_full()
            .min_w_0()
            .pt(self.top)
            .child(row)
            .into_any_element()
    }

    pub fn sync(self, list: &ListState, count: usize, previous: &mut ListInsets) {
        let count = count.min(list.item_count());
        let width = list.viewport_bounds().size.width;
        // Native first-layout/width invalidation discards even explicit hints.
        // Re-seed once per settled width/count from a measured real item; the
        // public API keeps measurements, focus and anchor, and measures nothing.
        if count == 0 {
            previous.invalidate_hint();
        } else if width > px(0.0) && previous.1.get() != Some((width, count)) {
            let index = list.logical_scroll_top().item_ix;
            if let Some(item) = list.bounds_for_item(index) {
                let leading = if index == 0 {
                    previous.get().top
                } else {
                    px(0.0)
                };
                let hint = item.size.height - leading;
                if hint > px(0.0) {
                    let _ = list.clone().with_uniform_item_height(hint);
                    previous.1.set(Some((width, count)));
                }
            }
        }
        if count > 0 && previous.get().top != self.top {
            list.remeasure_items(0..1);
        }
        previous.0.set(self);
    }
}

/// Native List supports scrollable bottom padding, but its scrollbar maximum
/// omits it. Include that known extent without waiting for a tail measurement.
/// Scroll through the public pixel API, whose layout respects native padding.
#[derive(Clone)]
pub struct InsetListTarget {
    list: ListState,
    insets: ListInsets,
}
impl InsetListTarget {
    pub fn new(list: ListState, insets: ListInsets) -> Self {
        Self { list, insets }
    }
}
impl ScrollTarget for InsetListTarget {
    fn supports_axis(&self, axis: Axis) -> bool {
        axis == Axis::Vertical
    }
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.list.viewport_bounds()
    }
    fn offset(&self) -> Point<Pixels> {
        self.list.scroll_px_offset_for_scrollbar()
    }
    fn max_offset(&self) -> Point<Pixels> {
        let count = self.list.item_count();
        if count == 0 {
            return point(px(0.0), px(0.0));
        }
        let viewport = self.list.viewport_bounds();
        let native = self.list.max_offset_for_scrollbar().y;
        let content = if native > px(0.0) {
            // This includes the native frozen height during thumb dragging.
            native + viewport.size.height
        } else if let Some(last) = self.list.bounds_for_item(count - 1) {
            last.bottom() - viewport.top() - self.offset().y
        } else if self.list.logical_scroll_top().item_ix == count {
            -self.offset().y - self.list.logical_scroll_top().offset_in_item
        } else {
            px(0.0)
        };
        point(
            px(0.0),
            (content + self.insets.get().bottom - viewport.size.height).max(px(0.0)),
        )
    }
    fn set_axis_offset(&self, axis: Axis, offset: Pixels) {
        if axis == Axis::Vertical {
            self.list.scroll_by(self.offset().y - offset.min(px(0.0)));
        }
    }
    fn drag_started(&self, axis: Axis) {
        if axis == Axis::Vertical {
            self.list.scrollbar_drag_started();
        }
    }
    fn drag_ended(&self, axis: Axis) {
        if axis == Axis::Vertical {
            self.list.scrollbar_drag_ended();
        }
    }
}

/// Uniform-list range calculation uses the padded height while its paint clip
/// covers full bounds. One additional real row covers the final inset strip,
/// without changing native item count, height, indices or input handlers.
pub fn with_trailing_overdraw(row: AnyElement, following: Option<AnyElement>) -> AnyElement {
    let Some(mut following) = following else {
        return row;
    };
    div()
        .relative()
        .w_full()
        .min_w_0()
        .child(row)
        .child(
            gpui::canvas(
                move |bounds, window: &mut Window, cx: &mut App| {
                    following.layout_as_root(
                        gpui::size(
                            AvailableSpace::Definite(bounds.size.width),
                            AvailableSpace::MinContent,
                        ),
                        window,
                        cx,
                    );
                    following.prepaint_at(bounds.origin, window, cx);
                    following
                },
                |_, mut following, window, cx| {
                    following.paint(window, cx);
                },
            )
            .absolute()
            .top(gpui::relative(1.0))
            .left_0()
            .w_full()
            .h(px(0.0)),
        )
        .into_any_element()
}
