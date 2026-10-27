//! One continuous shaped paragraph: prose in Noto Sans, inline code in the
//! theme's code font inside a padded rounded box.
//!
//! Code padding is a layout advance and participates in wrapping, painting and
//! pointer hit testing, so the box can never overlap its neighbours and a copy
//! never contains padding characters. Selection is owned by the document: a
//! paragraph receives the document-wide range to paint and reports pointer
//! positions as document offsets, so a drag can cross blocks. The paragraph
//! therefore keeps no selection of its own.

use std::{cell::RefCell, ops::Range, rc::Rc};

use ely_core::theme::ActiveTheme;
use gpui::{
    App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId, HighlightStyle,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, RenderOnce, ShapedLine, SharedString,
    Styled, Window, div, fill, point, prelude::*, px, size,
};
use unicode_segmentation::UnicodeSegmentation;

use super::ast::{Inline, Style};

/// Horizontal inset of a code box, on each side.
const PAD_X: f32 = 3.5;
/// Vertical inset of a code box, on each side.
const PAD_Y: f32 = 1.0;
/// Corner radius of a code box.
const RADIUS: f32 = 4.0;

#[derive(Clone)]
struct Piece {
    range: Range<usize>,
    line: ShapedLine,
    origin: Point<Pixels>,
    code: bool,
    /// The code region ends here, so the box closes with its right inset.
    closes: bool,
}

/// One paragraph's real layout, shared with the document that owns selection.
#[derive(Default)]
pub struct LocalLayout {
    pieces: Vec<Piece>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    /// Font families requested for code pieces, for the test snapshot.
    code_fonts: Vec<SharedString>,
}

impl LocalLayout {
    /// Where this paragraph was painted last frame.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    /// The local byte offset under `position`, or None outside the text.
    ///
    /// Hits use the real shaped rows, not the paragraph's padding box: a point
    /// in the gap below a line, or to the right of a short line, belongs to
    /// blank reader space so the document can clear or extend consistently.
    pub fn index_at(&self, position: Point<Pixels>) -> Option<usize> {
        if self.bounds.size.height <= Pixels::ZERO || !self.bounds.contains(&position) {
            return None;
        }
        let local = position - self.bounds.origin;
        let row = (local.y / self.line_height).floor().max(0.0);
        let mut rows: Vec<f32> = self
            .pieces
            .iter()
            .map(|piece| (piece.origin.y / self.line_height).floor())
            .collect();
        rows.sort_unstable_by(|a, b| a.total_cmp(b));
        rows.dedup();
        if !rows.contains(&row) {
            return None;
        }
        // The row ends after its last glyph: a press in the trailing space of
        // a short line is blank reader space, not that line's end.
        let right = self
            .pieces
            .iter()
            .filter(|piece| (piece.origin.y / self.line_height).floor() == row)
            .map(|piece| piece.origin.x + piece.line.width())
            .fold(Pixels::ZERO, Pixels::max);
        if local.x > right + px(PAD_X) {
            return None;
        }
        Some(self.index(position))
    }

    /// The nearest local byte offset for a pointer beside this paragraph's
    /// text: same row when the pointer is beside it, else the first/last row's
    /// nearest end. Used for whitespace inside a wrapped paragraph so a drag
    /// does not collapse to the whole block.
    pub fn nearest_index(&self, position: Point<Pixels>) -> Option<usize> {
        if self.pieces.is_empty() {
            return None;
        }
        let local = position - self.bounds.origin;
        let row = (local.y / self.line_height).floor().max(0.0);
        let mut rows: Vec<f32> = self
            .pieces
            .iter()
            .map(|piece| (piece.origin.y / self.line_height).floor())
            .collect();
        rows.sort_unstable_by(|a, b| a.total_cmp(b));
        rows.dedup();
        let nearest_row = rows
            .iter()
            .copied()
            .min_by(|a, b| (a - row).abs().total_cmp(&(b - row).abs()))
            .unwrap_or(row);
        let pieces: Vec<&Piece> = self
            .pieces
            .iter()
            .filter(|piece| (piece.origin.y / self.line_height).floor() == nearest_row)
            .collect();
        let first = pieces.first()?;
        let last = pieces.last()?;
        if local.x < first.origin.x {
            return Some(first.range.start);
        }
        if local.x > last.origin.x + last.line.width() {
            return Some(last.range.end);
        }
        Some(
            self.index(point(
                local
                    .x
                    .max(first.origin.x)
                    .min(last.origin.x + last.line.width()),
                local.y,
            )),
        )
    }

