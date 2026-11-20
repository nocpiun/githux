//! One selection background painted behind the sidebar's navigation and repository rows.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AnyElement, BorderStyle, Bounds, ContentMask, Edges, Hsla, IntoElement, Pixels, ScrollHandle,
    Styled, canvas, point, px, size, transparent_black,
};

use super::geometry::SIDEBAR_PADDING;

pub(super) const DURATION_MS: u64 = 180;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Item {
    Home,
    Settings,
    Repo(u64),
}

impl Item {
    fn is_repo(self) -> bool {
        matches!(self, Self::Repo(_))
    }
}

type Rect = Bounds<Pixels>;
type Measurement = (Item, Rect);

#[derive(Clone, Copy, PartialEq)]
struct Viewport {
    bounds: Rect,
    offset: Pixels,
}

struct Slide {
    from: Rect,
    started: u64,
    crosses_heading: bool,
}

struct Frame {
    target: Measurement,
    viewport: Viewport,
    scale: f32,
    painted: Rect,
    slide: Option<Slide>,
}

#[derive(Clone, Default)]
pub(super) struct SidebarActive {
    previous: Rc<RefCell<Option<Frame>>>,
}

impl SidebarActive {
    pub fn reset(&self) {
        self.previous.borrow_mut().take();
    }

    pub fn measure(&self, active: Option<Item>) -> Measurements {
        Measurements {
            active,
            previous: self.previous.borrow().as_ref().map(|frame| frame.target.0),
            rows: Rc::default(),
        }
    }

    pub fn layer(&self, measurements: Measurements, style: LayerStyle) -> AnyElement {
        let previous = self.previous.clone();
        canvas(
            |_, _, _| {},
            move |layer_bounds, _, window, _| {
                let rows = measurements.rows.borrow();
                let mut previous = previous.borrow_mut();
                let prior = previous.take();
                let Some(target @ (item, bounds)) = rows.active else {
                    return;
                };
                let viewport = Viewport {
                    bounds: style.scroll.bounds(),
                    offset: style.scroll.offset().y,
                };
                let mask = window.content_mask().bounds.intersect(&layer_bounds);
                let repo_clip = mask.intersect(&viewport.bounds);
                let target_clip = if item.is_repo() { repo_clip } else { mask };
                if !has_area(target_clip.intersect(&bounds)) {
                    return;
                }

                let scale = window.scale_factor();
                let mut slide = prior.and_then(|mut prior| {
                    if style.reduced
                        || prior.scale != scale
                        || rows.previous != Some(prior.target)
                        || (prior.viewport != viewport
                            && (item.is_repo()
                                || prior.target.0.is_repo()
                                || prior
                                    .slide
                                    .as_ref()
                                    .is_some_and(|slide| slide.crosses_heading)))
                    {
                        return None;
                    }
                    if prior.target.0 == item {
                        return prior.slide.take();
                    }
                    Some(Slide {
                        from: prior.painted,
                        started: style.now,
                        crosses_heading: prior.target.0.is_repo() != item.is_repo()
                            || prior.slide.is_some_and(|slide| slide.crosses_heading),
                    })
                });
                let (painted, clip) = match &slide {
                    Some(motion) if style.now.saturating_sub(motion.started) < DURATION_MS => {
                        let elapsed = style.now.saturating_sub(motion.started);
                        let progress =
                            ely_core::motion::ease_out_cubic(elapsed as f32 / DURATION_MS as f32);
                        // Transitions between fixed and scrolling rows can cross the heading.
                        // Their lower edge still stops at the real repository viewport boundary.
                        let clip = if motion.crosses_heading {
                            mask.intersect(&Bounds::from_corners(
                                mask.origin,
                                point(mask.right(), viewport.bounds.bottom()),
                            ))
                        } else {
                            target_clip
                        };
                        (interpolate(motion.from, bounds, progress), clip)
                    }
                    _ => {
                        slide = None;
                        (bounds, target_clip)
                    }
                };
                if has_area(clip.intersect(&painted)) {
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        window.paint_quad(gpui::quad(
                            painted,
                            style.radius,
                            if item.is_repo() {
                                style.repo_color
                            } else {
                                style.nav_color
                            },
                            Edges::default(),
                            transparent_black(),
                            BorderStyle::Solid,
                        ));
                    });
                }
                if slide.is_some() {
                    window.request_animation_frame();
                }
                *previous = Some(Frame {
                    target,
                    viewport,
                    scale,
                    painted,
                    slide,
                });
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }
}

pub(super) struct LayerStyle {
    pub scroll: ScrollHandle,
    pub radius: Pixels,
    pub reduced: bool,
    pub now: u64,
    pub nav_color: Hsla,
    pub repo_color: Hsla,
}

#[derive(Default)]
struct Rows {
    active: Option<Measurement>,
    previous: Option<Measurement>,
}

pub(super) struct Measurements {
    active: Option<Item>,
    previous: Option<Item>,
    rows: Rc<RefCell<Rows>>,
}

impl Measurements {
    /// Measure the selected row and its predecessor during prepaint. The probe covers the same
    /// inset background box as the row and is a sibling, so row borders cannot shrink it.
    pub fn probe(&self, item: Item) -> Option<AnyElement> {
        let active = self.active == Some(item);
        let previous = self.previous == Some(item);
        if !active && !previous {
            return None;
        }
        let rows = self.rows.clone();
        Some(
            canvas(
                move |bounds, _, _| {
                    let mut rows = rows.borrow_mut();
                    if active {
                        rows.active = Some((item, bounds));
                    }
                    if previous {
                        rows.previous = Some((item, bounds));
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
            .left(px(SIDEBAR_PADDING))
            .into_any_element(),
        )
    }
}

fn has_area(bounds: Rect) -> bool {
    bounds.size.width > Pixels::ZERO && bounds.size.height > Pixels::ZERO
}

fn interpolate(from: Rect, to: Rect, t: f32) -> Rect {
    let mix = |a: Pixels, b: Pixels| a + (b - a) * t;
    Bounds::new(
        point(mix(from.left(), to.left()), mix(from.top(), to.top())),
        size(
            mix(from.size.width, to.size.width),
            mix(from.size.height, to.size.height),
        ),
    )
}
