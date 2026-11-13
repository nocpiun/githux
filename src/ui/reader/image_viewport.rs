//! Layout-transparent measurement around an existing element's prepaint.
use super::*;
use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, Pixels, Window,
};
use std::{cell::RefCell, rc::Rc};
type Probe = Box<dyn FnOnce(Bounds<Pixels>, &mut Window, &mut App)>;
struct Measured {
    element: AnyElement,
    before: Option<Probe>,
    after: Option<Probe>,
}
fn measured(
    element: AnyElement,
    before: impl 'static + FnOnce(Bounds<Pixels>, &mut Window, &mut App),
    after: impl 'static + FnOnce(Bounds<Pixels>, &mut Window, &mut App),
) -> AnyElement {
    Measured {
        element,
        before: Some(Box::new(before)),
        after: Some(Box::new(after)),
    }
    .into_any_element()
}
pub(crate) fn before(
    element: AnyElement,
    probe: impl 'static + FnOnce(Bounds<Pixels>, &mut Window, &mut App),
) -> AnyElement {
    measured(element, probe, |_, _, _| {})
}
pub(crate) fn viewport(
    element: AnyElement,
    board: Rc<RefCell<ImageBoard>>,
    flow: bool,
) -> AnyElement {
    let finished = board.clone();
    measured(
        element,
        move |bounds, window, _| {
            board
                .borrow_mut()
                .begin_viewport(bounds, window.content_mask().bounds, flow)
        },
        move |_, _, _| finished.borrow_mut().finish_demand_frame(),
    )
}
impl IntoElement for Measured {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Measured {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
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
        (self.element.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.before.take().expect("measurement prepaint once")(bounds, window, cx);
        self.element.prepaint(window, cx);
        self.after.take().expect("measurement prepaint once")(bounds, window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(window, cx);
    }
}