    /// The glyph byte under `position`; None when it is not over a glyph.
    pub fn glyph_at(&self, position: Point<Pixels>) -> Option<usize> {
        let local = position - self.bounds.origin;
        let row = (local.y / self.line_height).floor().max(0.0);
        for piece in self
            .pieces
            .iter()
            .filter(|piece| (piece.origin.y / self.line_height).floor() == row)
        {
            let x = local.x - piece.origin.x;
            if x >= Pixels::ZERO && x < piece.line.width() {
                return piece
                    .line
                    .index_for_x(x)
                    .map(|index| piece.range.start + index)
                    .or(Some(piece.range.end));
            }
        }
        None
    }

    /// Byte position of a pointer, snapped to the nearest grapheme boundary.
    fn index(&self, position: Point<Pixels>) -> usize {
        let local = position - self.bounds.origin;
        let row = (local.y / self.line_height).floor().max(0.0);
        for piece in self
            .pieces
            .iter()
            .filter(|piece| (piece.origin.y / self.line_height).floor() == row)
        {
            if local.x < piece.origin.x + piece.line.width() {
                return piece.range.start
                    + piece.line.closest_index_for_x(local.x - piece.origin.x);
            }
        }
        self.pieces
            .iter()
            .rev()
            .find(|piece| (piece.origin.y / self.line_height).floor() <= row)
            .map_or(0, |piece| piece.range.end)
    }
}

/// A read-only paragraph drawn inside a document that owns selection.
#[derive(IntoElement)]
pub struct ParagraphView {
    id: ElementId,
    inline: Inline,
    /// This paragraph's range in the document text model.
    doc: Range<usize>,
    /// The document's current selection, in document offsets.
    selection: Range<usize>,
    layout: Rc<RefCell<LocalLayout>>,
    /// Shown before the inline text, such as a list bullet.
    prefix: Option<SharedString>,
}

impl ParagraphView {
    pub fn new(
        id: impl Into<ElementId>,
        inline: Inline,
        doc: Range<usize>,
        selection: Range<usize>,
        layout: Rc<RefCell<LocalLayout>>,
    ) -> Self {
        Self {
            id: id.into(),
            inline,
            doc,
            selection,
            layout,
            prefix: None,
        }
    }

    pub fn prefix(mut self, prefix: impl Into<SharedString>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }
}

impl RenderOnce for ParagraphView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let prefix = self.prefix;
        div()
            .id(self.id)
            .debug_selector(|| "reader-inline-paragraph".into())
            .flex()
            .items_start()
            .gap_2()
            .w_full()
            .when_some(prefix, |row, prefix| {
                row.child(
                    div()
                        .flex_none()
                        .text_color(theme.colors.fg_muted)
                        .child(prefix),
                )
            })
            .child(div().flex_1().min_w_0().child(Paragraph {
                inline: self.inline,
                doc: self.doc,
                selection: self.selection,
                layout: self.layout,
            }))
            .into_any_element()
    }
}

struct Paragraph {
    inline: Inline,
    doc: Range<usize>,
    selection: Range<usize>,
    layout: Rc<RefCell<LocalLayout>>,
}

