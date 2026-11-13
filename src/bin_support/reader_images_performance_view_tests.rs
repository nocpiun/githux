//! Combined viewport fixtures use the real account/cache/provider/decoder route.
#[path = "reader_images_swr_view_tests.rs"]
mod swr;
use super::*;
use crate::{
    content::images::ImageStage,
    performance_fixture::{PerformanceFixture, markdown},
    reader::{ImageLimits, ImageProvider, MarkdownLayout},
};
use gpui::{ScrollHandle, point, prelude::*};
use std::{collections::HashSet, sync::Arc};
fn target(i: usize) -> ImageTarget {
    ImageTarget::External {
        url: format!("https://originals.invalid/{i}.png"),
    }
}
struct Host {
    reader: Entity<ReaderView>,
    scroll: ScrollHandle,
    flow: bool,
}
impl gpui::Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "performance-host".to_string())
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("performance-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(self.flow, |element| {
                        element.overflow_y_scroll().track_scroll(&self.scroll)
                    })
                    .child(self.reader.clone()),
            )
    }
}
fn open_case(
    cx: &mut TestAppContext,
    table: bool,
    flow: bool,
    large: bool,
) -> (PerformanceFixture, Entity<Host>, &mut VisualTestContext) {
    let limits = ImageLimits {
        max_ready_images: 4,
        total_timeout: Duration::from_secs(120),
        ..Default::default()
    };
    let harness = PerformanceFixture::new(large, limits.clone());
    open_prepared(cx, harness, markdown(table), flow, large, limits)
}
fn open_prepared(
    cx: &mut TestAppContext,
    harness: PerformanceFixture,
    body: String,
    flow: bool,
    large: bool,
    limits: ImageLimits,
) -> (PerformanceFixture, Entity<Host>, &mut VisualTestContext) {
    setup(cx);
    let provider = harness.service.clone();
    let window = cx.open_window(size(px(640.0), px(400.0)), move |window, cx| {
        let weak = cx.entity().downgrade();
        let reader = cx.new(|cx| {
            let mut reader = ReaderView::new(
                crate::content::PreparedBody::Utf8 {
                    text: body,
                    has_bom: false,
                },
                crate::reader::DocumentKind::Markdown,
                crate::reader::ResourceContext::new(
                    crate::fixture::OWNER,
                    crate::fixture::NAME,
                    COMMIT,
                    crate::fixture::README_PATH,
                ),
                window,
                cx,
            );
            reader.set_markdown_layout(
                if flow {
                    MarkdownLayout::Flow
                } else {
                    MarkdownLayout::Repository
                },
                cx,
            );
            reader.set_image_limits(limits, cx);
            reader.set_image_provider(Some(provider as Arc<dyn ImageProvider>), cx);
            reader.on_layout_changed(std::rc::Rc::new(move |_, cx| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            }));
            reader
        });
        Host {
            reader,
            scroll: ScrollHandle::new(),
            flow,
        }
    });
    let host = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    if large {
        cx.simulate_scale_factor_change(2.0);
    }
    cx.update(|window, _| window.activate_window());
    frames(cx);
    (harness, host, cx)
}
fn settle_current(reader: &Entity<ReaderView>, cx: &mut VisualTestContext) {
    for _ in 0..12000 {
        frames(cx);
        if reader.read_with(cx, |reader, _| !reader.has_pending_images()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "current demand failed to settle: {:?}",
        reader.read_with(cx, |reader, _| reader.image_states_debug())
    );
}
fn scroll(host: &Entity<Host>, cx: &mut VisualTestContext, top: bool) {
    host.update_in(cx, |host, _, cx| {
        if host.flow {
            let y = if top {
                px(0.0)
            } else {
                host.scroll.offset().y - px(280.0)
            };
            host.scroll.set_offset(point(px(0.0), y));
        } else {
            let document = host.reader.read(cx).markdown().unwrap().clone();
            document.update(cx, |document, cx| {
                document.motion().cancel();
                if top {
                    document.list_state().scroll_to(gpui::ListOffset {
                        item_ix: 0,
                        offset_in_item: px(0.0),
                    });
                } else {
                    document.list_state().scroll_by(px(280.0));
                }
                cx.notify();
            });
        }
        cx.notify();
    });
    frames(cx);
}
fn run_case(cx: &mut TestAppContext, table: bool, flow: bool, large: bool) {
    let process_before = crate::performance_fixture::process_memory();
    let (harness, host, cx) = open_case(cx, table, flow, large);
    let reader = host.read_with(cx, |host, _| host.reader.clone());
    settle_current(&reader, cx);
    if flow {
        let (offset, reach) = host.read_with(cx, |host, _| {
            (host.scroll.offset(), host.scroll.max_offset())
        });
        assert!(
            reach.y > px(0.0),
            "Flow host must have real scroll reach: offset={offset:?}, reach={reach:?}, reader={:?}, document={:?}, host={:?}",
            cx.debug_bounds("reader"),
            cx.debug_bounds("reader-flow"),
            cx.debug_bounds("performance-host")
        );
    }
    let initial_reads = harness.source.reads.lock().unwrap().len();
    assert!(
        initial_reads < 9,
        "rendering a table/Flow document must not demand every offscreen cell: {initial_reads}"
    );
    let mut seen = HashSet::new();
    for _ in 0..48 {
        settle_current(&reader, cx);
        for i in 0..9 {
            if reader
                .read_with(cx, |reader, _| reader.image_state(&target(i).key()))
                .as_deref()
                == Some("ready")
            {
                seen.insert(i);
            }
        }
        let memory = reader.read_with(cx, |reader, _| reader.image_memory_debug());
        assert!(memory.0 <= 64 * 1024 * 1024);
        if seen.len() == 9 {
            break;
        }
        scroll(&host, cx, false);
    }
    assert_eq!(
        seen.len(),
        9,
        "every original makes progress as the viewport reaches it"
    );
    let before = harness.source.reads.lock().unwrap().len();
    let cache = crate::data::reader_image_cache::ReaderImageCache::new(
        harness.store.clone(),
        harness.scope.clone(),
        Some(42),
        false,
    );
    for i in 0..9 {
        assert_eq!(
            cache
                .read_external_image(&format!("https://originals.invalid/{i}.png"))
                .unwrap()
                .unwrap()
                .bytes,
            *harness.source.bytes,
            "persistent cache keeps the original encoded resource"
        );
    }
    scroll(&host, cx, true);
    settle_current(&reader, cx);
    assert_eq!(
        reader
            .read_with(cx, |reader, _| reader.image_state(&target(0).key()))
            .as_deref(),
        Some("ready")
    );
    assert_eq!(
        harness.source.reads.lock().unwrap().len(),
        before,
        "scroll-back reads fresh original cache bytes"
    );
    let decode = harness.service.metrics().stage(ImageStage::Decode).count;
    let memory = reader.read_with(cx, |reader, _| reader.image_memory_debug());
    for _ in 0..15 {
        frames(cx);
    }
    assert_eq!(
        harness.service.metrics().stage(ImageStage::Decode).count,
        decode,
        "settled scrolling must not decode/reload per frame"
    );
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.image_memory_debug()),
        memory
    );
    let text = reader.read_with(cx, |reader, cx| {
        reader
            .markdown()
            .unwrap()
            .read(cx)
            .document_text()
            .to_string()
    });
    for i in 0..9 {
        assert!(text.contains(&format!("Screenshot {i}")));
    }
    eprintln!(
        "viewport_case: table={table} flow={flow} original={} initial_reads={initial_reads} reads={before} retained={} peak_incoming_pixels={} evictions={} reloads={} decodes={decode}",
        harness.source.bytes.len(),
        memory.0,
        memory.1,
        memory.2,
        memory.3
    );
    eprintln!(
        "viewport_process_memory: native_gpu=false before={process_before:?} after={:?}",
        crate::performance_fixture::process_memory()
    );
    cx.update(|window, _| window.remove_window());
    drop(reader);
    drop(host);
    cx.run_until_parked();
    drop(harness);
}
#[gpui::test]
fn performance_viewport_table_repository_does_not_request_all_rendered_cells(
    cx: &mut TestAppContext,
) {
    run_case(cx, true, false, false);
}
#[gpui::test]
fn performance_viewport_table_flow_tracks_the_outer_scroll(cx: &mut TestAppContext) {
    run_case(cx, true, true, false);
}
#[gpui::test]
fn performance_viewport_rows_repository_recovers_without_churn(cx: &mut TestAppContext) {
    run_case(cx, false, false, false);
}
#[gpui::test]
fn performance_viewport_rows_flow_recovers_without_churn(cx: &mut TestAppContext) {
    run_case(cx, false, true, false);
}
#[gpui::test]
#[ignore = "explicit large original-byte Reader/Flow combined verification"]
fn performance_original_host_table_repository(cx: &mut TestAppContext) {
    run_case(cx, true, false, true);
}
#[gpui::test]
#[ignore = "explicit large original-byte Reader/Flow combined verification"]
fn performance_original_host_table_flow(cx: &mut TestAppContext) {
    run_case(cx, true, true, true);
}
#[gpui::test]
#[ignore = "explicit large original-byte Reader/Flow combined verification"]
fn performance_original_host_rows_repository(cx: &mut TestAppContext) {
    run_case(cx, false, false, true);
}
#[gpui::test]
#[ignore = "explicit original-byte resize/DPI combined verification"]
fn performance_original_host_resizes_columns_and_recovers_dpi_from_cache(cx: &mut TestAppContext) {
    let (harness, host, cx) = open_case(cx, true, false, true);
    let reader = host.read_with(cx, |host, _| host.reader.clone());
    let key = target(0).key();
    let selector = "markdown-cell-1-1-0-picture-0-frame";
    settle_current(&reader, cx);
    let first = cx.debug_bounds(selector).expect("first table image paints");
    let first_grid = reader
        .read_with(cx, |r, _| r.image_grid_debug(&key))
        .unwrap();
    let expected = (f32::from(first.size.width) * 2.0).ceil() as u32;
    assert!(
        first_grid >= expected && first_grid <= expected + 3,
        "first display grid {first_grid} for {first:?}"
    );
    let first_fetches = harness
        .source
        .reads
        .lock()
        .unwrap()
        .iter()
        .filter(|url| url.ends_with("/0.png"))
        .count();
    cx.simulate_resize(size(px(900.0), px(400.0)));
    frames(cx);
    settle_current(&reader, cx);
    let wider = cx.debug_bounds(selector).unwrap();
    let wider_grid = reader
        .read_with(cx, |r, _| r.image_grid_debug(&key))
        .unwrap();
    assert!(wider.size.width > first.size.width);
    assert!(wider_grid > first_grid);
    cx.simulate_scale_factor_change(3.0);
    frames(cx);
    settle_current(&reader, cx);
    let denser = cx.debug_bounds(selector).unwrap();
    let denser_grid = reader
        .read_with(cx, |r, _| r.image_grid_debug(&key))
        .unwrap();
    let expected = ((f32::from(denser.size.width) * 3.0).ceil() as u32).min(2559);
    assert!(
        denser_grid >= expected && denser_grid <= expected + 3,
        "DPI display grid {denser_grid} for {denser:?}"
    );
    assert!(denser_grid > wider_grid);
    assert_eq!(
        harness
            .source
            .reads
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.ends_with("/0.png"))
            .count(),
        first_fetches,
        "column/DPI regeneration uses original cache bytes"
    );
    let decode = harness.service.metrics().stage(ImageStage::Decode).count;
    for _ in 0..15 {
        frames(cx);
    }
    assert_eq!(
        harness.service.metrics().stage(ImageStage::Decode).count,
        decode
    );
    eprintln!(
        "viewport_resize: first={first_grid} wider={wider_grid} denser={denser_grid}, memory={:?}",
        reader.read_with(cx, |r, _| r.image_memory_debug())
    );
    cx.update(|window, _| window.remove_window());
    drop(reader);
    drop(host);
    cx.run_until_parked();
    drop(harness);
}
#[gpui::test]
#[ignore = "explicit large original-byte Reader/Flow combined verification"]
fn performance_original_host_rows_flow(cx: &mut TestAppContext) {
    run_case(cx, false, true, true);
}
