//! Acceptance tests for README picture loading in the reusable reader.
//!
//! They run the **real** pipeline: the account service over a temporary SQLite
//! directory and a synthetic PAT, the fixed-commit tree/blob walk, the bounded
//! decoder and the reader's own picture board. Only the two external boundaries
//! are injected (the GitHub transport and the public HTTPS picture source), so
//! no test depends on a network or a real credential.
//!
//! Windows native input and real GPU rendering remain manual checks; this suite
//! proves the state machine, the fixed-commit resolution, cancellation and the
//! measured layout, not the pixels on a device.

use std::time::Duration;

use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Styled, TestAppContext,
    VisualTestContext, Window, div, px, size,
};

use crate::fixture::{BANNER_URL, COMMIT, LOGO_URL, OVERSIZE_URL, Pipeline, Scene};
use crate::reader::{ImageKey, ImageTarget, ReaderView};

pub(super) fn setup(cx: &mut TestAppContext) {
    cx.update(ely_core::init_for_tests);
    cx.update(|cx| {
        ely_core::theme::Theme::update(cx, |theme| {
            theme.reduced_motion = true;
            theme.font_family = "Noto Sans".into();
            theme.mono_family = "Google Sans Code".into();
        })
    });
}

pub(super) fn frames(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        // The reader's picture pump is a light timer on the app's own
        // background executor, so the clock has to move for a beat to run.
        cx.background_executor
            .advance_clock(Duration::from_millis(25));
        cx.run_until_parked();
    }
}

/// Opens a scene in a real window view and returns the reader.
pub(super) fn open(
    cx: &mut TestAppContext,
    scene: Scene,
) -> (Pipeline, Entity<ReaderView>, &mut VisualTestContext) {
    setup(cx);
    let pipeline = Pipeline::new(scene);
    let opened = pipeline.open(scene, Scene::limits(scene));
    let (reader, cx) = cx.add_window_view(|window, cx| {
        let (reader, _provider) = crate::fixture::open_reader(opened, window, cx);
        reader
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(900.0), px(700.0)));
    frames(cx);
    (pipeline, reader, cx)
}

/// Runs frames until `done` holds, or panics with the reader's actual states.
pub(super) fn settle(
    reader: &Entity<ReaderView>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&ReaderView) -> bool,
) {
    for _ in 0..600 {
        frames(cx);
        std::thread::sleep(Duration::from_millis(2));
        if reader.read_with(cx, |reader, _| done(reader)) {
            return;
        }
    }
    let known = reader.read_with(cx, |reader, _| reader.image_states_debug());
    panic!("the reader did not settle; picture states: {known:?}");
}

fn repository_key(path: &str) -> ImageKey {
    ImageTarget::Repository {
        owner: crate::fixture::OWNER.into(),
        name: crate::fixture::NAME.into(),
        commit: COMMIT.into(),
        path: path.into(),
    }
    .key()
}

fn external_key(url: &str) -> ImageKey {
    ImageTarget::External { url: url.into() }.key()
}

