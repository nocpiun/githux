//! Rounded clipping for the island frame.
//!
//! GPUI 0.3.10's `ContentMask` carries only rectangular bounds. `rounded(...)` paints a
//! rounded background and `overflow_hidden()` clips children to a rectangle, so a child that
//! fills the island's bottom edge (the repository language bar) keeps painting square over
//! the island's corner arcs. Real rounded clipping would need a mask this version does not
//! have.
//!
//! [`RoundedClip`] wraps the island and does three things in order, after laying out and
//! painting exactly one child:
//!
//! 1. erase the four corner squares outside the island's rounded outline in the surrounding
//!    color, so no child pixel survives past the arc;
//! 2. repaint the island's own 1px border with the same radius, because the erasers replace
//!    the corner square including the part of the ring that lies in it.
//!
//! Nothing is resized, padded, hidden or re-laid out, so the language bar keeps its
//! proportions, bottom position, hover targets and window-fitted tooltip, and the page keeps
//! its own scroll region and hit testing. The wrapper itself adds no layout box: it takes the
//! child's layout id and paints over the same bounds.

use gpui::{
    AnyElement, App, Background, Bounds, Element, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window, point, px,
};

use super::geometry::{ISLAND_BORDER_WIDTH, ISLAND_RADIUS};

/// Circle-to-cubic approximation constant, the usual 4/3·(√2−1).
const KAPPA: f32 = 0.552_284_8;

/// One corner's eraser path, in window coordinates: the `radius`×`radius` corner square minus
/// the quarter disc the rounded outline owns.
///
/// `dx`/`dy` are `±1` and point from the corner into the shape, so one local geometry serves
/// all four corners. The curve starts on one edge `radius` from the corner and ends on the
/// other, bulging towards the corner through `(1 − 1/√2)·radius` on the diagonal.
fn corner_sliver(corner: gpui::Point<Pixels>, radius: f32, dx: f32, dy: f32) -> gpui::Path<Pixels> {
    let (x0, y0) = (f32::from(corner.x), f32::from(corner.y));
    let at = |u: f32, v: f32| point(px(x0 + dx * u), px(y0 + dy * v));
    let k = radius * KAPPA;
    let mut path = gpui::PathBuilder::fill();
    path.move_to(corner);
    path.line_to(at(radius, 0.0));
    path.cubic_bezier_to(at(0.0, radius), at(radius - k, 0.0), at(0.0, radius - k));
    path.close();
    path.build().unwrap_or_else(|_| gpui::Path::new(corner))
}

/// The four corner slivers of `bounds` with corner `radius`. Empty for a degenerate box
/// instead of producing negative or inverted geometry; the radius never exceeds half the box,
/// which is also how `Style` clamps the corner radii it paints.
pub(crate) fn corner_slivers(bounds: Bounds<Pixels>, radius: Pixels) -> Vec<gpui::Path<Pixels>> {
    let radius = f32::from(radius)
        .max(0.0)
        .min(f32::from(bounds.size.width) / 2.0)
        .min(f32::from(bounds.size.height) / 2.0);
    if radius <= 0.0 {
        return Vec::new();
    }
    [
        (bounds.origin, 1.0, 1.0),
        (point(bounds.right(), bounds.top()), -1.0, 1.0),
        (point(bounds.right(), bounds.bottom()), -1.0, -1.0),
        (point(bounds.left(), bounds.bottom()), 1.0, -1.0),
    ]
    .into_iter()
    .map(|(corner, dx, dy)| corner_sliver(corner, radius, dx, dy))
    .collect()
}

/// Paints one child and masks its corners to `radius`, restoring the outer `border` ring.
pub(crate) struct RoundedClip {
    child: AnyElement,
    radius: Pixels,
    border: Pixels,
    outside: Hsla,
    border_color: Hsla,
}

impl RoundedClip {
    pub(crate) fn new(
        child: AnyElement,
        radius: Pixels,
        border: Pixels,
        outside: Hsla,
        border_color: Hsla,
    ) -> Self {
        Self {
            child,
            radius,
            border,
            outside,
            border_color,
        }
    }
}