impl IntoElement for Paragraph {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The styled runs for one paragraph: prose keeps the inherited style, code
/// pieces take the theme's code font.
fn runs(
    inline: &Inline,
    style: &gpui::TextStyle,
    mono: &SharedString,
    link_color: gpui::Hsla,
) -> Vec<gpui::TextRun> {
    let mut edges = vec![0, inline.text.len()];
    for (range, _) in &inline.styles {
        edges.extend([range.start, range.end]);
    }
    for (range, _) in &inline.links {
        edges.extend([range.start, range.end]);
    }
    edges.sort_unstable();
    edges.dedup();
    // Sweep the style boundaries once. Scanning every style for every run
    // would be quadratic in the number of inline code spans.
    let mut events: Vec<(usize, bool, Style)> = inline
        .styles
        .iter()
        .flat_map(|(range, kind)| [(range.start, true, *kind), (range.end, false, *kind)])
        .collect();
    events.sort_by_key(|(at, enters, _)| (*at, !*enters));
    let mut active: Vec<Style> = Vec::new();
    let mut event = 0;
    // Link ranges are sorted and disjoint in an `Inline`; advance through them
    // with the run cursor instead of rescanning all links per run.
    let mut links = inline.links.clone();
    links.sort_unstable_by_key(|(range, _)| range.start);
    let mut link = 0;
    let mut runs = Vec::with_capacity(edges.len());
    for edge in edges.windows(2) {
        while event < events.len() && events[event].0 <= edge[0] {
            let (_, enters, kind) = events[event];
            if enters {
                active.push(kind);
            } else if let Some(at) = active.iter().position(|active| *active == kind) {
                active.remove(at);
            }
            event += 1;
        }
        let mut run_style = style.clone();
        for kind in &active {
            if !matches!(kind, Style::Code | Style::Math) {
                run_style = run_style.highlight(highlight(*kind));
            }
        }
        while links
            .get(link)
            .is_some_and(|(range, _)| range.end <= edge[0])
        {
            link += 1;
        }
        // A link range is drawn like the rest of the document's links: the
        // theme's link colour and a thin underline, so navigation is visible.
        if links
            .get(link)
            .is_some_and(|(range, _)| range.start <= edge[0] && edge[1] <= range.end)
        {
            run_style = run_style.highlight(HighlightStyle {
                color: Some(link_color),
                underline: Some(gpui::UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(link_color.opacity(0.4)),
                    wavy: false,
                }),
                ..HighlightStyle::default()
            });
        }
        let mut run = run_style.to_run(edge[1] - edge[0]);
        if active
            .iter()
            .any(|kind| matches!(kind, Style::Code | Style::Math))
        {
            run.font.family = mono.clone();
        }
        runs.push(run);
    }
    runs
}

fn highlight(style: Style) -> HighlightStyle {
    match style {
        Style::Strong => HighlightStyle {
            font_weight: Some(gpui::FontWeight::BOLD),
            ..HighlightStyle::default()
        },
        Style::Emphasis => HighlightStyle {
            font_style: Some(gpui::FontStyle::Italic),
            ..HighlightStyle::default()
        },
        Style::Strike => HighlightStyle {
            strikethrough: Some(gpui::StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            }),
            ..HighlightStyle::default()
        },
        _ => HighlightStyle::default(),
    }
}

