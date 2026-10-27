//! The project's one scrollbar: appearance, fade, thumb geometry and drag.
//!
//! Every scrolling region keeps its own [`Bars`] state and binds it to a real
//! [`ScrollTarget`] (today a `gpui::ScrollHandle`, or the public
//! `UniformListScrollHandle::base_handle` of a virtualized list). Placement is a
//! separate concern: the tab strip paints in a reserved band, while a content
//! region overlays the bar on its trailing edge with an inset.
//!
//! A hidden bar builds no element at all, so it can neither take a press nor a
//! drag. A thumb drag pins the bar on until the platform drag ends, wherever it
//! ends; a tab reorder keeps it visible through [`Bars::hold`] instead, so an
//! unrelated drag is never mistaken for a thumb drag.

mod fade;
mod geometry;
mod target;

#[cfg(test)]
mod tests;

pub use fade::Fade;
pub use geometry::Thumb;
pub use target::ScrollTarget;

use std::cell::Cell;
use std::rc::Rc;

use ely_core::theme::ActiveTheme;
use gpui::{
    AnyElement, App, Axis, EmptyView, EntityId, Hsla, IntoElement, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px,
};

/// Visible thickness of the line, in logical pixels.
pub const THICKNESS: f32 = 3.0;

/// How long a bar takes to fade in or out, in milliseconds.
pub const FADE_MS: u64 = 140;

/// Shortest a thumb may become, so a very long document still offers a grab.
pub const MIN_THUMB: f32 = 24.0;

/// Called when this region's thumb drag starts (`true`) or ends (`false`).
pub type OnDragging = Rc<dyn Fn(Axis, bool, &mut Window, &mut App) + 'static>;

/// Appearance of the shared bar. One value for every region and both axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub thickness: f32,
    pub min_thumb: f32,
    pub idle_opacity: f32,
    pub busy_opacity: f32,
}

impl Style {
    /// The accepted reader appearance: a hairline, both ends round, semi-transparent.
    pub const DEFAULT: Self = Self {
        thickness: THICKNESS,
        min_thumb: MIN_THUMB,
        idle_opacity: 0.20,
        busy_opacity: 0.38,
    };

    pub fn colors(self, cx: &App) -> (Hsla, Hsla) {
        let fg = cx.theme().colors.fg;
        (fg.opacity(self.idle_opacity), fg.opacity(self.busy_opacity))
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Which cross-axis edge a bar hugs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Top for a horizontal bar, left for a vertical one.
    Start,
    /// Bottom for a horizontal bar, right for a vertical one.
    End,
}

/// Where a bar paints inside the region it belongs to, kept apart from [`Style`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Gap from the region's edges along the track (both ends) and, for [`Edge::End`],
    /// on the hugging edge as well.
    pub inset: f32,
    /// Extra length left at the track's far end for the crossing axis, so two bars
    /// never meet in the same corner.
    pub corner: f32,
    /// Cross-axis size of the interactive track; the visible line is centered in it.
    pub hit: f32,
    pub edge: Edge,
}

impl Placement {
    /// The tab strip's reserved band: no inset, and the hit area is the line itself.
    pub const BAND: Self = Self {
        inset: 0.0,
        corner: 0.0,
        hit: THICKNESS,
        edge: Edge::Start,
    };

    /// An overlay on a content region's trailing edge.
    ///
    /// This is the placement the sidebar and content-reader consumers wire next; the tab strip
    /// uses [`Placement::BAND`] today, so only the tests call it this round.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn overlay(inset: f32, hit: f32) -> Self {
        Self {
            inset,
            corner: 0.0,
            hit,
            edge: Edge::End,
        }
    }

    /// Leaves room at the far end for the crossing bar; `other` is that bar's [`Placement`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear_of(mut self, other: &Self, gap: f32) -> Self {
        self.corner = other.inset + other.hit + gap;
        self
    }
}

/// The identity of one bar's thumb drag: owner, region and axis must all match.
#[derive(Clone, Copy)]
pub struct BarId {
    pub owner: EntityId,
    /// Unique per region under one owner; also the debug-selector name.
    pub name: &'static str,
    pub axis: Axis,
}