fn state_of(reader: &Entity<ReaderView>, cx: &mut VisualTestContext, key: &ImageKey) -> String {
    reader
        .read_with(cx, |reader, _| reader.image_state(key))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Every picture block that actually painted, by top-level block index.
fn painted_picture_blocks(reader: &Entity<ReaderView>, cx: &mut VisualTestContext) -> Vec<usize> {
    let blocks = reader.read_with(cx, |reader, cx| {
        reader
            .markdown()
            .map(|document| document.read(cx).block_count())
            .unwrap_or(0)
    });
    (0..blocks)
        .filter(|index| {
            let selector: &'static str =
                Box::leak(format!("markdown-picture-note-{index}").into_boxed_str());
            cx.debug_bounds(selector).is_some()
        })
        .collect()
}

// ---- acceptance: the healthy scene ----

#[gpui::test]
fn a_healthy_document_loads_relative_root_external_and_delayed_pictures(cx: &mut TestAppContext) {
    let (pipeline, reader, cx) = open(cx, Scene::Healthy);
    let relative = repository_key("docs/images/diagram.png");
    let banner = repository_key("docs/images/banner.png");
    let logo = external_key(LOGO_URL);

    settle(&reader, cx, |reader| {
        reader.image_state(&relative).as_deref() == Some("ready")
            && reader.image_state(&banner).as_deref() == Some("ready")
            && reader.image_state(&logo).as_deref() == Some("ready")
    });
    // Every picture is eventually ready, including the delayed one.
    let slow = repository_key("docs/images/slow.png");
    settle(&reader, cx, |reader| {
        reader.image_state(&slow).as_deref() == Some("ready")
    });

    // The pictures became real drawn elements.
    frames(cx);
    let painted = painted_picture_blocks(&reader, cx);
    assert!(
        painted.len() >= 2,
        "every visible picture block draws: {painted:?}"
    );

    // Every repository read named the fixed commit and the resolved path.
    let urls = pipeline.transport().recorded();
    assert!(
        urls.iter()
            .any(|url| url.contains(&format!("commits/{COMMIT}"))),
        "{urls:?}"
    );
    assert!(
        urls.iter().any(|url| url.contains("/git/blobs/")),
        "pictures are read as blobs, not through contents/download_url: {urls:?}"
    );
    assert!(
        !urls.iter().any(|url| url.contains("download_url")),
        "{urls:?}"
    );
    assert!(
        pipeline.transport().mismatches().is_empty(),
        "{:?}",
        pipeline.transport().mismatches()
    );
    // The public picture was answered by the injected source, not by GitHub.
    assert!(
        pipeline
            .external()
            .reads()
            .iter()
            .any(|url| url == LOGO_URL)
    );
}

#[gpui::test]
fn pictures_keep_their_alt_text_and_stay_inside_the_reading_column(cx: &mut TestAppContext) {
    let (_pipeline, reader, cx) = open(cx, Scene::Extremes);
    let wide = repository_key("docs/images/banner.png");
    settle(&reader, cx, |reader| {
        reader.image_state(&wide).as_deref() == Some("ready")
    });
    frames(cx);
    let root = cx.debug_bounds("reader-markdown").expect("root");
    // The 1600px banner and the tall figure both stay inside the column.
    let narrowest = reader.read_with(cx, |reader, cx| {
        let document = reader.markdown().expect("markdown").read(cx);
        document.document_text().to_string()
    });
    assert!(narrowest.contains("Extremes"));
    let _ = root;
    // The banner's own block is measured; a width wider than the column would
    // have been clipped rather than measured inside it.
    for index in 0..reader.read_with(cx, |reader, cx| {
        reader.markdown().expect("markdown").read(cx).block_count()
    }) {
        if let Some(bounds) = reader.read_with(cx, |reader, cx| {
            reader
                .markdown()
                .expect("markdown")
                .read(cx)
                .painted_content_bounds(index)
        }) {
            assert!(
                bounds.left() >= root.left() - px(1.0) && bounds.right() <= root.right() + px(1.0),
                "block {index} escaped the reading column: {bounds:?} vs {root:?}"
            );
        }
    }
}

#[gpui::test]
fn a_missing_or_refused_picture_shows_an_honest_placeholder(cx: &mut TestAppContext) {
    let (_pipeline, reader, cx) = open(cx, Scene::Failure);
    let text = repository_key("docs/images/notes.txt");
    let missing = repository_key("docs/images/missing.png");
    settle(&reader, cx, |reader| {
        reader
            .image_state(&text)
            .is_some_and(|state| state != "loading")
            && reader
                .image_state(&missing)
                .is_some_and(|state| state != "loading")
    });
    assert_eq!(
        state_of(&reader, cx, &text),
        "unsupported picture format",
        "a text file is not silently accepted"
    );
    assert_eq!(state_of(&reader, cx, &missing), "picture not found");
    // Every picture block still draws, with its alt text.
    frames(cx);
    let painted = painted_picture_blocks(&reader, cx);
    assert!(
        painted.len() >= 3,
        "a failed picture still draws its placeholder: {painted:?}"
    );
}

#[gpui::test]
fn oversized_pictures_are_refused_before_they_can_be_drawn(cx: &mut TestAppContext) {
    let (_pipeline, reader, cx) = open(cx, Scene::Oversize);
    let huge = repository_key("docs/images/huge.png");
    let canvas = external_key(OVERSIZE_URL);
    settle(&reader, cx, |reader| {
        let huge = reader.image_state(&huge);
        let canvas = reader.image_state(&canvas);
        matches!(huge.as_deref(), Some(label) if label != "loading")
            && matches!(canvas.as_deref(), Some(label) if label != "loading")
    });
    let huge = state_of(&reader, cx, &huge);
    let canvas = state_of(&reader, cx, &canvas);
    assert_eq!(huge, "picture too large", "{huge}");
    assert_eq!(canvas, "picture has too many pixels", "{canvas}");
}

#[gpui::test]
fn a_long_document_lazily_loads_only_the_pictures_it_scrolls_to(cx: &mut TestAppContext) {
    let (_pipeline, reader, cx) = open(cx, Scene::LongDocument);
    let diagram = repository_key("docs/images/diagram.png");
    // The first picture is visible, so it loads; the banner is far below.
    let banner = external_key(BANNER_URL);
    settle(&reader, cx, |reader| {
        reader.image_state(&diagram).as_deref() == Some("ready")
    });
    assert_eq!(
        state_of(&reader, cx, &banner),
        "unknown",
        "a picture below the viewport must not be requested yet"
    );
    // Scroll to the end: the far picture is met and loads.
    reader.update_in(cx, |reader, _window, cx| {
        let document = reader.markdown().expect("markdown").clone();
        document.update(cx, |document, cx| {
            document.list_state().scroll_to_end();
            cx.notify();
        });
    });
    frames(cx);
    let blocks = painted_picture_blocks(&reader, cx);
    assert!(
        !blocks.is_empty(),
        "the last picture block is visible after scrolling to the end"
    );
    settle(&reader, cx, |reader| {
        reader.image_state(&banner).as_deref() == Some("ready")
    });
}

#[gpui::test]
fn switching_documents_discards_a_picture_that_is_still_in_flight(cx: &mut TestAppContext) {
    let (pipeline, reader, cx) = open(cx, Scene::Switch);
    // `slow.png` takes real time to answer, so the switch below happens while
    // the first document's picture is genuinely in flight.
    let slow = repository_key("docs/images/slow.png");
    settle(&reader, cx, |reader| reader.image_state(&slow).is_some());
    assert_eq!(state_of(&reader, cx, &slow), "loading");

    let opened = pipeline.open_document(
        crate::fixture::Document::second(),
        Scene::limits(Scene::Switch),
    );
    let next_key = repository_key("docs/images/diagram.png");
    reader.update_in(cx, |reader, window, cx| {
        reader.show(opened.content.body, opened.document.kind(), window, cx);
        reader.set_image_provider(
            Some(opened.provider.clone() as std::sync::Arc<dyn crate::reader::ImageProvider>),
            cx,
        );
    });
    // The new document's own picture resolves under its own fixed commit.
    settle(&reader, cx, |reader| {
        reader.image_state(&next_key).as_deref() == Some("ready")
    });

    // Wait past the old document's delay. Its completion may arrive, but it
    // belongs to the previous generation and must be dropped.
    let until = std::time::Instant::now() + crate::fixture::SLOW_PICTURE_DELAY * 2;
    while std::time::Instant::now() < until {
        frames(cx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let states = reader.read_with(cx, |reader, _| reader.image_states_debug());
    assert!(
        states
            .iter()
            .all(|(key, _)| !key.ends_with("docs/images/slow.png")),
        "an old document's slow picture must not be published into the new one: {states:?}"
    );
    assert!(
        states
            .iter()
            .any(|(key, state)| key.ends_with("docs/images/diagram.png") && *state == "ready"),
        "the new document's picture is the one that stays: {states:?}"
    );
}

#[gpui::test]
fn a_reader_without_a_provider_keeps_an_honest_placeholder(cx: &mut TestAppContext) {
    setup(cx);
    let pipeline = Pipeline::new(Scene::Healthy);
    let opened = pipeline.open(Scene::Healthy, Scene::limits(Scene::Healthy));
    let context = opened.context.clone();
    let body = opened.content.body;
    let kind = opened.document.kind();
    let (reader, cx) =
        cx.add_window_view(|window, cx| ReaderView::new(body, kind, context, window, cx));
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(900.0), px(700.0)));
    frames(cx);
    let relative = repository_key("docs/images/diagram.png");
    assert!(
        reader
            .read_with(cx, |reader, _| reader.image_state(&relative))
            .is_none(),
        "without a provider nothing is requested"
    );
    // The placeholder is still drawn and says so.
    let painted = painted_picture_blocks(&reader, cx);
    assert!(!painted.is_empty(), "the placeholder blocks still draw");
    assert!(cx.debug_bounds("reader-markdown").is_some());
}

/// A host that draws two independent readers side by side, the way two tabs
/// of the same file would be two different reader entities.
struct TwoReaders {
    left: Entity<ReaderView>,
    right: Entity<ReaderView>,
}

impl gpui::Render for TwoReaders {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(div().flex_1().min_w_0().h_full().child(self.left.clone()))
            .child(div().flex_1().min_w_0().h_full().child(self.right.clone()))
    }
}

