//! The tab strip's own horizontal scrollbar.
//!
//! Ely's `Scrollbar` supports both explicit presence and activity-driven visibility. Its fixed
//! theme thickness is 6 logical pixels and its activity policy appears immediately, then fades
//! after idle. The accepted strip uses a 3-pixel thumb in a reserved band and a reversible fade
//! on both hover edges. Keep this adapter until upstream exposes those styling/placement hooks;
//! changing the dependency must not silently change the strip's interaction or hit area.
//!
//! Fade behaviour: entering fades in, leaving fades out, each over
//! [`TAB_SCROLLBAR_FADE_MS`]. A reversal starts from the opacity currently painted rather than from
//! an end point, so quick movements do not flash. A thumb drag pins the bar fully visible and hands
//! control back to the hover state on release. Nothing animates once the target is reached, so
//! there is no standing timer, and no element is built at all while the bar is fully hidden — a
//! hidden bar can neither take a press nor a drag.

use ely_core::theme::ActiveTheme;
use gpui::{
    AnyElement, App, EmptyView, IntoElement, ScrollHandle, Styled, Window, div, point, prelude::*,
    px,
};

use super::geometry::{TAB_SCROLLBAR_FADE_MS, TAB_SCROLLBAR_THICKNESS};

/// A callback that reports whether a thumb drag is in progress.
pub type OnDragging = std::rc::Rc<dyn Fn(bool, &mut Window, &mut App) + 'static>;

/// The strip's scrollbar visibility state. Owned by the shell so it survives a frame; the opacity
/// it last painted is what a reversal continues from.
#[derive(Debug, Default)]
pub struct Fade {
    /// Whether the pointer is over the strip.
    hovered: bool,
    /// Whether a thumb drag is in progress, which pins the bar on.
    dragging: bool,
    /// Opacity painted last frame.
    painted: f32,
    /// The fade in progress, if any.
    run: Option<FadeRun>,
}

#[derive(Clone, Copy, Debug)]
struct FadeRun {
    from: f32,
    to: f32,
    started: u64,
}

impl Fade {
    /// How visible the bar should be when nothing is moving.
    fn resting(&self) -> f32 {
        if self.hovered || self.dragging {
            1.0
        } else {
            0.0
        }
    }

    /// Records that the pointer entered or left the strip; true when the target changed.
    pub fn set_hovered(&mut self, hovered: bool, now: u64) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        self.retarget(now);
        true
    }

    /// Records a thumb drag starting or ending; true when the target changed.
    pub fn set_dragging(&mut self, dragging: bool, now: u64) -> bool {
        if self.dragging == dragging {
            return false;
        }
        self.dragging = dragging;
        self.retarget(now);
        true
    }

    /// Starts a fade from the opacity currently painted, so a reversal continues instead of
    /// jumping to an end point.
    fn retarget(&mut self, now: u64) {
        let to = self.resting();
        self.run = Some(FadeRun {
            from: self.painted,
            to,
            started: now,
        });
    }

    /// Advances the fade to `now`; the opacity to paint this frame.
    pub fn opacity(&mut self, now: u64) -> f32 {
        let Some(run) = self.run else {
            return self.painted;
        };
        let t =
            (now.saturating_sub(run.started) as f32 / TAB_SCROLLBAR_FADE_MS as f32).clamp(0.0, 1.0);
        if t >= 1.0 {
            self.painted = run.to;
            self.run = None;
            return self.painted;
        }
        // Smoothstep, so neither end of the fade looks cut.
        let eased = t * t * (3.0 - 2.0 * t);
        self.painted = run.from + (run.to - run.from) * eased;
        self.painted
    }

    pub fn is_animating(&self) -> bool {
        self.run.is_some()
    }

    /// Jumps straight to the resting visibility, skipping the fade. Tests use it when they need
    /// the bar present without stepping a fade; the fade itself is covered separately.
    #[cfg(test)]
    pub fn snap(&mut self, showing: bool) {
        self.hovered = showing;
        self.dragging = self.dragging && showing;
        self.painted = if showing { 1.0 } else { 0.0 };
        self.run = None;
    }

    /// Whether the bar has anything to paint at all.
    #[cfg(test)]
    pub fn is_shown(&self) -> bool {
        self.painted > 0.0 || self.run.is_some()
    }
}

