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

use ely_gpui_component::{
    forms::{Highlight, code_highlights},
    theme::{ActiveTheme, TextSize},
};

use gpui::{
    AnyElement, App, ClipboardItem, Context, FocusHandle, Focusable, FontStyle, HighlightStyle,
    Hsla, InteractiveElement, IntoElement, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render, StrikethroughStyle, Styled,
    StyledText, TextRun, UniformListScrollHandle, Window, div, font, px, uniform_list,
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
    /// The line `uniform_list` measures for the horizontal extent.
    widest: usize,
    /// That line's measured width, so a test can compare it to the viewport.
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

    /// How wide the widest candidate line measures, for the horizontal test.
    pub fn content_width(&self) -> Pixels {
        self.width
    }

    /// The scroll offset, for the bounded-scroll test.
    pub fn offset(&self) -> gpui::Point<Pixels> {
        self.scroll.0.borrow().base_handle.offset()
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

    fn row(
        &self,
        ix: usize,
        line_height: Pixels,
        size: Pixels,
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
            .pl_2()
            .pr_3()
            .text_color(colors.fg_subtle)
            .child(format!("{:>width$}", ix + 1, width = digits));
        let body = div()
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
            )
            .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
                view.drag(ix, &dragged, event, cx);
            }));
        div()
            .id(("reader-source-row", ix))
            .debug_selector(move || format!("reader-source-row-{ix}"))
            .flex()
            .items_start()
            .h(line_height)
            .font_family(theme.mono_family.clone())
            .text_size(size)
            .child(gutter)
            .child(body)
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
        let count = self.line_starts.len();
        let widest = self.widest;
        let list = uniform_list(
            "reader-source",
            count,
            cx.processor(move |view: &mut Self, range: Range<usize>, _window, cx| {
                range
                    .map(|ix| view.row(ix, line_height, size, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(widest))
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .id("reader-source")
            .debug_selector(|| "reader-source".to_string())
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .bg(colors.surface)
            .font_family(mono)
            .text_color(colors.fg)
            .child(list)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                let keystroke = &event.keystroke;
                if !keystroke.modifiers.secondary() {
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
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.dragging = false;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.dragging = false;
                    cx.notify();
                }),
            )
    }
}

/// The byte offset where each line starts.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (at, _) in text.match_indices('\n') {
        starts.push(at + 1);
    }
    starts
}

/// The line whose measured width is largest.
///
/// Every line is not measured: a two-megabyte file would put that work on the
/// UI thread. The widest candidates by byte length are measured instead, which
/// covers ASCII and, because a CJK character is three bytes and two cells, most
/// wide-character lines too. The `uniform_list` then measures the chosen line
/// and grows the horizontal extent to fit it.
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
    let mut cuts: Vec<usize> = vec![0, line.len()];
    for (range, _) in &base {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    if let Some(selection) = &selection {
        cuts.push(selection.start);
        cuts.push(selection.end);
    }
    cuts.retain(|cut| *cut <= line.len() && line.is_char_boundary(*cut));
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::with_capacity(cuts.len().saturating_sub(1));
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start >= end {
            continue;
        }
        let mut style = base
            .iter()
            .find(|(range, _)| range.start <= start && end <= range.end)
            .map_or_else(HighlightStyle::default, |(_, style)| *style);
        if selection
            .as_ref()
            .is_some_and(|selection| selection.start < end && start < selection.end)
        {
            style.background_color = Some(selection_color);
        }
        out.push((start..end, style));
    }
    out
}
