//! Invisible measurement hooks. Pending images add no flex child or gap.
use super::*;
use gpui::{Bounds, Pixels};
use std::{cell::Cell, rc::Rc};
pub(crate) type Visibility = Option<(Rc<std::cell::RefCell<ImageBoard>>, ImageKey)>;
pub(crate) type PendingDemand = (ImageKey, ImageHints, usize);

pub(crate) fn dimensions(width: u32, height: u32, hints: ImageHints) -> (f32, f32) {
    let width = width as f32;
    let height = (height as f32).max(1.0);
    let ratio = width / height;
    let w = hints.width.map(|value| (value as f32).min(width).max(1.0));
    let h = hints
        .height
        .map(|value| (value as f32).min(height).max(1.0));
    match (w, h) {
        (Some(w), Some(h)) => (w, w / h),
        (Some(w), None) => (w, ratio),
        (None, Some(h)) => (h * ratio, ratio),
        _ => (width, ratio),
    }
}

pub(crate) fn measured(picture: AnyElement, visibility: Visibility, animate: bool) -> AnyElement {
    div()
        .max_w_full()
        .child(picture)
        .on_children_prepainted(move |bounds, window, _| {
            if let Some((board, key)) = &visibility {
                for bounds in bounds {
                    board
                        .borrow_mut()
                        .observe_bounds(key.clone(), bounds, ImageHints::default());
                    let visible = window.content_mask().bounds.intersect(&bounds);
                    if animate && visible.size.width > px(0.0) && visible.size.height > px(0.0) {
                        board.borrow_mut().painted(key);
                    }
                }
            }
        })
        .into_any_element()
}

/// `position` counts real children before an omitted image. This wrapper
/// forwards the same layout id and adds no child, width, height or flex gap.
pub(crate) fn pending_container(
    content: gpui::Div,
    board: Option<Rc<std::cell::RefCell<ImageBoard>>>,
    pending: Vec<PendingDemand>,
) -> AnyElement {
    let Some(board) = board.filter(|_| !pending.is_empty()) else {
        return content.into_any_element();
    };
    let anchor = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let measured_anchor = anchor.clone();
    let content = content.on_children_prepainted(move |children, _, _| {
        let anchor = anchor.get();
        for (key, hints, position) in &pending {
            let y = if *position == 0 {
                anchor.top()
            } else {
                children
                    .get(*position - 1)
                    .map_or(anchor.bottom(), |bounds| bounds.bottom())
            };
            let point = Bounds::new(
                gpui::point(anchor.left(), y),
                gpui::size(anchor.size.width, px(0.0)),
            );
            board
                .borrow_mut()
                .observe_bounds(key.clone(), point, *hints);
        }
    });
    super::viewport::before(content.into_any_element(), move |bounds, _, _| {
        measured_anchor.set(bounds)
    })
}
