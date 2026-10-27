//! Thumb geometry. Uses the *track* length, not the content viewport, so a bar inset
//! from a region's edge measures against the line it actually paints.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thumb {
    pub length: f32,
    pub start: f32,
    pub travel: f32,
    pub reach: f32,
}

impl Thumb {
    /// `None` when there is nothing to paint or nothing to drag: a zero viewport or
    /// range, non-finite input, or a track too short for the minimum thumb to move in.
    pub fn new(viewport: f32, reach: f32, offset: f32, track: f32, min_thumb: f32) -> Option<Self> {
        if ![viewport, reach, offset, track, min_thumb]
            .iter()
            .all(|v| v.is_finite())
            || viewport <= 0.0
            || reach <= 0.0
            || track <= 0.0
        {
            return None;
        }
        let length = (track * (viewport / (viewport + reach)))
            .max(min_thumb)
            .min(track);
        let travel = track - length;
        // A track too short to move cannot provide a working drag target.
        if travel <= 0.0 {
            return None;
        }
        Some(Self {
            length,
            travel,
            reach,
            start: travel * (-offset / reach).clamp(0.0, 1.0),
        })
    }

    /// The offset a thumb dragged to `start` represents. Clamped to the track, so a drag
    /// past either end cannot leave the range.
    pub fn offset_at(self, start: f32) -> f32 {
        if !start.is_finite() {
            return 0.0;
        }
        -self.reach * (start / self.travel).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degenerate_inputs_paint_nothing() {
        let cases = [
            (0.0, 10.0, 0.0, 100.0),
            (100.0, 0.0, 0.0, 100.0),
            (100.0, -1.0, 0.0, 100.0),
            (100.0, 10.0, 0.0, 0.0),
            (100.0, 500.0, 0.0, 12.0),
            (100.0, 10.0, 0.0, -4.0),
        ];
        for (viewport, reach, offset, track) in cases {
            assert!(
                Thumb::new(viewport, reach, offset, track, MIN).is_none(),
                "{viewport} {reach} {offset} {track}"
            );
        }
        assert!(Thumb::new(f32::NAN, 1.0, 0.0, 100.0, MIN).is_none());
        assert!(Thumb::new(100.0, 1.0, f32::INFINITY, 100.0, MIN).is_none());
    }

    #[test]
    fn proportional_length_position_and_end_clamp() {
        let t = Thumb::new(200.0, 800.0, -400.0, 180.0, MIN).unwrap();
        assert_eq!(t.length, 36.0);
        assert_eq!(t.travel, 144.0);
        assert_eq!(t.start, 72.0);
        assert_eq!(t.offset_at(-100.0), 0.0);
        assert_eq!(t.offset_at(500.0), -800.0);
        assert_eq!(t.offset_at(f32::NAN), 0.0);
    }

    #[test]
    fn minimum_thumb_and_a_shrinking_range_stay_inside_the_track() {
        let t = Thumb::new(100.0, 10_000.0, 0.0, 300.0, MIN).unwrap();
        assert_eq!(t.length, MIN);
        let shrunk = Thumb::new(400.0, 10.0, -400.0, 380.0, MIN).unwrap();
        assert_eq!(shrunk.start, shrunk.travel);
        assert_eq!(shrunk.offset_at(500.0), -10.0);
        let tiny = Thumb::new(1.0, 10_000.0, 0.0, 25.0, MIN).unwrap();
        assert_eq!(tiny.length, 24.0);
        assert_eq!(tiny.offset_at(1.0), -10_000.0);
    }

    const MIN: f32 = 24.0;
}