impl IntoElement for RoundedClip {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RoundedClip {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Exactly the child's layout: the wrapper adds no padding, margin or size of its own.
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // The island frame paints its surface, clipped content and border first.
        self.child.paint(window, cx);
        // Then the corner squares outside the rounded outline are replaced with the color
        // behind the island, so the child's square corner cannot show.
        for path in corner_slivers(bounds, self.radius) {
            window.paint_path(path, Background::from(self.outside));
        }
        // The erasers covered the part of the ring inside those squares; repaint the border
        // with the same outer radius so the island keeps one continuous 1px outline.
        window.paint_quad(gpui::quad(
            bounds,
            self.radius,
            gpui::transparent_black(),
            self.border,
            self.border_color,
            gpui::BorderStyle::Solid,
        ));
    }
}

/// The island frame with its square-cornered children clipped to the rounded outline.
pub(crate) fn rounded_island(frame: AnyElement, outside: Hsla, border_color: Hsla) -> AnyElement {
    RoundedClip::new(
        frame,
        px(ISLAND_RADIUS),
        px(ISLAND_BORDER_WIDTH),
        outside,
        border_color,
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, Pixels, point, px, size};

    fn island() -> Bounds<Pixels> {
        Bounds::new(point(px(247.0), px(55.0)), size(px(633.0), px(505.0)))
    }

    fn distance(from: gpui::Point<Pixels>, to: gpui::Point<Pixels>) -> f32 {
        let dx = f32::from(from.x) - f32::from(to.x);
        let dy = f32::from(from.y) - f32::from(to.y);
        (dx * dx + dy * dy).sqrt()
    }

    /// Each eraser is the island's own corner square, its curve follows the island radius, and
    /// a square child's corner is inside the erased region.
    #[test]
    fn erasers_cover_the_corner_square_outside_the_island_arc() {
        let bounds = island();
        let radius = px(ISLAND_RADIUS);
        let slivers = corner_slivers(bounds, radius);
        assert_eq!(slivers.len(), 4, "one eraser per corner");
        let corners = [
            (bounds.origin, 1.0, 1.0),
            (point(bounds.right(), bounds.top()), -1.0, 1.0),
            (point(bounds.right(), bounds.bottom()), -1.0, -1.0),
            (point(bounds.left(), bounds.bottom()), 1.0, -1.0),
        ];
        for (sliver, (corner, dx, dy)) in slivers.iter().zip(corners) {
            assert!(sliver.vertices.len() > 4, "corners are curved, not beveled");
            assert_eq!(
                sliver.bounds.left(),
                if dx < 0.0 {
                    corner.x - radius
                } else {
                    corner.x
                }
            );
            assert_eq!(
                sliver.bounds.top(),
                if dy < 0.0 {
                    corner.y - radius
                } else {
                    corner.y
                }
            );
            assert_eq!(sliver.bounds.size, size(radius, radius));

            // The cubic's midpoint is on the island's corner circle...
            let arc_center = point(
                corner.x + px(dx * ISLAND_RADIUS),
                corner.y + px(dy * ISLAND_RADIUS),
            );
            let diagonal = ISLAND_RADIUS * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
            let mid = point(corner.x + px(dx * diagonal), corner.y + px(dy * diagonal));
            assert!((distance(mid, arc_center) - ISLAND_RADIUS).abs() < 0.02);

            // ... and the square child's own corner (one border width inside the box) lies
            // outside it, so the child's square paint is erased there.
            let child_corner =
                corner + point(px(dx * ISLAND_BORDER_WIDTH), px(dy * ISLAND_BORDER_WIDTH));
            assert!(
                distance(child_corner, arc_center) > ISLAND_RADIUS,
                "the child's square corner must be outside the arc"
            );
        }
    }

    /// A narrow or degenerate island clamps instead of inverting the geometry, and never draws
    /// an eraser larger than the box it belongs to.
    #[test]
    fn narrow_and_degenerate_islands_clamp_instead_of_panicking() {
        let narrow = Bounds::new(point(px(0.0), px(0.0)), size(px(9.0), px(400.0)));
        for sliver in corner_slivers(narrow, px(ISLAND_RADIUS)) {
            assert!(sliver.bounds.size.width <= px(4.5) + px(0.01));
            assert!(sliver.bounds.size.height <= px(4.5) + px(0.01));
        }

        let flat = Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(0.0)));
        assert!(corner_slivers(flat, px(ISLAND_RADIUS)).is_empty());
        let empty = Bounds::new(point(px(0.0), px(0.0)), size(Pixels::ZERO, Pixels::ZERO));
        assert!(corner_slivers(empty, px(ISLAND_RADIUS)).is_empty());
    }
}
