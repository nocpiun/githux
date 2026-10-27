//! A read-only, virtualized source view.
//!
//! Ely's `CodeEditor` was evaluated first (see `docs/content-reader.md`): it
//! gives line numbers, selection, copy and vertical virtualization, but its
//! read-only mode still draws an "Make editable" banner and it has no
//! horizontal scrolling. The reader must never accept an edit and must show
//! long lines, so this view keeps the good parts — Ely's `uniform_list`, the
//! public `code_highlights` and the code font — and drops every input path:
//! there is no text input handler, no IME handler and no edit action, so
//! typing, pasting or deleting cannot change the document.
//!
//! Only the visible rows are rendered, and horizontal extent grows from the
//! widest measured line, so a two-megabyte file does not build a node per
//! line.

use std::ops::Range;

use ely_core::{
    forms::{Highlight, code_highlights},
    theme::{ActiveTheme, TextSize},
};

use gpui::{
    AnyElement, App, ClipboardItem, Context, FocusHandle, Focusable, FontStyle, HighlightStyle,
    Hsla, InteractiveElement, IntoElement, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render, StatefulInteractiveElement,
    StrikethroughStyle, Styled, StyledText, TextRun, UniformListScrollHandle, Window, div, font,
    px, uniform_list,
};

/// One end of a selection, in document byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    anchor: usize,
    head: usize,
}

impl Selection {
    fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

/// A read-only view of one UTF-8 text.
///
/// `language` is only a label. Syntax coloring comes from Ely's generic code
/// highlighter: the reader does not pick a grammar per fence, because Ely keeps
/// `syntax_of`/`code_highlights_with` crate-private. Highlighting a file as
/// "code" is the documented behaviour, not a claim of per-language accuracy.
pub struct SourceView {
    text: std::sync::Arc<str>,
    /// Byte offset where each line starts. Always holds at least one entry.
    line_starts: Vec<usize>,
    language: Option<String>,
    selection: Option<Selection>,
    dragging: bool,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    fade: crate::ui::scrollbar::Fade,
    clock: std::time::Instant,
    thumb_dragging: bool,
    last_reach: gpui::Point<Pixels>,
    /// Body bounds and shaped layout of each row actually painted this frame,
    /// so a drag over trailing whitespace still maps to a real column.
    rows: std::collections::HashMap<usize, (gpui::Bounds<Pixels>, gpui::TextLayout)>,
    /// Rows recorded by the prepaint that is running now. They replace `rows`
    /// once the frame's prepaint ends, so a scrolled-away line can never keep
    /// winning a hit test at its old screen position.
    painting: std::cell::RefCell<Vec<(usize, gpui::Bounds<Pixels>, gpui::TextLayout)>>,
    #[cfg(test)]
    pinned_clock: Option<u64>,
    /// The widest row observed so far; visible rows can extend this after layout.
    widest: usize,
    width: Pixels,
}

impl SourceView {
    pub fn new(
        text: impl Into<String>,
        language: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text: std::sync::Arc<str> = std::sync::Arc::from(text.into());
        let line_starts = line_starts(&text);
        let (widest, width) = widest_line(&text, &line_starts, window, cx);
        Self {
            text,
            line_starts,
            language,
            selection: None,
            dragging: false,
            focus: cx.focus_handle().tab_stop(true),
            scroll: UniformListScrollHandle::new(),
            fade: Default::default(),
            clock: std::time::Instant::now(),
            thumb_dragging: false,
            last_reach: Default::default(),
            rows: std::collections::HashMap::new(),
            painting: Default::default(),
            #[cfg(test)]
            pinned_clock: None,
            widest,
            width,
        }
    }

    /// The text as last set; a copy or a test reads it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The language label, if the caller named one.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Whether a selection exists. The selection itself stays private.
    pub fn has_selection(&self) -> bool {
        self.selection.is_some_and(|s| !s.range().is_empty())
    }

    /// The selected bytes, if any.
    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection?.range();
        (range.start < range.end).then(|| self.text[range].to_string())
    }

    /// The row count, for the virtualization test.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The widest text observed during initial probing or subsequent row layout.
    pub fn content_width(&self) -> Pixels {
        self.width
    }

