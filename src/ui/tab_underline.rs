//! One horizontal active-tab underline, shared by every in-page tab strip.
//!
//! The repository content tabs, the narrow Settings category tabs and the Profile Overview strip
//! all draw the same 2px accent band under the selected tab. Before this module each strip put the
//! band on the selected tab itself, so changing tabs removed it from one label and drew it under
//! the next in the same frame: a jump with no movement and no width change. The band also could not
//! follow a label that was not measured yet (a count badge arriving later) as one object.
//!
//! This module owns the single presentation:
//!
//! * every tab hosts an invisible [`probe`](TabUnderline::probe) reporting its real painted bounds;
//! * the strip hosts one [`layer`](TabUnderline::layer) that paints the band at the measured
//!   active tab, interpolating `left` and `width` from wherever the band currently is;
//! * an interrupted change continues from the interpolated band, so rapid re-selection never
//!   restarts from a stale tab;
//! * the first measurement, a resize, a font/DPI change, a count badge that changes a label's
//!   width and any strip scrolling are treated as layout facts and adopted immediately, so the
//!   band never slides through a path the tabs did not take;
//! * reduced motion paints the destination at once;
//! * frames are requested only while the band is actually moving, so an idle strip costs nothing.
//!
//! It is presentation-only: no strip's state, roles, spacing, focus handling, scrolling or click
//! targets move into it. The caller keeps drawing the tab entries exactly as before and hands in
//! the selected index.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, Background, Bounds, Hsla, IntoElement, ParentElement, Pixels, Styled, Window,
    canvas, div, fill, point, px, size,
};

/// How long one move between two tabs takes.
pub(crate) const TAB_UNDERLINE_DURATION_MS: u64 = 180;
/// Painted thickness of the band, in logical pixels.
pub(crate) const TAB_UNDERLINE_THICKNESS: f32 = 2.0;
/// Two measurements closer than this are the same layout, not a change to animate.
const SAME_LAYOUT: f32 = 0.05;

fn same_rect(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    (a.left() - b.left()).abs() <= px(SAME_LAYOUT)
        && (a.top() - b.top()).abs() <= px(SAME_LAYOUT)
        && (a.size.width - b.size.width).abs() <= px(SAME_LAYOUT)
        && (a.size.height - b.size.height).abs() <= px(SAME_LAYOUT)
}

fn lerp(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let mix = |a: Pixels, b: Pixels| a + (b - a) * t;
    Bounds::new(
        point(
            mix(from.origin.x, to.origin.x),
            mix(from.origin.y, to.origin.y),
        ),
        size(
            mix(from.size.width, to.size.width),
            mix(from.size.height, to.size.height),
        ),
    )
}

#[derive(Default)]
struct Shared {
    /// The last bounds each tab reported while painting, in window coordinates.
    measured: Vec<Option<Bounds<Pixels>>>,
    /// The tab index the band currently belongs to.
    active: Option<usize>,
    /// The band painted on the last frame, or `None` before the first measurement.
    painted: Option<Bounds<Pixels>>,
    from: Option<Bounds<Pixels>>,
    to: Option<Bounds<Pixels>>,
    started_ms: u64,
}

impl Shared {
    /// The band the active tab asks for: its own width, its own bottom edge.
    fn target(&self, active: usize) -> Option<Bounds<Pixels>> {
        let tab = (*self.measured.get(active)?)?;
        if tab.size.width <= px(0.0) || tab.size.height <= px(0.0) {
            return None;
        }
        Some(Bounds::new(
            point(tab.left(), tab.bottom() - px(TAB_UNDERLINE_THICKNESS)),
            size(tab.size.width, px(TAB_UNDERLINE_THICKNESS)),
        ))
    }

