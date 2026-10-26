//! Read-only content scrollbar bindings for real scroll handles.
//!
//! The appearance and fade are shared with the workspace tabs. The interaction
//! is deliberately the same as Ely's public `layout::Scrollbar`: one overlay
//! track, one thumb, and a direct ratio from the drag onto the real offset.

use gpui::{
    AnyElement, App, Axis, EmptyView, EntityId, Pixels, ScrollHandle, Styled, Window, div, point,
    prelude::*, px,
};

use crate::ui::scrollbar::Style;

pub type OnDragging = std::rc::Rc<dyn Fn(bool, &mut Window, &mut App)>;

pub fn render(
    handle: &ScrollHandle,
    axis: Axis,
    opacity: f32,
    owner: EntityId,
    name: &'static str,
    on_dragging: OnDragging,
    cx: &App,
) -> AnyElement {
    let size = handle.bounds().size;
    let (span, reach, scrolled) = match axis {
        Axis::Horizontal => (size.width, handle.max_offset().x, -handle.offset().x),
        Axis::Vertical => (size.height, handle.max_offset().y, -handle.offset().y),
    };
    let style = Style::READER;
    let length = span * (span / (span + reach));
    let length = length.max(px(style.min_thumb)).min(span);
    let travel = span - length;
    if reach <= Pixels::ZERO || travel <= Pixels::ZERO || opacity <= 0.0 {
        return div().into_any_element();
    }
    let start = travel * (scrolled / reach).clamp(0.0, 1.0);
    let thickness = px(style.thickness);
    let handle = handle.clone();
    let (idle, busy) = style.colors(cx);
    let thumb = div()
        .id(gpui::ElementId::Name(
            format!("{name}-thumb-{}", owner.as_u64()).into(),
        ))
        .debug_selector(move || format!("{name}-thumb"))
        .absolute()
        .rounded_full()
        .bg(idle)
        .hover(move |style| style.bg(busy))
        .on_drag(
            Drag {
                owner,
                start: f32::from(start),
                axis,
                anchor: Default::default(),
            },
            {
                let on_dragging = on_dragging.clone();
                move |_, _, window, cx| {
                    on_dragging(true, window, cx);
                    cx.new(|_| EmptyView)
                }
            },
        )
        .map(|thumb| match axis {
            Axis::Vertical => thumb.top(start).left_0().w(thickness).h(length),
            Axis::Horizontal => thumb.left(start).top_0().h(thickness).w(length),
        });
    let track = div()
        .id(gpui::ElementId::Name(
            format!("{name}-track-{}", owner.as_u64()).into(),
        ))
        .debug_selector(move || name.to_string())
        .absolute()
        .opacity(opacity)
        .on_drag_move(move |event: &gpui::DragMoveEvent<Drag>, window, cx| {
            let drag = event.drag(cx);
            // Both axes share the owner, so the axis is part of the identity:
            // a vertical drag must never move the horizontal offset.
            if drag.owner != owner || drag.axis != axis {
                return;
            }
            let local = event.event.position - event.bounds.origin;
            let local: f32 = match axis {
                Axis::Vertical => local.y,
                Axis::Horizontal => local.x,
            }
            .into();
            let anchor = drag.anchor.get().unwrap_or_else(|| {
                drag.anchor.set(Some(local));
                local
            });
            let ratio = ((drag.start + local - anchor) / f32::from(travel)).clamp(0.0, 1.0);
            let offset = handle.offset();
            let reach = f32::from(reach);
            handle.set_offset(match axis {
                Axis::Vertical => point(offset.x, px(-(reach * ratio))),
                Axis::Horizontal => point(px(-(reach * ratio)), offset.y),
            });
            window.refresh();
        });
    let track = match axis {
        Axis::Vertical => track.top_0().bottom(px(4.0)).right(px(4.0)).w(px(8.0)),
        Axis::Horizontal => track.left_0().right_0().bottom(px(4.0)).h(px(8.0)),
    };
    track.child(thumb).into_any_element()
}

struct Drag {
    owner: EntityId,
    start: f32,
    axis: Axis,
    anchor: std::rc::Rc<std::cell::Cell<Option<f32>>>,
}