    /// The scroll offset, for the bounded-scroll test.
    pub fn offset(&self) -> gpui::Point<Pixels> {
        self.scroll.0.borrow().base_handle.offset()
    }

    /// True extent reported by the virtualized list itself.
    pub fn reach(&self) -> gpui::Point<Pixels> {
        self.scroll.0.borrow().base_handle.max_offset()
    }

    pub fn scrollbar_opacity(&mut self, now: u64) -> f32 {
        self.fade.opacity(now)
    }

    /// The opacity the last frame painted through the real hover path.
    pub fn scrollbar_opacity_at_now(&self) -> f32 {
        self.fade.painted()
    }

    #[cfg(test)]
    pub fn pin_scrollbar_clock(&mut self, ms: u64) {
        self.pinned_clock = Some(ms);
    }

    #[cfg(test)]
    pub fn show_scrollbar(&mut self) {
        self.fade.snap(true);
    }

    fn line_range(&self, ix: usize) -> Range<usize> {
        let start = self.line_starts[ix];
        let mut end = self
            .line_starts
            .get(ix + 1)
            .map_or(self.text.len(), |next| next - 1);
        if end > start && self.text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        start..end
    }

    fn line_text(&self, ix: usize) -> &str {
        let range = self.line_range(ix);
        &self.text[range]
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.selected_text() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.text.is_empty() {
            return;
        }
        self.selection = Some(Selection {
            anchor: 0,
            head: self.text.len(),
        });
        cx.notify();
    }

    fn press(
        &mut self,
        line: usize,
        layout: &gpui::TextLayout,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let range = self.line_range(line);
        let at = range.start
            + layout
                .index_for_position(event.position)
                .unwrap_or_else(|near| near)
                .min(range.len());
        self.dragging = true;
        self.selection = Some(match event.modifiers.shift {
            true => {
                let anchor = self.selection.map_or(at, |s| s.anchor);
                Selection { anchor, head: at }
            }
            false => Selection {
                anchor: at,
                head: at,
            },
        });
        cx.notify();
    }

