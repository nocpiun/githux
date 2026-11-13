//! RenderImage bypasses GPUI's asset cache: retire its atlas tiles explicitly.
use super::*;
impl ImageBoard {
    pub(crate) fn retire_source(&mut self, source: ImageSource) {
        match source {
            ImageSource::Render(image) => {
                self.retired.insert(image.id, image);
            }
            source => self.retired_encoded.push(source),
        }
    }
    pub(crate) fn retire_all(&mut self) {
        let mut sources = self
            .states
            .values()
            .filter_map(|state| match state {
                ImageState::Ready(image, ..) => Some(image.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for playback in self.animations.values() {
            sources.push(playback.final_image.clone());
            sources.extend(playback.frames.iter().map(|(_, image)| image.clone()));
        }
        for source in sources {
            self.retire_source(source);
        }
    }
    pub(crate) fn take_retired(&mut self) -> Vec<ImageSource> {
        let mut retired = std::mem::take(&mut self.retired_encoded);
        retired.extend(
            self.retired
                .drain()
                .map(|(_, image)| ImageSource::Render(image)),
        );
        retired
    }
}
pub(crate) fn release_retired(
    sources: Vec<ImageSource>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    for source in sources {
        source.remove_asset(cx);
        if let ImageSource::Render(image) = source {
            let _ = window.drop_image(image);
        }
    }
}