    /// Adopts the active tab's measured geometry and returns the band for this frame.
    ///
    /// The returned flag asks the painter for another frame: true only while the band is still
    /// moving, so an idle strip stops requesting frames.
    fn step(
        &mut self,
        active: usize,
        now_ms: u64,
        reduced: bool,
    ) -> (Option<Bounds<Pixels>>, bool) {
        let Some(target) = self.target(active) else {
            // The strip is not measured yet (first layout) or this tab is not painted: keep the
            // last band instead of flying to the origin.
            return (self.painted, false);
        };

        if reduced {
            // Reduced motion is a destination, not a move: adopt the tab and retire at once so an
            // idle strip never asks for another frame.
            self.active = Some(active);
            self.from = Some(target);
            self.to = Some(target);
            self.started_ms = now_ms;
            self.painted = Some(target);
            return (Some(target), false);
        }

        let switched_tab = self.active != Some(active);
        let target_moved = self.to.is_some_and(|to| !same_rect(to, target));
        if switched_tab || self.to.is_none() || self.painted.is_none() {
            // Start from where the band actually is. A first measurement has nothing to slide
            // from, so it is adopted at once instead of sweeping in from the strip origin.
            let from = self.painted.unwrap_or(target);
            self.from = Some(from);
            self.to = Some(target);
            self.started_ms = now_ms;
            self.active = Some(active);
        } else if target_moved {
            // Resize, font/DPI change, a count badge widening a label or a scrolled strip: the tab
            // really moved, so the band stays exactly on it rather than animating a layout change.
            self.from = Some(target);
            self.to = Some(target);
            self.started_ms = now_ms;
        }

        let (from, to) = (self.from.unwrap_or(target), self.to.unwrap_or(target));
        let elapsed = now_ms.saturating_sub(self.started_ms);
        let progress = (elapsed as f32 / TAB_UNDERLINE_DURATION_MS as f32).clamp(0.0, 1.0);
        let band = if progress >= 1.0 {
            to
        } else {
            lerp(from, to, ely_core::motion::ease_out_cubic(progress))
        };
        self.painted = Some(band);
        (Some(band), progress < 1.0 && !same_rect(from, to))
    }

    fn animating(&self, now_ms: u64) -> bool {
        match (self.from, self.to) {
            (Some(from), Some(to)) => {
                !same_rect(from, to)
                    && now_ms < self.started_ms.saturating_add(TAB_UNDERLINE_DURATION_MS)
            }
            _ => false,
        }
    }
}

/// The animated underline of one horizontal tab strip. Cheap to clone: every clone shares one
/// measurement/animation state.
#[derive(Clone, Default)]
pub(crate) struct TabUnderline {
    shared: Rc<RefCell<Shared>>,
}

