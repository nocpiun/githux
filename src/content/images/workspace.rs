//! Conservative resvg scratch-surface budget, checked before painting.
//! Isolated groups and nested vector images retain their parent destinations.
use resvg::usvg;

#[derive(Clone, Copy)]
struct Canvas {
    width: u32,
    height: u32,
}
impl Canvas {
    fn pixels(self) -> u64 {
        u64::from(self.width).saturating_mul(u64::from(self.height))
    }
}
#[derive(Clone, Copy)]
struct Space {
    // A subroot's absolute coordinates belong to its own SVG tree. This prefix
    // carries the outer image's fit/ancestor transforms into destination pixels.
    transform: usvg::Transform,
    limit: Canvas,
    target: Canvas,
}

fn surface(group: &usvg::Group, space: Space) -> Option<(Canvas, u64)> {
    if !group.should_isolate() {
        return Some((space.target, 0));
    }
    let bbox = group.abs_layer_bounding_box().transform(space.transform)?;
    let full_width = f64::from(bbox.width()).ceil() + 4.0;
    let full_height = f64::from(bbox.height()).ceil() + 4.0;
    let width = full_width
        .min(f64::from(space.limit.width) * 5.0 + 4.0)
        .max(1.0) as u32;
    let height = full_height
        .min(f64::from(space.limit.height) * 5.0 + 4.0)
        .max(1.0) as u32;
    let canvas = Canvas { width, height };
    let primitives = group
        .filters()
        .iter()
        .map(|f| f.primitives().len() as u64)
        .sum::<u64>();
    // This bounded chain holds the layer, SourceGraphic copy, noise result,
    // noise working copy and output. Other graphs keep the generic estimate.
    let displacement_chain = group.filters().len() == 1
        && {
            let primitives = group.filters()[0].primitives();
            primitives.len() == 2
                && matches!(primitives[0].kind(), usvg::filter::Kind::Turbulence(_))
                && match primitives[1].kind() {
                    usvg::filter::Kind::DisplacementMap(map) => {
                        matches!(map.input1(), usvg::filter::Input::SourceGraphic)
                            && matches!(map.input2(),usvg::filter::Input::Reference(name) if name == primitives[0].result())
                    }
                    _ => false,
                }
        };
    let factor = if group.filters().is_empty() {
        1
    } else if displacement_chain {
        5
    } else {
        8 + primitives.saturating_mul(4)
    } + if group.clip_path().is_some() { 3 } else { 0 }
        + if group.mask().is_some() { 4 } else { 0 };
    // The group layer is clipped to the render context. Filter primitives
    // allocate their full transformed region separately (filter::apply_inner),
    // so their working surfaces must not inherit that layer-size clamp.
    let working = if group.filters().is_empty() {
        canvas.pixels()
    } else {
        (full_width as u64)
            .saturating_mul(full_height as u64)
            .max(canvas.pixels())
    };
    Some((canvas, working.saturating_mul(factor)))
}

fn group_cost(group: &usvg::Group, space: Space, depth: usize, nodes: &mut usize) -> u64 {
    *nodes += 1;
    if depth > 256 || *nodes > 50_000 {
        return u64::MAX;
    }
    let Some((target, own)) = surface(group, space) else {
        return u64::MAX;
    };
    let child_space = Space { target, ..space };
    let mut child_cost = 0;
    for node in group.children() {
        if let usvg::Node::Image(image) = node
            && let usvg::ImageKind::SVG(tree) = image.kind()
        {
            *nodes += 1;
            if !image.is_visible() {
                continue;
            }
            // resvg::image::render_vector allocates the full current
            // destination, not the resource's intrinsic width/height. Each
            // nested image keeps this canvas alive while its tree renders.
            let nested = Space {
                transform: space.transform.pre_concat(image.abs_transform()),
                limit: target,
                target,
            };
            child_cost = child_cost.max(target.pixels().saturating_add(group_cost(
                tree.root(),
                nested,
                depth + 1,
                nodes,
            )));
            continue;
        }
        let cost = match node {
            usvg::Node::Group(group) => group_cost(group, child_space, depth + 1, nodes),
            _ => {
                *nodes += 1;
                0
            }
        };
        let mut resources = 0;
        let resource_target = if let usvg::Node::Group(group) = node {
            let Some((mut target, _)) = surface(group, child_space) else {
                return u64::MAX;
            };
            if !group.filters().is_empty() {
                // feImage subroots render into the full filter region, which
                // can exceed the clipped group layer used for its children.
                let Some(bbox) = group.abs_layer_bounding_box().transform(space.transform) else {
                    return u64::MAX;
                };
                target.width = target
                    .width
                    .max((f64::from(bbox.width()).ceil() + 4.0) as u32);
                target.height = target
                    .height
                    .max((f64::from(bbox.height()).ceil() + 4.0) as u32);
            }
            target
        } else {
            target
        };
        let resource_space = Space {
            transform: if matches!(node, usvg::Node::Text(_)) {
                // Flattened text remains in this SVG tree's absolute space.
                space.transform
            } else {
                space.transform.pre_concat(node.abs_transform())
            },
            limit: resource_target,
            target: resource_target,
        };
        node.subroots(|root| {
            resources = resources.max(group_cost(root, resource_space, depth + 1, nodes));
        });
        child_cost = child_cost.max(cost.saturating_add(resources));
    }
    if *nodes > 50_000 {
        u64::MAX
    } else {
        own.saturating_add(child_cost)
    }
}

pub(super) fn pixels(tree: &usvg::Tree, width: u32, height: u32, embedded: u64) -> u64 {
    let canvas = Canvas { width, height };
    let space = Space {
        transform: usvg::Transform::from_scale(
            width as f32 / tree.size().width(),
            height as f32 / tree.size().height(),
        ),
        limit: canvas,
        target: canvas,
    };
    canvas
        .pixels()
        .saturating_add(embedded.saturating_mul(2))
        .saturating_add(group_cost(tree.root(), space, 0, &mut 0))
}
