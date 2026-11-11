//! Conservative resvg scratch-surface budget, checked before painting.
//! resvg bounds isolated layers to five canvas widths/heights, but that can
//! still exceed the image budget, and nested opacity/filter groups retain
//! their parent surfaces while rendering children.
use resvg::usvg;

pub(super) fn pixels(tree: &usvg::Tree, width: u32, height: u32, embedded: u64) -> u64 {
    fn group_cost(
        group: &usvg::Group,
        sx: f64,
        sy: f64,
        width: u32,
        height: u32,
        depth: usize,
        nodes: &mut usize,
    ) -> u64 {
        *nodes += 1;
        if depth > 256 || *nodes > 50_000 {
            return u64::MAX;
        }
        let mut child_cost = 0;
        for node in group.children() {
            let cost = match node {
                usvg::Node::Group(group) => {
                    group_cost(group, sx, sy, width, height, depth + 1, nodes)
                }
                _ => {
                    *nodes += 1;
                    0
                }
            };
            let mut resources = 0;
            node.subroots(|root| {
                resources = resources.max(group_cost(root, sx, sy, width, height, depth + 1, nodes))
            });
            child_cost = child_cost.max(cost.saturating_add(resources));
        }
        if *nodes > 50_000 {
            return u64::MAX;
        }
        if !group.should_isolate() {
            return child_cost;
        }
        let bbox = group.abs_layer_bounding_box();
        let w = ((f64::from(bbox.width()) * sx).ceil() + 4.0)
            .min(f64::from(width) * 5.0 + 4.0)
            .max(1.0) as u64;
        let h = ((f64::from(bbox.height()) * sy).ceil() + 4.0)
            .min(f64::from(height) * 5.0 + 4.0)
            .max(1.0) as u64;
        let primitives = group
            .filters()
            .iter()
            .map(|f| f.primitives().len() as u64)
            .sum::<u64>();
        // Filter result graphs keep intermediate images. Charge four surfaces
        // per primitive plus working copies; clip/mask surfaces are extra.
        let factor = if group.filters().is_empty() {
            1
        } else {
            8 + primitives.saturating_mul(4)
        } + if group.clip_path().is_some() { 3 } else { 0 }
            + if group.mask().is_some() { 4 } else { 0 };
        w.saturating_mul(h)
            .saturating_mul(factor)
            .saturating_add(child_cost)
    }
    let sx = f64::from(width) / f64::from(tree.size().width());
    let sy = f64::from(height) / f64::from(tree.size().height());
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_add(embedded.saturating_mul(2))
        .saturating_add(group_cost(tree.root(), sx, sy, width, height, 0, &mut 0))
}