impl TabUnderline {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The invisible measurement element to place inside one tab, covering that tab's box.
    ///
    /// It paints nothing and registers no hitbox, so the tab's own cursor, hover, focus, click and
    /// tooltip keep their exact targets. `index` is the same index the [`layer`](Self::layer) is
    /// given as the active tab.
    pub(crate) fn probe(&self, index: usize) -> AnyElement {
        let shared = self.shared.clone();
        canvas(
            move |bounds: Bounds<Pixels>, _, _| {
                let mut shared = shared.borrow_mut();
                if shared.measured.len() <= index {
                    shared.measured.resize(index + 1, None);
                }
                shared.measured[index] = Some(bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// The painted band. Place it in a `relative()` container that covers the tab row and clips
    /// what must not paint (a scrolled strip), after the tabs themselves in paint order.
    pub(crate) fn layer(
        &self,
        active: usize,
        color: Hsla,
        reduced: bool,
        now_ms: u64,
    ) -> AnyElement {
        let shared = self.shared.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window: &mut Window, _| {
                let (band, moving) = shared.borrow_mut().step(active, now_ms, reduced);
                if let Some(band) = band {
                    window.paint_quad(fill(band, Background::from(color)));
                }
                if moving {
                    // Drive the move from the painter: the selection happened before this frame's
                    // render, so the strip's own render has already run for this frame.
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// A tab label whose layout box is the same whether its tab is selected or not.
    ///
    /// `MEDIUM` glyphs have a wider horizontal advance than `NORMAL`, so switching the weight on
    /// the flow-layout text used to widen the selected tab by a few pixels and push every tab to
    /// its right. The visible label keeps its real weight on top of an invisible sizer carrying the
    /// active weight: the box is always the active-width box, so selection changes only colour,
    /// weight and the shared band — never `origin.x` or `size.width` of any tab.
    ///
    /// The sizer is a zero-height, clipped, fully transparent text node with no id, no role, no
    /// hitbox and no tooltip, so it is neither reachable nor announced; it only contributes its
    /// advance to the parent's intrinsic width.
    pub(crate) fn label(
        text: impl Into<gpui::SharedString>,
        active: bool,
        color: Hsla,
    ) -> AnyElement {
        let text = text.into();
        div()
            .flex()
            .flex_col()
            .items_center()
            .flex_none()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .font_weight(if active {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(color)
                    .child(text.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(px(0.0))
                    .overflow_hidden()
                    .opacity(0.0)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(color)
                    .child(text),
            )
            .into_any_element()
    }

    /// Whether the band still has frames to draw at `now_ms`.
    ///
    /// A strip uses this in its own render as well; the layer also requests its own frames, so the
    /// move cannot stall if a strip forgets. Both stop as soon as the band is settled.
    pub(crate) fn is_animating(&self, now_ms: u64) -> bool {
        self.shared.borrow().animating(now_ms)
    }

    /// The band painted on the last frame, in window coordinates, for renderer tests.
    #[cfg(test)]
    pub(crate) fn painted(&self) -> Option<Bounds<Pixels>> {
        self.shared.borrow().painted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(left: f32, width: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(left), px(100.0)), size(px(width), px(24.0)))
    }

    fn measure(underline: &TabUnderline, tabs: &[Bounds<Pixels>]) {
        let shared = &mut *underline.shared.borrow_mut();
        shared.measured = tabs.iter().copied().map(Some).collect();
    }

    fn band(underline: &TabUnderline, active: usize, now: u64) -> Bounds<Pixels> {
        let (band, _) = underline.shared.borrow_mut().step(active, now, false);
        band.expect("a measured strip paints a band")
    }

    /// The first layout puts the band under the active tab without sweeping in from the origin.
    #[test]
    fn first_layout_adopts_the_measured_tab_and_does_not_request_frames() {
        let underline = TabUnderline::new();
        measure(&underline, &[tab(20.0, 60.0), tab(88.0, 90.0)]);
        let (band, moving) = underline.shared.borrow_mut().step(1, 0, false);
        assert_eq!(
            band,
            Some(Bounds::new(
                point(px(88.0), px(122.0)),
                size(px(90.0), px(2.0))
            ))
        );
        assert!(!moving, "nothing slides on the first layout");
        assert!(!underline.is_animating(0));
    }

    /// Movement interpolates `left` and `width` separately, so unequal labels stretch the band.
    #[test]
    fn a_tab_change_moves_and_resizes_the_band_between_the_two_tabs() {
        let underline = TabUnderline::new();
        measure(&underline, &[tab(20.0, 60.0), tab(88.0, 140.0)]);
        band(&underline, 0, 0);
        // The selection changes on the next frame.
        let start = band(&underline, 1, 16);
        assert_eq!(
            (start.left(), start.size.width),
            (px(20.0), px(60.0)),
            "the band leaves from the tab it was under, not from the new tab"
        );
        let middle = band(&underline, 1, 16 + TAB_UNDERLINE_DURATION_MS / 2);
        assert!(middle.left() > px(20.0) && middle.left() < px(88.0));
        assert!(middle.size.width > px(60.0) && middle.size.width < px(140.0));
        assert!(underline.is_animating(16 + TAB_UNDERLINE_DURATION_MS / 2));
        let end = band(&underline, 1, 16 + TAB_UNDERLINE_DURATION_MS);
        assert_eq!((end.left(), end.size.width), (px(88.0), px(140.0)));
        assert!(!underline.is_animating(16 + TAB_UNDERLINE_DURATION_MS));
    }

    /// Re-selecting while the band is still moving continues from the interpolated band.
    #[test]
    fn an_interrupted_move_restarts_from_the_band_in_flight() {
        let underline = TabUnderline::new();
        measure(
            &underline,
            &[tab(0.0, 40.0), tab(60.0, 80.0), tab(160.0, 40.0)],
        );
        band(&underline, 0, 0);
        band(&underline, 1, 10);
        let halfway = band(&underline, 1, 10 + TAB_UNDERLINE_DURATION_MS / 2);
        let restart = band(&underline, 2, 10 + TAB_UNDERLINE_DURATION_MS / 2);
        assert!(
            (restart.left() - halfway.left()).abs() <= px(0.01)
                && (restart.size.width - halfway.size.width).abs() <= px(0.01),
            "an interrupted move starts where the band was, not at the previous tab"
        );
        // The second move gets its own full duration from the moment it restarted.
        let end = band(
            &underline,
            2,
            10 + TAB_UNDERLINE_DURATION_MS / 2 + TAB_UNDERLINE_DURATION_MS,
        );
        assert_eq!((end.left(), end.size.width), (px(160.0), px(40.0)));
    }

    /// Reduced motion is immediate and never asks for animation frames.
    #[test]
    fn reduced_motion_lands_on_the_tab_at_once() {
        let underline = TabUnderline::new();
        measure(&underline, &[tab(20.0, 60.0), tab(88.0, 90.0)]);
        let (band, _moving) = underline.shared.borrow_mut().step(0, 0, true);
        assert_eq!(band.map(|band| band.left()), Some(px(20.0)));
        let (band, moving) = underline.shared.borrow_mut().step(1, 0, true);
        assert_eq!(
            band.map(|band| (band.left(), band.size.width)),
            Some((px(88.0), px(90.0)))
        );
        assert!(!moving);
        assert!(!underline.is_animating(0));
    }

    /// A layout change on the same tab is adopted instead of animated.
    #[test]
    fn a_resize_font_or_badge_change_is_adopted_without_sliding() {
        let underline = TabUnderline::new();
        measure(&underline, &[tab(20.0, 60.0), tab(88.0, 90.0)]);
        band(&underline, 1, 0);
        // A count badge widens the selected label, and a DPI/resize change shifts the row.
        measure(&underline, &[tab(12.5, 60.0), tab(80.5, 118.0)]);
        let (band, moving) = underline.shared.borrow_mut().step(1, 4, false);
        assert_eq!(
            band.map(|band| (band.left(), band.size.width)),
            Some((px(80.5), px(118.0)))
        );
        assert!(!moving);
    }

    /// A scrolled strip keeps the band on the tab instead of easing after the scroll.
    #[test]
    fn scrolling_the_strip_keeps_the_band_on_its_tab() {
        let underline = TabUnderline::new();
        measure(&underline, &[tab(20.0, 60.0), tab(88.0, 90.0)]);
        band(&underline, 1, 0);
        measure(&underline, &[tab(-40.0, 60.0), tab(28.0, 90.0)]);
        let (band, moving) = underline.shared.borrow_mut().step(1, 4, false);
        assert_eq!(band.map(|band| band.left()), Some(px(28.0)));
        assert!(!moving);
    }

    /// Without a measurement the band is not invented at the origin.
    #[test]
    fn an_unmeasured_strip_paints_nothing() {
        let underline = TabUnderline::new();
        let (band, moving) = underline.shared.borrow_mut().step(0, 0, false);
        assert!(band.is_none());
        assert!(!moving);
    }
}