#[gpui::test]
fn two_readers_showing_the_same_picture_keep_independent_state(cx: &mut TestAppContext) {
    setup(cx);
    let pipeline = Pipeline::new(Scene::Healthy);
    let first = pipeline.open(Scene::Healthy, Scene::limits(Scene::Healthy));
    let second = pipeline.open(Scene::Healthy, Scene::limits(Scene::Healthy));
    let relative = repository_key("docs/images/diagram.png");

    let (host, cx) = cx.add_window_view(|window, cx| {
        let left = cx.new(|cx| crate::fixture::open_reader_entity(first, window, cx));
        let right = cx.new(|cx| crate::fixture::open_reader_entity(second, window, cx));
        TwoReaders { left, right }
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1200.0), px(700.0)));
    frames(cx);

    let (left, right) = host.read_with(cx, |host, _| (host.left.clone(), host.right.clone()));
    settle(&left, cx, |reader| {
        reader.image_state(&relative).as_deref() == Some("ready")
    });
    settle(&right, cx, |reader| {
        reader.image_state(&relative).as_deref() == Some("ready")
    });

    // Replacing one reader's document must not disturb the other reader, even
    // though both showed the same source.
    left.update_in(cx, |reader, window, cx| {
        reader.show(
            crate::content::PreparedBody::Empty,
            crate::reader::DocumentKind::Markdown,
            window,
            cx,
        );
    });
    frames(cx);
    assert!(
        left.read_with(cx, |reader, _| reader.image_state(&relative))
            .is_none(),
        "the replaced reader dropped its picture state"
    );
    assert_eq!(
        right.read_with(cx, |reader, _| reader.image_state(&relative)),
        Some("ready".to_string()),
        "the other reader keeps its own decoded picture"
    );
}

