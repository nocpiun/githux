//! One continuous shaped reader paragraph. Padding affects measured advances,
//! wrapping, paint and hit testing, never the source or copied bytes.
use std::{cell::RefCell, ops::Range, rc::Rc};
use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Element, ElementId, Entity, FocusHandle,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, MouseButton, ParentElement,
    Pixels, Point, RenderOnce, ShapedLine, SharedString, Styled, Window, div, fill, point,
    prelude::*, px, size,
};
use unicode_segmentation::UnicodeSegmentation;
use super::{OnLink, highlight, open_link};
use crate::{documents::tree::{Inline, Style}, theme::ActiveTheme};

const PAD: f32 = 3.5;
#[derive(Clone)]
struct Piece { range: Range<usize>, line: ShapedLine, origin: Point<Pixels>, code: bool }
#[derive(Default)]
struct Layout { pieces: Vec<Piece>, bounds: Bounds<Pixels>, line_height: Pixels, code_run_fonts: Vec<SharedString> }
impl Layout {
    fn index(&self, position: Point<Pixels>) -> usize {
        let local = position - self.bounds.origin;
        let row = (local.y / self.line_height).floor().max(0.0);
        let candidates: Vec<_> = self.pieces.iter().filter(|p| (p.origin.y / self.line_height).floor() == row).collect();
        for piece in &candidates {
            if local.x < piece.origin.x + piece.line.width() {
                return piece.range.start + piece.line.closest_index_for_x(local.x - piece.origin.x);
            }
        }
        candidates.last().map_or_else(|| self.pieces.last().map_or(0, |p| p.range.end), |p| p.range.end)
    }
}
struct Selection { focus: FocusHandle, anchor: usize, head: usize, dragging: bool }
impl Selection { fn range(&self) -> Range<usize> { self.anchor.min(self.head)..self.anchor.max(self.head) } }
#[derive(IntoElement)]
pub(super) struct ReaderInline { id: ElementId, inline: Inline, on_link: Option<OnLink> }
impl ReaderInline { pub fn new(id: ElementId, inline: Inline, on_link: Option<OnLink>) -> Self { Self { id, inline, on_link } } }
impl RenderOnce for ReaderInline {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| Selection { focus: cx.focus_handle(), anchor: 0, head: 0, dragging: false });
        let focus = state.read(cx).focus.clone();
        let layout = Rc::new(RefCell::new(Layout::default()));
        let (down, moving, up, keys) = (state.clone(), state.clone(), state.clone(), state.clone());
        let (down_layout, move_layout) = (layout.clone(), layout.clone());
        let text = self.inline.text.clone();
        let links = self.inline.links.clone();
        let on_link = self.on_link;
        div().id(self.id).debug_selector(|| "reader-inline-paragraph".into()).track_focus(&focus).cursor_text()
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let at = down_layout.borrow().index(event.position);
                down.update(cx, |s, cx| { window.focus(&s.focus, cx); if !event.modifiers.shift { s.anchor = at; } s.head = at; s.dragging = true; cx.notify(); });
            })
            .on_mouse_move(move |event, _, cx| { if moving.read(cx).dragging && event.dragging() { let at = move_layout.borrow().index(event.position); moving.update(cx, |s, cx| { s.head = at; cx.notify(); }); } })
            .on_mouse_up(MouseButton::Left, { let up = up.clone(); move |_, _, cx| up.update(cx, |s, cx| { s.dragging = false; cx.notify(); }) })
            .on_mouse_up_out(MouseButton::Left, move |_, _, cx| up.update(cx, |s, cx| { s.dragging = false; cx.notify(); }))
            .on_click({ let state = state.clone(); move |_, window, cx| { let s = state.read(cx); if s.anchor == s.head { if let Some((_, url)) = links.iter().find(|(r, _)| r.contains(&s.head)) { open_link(&on_link, url, window, cx); } } } })
            .on_key_down(move |event, _, cx| { if event.keystroke.modifiers.secondary() { match event.keystroke.key.as_str() { "c" => { let r = keys.read(cx).range(); if !r.is_empty() { cx.write_to_clipboard(ClipboardItem::new_string(text[r].to_string())); cx.stop_propagation(); } }, "a" => keys.update(cx, |s, cx| { s.anchor = 0; s.head = text.len(); cx.notify(); }), _ => {} } } })
            .child(Paragraph { inline: self.inline, layout, selection: state })
    }
}
struct Paragraph { inline: Inline, layout: Rc<RefCell<Layout>>, selection: Entity<Selection> }
impl IntoElement for Paragraph { type Element = Self; fn into_element(self) -> Self { self } }
impl Element for Paragraph {
    type RequestLayoutState = (); type PrepaintState = ();
    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> { None }
    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height().max(font_size + px(4.0));
        let theme = cx.theme();
        let mut edges = vec![0, self.inline.text.len()];
        for (r, _) in &self.inline.styles { edges.extend([r.start, r.end]); }
        edges.sort_unstable(); edges.dedup();
        let runs: Vec<_> = edges.windows(2).map(|e| {
            let mut run_style = style.clone();
            let mut code = false;
            for (range, kind) in &self.inline.styles { if range.start <= e[0] && e[1] <= range.end { if *kind == Style::Code { code = true; } else { run_style = run_style.highlight(highlight(kind, &theme.colors, theme.underline_thickness())); } } }
            let mut run = run_style.to_run(e[1]-e[0]); if code { run.font.family = theme.mono_family.clone(); } run
        }).collect();
        let inline = self.inline.clone(); let layout = self.layout.clone();
        let code_run_fonts = runs.iter().filter(|r| r.font.family == theme.mono_family).map(|r| r.font.family.clone()).collect::<Vec<_>>();
        let id = window.request_measured_layout(gpui::Style::default(), move |known, available, window, _| {
            let width = known.width.or(match available.width { AvailableSpace::Definite(w) => Some(w), _ => None }).unwrap_or(px(100000.0)).max(px(1.0));
            let text = inline.text.trim_end_matches('\n').replace('\n', " " );
            let shaped = window.text_system().shape_line(SharedString::from(text.clone()), font_size, &runs, None);
            let codes: Vec<_> = inline.styles.iter().filter(|(_, s)| *s == Style::Code).map(|(r, _)| r.clone()).collect();
            let is_code = |at| codes.iter().any(|r| r.contains(&at));
            let chars: Vec<_> = text.grapheme_indices(true).map(|(at, g)| (at, at+g.len(), g)).collect();
            let mut rows = Vec::new(); let mut start = 0;
            while start < chars.len() {
                let mut end = start; let mut used = Pixels::ZERO; let mut candidate = None;
                while end < chars.len() {
                    let (a,b,g) = chars[end]; let code = is_code(a);
                    let edge = if code && (end == start || !is_code(chars[end-1].0)) { px(PAD) } else { Pixels::ZERO };
                    let closing = if code { px(PAD) } else { Pixels::ZERO };
                    let advance = shaped.x_for_index(b)-shaped.x_for_index(a);
                    if used + edge + advance + closing > width && end > start { if let Some(c) = candidate { end = c; } break; }
                    used += edge + advance;
                    if code && (end+1 == chars.len() || !is_code(chars[end+1].0)) { used += px(PAD); }
                    end += 1;
                    if g.chars().all(char::is_whitespace) || !g.is_ascii() { candidate = Some(end); }
                }
                end = end.max(start+1); rows.push(chars[start].0..chars[end-1].1); start=end;
            }
            let mut pieces = Vec::new(); let mut cursor = shaped.cursor();
            for (row, range) in rows.iter().enumerate() {
                let mut at=range.start; let mut x=Pixels::ZERO;
                while at < range.end {
                    let code=is_code(at); let end=codes.iter().flat_map(|r| [r.start,r.end]).filter(|e| *e>at && *e<range.end).min().unwrap_or(range.end);
                    let line=cursor.take_until(end); if code { x += px(PAD); }
                    let advance=line.width(); pieces.push(Piece { range: at..end, line, origin: point(x,line_height*row), code });
                    x += advance; if code { x += px(PAD); } at=end;
                }
            }
            let height=line_height*rows.len().max(1); *layout.borrow_mut()=Layout { pieces,bounds:Bounds::default(),line_height, code_run_fonts: code_run_fonts.clone() }; size(width,height)
        }); (id,())
    }
    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut Window, cx: &mut App) {
        self.layout.borrow_mut().bounds=bounds;
        #[cfg(feature = "test-support")]
        {
            let l = self.layout.borrow();
            let snapshot = ReaderInlineLayout {
                text: self.inline.text.clone(), bounds, line_height: l.line_height,
                code_font: cx.theme().mono_family.clone(),
                code_run_fonts: l.code_run_fonts.clone(),
                pieces: l.pieces.iter().map(|p| ReaderInlinePiece {
                    range: p.range.clone(), code: p.code,
                    text_bounds: Bounds::new(bounds.origin+p.origin, size(p.line.width(), l.line_height)),
                    box_bounds: Bounds::new(bounds.origin+p.origin-point(px(PAD),px(-1.0)), size(p.line.width()+px(PAD*2.0),l.line_height-px(2.0))),
                    fonts: p.line.runs.iter().map(|r| r.font_id).collect(),
                }).collect(),
            };
            if !cx.has_global::<ReaderInlineLayouts>() { cx.set_global(ReaderInlineLayouts::default()); }
            cx.global_mut::<ReaderInlineLayouts>().0.insert(snapshot.text.clone(), snapshot);
        }
        #[cfg(not(feature = "test-support"))]
        let _ = cx;
    }
    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        let l=self.layout.borrow(); let selected=self.selection.read(cx).range(); let theme=cx.theme(); let bg=theme.colors.sunken; let selection=theme.colors.selection;
        for p in &l.pieces {
            let origin=l.bounds.origin+p.origin;
            if p.code { let bounds=Bounds::new(origin-point(px(PAD),px(-1.0)),size(p.line.width()+px(PAD*2.0),l.line_height-px(2.0))); window.paint_quad(gpui::quad(bounds,px(4.0),bg,gpui::Edges::default(),gpui::transparent_black(),gpui::BorderStyle::default())); }
            let a=selected.start.max(p.range.start); let b=selected.end.min(p.range.end);
            if a<b { let x=p.line.x_for_index(a-p.range.start); let end=p.line.x_for_index(b-p.range.start); window.paint_quad(fill(Bounds::new(origin+point(x,px(0.0)),size(end-x,l.line_height)),selection)); }
            let _=p.line.paint_background(origin,l.line_height,gpui::TextAlign::Left,None,window,cx);
            let _=p.line.paint(origin,l.line_height,gpui::TextAlign::Left,None,window,cx);
        }
    }
}
/// Last actual paragraph layouts on the test platform; no extra measurement.
#[cfg(feature = "test-support")]
#[derive(Clone, Debug)]
pub struct ReaderInlinePiece { pub range: Range<usize>, pub code: bool, pub text_bounds: Bounds<Pixels>, pub box_bounds: Bounds<Pixels>, pub fonts: Vec<gpui::FontId> }
#[cfg(feature = "test-support")]
#[derive(Clone, Debug)]
pub struct ReaderInlineLayout { pub text: String, pub bounds: Bounds<Pixels>, pub line_height: Pixels, pub code_font: SharedString, pub code_run_fonts: Vec<SharedString>, pub pieces: Vec<ReaderInlinePiece> }
#[cfg(feature = "test-support")]
#[derive(Default)]
struct ReaderInlineLayouts(std::collections::HashMap<String, ReaderInlineLayout>);
#[cfg(feature = "test-support")]
impl gpui::Global for ReaderInlineLayouts {}
#[cfg(feature = "test-support")]
pub fn reader_inline_layout(text: &str, cx: &App) -> Option<ReaderInlineLayout> { cx.try_global::<ReaderInlineLayouts>()?.0.get(text).cloned() }
