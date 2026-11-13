//! Demand is derived from measured intersections, never from table construction.
use super::*;
#[derive(Default)]
pub(super) struct Demand {
    pub priority: u8,
    pub last_used: u64,
    epoch: u64,
    attempted_epoch: u64,
}
impl ImageBoard {
    pub(crate) fn manage_geometry(&mut self) {
        self.geometry_managed = true;
    }
    pub(crate) fn begin_viewport(
        &mut self,
        bounds: gpui::Bounds<gpui::Pixels>,
        mask: gpui::Bounds<gpui::Pixels>,
        flow: bool,
    ) {
        self.begin_demand_frame();
        self.viewport = Some(if flow {
            let left = bounds.left().max(mask.left());
            let right = bounds.right().min(mask.right());
            gpui::Bounds::new(
                gpui::point(left, mask.top()),
                gpui::size((right - left).max(px(0.0)), mask.size.height),
            )
        } else {
            bounds.intersect(&mask)
        });
    }
    pub(crate) fn observe_bounds(
        &mut self,
        key: ImageKey,
        bounds: gpui::Bounds<gpui::Pixels>,
        hints: ImageHints,
    ) {
        let Some(viewport) = self.viewport else {
            return;
        };
        if viewport.size.width <= px(0.0) || viewport.size.height <= px(0.0) {
            return;
        }
        let horizontal = bounds.right() >= viewport.left() && bounds.left() <= viewport.right();
        let visible =
            horizontal && bounds.bottom() >= viewport.top() && bounds.top() <= viewport.bottom();
        let margin = viewport.size.height * 0.75;
        let near = horizontal
            && bounds.bottom() >= viewport.top() - margin
            && bounds.top() <= viewport.bottom() + margin;
        let priority = if visible {
            2
        } else if near {
            1
        } else {
            0
        };
        let available = if bounds.size.width > px(0.0) {
            f32::from(bounds.size.width)
        } else {
            f32::from(viewport.size.width)
        };
        let width = self.intrinsic_size(&key).map_or_else(
            || {
                hints
                    .width
                    .map_or(available, |width| available.min(width as f32))
            },
            |(width, height)| available.min(super::measurement::dimensions(width, height, hints).0),
        );
        self.observe_demand(key, priority, width);
    }
    pub(crate) fn has_layout_geometry(&self, key: &ImageKey) -> bool {
        self.states.get(key).is_some_and(ImageState::is_ready)
    }
    pub(crate) fn begin_demand_frame(&mut self) {
        self.observations.clear();
        self.observation_order.clear();
    }
    /// 2 = visible, 1 = near the viewport, 0 = outside. A zero-height
    /// unmeasured image reports its insertion point and available column width.
    pub(crate) fn observe_demand(&mut self, key: ImageKey, priority: u8, width: f32) {
        if !self.observations.contains_key(&key) {
            self.observation_order.push(key.clone());
        }
        let observation = self.observations.entry(key).or_default();
        observation.0 = observation.0.max(priority);
        observation.1 = observation.1.max(width);
    }
    pub(crate) fn finish_demand_frame(&mut self) {
        if !self.geometry_managed {
            return;
        }
        let mut keys = self.observation_order.clone();
        keys.extend(
            self.targets
                .keys()
                .filter(|key| !self.observations.contains_key(*key))
                .cloned(),
        );
        let usable_viewport = self
            .viewport
            .is_none_or(|bounds| bounds.size.width > px(0.0) && bounds.size.height > px(0.0));
        for key in keys {
            // A virtual list may skip zero-height rows before its anchor.
            // Missing measurement is not proof that an admitted, still
            // unmeasured load left demand. Explicit outside bounds still cancel.
            if usable_viewport
                && !self.observations.contains_key(&key)
                && matches!(self.states.get(&key), Some(ImageState::Loading))
                && (self.requests.contains_key(&key)
                    || self.pending.iter().any(|request| request.key == key))
            {
                continue;
            }
            let (priority, width) = self.observations.get(&key).copied().unwrap_or_default();
            let demand = self.demand.entry(key.clone()).or_default();
            if priority > 0 && demand.priority == 0 {
                demand.epoch = demand.epoch.wrapping_add(1);
            }
            demand.priority = priority;
            if priority == 0 {
                let queued = self.pending.iter().any(|request| request.key == key);
                if let Some(sink) = self.request_sinks.remove(&key) {
                    sink.cancel();
                }
                if self.requests.remove(&key).is_some() {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.finish_animation(&key);
                    self.attempted_options.remove(&key);
                    self.requested_options.remove(&key);
                } else if queued {
                    self.attempted_options.remove(&key);
                    self.requested_options.remove(&key);
                }
                continue;
            }
            self.use_clock = self.use_clock.wrapping_add(1);
            demand.last_used = self.use_clock;
            let options = self.render_options.with_display_width(width);
            let required_grid = self
                .sizes
                .get(&key)
                .and_then(|(native, _)| options.raster_width().map(|width| width.min(*native)));
            let scale_upgrade = self
                .completed_options
                .get(&key)
                .is_some_and(|known| known.device_scale() < options.device_scale());
            let grid_upgrade = required_grid.is_some_and(|required| {
                self.grids.get(&key).is_some_and(|known| *known < required)
            });
            let state = self.states.get(&key);
            let unfinished = matches!(state, Some(ImageState::Loading));
            let released = matches!(state, Some(ImageState::Released))
                && demand.attempted_epoch != demand.epoch;
            // Initial layout and column changes can leave a much larger
            // raster than the final display grid. Rebuild from original bytes
            // after a 25% width reduction; small rounding changes do not churn.
            // SVG/GIF/WebP timelines never enter this shrinking path.
            let shrink_raster = self.static_rasters.contains(&key)
                && required_grid.is_some_and(|required| {
                    self.grids
                        .get(&key)
                        .is_some_and(|known| u64::from(required) * 4 <= u64::from(*known) * 3)
                });
            let upgrade = state.is_some_and(ImageState::is_ready)
                && ((self.density_dependent.contains(&key) && (scale_upgrade || grid_upgrade))
                    || shrink_raster)
                && self
                    .attempted_options
                    .get(&key)
                    .is_none_or(|known| *known != options);
            if self.attached
                && (unfinished || released || upgrade)
                && !self.requests.contains_key(&key)
                && !self.pending.iter().any(|r| r.key == key)
            {
                demand.attempted_epoch = demand.epoch;
                if released {
                    self.reloads += 1;
                }
                self.requested_options.insert(key.clone(), options);
                self.attempted_options.insert(key.clone(), options);
                self.pending.push_back(ImageRequest {
                    scope: self.scope(),
                    key: key.clone(),
                    target: self.targets[&key].clone(),
                });
            }
        }
        // A queued image that left the viewport consumes no admission. Its
        // insertion point will be measured again on renewed demand.
        self.pending
            .retain(|r| self.demand.get(&r.key).is_some_and(|d| d.priority > 0));
    }
    pub(crate) fn options_for(&self, key: &ImageKey) -> ImageRenderOptions {
        self.requested_options
            .get(key)
            .copied()
            .unwrap_or(self.render_options)
    }
    pub(crate) fn intrinsic_size(&self, key: &ImageKey) -> Option<(u32, u32)> {
        self.sizes.get(key).copied()
    }
}