    fn end_selection_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging {
            self.dragging = false;
            cx.notify();
        }
    }

    fn drag(
        &mut self,
        line: usize,
        layout: &gpui::TextLayout,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.dragging || !event.dragging() {
            return;
        }
        let range = self.line_range(line);
        let at = range.start
            + layout
                .index_for_position(event.position)
                .unwrap_or_else(|near| near)
                .min(range.len());
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        if selection.head == at {
            return;
        }
        selection.head = at;
        cx.notify();
    }

    #[cfg(test)]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Extends the selection from a pointer anywhere in the viewport: the row
    /// under `y`, and a column clamped to that row's own length.
    fn drag_anywhere(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging || !event.dragging() {
            return;
        }
        let Some((line, (bounds, layout))) = self
            .rows
            .iter()
            .filter(|(_, (bounds, _))| bounds.top() <= event.position.y)
            .max_by(|a, b| {
                f32::from(a.1.0.top())
                    .partial_cmp(&f32::from(b.1.0.top()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(line, row)| (*line, row.clone()))
        else {
            return;
        };
        let range = self.line_range(line);
        let at = range.start
            + layout
                .index_for_position(event.position)
                .unwrap_or_else(|near| near)
                .min(range.len());
        let at = at.max(range.start);
        let _ = bounds;
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        if selection.head == at {
            return;
        }
        selection.head = at;
        cx.notify();
    }

    fn row(
        &self,
        ix: usize,
        line_height: Pixels,
        size: Pixels,
        gutter_gap: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let line = self.line_text(ix);
        let range = self.line_range(ix);
        let selection = self.selection.map(|s| s.range()).and_then(|s| {
            let start = s.start.max(range.start).min(range.end) - range.start;
            let end = s.end.max(range.start).min(range.end) - range.start;
            (start < end).then_some(start..end)
        });
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let styles = line_highlights(line, selection, colors.selection, cx);
        let shown = StyledText::new(line.to_string()).with_highlights(styles);
        let layout = shown.layout().clone();
        let dragged = layout.clone();
        let digits = self.line_starts.len().to_string().len();
        let gutter = div()
            .flex_none()
            .debug_selector(move || format!("reader-source-gutter-{ix}"))
            .pl_2()
            // Real layout space, never typed spaces: the original 0.75rem gap
            // plus 6px, and the body's `TextLayout` keeps this same origin so
            // selection and horizontal scrolling stay aligned.
            .pr(gutter_gap)
            .text_color(colors.fg_subtle)
            .child(format!("{:>width$}", ix + 1, width = digits));
        let view = cx.entity().downgrade();
        let row_layout = layout.clone();
        let row_layout_for_paint = row_layout.clone();
        let body = div()
            // The initial shortlist is only a probe. Every rendered line reports
            // its actual shaped width, including font fallback, so a wider line
            // outside that shortlist can extend the list on the next frame.
            .on_children_prepainted(move |bounds, _, cx| {
                let Some(bounds) = bounds.first() else {
                    return;
                };
                let bounds = *bounds;
                let layout = row_layout_for_paint.clone();
                let _ = view.update(cx, |view, cx| {
                    if bounds.size.width > view.width {
                        view.width = bounds.size.width;
                        view.widest = ix;
                        cx.notify();
                    }
                    view.painting.borrow_mut().push((ix, bounds, layout));
                });
            })
            .id(("reader-source-body", ix))
            .debug_selector(move || format!("reader-source-body-{ix}"))
            .whitespace_nowrap()
            .text_color(colors.fg)
            .child(shown)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.press(ix, &layout, event, window, cx);
                }),
            );
        // Selection extends across the whole row, including its trailing
        // whitespace and an empty line, so a drag keeps moving after the text
        // ends. `drag` clamps the column to the line's own length.
        let drag_view = cx.entity().downgrade();
        let press_layout = dragged.clone();
        let row_layout = dragged.clone();
        let hit = div()
            .id(("reader-source-row-hit", ix))
            .debug_selector(move || format!("reader-source-row-hit-{ix}"))
            .flex_1()
            .min_w_0()
            .h(line_height)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.press(ix, &press_layout, event, window, cx);
                }),
            )
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                let _ = drag_view.update(cx, |view, cx| {
                    view.drag(ix, &row_layout, event, cx);
                });
            })
            .child(body)
            .into_any_element();
        div()
            .id(("reader-source-row", ix))
            .debug_selector(move || format!("reader-source-row-{ix}"))
            .flex()
            .items_start()
            .h(line_height)
            .font_family(theme.mono_family.clone())
            .text_size(size)
            .child(gutter)
            .child(hit)
            .into_any_element()
    }
}

