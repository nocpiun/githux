//! Typography, callout and link-cursor regressions for the Markdown reader.
//!
//! The numbers asserted here are the shared policy in
//! [`super::markdown::rhythm`], measured on the real laid-out blocks: headings
//! follow GitHub's Primer scale, adjacent prose blocks are exactly one block
//! gap apart (never two stacked margins), tight lists are tighter than loose
//! ones, and all five callouts keep their own tint, bar and title row.
//!
//! The pointer-cursor cases assert the painted link rectangles and the
//! document's hover state; the platform cursor itself is applied from exactly
//! those rectangles and is checked in the native window instead.

use gpui::{Bounds, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px};

use super::markdown::{AlertKind, Snapshot};
use super::tests::open;
use super::*;

/// `TextSize::Base` at 100% font scale.
const BODY: f32 = 13.0;

fn frames(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|w, cx| w.simulate_next_frame(cx));
    }
}

fn bound(cx: &mut VisualTestContext, selector: String) -> Option<Bounds<Pixels>> {
    let key: &'static str = Box::leak(selector.into_boxed_str());
    cx.debug_bounds(key)
}

fn snapshot(cx: &mut VisualTestContext, text: &str) -> Snapshot {
    cx.update(|_, cx| {
        super::markdown::snapshot(text, cx).unwrap_or_else(|| {
            panic!(
                "the paragraph {text:?} was laid out; known: {:?}",
                super::markdown::snapshot_texts(cx)
            )
        })
    })
}

fn open_markdown(
    cx: &mut TestAppContext,
    body: PreparedBody,
) -> (gpui::Entity<ReaderView>, &mut VisualTestContext) {
    open(cx, body, DocumentKind::Markdown)
}

