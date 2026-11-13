use super::*;
impl ImageBoard {
    pub(crate) fn request_sink(&self, key: &ImageKey) -> ImageSink {
        self.request_sinks
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.sink.clone())
    }

    pub(crate) fn cancel_request_sinks(&mut self) {
        for (_, sink) in self.request_sinks.drain() {
            sink.cancel();
        }
        drop(self.sink.drain_events());
    }

    pub(crate) fn apply_event(&mut self, event: ImageEvent) -> Vec<usize> {
        if event.scope != self.scope() {
            return Vec::new();
        }
        let Some((request, phase)) = self.requests.get_mut(&event.key) else {
            return Vec::new();
        };
        if event.request != *request || event.phase != *phase {
            return Vec::new();
        }
        *phase += 1;
        if event.terminal {
            if let Some(sink) = self.request_sinks.remove(&event.key) {
                sink.cancel();
            }
            self.requests.remove(&event.key);
            self.in_flight = self.in_flight.saturating_sub(1);
        }
        if event
            .validity
            .as_ref()
            .is_some_and(|guard| !guard.is_current())
        {
            return self.revoke(&event.key);
        }
        if let Some(validity) = event.validity {
            self.validity.insert(event.key.clone(), validity);
        }
        match event.result {
            Ok(ImageUpdate::Ready(decoded)) => {
                let in_flight = self.in_flight;
                let affected = self.apply(ImageCompletion {
                    scope: event.scope,
                    key: event.key,
                    result: Ok(decoded),
                });
                self.in_flight = in_flight;
                affected
            }
            Ok(ImageUpdate::Frames(frames)) => {
                if let Some(playback) = self.animations.get_mut(&event.key) {
                    for frame in frames {
                        if playback
                            .frames
                            .last()
                            .is_some_and(|(at, _)| *at >= frame.at_ms)
                        {
                            continue;
                        }
                        let cost = frame.pixels.bgra.len();
                        playback.extra_bytes += cost;
                        self.ready_bytes += cost;
                        *self.ready_costs.entry(event.key.clone()).or_default() += cost;
                        playback
                            .frames
                            .push((frame.at_ms, decoded_pixels_source(frame.pixels)));
                    }
                }
                Vec::new()
            }
            Ok(ImageUpdate::Discard) => self.revoke(&event.key),
            Ok(ImageUpdate::Validated) => {
                if self.is_ready(&event.key) {
                    return Vec::new();
                }
                // Expiry may already have retired the provisional pixels.
                // Complete this validation and ask the provider for the now
                // validated representation, respecting measured demand.
                if let Some(sink) = self.request_sinks.remove(&event.key) {
                    sink.cancel();
                }
                if self.requests.remove(&event.key).is_some() {
                    self.in_flight = self.in_flight.saturating_sub(1);
                }
                self.states.insert(event.key.clone(), ImageState::Loading);
                self.attempted_options.remove(&event.key);
                if !self.geometry_managed
                    && self.attached
                    && !self.pending.iter().any(|r| r.key == event.key)
                {
                    self.pending.push_back(ImageRequest {
                        scope: self.scope(),
                        key: event.key.clone(),
                        target: self.targets[&event.key].clone(),
                    });
                }
                self.blocks.get(&event.key).cloned().unwrap_or_default()
            }
            Ok(ImageUpdate::Finished) => Vec::new(),
            Err(error) => {
                if matches!(
                    error,
                    ImageFailure::StaleContext | ImageFailure::NotFound | ImageFailure::Invalidated
                ) {
                    let affected = self.revoke(&event.key);
                    self.states.insert(event.key, ImageState::Failed(error));
                    return affected;
                }
                if self
                    .states
                    .get(&event.key)
                    .is_some_and(ImageState::is_ready)
                {
                    let animated = self.animations.contains_key(&event.key);
                    self.finish_animation(&event.key);
                    if animated {
                        self.blocks.get(&event.key).cloned().unwrap_or_default()
                    } else {
                        Vec::new()
                    }
                } else {
                    let in_flight = self.in_flight;
                    let affected = self.apply(ImageCompletion {
                        scope: event.scope,
                        key: event.key,
                        result: Err(error),
                    });
                    self.in_flight = in_flight;
                    affected
                }
            }
        }
    }
    fn revoke(&mut self, key: &ImageKey) -> Vec<usize> {
        self.finish_animation(key);
        if let Some(ImageState::Ready(image, ..)) = self.states.get(key) {
            self.retire_source(image.clone());
        }
        self.animations.remove(key);
        self.ready_bytes = self
            .ready_bytes
            .saturating_sub(self.ready_costs.remove(key).unwrap_or(0));
        self.ready_order.retain(|known| known != key);
        self.validity.remove(key);
        self.states
            .insert(key.clone(), ImageState::Failed(ImageFailure::StaleContext));
        self.blocks.get(key).cloned().unwrap_or_default()
    }
    pub(crate) fn revoke_invalid(&mut self) -> Vec<usize> {
        let invalid = self
            .validity
            .iter()
            .filter(|(_, guard)| !guard.is_current())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        invalid
            .into_iter()
            .flat_map(|key| self.revoke(&key))
            .collect()
    }
}