#[gpui::test]
fn a_picture_becoming_ready_remeasures_its_block_without_moving_the_viewport(
    cx: &mut TestAppContext,
) {
    let (_pipeline, reader, cx) = open(cx, Scene::LongDocument);
    let diagram = repository_key("docs/images/diagram.png");
    settle(&reader, cx, |reader| {
        reader.image_state(&diagram).as_deref() == Some("ready")
    });
    let before = reader.read_with(cx, |reader, cx| {
        reader.markdown().expect("markdown").read(cx).top_block()
    });
    frames(cx);
    let after = reader.read_with(cx, |reader, cx| {
        reader.markdown().expect("markdown").read(cx).top_block()
    });
    assert_eq!(
        before, after,
        "a picture landing must not jump the reading position"
    );
}

/// Diagnostic: the image block must be measured at its own size and must not
/// swallow the paragraphs around it.
#[gpui::test]
fn picture_blocks_measure_next_to_their_paragraphs(cx: &mut TestAppContext) {
    let (_pipeline, reader, cx) = open(cx, Scene::Healthy);
    let relative = repository_key("docs/images/diagram.png");
    settle(&reader, cx, |reader| {
        reader.image_state(&relative).as_deref() == Some("ready")
    });
    cx.simulate_resize(size(px(900.0), px(1400.0)));
    frames(cx);

    let blocks = reader.read_with(cx, |reader, cx| {
        reader
            .markdown()
            .map(|document| document.read(cx).block_count())
    });
    let paragraph = (0..blocks.unwrap_or(0)).find_map(|index| {
        let selector: &'static str =
            Box::leak(format!("markdown-paragraph-{index}").into_boxed_str());
        cx.debug_bounds(selector).map(|bounds| (index, bounds))
    });
    let (_index, paragraph) = paragraph.expect("a paragraph near a picture");
    assert!(paragraph.size.height > px(1.0) && paragraph.size.height < px(80.0));
    let picture = cx
        .debug_bounds("markdown-picture-note-2")
        .expect("the picture block is measured");
    let frame = cx
        .debug_bounds("markdown-picture-frame-2")
        .expect("the picture frame is measured");

    // The 320x180 picture must be measured by its own aspect ratio, not fill
    // the list viewport: a 320px-wide picture is about 180px tall.
    assert!(
        frame.size.width <= px(330.0),
        "the picture frame is bounded by the intrinsic width: {frame:?}"
    );
    assert!(
        (frame.size.width / frame.size.height - 320.0 / 180.0).abs() < 0.1,
        "the picture keeps its aspect ratio: {frame:?}"
    );
    assert!(
        paragraph.origin.y + paragraph.size.height <= picture.origin.y + px(2.0),
        "the picture sits below the preceding paragraph: {paragraph:?} {picture:?}"
    );
}