/// One frame of a bar. `opacity` is resolved by the region that owns [`Bars`].
pub struct Bar<'a, T: ScrollTarget + Clone + 'static> {
    pub target: &'a T,
    pub id: BarId,
    pub placement: Placement,
    pub style: Style,
    pub opacity: f32,
    pub on_drag: OnDragging,
}

struct ThumbDrag {
    owner: EntityId,
    name: &'static str,
    axis: Axis,
    /// Thumb position at press as a fraction of its travel, so a range change during
    /// the drag does not jump the thumb.
    press_ratio: f32,
    anchor: Rc<Cell<Option<f32>>>,
}

impl<T: ScrollTarget + Clone + 'static> Bar<'_, T> {
    pub fn render(self, cx: &App) -> AnyElement {
        if self.opacity <= 0.0 {
            return div().into_any_element();
        }
        let name = self.id.name;
        let axis = self.id.axis;
        let owner = self.id.owner;
        let placement = self.placement;
        let style = self.style;
        let Some((viewport, reach, offset)) = axis_metrics(self.target, axis) else {
            return div().into_any_element();
        };
        let track = (viewport - 2.0 * placement.inset - placement.corner).max(0.0);
        let Some(thumb) = Thumb::new(viewport, reach, offset, track, style.min_thumb) else {
            return div().into_any_element();
        };
        let thickness = px(style.thickness);
        let hit = px(placement.hit);
        let inset = px(placement.inset);
        let corner = px(placement.corner);
        let (idle, busy) = style.colors(cx);
        let press_ratio = if thumb.travel > 0.0 {
            thumb.start / thumb.travel
        } else {
            0.0
        };

        let move_target = self.target.clone();
        let drag = ThumbDrag {
            owner,
            name,
            axis,
            press_ratio,
            anchor: Rc::new(Cell::new(None)),
        };
        let on_drag = self.on_drag;
        let element_id = |suffix: &str| {
            gpui::ElementId::Name(format!("{}-{suffix}-{}", name, owner.as_u64()).into())
        };
        let track_el = div()
            .id(element_id("track"))
            .debug_selector(move || name.to_string())
            .absolute()
            .opacity(self.opacity)
            .on_drag_move(move |event: &gpui::DragMoveEvent<ThumbDrag>, window, cx| {
                let drag = event.drag(cx);
                if drag.owner != owner || drag.name != name || drag.axis != axis {
                    return;
                }
                let Some((viewport, reach, offset)) = axis_metrics(&move_target, axis) else {
                    return;
                };
                let track = (viewport - 2.0 * placement.inset - placement.corner).max(0.0);
                let Some(thumb) = Thumb::new(viewport, reach, offset, track, style.min_thumb)
                else {
                    return;
                };
                let local: f32 = match axis {
                    Axis::Horizontal => (event.event.position.x - event.bounds.origin.x).into(),
                    Axis::Vertical => (event.event.position.y - event.bounds.origin.y).into(),
                };
                let anchor = match drag.anchor.get() {
                    Some(anchor) => anchor,
                    None => {
                        drag.anchor.set(Some(local));
                        local
                    }
                };
                let start = thumb.travel * drag.press_ratio + (local - anchor);
                move_target.set_axis_offset(axis, px(thumb.offset_at(start)));
                move_target.redraw(window);
            });
        let track_el = match (axis, placement.edge) {
            (Axis::Horizontal, Edge::Start) => {
                track_el.left(inset).right(inset + corner).top_0().h(hit)
            }
            (Axis::Horizontal, Edge::End) => track_el
                .left(inset)
                .right(inset + corner)
                .bottom(inset)
                .h(hit),
            (Axis::Vertical, Edge::Start) => {
                track_el.top(inset).bottom(inset + corner).left_0().w(hit)
            }
            (Axis::Vertical, Edge::End) => track_el
                .top(inset)
                .bottom(inset + corner)
                .right(inset)
                .w(hit),
        };

        let group: gpui::SharedString = format!("{name}-thumb").into();
        let line = div()
            .debug_selector(move || format!("{name}-line"))
            .rounded_full()
            .bg(idle)
            .group_hover(group.clone(), move |style| style.bg(busy))
            .map(|line| match axis {
                Axis::Horizontal => line.w_full().h(thickness),
                Axis::Vertical => line.h_full().w(thickness),
            });
        let thumb_el = div()
            .id(element_id("thumb"))
            .debug_selector(move || format!("{name}-thumb"))
            .group(group)
            .absolute()
            .flex()
            .items_center()
            .justify_center()
            .on_drag(drag, move |_, _, window, cx| {
                on_drag(axis, true, window, cx);
                cx.new(|_| EmptyView)
            });
        let thumb_el = match axis {
            Axis::Horizontal => thumb_el
                .left(px(thumb.start))
                .w(px(thumb.length))
                .top_0()
                .h(hit),
            Axis::Vertical => thumb_el
                .top(px(thumb.start))
                .h(px(thumb.length))
                .left_0()
                .w(hit),
        };
        track_el.child(thumb_el.child(line)).into_any_element()
    }
}

