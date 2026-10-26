//! Region-local visibility, with no standing timer.

use super::FADE_MS;

#[derive(Debug, Default)]
pub struct Fade {
    hovered: bool,
    dragging: bool,
    held: bool,
    painted: f32,
    run: Option<Run>,
}

#[derive(Clone, Copy, Debug)]
struct Run {
    from: f32,
    to: f32,
    started: u64,
}

impl Fade {
    fn resting(&self) -> f32 {
        if self.hovered || self.dragging || self.held {
            1.0
        } else {
            0.0
        }
    }

    pub fn set_hovered(&mut self, value: bool, now: u64, reduce_motion: bool) -> bool {
        if self.hovered == value {
            return false;
        }
        self.hovered = value;
        self.retarget(now, reduce_motion);
        true
    }

    pub fn set_dragging(&mut self, value: bool, now: u64, reduce_motion: bool) -> bool {
        if self.dragging == value {
            return false;
        }
        self.dragging = value;
        self.retarget(now, reduce_motion);
        true
    }

    /// An explicit visibility hold (e.g. tab reordering), independent of a thumb drag.
    pub fn set_held(&mut self, value: bool, now: u64, reduce_motion: bool) -> bool {
        if self.held == value {
            return false;
        }
        self.held = value;
        self.retarget(now, reduce_motion);
        true
    }

    /// Starts a fade from the opacity currently painted, so a reversal continues instead
    /// of jumping to an end point. Reduced motion jumps straight to the target instead.
    fn retarget(&mut self, now: u64, reduce_motion: bool) {
        let to = self.resting();
        if self.run.is_some_and(|run| run.to == to) || (self.run.is_none() && self.painted == to) {
            return;
        }
        if reduce_motion {
            self.painted = to;
            self.run = None;
            return;
        }
        self.run = (self.painted != to).then_some(Run {
            from: self.painted,
            to,
            started: now,
        });
    }

    pub fn opacity(&mut self, now: u64) -> f32 {
        if let Some(run) = self.run {
            let t = (now.saturating_sub(run.started) as f32 / FADE_MS as f32).clamp(0.0, 1.0);
            self.painted = run.from + (run.to - run.from) * t * t * (3.0 - 2.0 * t);
            if t >= 1.0 {
                self.painted = run.to;
                self.run = None;
            }
        }
        self.painted
    }

    pub fn is_animating(&self) -> bool {
        self.run.is_some()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    pub fn snap(&mut self, showing: bool) {
        self.hovered = showing;
        self.painted = self.resting();
        self.run = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_reverses_from_painted_value_and_stops_requesting_frames() {
        let mut f = Fade::default();
        assert!(!f.is_animating());
        f.set_hovered(true, 0, false);
        assert!(!f.set_hovered(true, 20, false));
        let mid = f.opacity(35);
        assert_eq!(mid, 0.15625);
        f.set_hovered(false, 35, false);
        assert_eq!(f.opacity(35), mid);
        assert!(f.opacity(70) < mid);
        assert_eq!(f.opacity(175), 0.0);
        assert!(!f.is_animating());
        f.set_dragging(true, 200, false);
        f.set_hovered(true, 210, false);
        assert_eq!(f.opacity(340), 1.0);
        f.set_hovered(false, 350, false);
        assert!(
            !f.is_animating(),
            "unchanged target must not request frames"
        );
        f.set_held(true, 350, false);
        f.set_dragging(false, 360, false);
        assert_eq!(f.opacity(500), 1.0);
        f.set_held(false, 510, false);
        assert_eq!(f.opacity(650), 0.0);
        assert!(!f.is_animating());
    }

    #[test]
    fn reduced_motion_jumps_and_never_requests_a_frame() {
        let mut f = Fade::default();
        assert!(f.set_hovered(true, 0, true));
        assert_eq!(f.opacity(0), 1.0);
        assert!(!f.is_animating());
        assert!(f.set_hovered(false, 10, true));
        assert_eq!(f.opacity(10), 0.0);
        assert!(!f.is_animating());
        // A reversal already in flight is finished by the next reduced-motion change.
        f.set_hovered(true, 20, false);
        assert!(f.opacity(25) > 0.0 && f.opacity(25) < 1.0);
        f.set_hovered(false, 30, true);
        assert_eq!(f.opacity(30), 0.0);
        assert!(!f.is_animating());
    }
}