/// Finding: a first frame with a provider queues demand after the pump already
/// settled for that render. The pictures must still reach the provider without
/// an unrelated redraw, so this test never calls `window.refresh()`.
#[gpui::test]
fn pictures_load_without_an_unrelated_redraw(cx: &mut TestAppContext) {
    setup(cx);
    let pipeline = Pipeline::new(Scene::Healthy);
    let opened = pipeline.open(Scene::Healthy, Scene::limits(Scene::Healthy));
    let (reader, cx) = cx.add_window_view(|window, cx| {
        let (reader, _provider) = crate::fixture::open_reader(opened, window, cx);
        reader
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(900.0), px(700.0)));
    let relative = repository_key("docs/images/diagram.png");
    // Only the executor's own pump beat is driven: no `refresh`, no resize and
    // no input after the first frame.
    for _ in 0..600 {
        cx.background_executor
            .advance_clock(Duration::from_millis(25));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
        if reader
            .read_with(cx, |reader, _| reader.image_state(&relative))
            .as_deref()
            == Some("ready")
        {
            return;
        }
    }
    panic!(
        "pictures never loaded without an unrelated redraw: {:?}",
        reader.read_with(cx, |reader, _| reader.image_states_debug())
    );
}

/// Finding: replacing an attached provider must re-ask the new provider instead
/// of leaving the old `Loading` states in place.
#[gpui::test]
fn replacing_an_attached_provider_asks_the_new_one(cx: &mut TestAppContext) {
    let (pipeline, reader, cx) = open(cx, Scene::Healthy);
    let relative = repository_key("docs/images/diagram.png");
    settle(&reader, cx, |reader| {
        reader.image_state(&relative).as_deref() == Some("ready")
    });

    // A provider for the same document replaces the first one.
    let replacement = pipeline.open(Scene::Healthy, Scene::limits(Scene::Healthy));
    reader.update_in(cx, |reader, _window, cx| {
        reader.set_image_provider(
            Some(replacement.provider.clone() as std::sync::Arc<dyn crate::reader::ImageProvider>),
            cx,
        );
    });
    // The new generation starts empty: the picture is either not known yet or
    // already re-requested, and it is never still "ready" from the old provider.
    let state = reader.read_with(cx, |reader, _| reader.image_state(&relative));
    assert!(
        state.is_none() || state.as_deref() == Some("loading"),
        "a replaced provider starts a new picture generation, got {state:?}"
    );
    settle(&reader, cx, |reader| {
        reader.image_state(&relative).as_deref() == Some("ready")
    });
}