impl Focusable for SourceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SourceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let mono = theme.mono_family.clone();
        let size = theme.text_size(TextSize::Sm).to_pixels(window.rem_size());
        let line_height = (size * 1.6).round();
        let gutter_gap = gutter_gap(window.rem_size());
        let count = self.line_starts.len();
        let widest = self.widest;
        // Rows recorded by the *previous* frame's prepaint were complete by the
        // time this render ran, so swapping here always matches what is on
        // screen and drops rows that scrolled away.
        if !self.painting.borrow().is_empty() {
            self.rows = self
                .painting
                .borrow_mut()
                .drain(..)
                .map(|(ix, bounds, layout)| (ix, (bounds, layout)))
                .collect();
        }
        if self.thumb_dragging && !cx.has_active_drag() {
            self.thumb_dragging = false;
            self.fade.set_dragging(false, self.now_ms());
        }
        let opacity = self.fade.opacity(self.now_ms());
        if self.fade.is_animating() {
            window.request_animation_frame();
        }
        let handle = self.scroll.0.borrow().base_handle.clone();
        let reach = handle.max_offset();
        if reach != self.last_reach {
            self.last_reach = reach;
            cx.notify();
        }
        let list = uniform_list(
            "reader-source-list",
            count,
            cx.processor(move |view: &mut Self, range: Range<usize>, _window, cx| {
                range
                    .map(|ix| view.row(ix, line_height, size, gutter_gap, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(widest))
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .id("reader-source-root")
            .debug_selector(|| "reader-source".to_string())
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(colors.surface)
            .font_family(mono)
            .text_color(colors.fg)
            .child(list)
            .children(self.scrollbar(gpui::Axis::Vertical, opacity, &handle, cx))
            .children(self.scrollbar(gpui::Axis::Horizontal, opacity, &handle, cx))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, cx| {
                view.drag_anywhere(event, cx);
            }))
            // The gesture ends on release, including a release outside the
            // viewport; otherwise a later drag that never started on the text
            // would keep extending the previous selection.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.end_selection_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.end_selection_drag(cx)),
            )
            .on_hover(cx.listener(|view, hovered, _, cx| {
                if view.fade.set_hovered(*hovered, view.now_ms()) {
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                let keystroke = &event.keystroke;
                if !keystroke.modifiers.secondary(cx) {
                    return;
                }
                match keystroke.key.as_str() {
                    "c" => {
                        view.copy(cx);
                        cx.stop_propagation();
                    }
                    "a" => {
                        view.select_all(cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
    }
}

impl SourceView {
    fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(ms) = self.pinned_clock {
            return ms;
        }
        self.clock.elapsed().as_millis() as u64
    }

    fn scrollbar(
        &self,
        axis: gpui::Axis,
        opacity: f32,
        handle: &gpui::ScrollHandle,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        (opacity > 0.0).then(|| {
            let name = match axis {
                gpui::Axis::Vertical => "reader-scrollbar-y",
                gpui::Axis::Horizontal => "reader-scrollbar-x",
            };
            let this = cx.entity().downgrade();
            let on_dragging = std::rc::Rc::new(move |dragging, _: &mut Window, cx: &mut App| {
                let _ = this.update(cx, |view, cx| {
                    view.thumb_dragging = dragging;
                    view.fade.set_dragging(dragging, view.now_ms());
                    cx.notify();
                });
            });
            super::scrollbar::render(handle, axis, opacity, cx.entity_id(), name, on_dragging, cx)
        })
    }
}

/// The gap between the gutter's number and the source text.
///
/// It keeps the original 0.75rem spacing and adds 6px on top of it, so the
/// change is visibly larger than before at any DPI rather than replacing it.
pub fn gutter_gap(rem: Pixels) -> Pixels {
    rem * 0.75 + px(6.0)
}

/// The byte offset where each line starts.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (at, _) in text.match_indices('\n') {
        starts.push(at + 1);
    }
    starts
}

/// An initial width probe, not an exhaustive search for the widest line.
///
/// Shape eight candidates initially, then let each visible row report its
/// actual text width during prepaint. Byte length cannot order visual widths
/// across ASCII, CJK and emoji; off-screen rows extend the horizontal extent
/// when scrolled into view. No font widths are guessed from character counts.
fn widest_line(text: &str, starts: &[usize], window: &mut Window, cx: &App) -> (usize, Pixels) {
    let theme = cx.theme();
    let size = theme.text_size(TextSize::Sm).to_pixels(window.rem_size());
    let base = font(theme.mono_family.clone());
    let mut candidates: Vec<(usize, usize)> = (0..starts.len())
        .map(|ix| {
            let start = starts[ix];
            let end = starts.get(ix + 1).map_or(text.len(), |next| next - 1);
            (ix, end.saturating_sub(start))
        })
        .collect();
    candidates.sort_unstable_by_key(|(_, len)| std::cmp::Reverse(*len));
    candidates.truncate(8);
    let mut widest = candidates.first().map_or(0, |(ix, _)| *ix);
    let mut width = Pixels::ZERO;
    for (ix, _) in candidates {
        let line = &text[starts[ix]..starts.get(ix + 1).map_or(text.len(), |next| next - 1)];
        let run = TextRun {
            len: line.len(),
            font: base.clone(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let measured = window
            .text_system()
            .layout_line(line, size, std::slice::from_ref(&run), None)
            .width;
        if measured > width {
            width = measured;
            widest = ix;
        }
    }
    (widest, width)
}

/// Ely's `Highlight` as gpui's `HighlightStyle`.
fn highlight_style(highlight: Highlight) -> HighlightStyle {
    HighlightStyle {
        color: Some(highlight.color),
        font_weight: highlight.weight,
        font_style: highlight.italic.then_some(FontStyle::Italic),
        background_color: highlight.background,
        strikethrough: highlight.strike.then(|| StrikethroughStyle {
            thickness: px(1.0),
            color: Some(highlight.color),
        }),
        ..HighlightStyle::default()
    }
}

/// Code colors with the selection's background laid over `selection`.
///
/// `StyledText` wants non-overlapping runs, so the code spans and the
/// selection are split at each other's boundaries and every piece becomes one
/// run. The selection highlight is therefore drawn by the same layout that
/// places the glyphs, never by a second width estimate.
fn line_highlights(
    line: &str,
    selection: Option<Range<usize>>,
    selection_color: Hsla,
    cx: &App,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let base: Vec<(Range<usize>, HighlightStyle)> = code_highlights(line, cx)
        .into_iter()
        .map(|(range, highlight)| (range, highlight_style(highlight)))
        .collect();
    selection_highlights(line.len(), base, selection, selection_color)
}

/// Ely's lexer emits sorted, disjoint syntax spans. Walk them once, filling
/// gaps and splitting only at the selection's two edges. No output interval
/// searches the span list again; the work is linear in the number of spans.
fn selection_highlights(
    len: usize,
    base: Vec<(Range<usize>, HighlightStyle)>,
    selection: Option<Range<usize>>,
    color: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out = Vec::with_capacity(base.len() + 4);
    let mut emit = |range: Range<usize>, style: HighlightStyle| {
        let mut start = range.start;
        let end = range.end;
        if let Some(selected) = &selection {
            if start < selected.start && selected.start < end {
                out.push((start..selected.start, style));
                start = selected.start;
            }
            if start < selected.end && selected.start < end {
                let stop = end.min(selected.end);
                out.push((
                    start..stop,
                    HighlightStyle {
                        background_color: Some(color),
                        ..style
                    },
                ));
                start = stop;
            }
        }
        if start < end {
            out.push((start..end, style));
        }
    };
    let mut at = 0;
    for (range, style) in base {
        if at < range.start {
            emit(at..range.start, HighlightStyle::default());
        }
        at = range.end;
        emit(range, style);
    }
    if at < len {
        emit(at..len, HighlightStyle::default());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_splits_syntax_and_whitespace_without_losing_styles() {
        let code = HighlightStyle {
            color: Some(gpui::rgb(0x123456).into()),
            ..HighlightStyle::default()
        };
        let selected = gpui::rgb(0xabcdef).into();
        // UTF-8 byte ranges in "中文 x": the selection crosses both the
        // multibyte token and an unstyled whitespace gap.
        let result =
            selection_highlights(8, vec![(0..6, code), (7..8, code)], Some(3..7), selected);
        assert_eq!(
            result.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            vec![0..3, 3..6, 6..7, 7..8]
        );
        assert_eq!(result[0].1, code);
        assert_eq!(result[1].1.color, code.color);
        assert_eq!(result[1].1.background_color, Some(selected));
        assert_eq!(result[2].1.background_color, Some(selected));
        assert_eq!(result[3].1, code);
    }

    #[gpui::test]
    fn dense_line_highlighting_stays_within_an_interaction_budget(cx: &mut gpui::TestAppContext) {
        cx.update(ely_core::init_for_tests);
        // 131,072 syntax spans on a single visible 128 KiB minified line.
        // The former per-interval find performs billions of comparisons here.
        let line = "x=1;".repeat(32_768);
        let start = std::time::Instant::now();
        let styles = cx.update(|cx| {
            line_highlights(
                &line,
                Some(1..line.len() - 1),
                gpui::rgb(0xabcdef).into(),
                cx,
            )
        });
        assert!(styles.len() >= 100_000);
        assert_eq!(styles.first().unwrap().0.start, 0);
        assert_eq!(styles.last().unwrap().0.end, line.len());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "highlighting took {:?}",
            start.elapsed()
        );
    }
}