/// The alert kinds of every quote block, in document order.
fn callout_kinds(text: &str) -> Vec<Option<AlertKind>> {
    super::markdown::parse(text)
        .blocks
        .into_iter()
        .filter_map(|block| match block {
            super::markdown::Block::Quote { kind, .. } => Some(kind),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn callouts_use_the_shared_policy_with_title_icon_and_breathing_room(cx: &mut TestAppContext) {
    let (_, cx) = open_markdown(cx, fixtures::profile());
    cx.simulate_resize(gpui::size(px(760.0), px(1400.0)));
    frames(cx);

    assert_eq!(
        callout_kinds(fixtures::PROFILE),
        vec![
            Some(AlertKind::Note),
            Some(AlertKind::Tip),
            Some(AlertKind::Important),
            Some(AlertKind::Warning),
            Some(AlertKind::Caution),
            None,
        ],
        "the parser keeps every alert kind and the plain quote"
    );

    // The quote blocks are the last six in a row; find their block indices by
    // looking for the frames the shared policy draws.
    let mut alert_frames = Vec::new();
    let mut neutral = Vec::new();
    for ix in 0..40 {
        if let Some(frame) = bound(cx, format!("markdown-quote-{ix}")) {
            let title = bound(cx, format!("markdown-quote-{ix}-title"));
            match title {
                Some(title) => {
                    assert!(
                        title.size.height >= px(BODY),
                        "alert {ix} draws its title row: {title:?}"
                    );
                    assert!(
                        frame.size.height > title.size.height + px(2.0 * BODY * 0.5),
                        "alert {ix} keeps vertical padding around its title: {frame:?}"
                    );
                    alert_frames.push((ix, frame));
                }
                None => neutral.push((ix, frame)),
            }
        }
    }
    assert_eq!(alert_frames.len(), 5, "one titled callout per alert kind");
    assert_eq!(neutral.len(), 1, "the plain quote stays neutral");
    // The accent bar and inset give the body a real left indent.
    let body = snapshot(cx, "A note body with a nested guide.");
    if let Some((_, frame)) = alert_frames.first() {
        assert!(
            body.bounds.left() >= frame.left() + px(BODY * 0.5),
            "the callout body is inset from its accent bar: {:?} / {frame:?}",
            body.bounds
        );
    }
}

#[gpui::test]
fn readme_rhythm_matches_the_github_scale(cx: &mut TestAppContext) {
    let (reader, cx) = open_markdown(cx, fixtures::lists());
    cx.simulate_resize(gpui::size(px(760.0), px(1400.0)));
    frames(cx);

    let lead = snapshot(cx, "lead paragraph");
    let tight_one = snapshot(cx, "tight one");
    let tight_two = snapshot(cx, "tight two");
    let loose_one = snapshot(cx, "loose one");
    let loose_two = snapshot(cx, "loose two");
    let parent = snapshot(cx, "parent");
    let child_one = snapshot(cx, "child one");
    let child_two = snapshot(cx, "child two");
    let after = snapshot(cx, "After the lists");
    let tail = snapshot(cx, "tail paragraph");

    let line = BODY * 1.6;
    let block = BODY * 0.85;

    // One prose line box, and one block gap between a paragraph and a list.
    assert!((f32::from(lead.line_height) - line).abs() < 0.5, "{lead:?}");
    let paragraph_to_list = f32::from(tight_one.bounds.top() - lead.bounds.bottom());
    assert!(
        (paragraph_to_list - block).abs() < 0.75,
        "paragraph -> list gap is one policy gap, not two stacked margins: {paragraph_to_list}"
    );

    // Tight list items sit on the Primer `li + li` gap; loose items take a
    // full paragraph gap instead.
    let tight_pitch = f32::from(tight_two.bounds.top() - tight_one.bounds.top());
    let loose_pitch = f32::from(loose_two.bounds.top() - loose_one.bounds.top());
    assert!(
        (tight_pitch - (line + BODY * 0.25)).abs() < 0.75,
        "{tight_pitch}"
    );
    assert!(
        loose_pitch > tight_pitch + 4.0,
        "a loose list keeps paragraph spacing: {loose_pitch} vs {tight_pitch}"
    );

    // A loose item with two paragraphs keeps the paragraph gap inside it,
    // even though the item column is not one of the top-level blocks.
    let item_first = snapshot(cx, "first paragraph");
    let item_second = snapshot(cx, "second paragraph of the same item");
    let inside_item = f32::from(item_second.bounds.top() - item_first.bounds.bottom());
    assert!(
        (inside_item - block).abs() < 0.75,
        "paragraphs inside one loose item keep their gap: {inside_item}"
    );

    // A nested list is indented and keeps the tight pitch.
    assert!(
        child_one.bounds.left() > parent.bounds.left() + px(BODY),
        "nested list indents: {:?} / {:?}",
        child_one.bounds,
        parent.bounds
    );
    let nested_pitch = f32::from(child_two.bounds.top() - child_one.bounds.top());
    assert!((nested_pitch - tight_pitch).abs() < 0.75, "{nested_pitch}");

    // A heading leads in with Primer's 24px-equivalent margin.
    let list_to_heading = f32::from(after.bounds.top() - item_second.bounds.bottom());
    assert!(
        (list_to_heading - BODY * 1.6).abs() < 1.0,
        "heading lead-in: {list_to_heading}"
    );
    assert!(
        tail.bounds.top() > after.bounds.top(),
        "the tail paragraph follows the heading"
    );

    // Primer's heading scale: h2 is 1.5em here, h1 is 2em, h3 is 1.25em.
    assert!(
        (f32::from(after.font_size) - BODY * 1.5).abs() < 0.5,
        "{after:?}"
    );
    let h1 = snapshot(cx, "Top one");
    let h3 = snapshot(cx, "Small three");
    assert!((f32::from(h1.font_size) - BODY * 2.0).abs() < 0.5, "{h1:?}");
    assert!(
        (f32::from(h3.font_size) - BODY * 1.25).abs() < 0.5,
        "{h3:?}"
    );

    // Selecting and copying still walks the model, bullets and all.
    let document = reader.read_with(cx, |r, _| r.markdown().unwrap().clone());
    document.update(cx, |document, cx| document.select_all(cx));
    let copied = reader.read_with(cx, |r, cx| r.markdown().unwrap().read(cx).selected_text());
    assert!(
        copied.contains("tight one") && copied.contains("child two"),
        "{copied:?}"
    );
}

#[gpui::test]
fn narrow_width_and_font_scale_keep_the_same_rhythm(cx: &mut TestAppContext) {
    let (_, cx) = open_markdown(cx, fixtures::profile());
    cx.simulate_resize(gpui::size(px(340.0), px(1600.0)));
    frames(cx);
    let note = snapshot(cx, "A note body with a nested guide.");
    assert!(
        note.bounds.right() <= note.bounds.left() + px(340.0),
        "narrow prose stays inside the column: {:?}",
        note.bounds
    );

    cx.update(|_, cx| {
        ely_core::theme::Theme::update(cx, |theme| theme.font_scale = 1.5);
    });
    frames(cx);
    let scaled = snapshot(cx, "A note body with a nested guide.");
    let body = f32::from(scaled.font_size);
    assert!(
        (body - BODY * 1.5).abs() < 0.5,
        "the body follows the theme font scale: {body}"
    );
    assert!(
        (f32::from(scaled.line_height) - body * 1.6).abs() < 0.5,
        "the line box scales with the body: {scaled:?}"
    );
}

#[gpui::test]
fn pointer_cursor_covers_only_real_links(cx: &mut TestAppContext) {
    let (reader, cx) = open_markdown(cx, fixtures::nested_links());
    cx.simulate_resize(gpui::size(px(760.0), px(700.0)));
    frames(cx);
    let document = reader.read_with(cx, |r, _| r.markdown().unwrap().clone());

    // Relative link + anchor navigate; the refused scheme keeps link styling
    // but paints no cursor target.
    let paragraph = snapshot(
        cx,
        "A paragraph with a relative link, plain words and a refused link and an anchor.",
    );
    assert_eq!(
        paragraph.link_boxes.len(),
        2,
        "relative link and anchor are interactive, the refused one is not: {:?}",
        paragraph.link_boxes
    );
    for rect in &paragraph.link_boxes {
        assert!(
            paragraph.bounds.contains(&rect.center()),
            "a link rectangle stays inside the paragraph: {rect:?}"
        );
        assert!(
            rect.size.width < paragraph.bounds.size.width,
            "a link never covers the whole paragraph: {rect:?}"
        );
    }

    let first = paragraph.link_boxes[0].center();
    cx.simulate_mouse_move(first, None, Modifiers::none());
    frames(cx);
    assert!(
        document.read_with(cx, |d, _| d.pointer_over_link()),
        "the pointer over a real link is interactive"
    );

    // Plain words beside the link are not.
    let plain = point(paragraph.bounds.left() + px(2.0), first.y);
    cx.simulate_mouse_move(plain, None, Modifiers::none());
    frames(cx);
    assert!(
        !document.read_with(cx, |d, _| d.pointer_over_link()),
        "non-link text is not a pointer target"
    );

    // Selecting from the link keeps the platform's own cursor.
    cx.simulate_mouse_down(first, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(first.x + px(40.0), first.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    frames(cx);
    assert!(
        !document.read_with(cx, |d, _| d.pointer_over_link()),
        "a selection drag never shows the pointing hand"
    );
    cx.simulate_mouse_up(first, MouseButton::Left, Modifiers::none());
    frames(cx);

    // Links inside a list, a quote and a table cell are all interactive.
    for text in ["item with a list link", "quoted quote link"] {
        let target = snapshot(cx, text).link_boxes[0].center();
        cx.simulate_mouse_move(target, None, Modifiers::none());
        frames(cx);
        assert!(
            document.read_with(cx, |d, _| d.pointer_over_link()),
            "{text:?} keeps its pointer target"
        );
    }
    let cell = snapshot(cx, "cell link");
    assert_eq!(cell.link_boxes.len(), 1, "table links are interactive");

    // Moving straight from one paragraph's link to another paragraph's link,
    // with no plain-text event in between, must not leave the arrow behind.
    // The cursor is declared by per-link rectangles rather than by a paint-time
    // mouse position, so no repaint is needed for that move to be correct.
    let list_link = snapshot(cx, "item with a list link").link_boxes[0].center();
    cx.simulate_mouse_move(list_link, None, Modifiers::none());
    cx.simulate_mouse_move(first, None, Modifiers::none());
    frames(cx);
    assert!(
        document.read_with(cx, |d, _| d.pointer_over_link()),
        "a direct link-to-link move keeps the pointer interactive"
    );
    // Both links own their own cursor target, so whichever one the pointer is
    // over the platform has a hitbox to read the style from.
    assert!(
        !snapshot(cx, "item with a list link").link_boxes.is_empty()
            && !snapshot(
                cx,
                "A paragraph with a relative link, plain words and a refused link and an anchor."
            )
            .link_boxes
            .is_empty(),
        "each paragraph keeps its own link target"
    );
}

#[gpui::test]
fn a_refused_scheme_is_styled_but_never_claims_the_pointer(cx: &mut TestAppContext) {
    let (reader, cx) = open_markdown(cx, fixtures::dangerous_link_only());
    cx.simulate_resize(gpui::size(px(700.0), px(400.0)));
    frames(cx);
    let document = reader.read_with(cx, |r, _| r.markdown().unwrap().clone());
    let paragraph = snapshot(cx, "do not run");
    assert!(
        paragraph.link_boxes.is_empty(),
        "a refused address paints no pointer target: {:?}",
        paragraph.link_boxes
    );
    let target = paragraph.pieces[0].text_bounds.center();
    cx.simulate_mouse_move(target, None, Modifiers::none());
    frames(cx);
    assert!(!document.read_with(cx, |d, _| d.pointer_over_link()));
    // Navigation policy is untouched: the press still reports a refused link.
    cx.simulate_click(target, Modifiers::none());
    cx.run_until_parked();
    assert!(matches!(
        reader.read_with(cx, |r, _| r.last_action().cloned()),
        Some(ReaderAction::Link(LinkTarget::Refused { .. }))
    ));
}

#[gpui::test]
fn a_wrapped_link_keeps_one_pointer_rectangle_per_row(cx: &mut TestAppContext) {
    let (_, cx) = open_markdown(cx, fixtures::profile());
    cx.simulate_resize(gpui::size(px(300.0), px(1600.0)));
    frames(cx);
    let paragraph = snapshot(
        cx,
        "Tail paragraph with a wrapped long link label that is long enough.",
    );
    assert!(
        paragraph.link_boxes.len() >= 2,
        "the wrapped link paints one rectangle per row: {:?}",
        paragraph.link_boxes
    );
    for rect in &paragraph.link_boxes {
        assert!(
            rect.right() <= paragraph.bounds.right() + px(1.0),
            "a wrapped link segment stays inside the column: {rect:?}"
        );
    }
}