impl Element for Paragraph {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height().max(font_size + px(4.0));
        let theme = cx.theme();
        let mono = theme.mono_family.clone();
        let inline = self.inline.clone();
        let link_color = cx.theme().colors.link;
        let shaped_runs = runs(&inline, &style, &mono, link_color);
        let layout = self.layout.clone();
        let code_fonts = shaped_runs
            .iter()
            .filter(|run| run.font.family == mono)
            .map(|run| run.font.family.clone())
            .collect::<Vec<_>>();
        let id = window.request_measured_layout(
            gpui::Style::default(),
            move |known, available, window, _| {
                let width = known
                    .width
                    .or(match available.width {
                        AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    })
                    .unwrap_or(px(100_000.0))
                    .max(px(1.0));
                // `shape_line` is single-line: hard breaks are kept as row
                // boundaries by the wrapper below, not by the shaper. Replacing
                // each newline with one space keeps byte offsets identical, so
                // rows, selection and links still index the original text.
                let text = inline.text.trim_end_matches('\n').to_string();
                let shaped_text = text.replace('\n', " ");
                let shaped = window.text_system().shape_line(
                    SharedString::from(shaped_text),
                    font_size,
                    &shaped_runs,
                    None,
                );
                // Glyph positions are indexed once; GPUI's `x_for_index`
                // scans from the start for every call.
                let mut positions = vec![shaped.width(); text.len() + 1];
                for run in &shaped.runs {
                    for glyph in &run.glyphs {
                        positions[glyph.index] = glyph.position.x;
                    }
                }
                let mut next = shaped.width();
                for at in (0..text.len()).rev() {
                    if positions[at] == shaped.width() {
                        positions[at] = next;
                    } else {
                        next = positions[at];
                    }
                }
                let mut code_at = vec![false; text.len() + 1];
                for (range, kind) in &inline.styles {
                    if *kind == Style::Code && range.end <= text.len() {
                        for at in range.clone() {
                            code_at[at] = true;
                        }
                    }
                }
                let is_code = |at: usize| code_at.get(at).copied().unwrap_or(false);
                // Style-piece boundaries: wrapping and painting must agree on
                // where the padding insets go.
                let mut edges = vec![0, text.len()];
                for (range, _) in &inline.styles {
                    if range.end <= text.len() {
                        edges.extend([range.start, range.end]);
                    }
                }
                edges.sort_unstable();
                edges.dedup();
                let chars: Vec<_> = text
                    .grapheme_indices(true)
                    .map(|(at, grapheme)| (at, at + grapheme.len(), grapheme))
                    .collect();
                let mut rows = Vec::new();
                let mut start = 0;
                while start < chars.len() {
                    let mut end = start;
                    let mut used = Pixels::ZERO;
                    let mut candidate = None;
                    while end < chars.len() {
                        let (a, b, grapheme) = chars[end];
                        if grapheme == "\n" {
                            end += 1;
                            break;
                        }
                        let code = is_code(a);
                        // A code piece that continues from the previous row
                        // still starts with its left inset in construction.
                        let opens_piece = code && (end == start || edges.binary_search(&a).is_ok());
                        let closes_piece = code && edges.binary_search(&b).is_ok();
                        let advance = positions[b] - positions[a];
                        let extra = if opens_piece { px(PAD_X) } else { Pixels::ZERO }
                            + if closes_piece {
                                px(PAD_X)
                            } else {
                                Pixels::ZERO
                            };
                        if used + advance + extra > width && end > start {
                            if let Some(candidate) = candidate {
                                end = candidate;
                            }
                            break;
                        }
                        used += advance + extra;
                        end += 1;
                        if grapheme.chars().all(char::is_whitespace) || !grapheme.is_ascii() {
                            candidate = Some(end);
                        }
                    }
                    end = end.max(start + 1);
                    rows.push(chars[start].0..chars[end - 1].1);
                    start = end;
                }
                let mut pieces = Vec::new();
                let mut cursor = shaped.cursor();
                let mut measured = Pixels::ZERO;
                for (row, range) in rows.iter().enumerate() {
                    let mut at = range.start;
                    let mut x = Pixels::ZERO;
                    while at < range.end {
                        let code = is_code(at);
                        let next = edges.partition_point(|edge| *edge <= at);
                        let end = edges
                            .get(next)
                            .copied()
                            .filter(|edge| *edge < range.end)
                            .unwrap_or(range.end);
                        let line = cursor.take_until(end);
                        if code {
                            x += px(PAD_X);
                        }
                        let advance = line.width();
                        let closes = code && edges.binary_search(&end).is_ok();
                        pieces.push(Piece {
                            range: at..end,
                            line,
                            origin: point(x, line_height * row),
                            code,
                            closes,
                        });
                        x += advance;
                        if closes {
                            x += px(PAD_X);
                        }
                        at = end;
                    }
                    measured = measured.max(x);
                }
                let height = line_height * rows.len().max(1);
                *layout.borrow_mut() = LocalLayout {
                    pieces,
                    bounds: Bounds::default(),
                    line_height,
                    code_fonts: code_fonts.clone(),
                };
                size(known.width.unwrap_or(measured.ceil().min(width)), height)
            },
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        cx: &mut App,
    ) {
        self.layout.borrow_mut().bounds = bounds;
        #[cfg(test)]
        {
            let layout = self.layout.borrow();
            let snapshot = Snapshot {
                text: self.inline.text.clone(),
                bounds,
                line_height: layout.line_height,
                code_fonts: layout.code_fonts.clone(),
                pieces: layout
                    .pieces
                    .iter()
                    .map(|piece| PieceSnapshot {
                        range: piece.range.clone(),
                        code: piece.code,
                        text_bounds: Bounds::new(
                            bounds.origin + piece.origin,
                            size(piece.line.width(), layout.line_height),
                        ),
                        box_bounds: code_box(bounds.origin + piece.origin, piece, &layout),
                    })
                    .collect(),
            };
            if !cx.has_global::<Snapshots>() {
                cx.set_global(Snapshots::default());
            }
            cx.global_mut::<Snapshots>()
                .0
                .insert(snapshot.text.clone(), snapshot);
        }
        #[cfg(not(test))]
        let _ = cx;
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.layout.borrow();
        // The document's selection, translated into this paragraph's bytes.
        let selected =
            self.selection.start.max(self.doc.start)..self.selection.end.min(self.doc.end);
        let selected = if selected.start < selected.end {
            selected.start - self.doc.start..selected.end - self.doc.start
        } else {
            0..0
        };
        let (box_color, selection) = {
            let theme = cx.theme();
            (theme.colors.sunken, theme.colors.selection)
        };
        let visible = window.content_mask().bounds;
        for piece in &layout.pieces {
            let origin = layout.bounds.origin + piece.origin;
            // Only visible rows are painted; the layout is still complete so
            // selection coordinates and links stay valid.
            if origin.y + layout.line_height < visible.top() || origin.y > visible.bottom() {
                continue;
            }
            if piece.code {
                let bounds = code_box(origin, piece, &layout);
                window.paint_quad(gpui::quad(
                    bounds,
                    px(RADIUS),
                    box_color,
                    gpui::Edges::default(),
                    gpui::transparent_black(),
                    gpui::BorderStyle::default(),
                ));
            }
            let start = selected.start.max(piece.range.start);
            let end = selected.end.min(piece.range.end);
            if start < end {
                let x = piece.line.x_for_index(start - piece.range.start);
                let to = piece.line.x_for_index(end - piece.range.start);
                window.paint_quad(fill(
                    Bounds::new(origin + point(x, px(0.0)), size(to - x, layout.line_height)),
                    selection,
                ));
            }
            let _ = piece.line.paint(
                origin,
                layout.line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

fn code_box(origin: Point<Pixels>, piece: &Piece, layout: &LocalLayout) -> Bounds<Pixels> {
    Bounds::new(
        origin - point(px(PAD_X), px(-PAD_Y)),
        size(
            piece.line.width()
                + px(PAD_X)
                + if piece.closes {
                    px(PAD_X)
                } else {
                    Pixels::ZERO
                },
            layout.line_height - px(PAD_Y * 2.0),
        ),
    )
}

/// Last real paragraph layouts, so tests inspect actual geometry and fonts
/// instead of estimating widths.
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct PieceSnapshot {
    pub range: Range<usize>,
    pub code: bool,
    pub text_bounds: Bounds<Pixels>,
    pub box_bounds: Bounds<Pixels>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub text: String,
    pub bounds: Bounds<Pixels>,
    pub line_height: Pixels,
    pub code_fonts: Vec<SharedString>,
    pub pieces: Vec<PieceSnapshot>,
}

#[cfg(test)]
#[derive(Default)]
struct Snapshots(std::collections::HashMap<String, Snapshot>);

#[cfg(test)]
impl gpui::Global for Snapshots {}

#[cfg(test)]
pub fn snapshot(text: &str, cx: &App) -> Option<Snapshot> {
    cx.try_global::<Snapshots>()?.0.get(text).cloned()
}