fn axis_metrics(target: &impl ScrollTarget, axis: Axis) -> Option<(f32, f32, f32)> {
    let bounds = target.viewport_bounds();
    let values = match axis {
        Axis::Horizontal => (bounds.size.width, target.max_offset().x, target.offset().x),
        Axis::Vertical => (bounds.size.height, target.max_offset().y, target.offset().y),
    };
    let (viewport, reach, offset) = (
        f32::from(values.0),
        f32::from(values.1),
        f32::from(values.2),
    );
    (viewport > 0.0 && reach > 0.0).then_some((viewport, reach, offset))
}

/// One region's scrollbar state: the fade, and the thumb drag currently in progress.
///
/// Every region owns its own value; nothing here is global. A region that keeps its
/// bar visible during another interaction (a tab reorder) uses [`Bars::hold`], which
/// is deliberately not a thumb drag.
#[derive(Debug, Default)]
pub struct Bars {
    fade: Fade,
    dragging: Option<Axis>,
}

impl Bars {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records whether the pointer is over the region.
    pub fn hover(&mut self, hovered: bool, now: u64, reduce_motion: bool) -> bool {
        self.fade.set_hovered(hovered, now, reduce_motion)
    }

    /// Keeps the bar visible during another interaction in this region.
    pub fn hold(&mut self, held: bool, now: u64, reduce_motion: bool) -> bool {
        self.fade.set_held(held, now, reduce_motion)
    }

    /// Records this region's own thumb drag starting or ending, and forwards both ends to the
    /// target's hooks (where a future smooth-motion controller cancels its animation).
    pub fn drag(
        &mut self,
        target: &impl ScrollTarget,
        axis: Axis,
        dragging: bool,
        now: u64,
        reduce_motion: bool,
    ) -> bool {
        if dragging {
            // The platform runs one drag at a time, so a second axis cannot start while this one
            // is live; ignoring it keeps the recorded axis and its end hook exact.
            if self.dragging.is_some() {
                return false;
            }
            self.dragging = Some(axis);
            target.drag_started(axis);
            self.fade.set_dragging(true, now, reduce_motion)
        } else {
            self.settle(target, now, reduce_motion)
        }
    }

    pub fn dragging(&self) -> Option<Axis> {
        self.dragging
    }

    /// Ends a thumb drag the platform has already released, including a release outside
    /// the region. Call only while `!cx.has_active_drag()`.
    pub fn settle(&mut self, target: &impl ScrollTarget, now: u64, reduce_motion: bool) -> bool {
        let Some(axis) = self.dragging.take() else {
            return false;
        };
        target.drag_ended(axis);
        self.fade.set_dragging(false, now, reduce_motion);
        true
    }

    /// Drops hover, hold and drag state without a fade: the region closed, or the window
    /// lost focus.
    pub fn cancel(&mut self, target: &impl ScrollTarget) {
        if let Some(axis) = self.dragging.take() {
            target.drag_ended(axis);
        }
        self.fade.clear();
    }

    pub fn opacity(&mut self, now: u64) -> f32 {
        self.fade.opacity(now)
    }

    pub fn is_animating(&self) -> bool {
        self.fade.is_animating()
    }

    #[cfg(test)]
    pub fn snap(&mut self, showing: bool) {
        self.fade.snap(showing)
    }
}
