//! Physical density of an image consumer, independent of logical author size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImageRenderOptions {
    device_scale_milli: u16,
}
impl ImageRenderOptions {
    pub fn for_device_scale(scale: f32) -> Self {
        let scale = if scale.is_finite() {
            scale.clamp(1.0, 4.0)
        } else {
            1.0
        };
        Self {
            device_scale_milli: (scale * 1000.0).ceil() as u16,
        }
    }
    pub fn device_scale(self) -> f32 {
        f32::from(self.device_scale_milli) / 1000.0
    }
    pub fn final_scale(self) -> f32 {
        self.device_scale() * 2.0
    }
}
impl Default for ImageRenderOptions {
    fn default() -> Self {
        Self::for_device_scale(1.0)
    }
}