/// One frame of the strip scrollbar: a hairline thumb over the strip's viewport.
///
/// `opacity` is resolved by the shell from [`Fade`], so the fade is a pure value the tests can
/// drive without a window. A fully faded-out bar builds nothing at all, so it can neither take a
/// press nor start a drag.
pub fn render(
    scroll: &ScrollHandle,
    opacity: f32,
    owner: gpui::EntityId,
    min_thumb: f32,
    on_dragging: OnDragging,
    cx: &mut App,
) -> AnyElement {
    if opacity <= 0.0 {
        return div().into_any_element();
    }
    let (reach, offset) = (scroll.max_offset().x, scroll.offset().x);
    let span: f32 = scroll.bounds().size.width.into();
    let reach: f32 = reach.into();
    if reach <= 0.0 || span <= 0.0 {
        return div().into_any_element();
    }
    let length = (span * (span / (span + reach))).max(min_thumb).min(span);
    let travel = span - length;
    let scrolled: f32 = (-offset).into();
    let start = travel * (scrolled / reach).clamp(0.0, 1.0);
    let thickness = px(TAB_SCROLLBAR_THICKNESS);
    let handle = scroll.clone();
    let idle = cx.theme().colors.fg.opacity(0.20);
    let busy = cx.theme().colors.fg.opacity(0.38);

    div()
        .id("tab-scrollbar-track")
        .debug_selector(|| "tab-scrollbar".to_string())
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(thickness)
        .opacity(opacity)
        .child(
            div()
                .id("tab-scrollbar-thumb")
                .debug_selector(|| "tab-scrollbar-thumb".to_string())
                .absolute()
                .left(px(start))
                .top_0()
                .w(px(length))
                .h(thickness)
                .rounded_full()
                .bg(idle)
                .hover(move |style| style.bg(busy))
                .on_drag(ThumbDrag::new(owner, start), {
                    let on_dragging = on_dragging.clone();
                    move |_, _, window, cx| {
                        // A thumb drag pins the bar on, so leaving the strip mid-drag cannot fade
                        // the thumb (and its handler) out of the tree.
                        on_dragging(true, window, cx);
                        cx.new(|_| EmptyView)
                    }
                }),
        )
        .on_drag_move(move |event: &gpui::DragMoveEvent<ThumbDrag>, window, cx| {
            let drag = event.drag(cx);
            if drag.owner != owner {
                return;
            }
            let local: f32 = (event.event.position.x - event.bounds.origin.x).into();
            let anchor = match drag.anchor.get() {
                Some(anchor) => anchor,
                None => {
                    drag.anchor.set(Some(local));
                    local
                }
            };
            let ratio = ((drag.start + local - anchor) / travel).clamp(0.0, 1.0);
            handle.set_offset(point(px(-(reach * ratio)), px(0.0)));
            window.refresh();
        })
        .into_any_element()
}

/// Pointer offset at press, so a drag continues from wherever the thumb was grabbed.
pub struct ThumbDrag {
    owner: gpui::EntityId,
    start: f32,
    anchor: std::rc::Rc<std::cell::Cell<Option<f32>>>,
}

impl ThumbDrag {
    fn new(owner: gpui::EntityId, start: f32) -> Self {
        Self {
            owner,
            start,
            anchor: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: u64 = TAB_SCROLLBAR_FADE_MS / 4;

    #[test]
    fn entering_and_leaving_fade_and_reach_both_ends() {
        let mut fade = Fade::default();
        assert_eq!(fade.opacity(0), 0.0);
        assert!(!fade.is_shown());

        // Enter: opacity climbs from 0 to 1 over the fade, not instantly.
        assert!(fade.set_hovered(true, 0));
        let early = fade.opacity(STEP);
        assert!(early > 0.0 && early < 1.0, "mid-fade {early}");
        assert!(fade.is_animating());
        assert_eq!(fade.opacity(TAB_SCROLLBAR_FADE_MS), 1.0);
        assert!(!fade.is_animating(), "no standing timer after the fade");

        // Leave: it fades back down.
        assert!(fade.set_hovered(false, 1_000));
        let leaving = fade.opacity(1_000 + STEP);
        assert!(leaving > 0.0 && leaving < 1.0, "mid-fade out {leaving}");
        assert_eq!(fade.opacity(1_000 + TAB_SCROLLBAR_FADE_MS), 0.0);
        assert!(!fade.is_shown());
    }

    #[test]
    fn a_reversal_continues_from_the_painted_opacity() {
        let mut fade = Fade::default();
        fade.set_hovered(true, 0);
        let mid = fade.opacity(STEP);
        assert!(mid > 0.0 && mid < 1.0);

        // Leave half way in: the fade back down starts where it was, so nothing jumps to 1.
        assert!(fade.set_hovered(false, STEP));
        let turned = fade.opacity(STEP);
        assert!(
            (turned - mid).abs() < 1e-6,
            "a reversal starts at the painted value: {turned} vs {mid}"
        );
        let next = fade.opacity(STEP + STEP);
        assert!(next < turned, "and then moves toward zero");
    }

    #[test]
    fn dragging_pins_the_bar_and_release_hands_back_to_hover() {
        let mut fade = Fade::default();
        fade.set_hovered(true, 0);
        fade.opacity(TAB_SCROLLBAR_FADE_MS);
        assert_eq!(fade.opacity(TAB_SCROLLBAR_FADE_MS), 1.0);

        // Dragging away from the strip keeps it visible.
        fade.set_dragging(true, 100);
        assert!(fade.set_hovered(false, 110));
        assert_eq!(fade.opacity(200), 1.0, "a drag pins the bar");

        // Releasing outside the strip fades it out from there.
        fade.set_dragging(false, 300);
        assert_eq!(fade.opacity(300), 1.0, "no jump on release");
        let mid = fade.opacity(300 + STEP);
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(fade.opacity(300 + TAB_SCROLLBAR_FADE_MS), 0.0);
    }

    #[test]
    fn repeating_the_same_state_does_not_restart_the_fade() {
        let mut fade = Fade::default();
        assert!(fade.set_hovered(true, 0));
        assert!(!fade.set_hovered(true, 1), "already hovered");
        assert!(!fade.set_hovered(true, 2));
        // Repeating the same hover does not restart the fade: half way through the fade the bar is
        // already partly visible rather than back at zero.
        let late = fade.opacity(TAB_SCROLLBAR_FADE_MS / 2);
        assert!(
            late > 0.0 && late < 1.0,
            "still mid fade, not restarted: {late}"
        );
        assert!(fade.is_shown());
        assert_eq!(fade.opacity(TAB_SCROLLBAR_FADE_MS), 1.0);
    }
}
