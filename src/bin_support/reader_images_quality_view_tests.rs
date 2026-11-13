use super::*;
use crate::reader::{
    ImageCompletion, ImageProvider, ImageRequest, ImageScope, ImageSink, MarkdownLayout,
};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Delayed {
    pending: Mutex<Vec<(ImageRequest, ImageSink, crate::reader::ImageRenderOptions)>>,
    options: Mutex<Vec<crate::reader::ImageRenderOptions>>,
}
impl ImageProvider for Delayed {
    fn start(&self, request: ImageRequest, sink: ImageSink) {
        self.start_with_options(request, sink, crate::reader::ImageRenderOptions::default());
    }
    fn start_with_options(
        &self,
        request: ImageRequest,
        sink: ImageSink,
        options: crate::reader::ImageRenderOptions,
    ) {
        self.options.lock().unwrap().push(options);
        self.pending.lock().unwrap().push((request, sink, options));
    }
    fn cancel(&self, _: ImageScope) {}
}
impl Delayed {
    fn complete_one(&self) -> bool {
        let next = {
            let mut pending = self.pending.lock().unwrap();
            if pending.is_empty() {
                return false;
            }
            pending.remove(0)
        };
        let (request, sink, options) = next;
        let bytes = if matches!(&request.target,ImageTarget::External{url} if url.ends_with(".png"))
        {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                80,
                40,
                image::Rgba([255, 0, 0, 255]),
            ))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
            out.into_inner()
        } else {
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40"><rect width="80" height="40" fill="red"/></svg>"#.to_vec()
        };
        let result = crate::content::images::decode_image_with_options_checked(
            bytes,
            &crate::reader::ImageLimits::default(),
            options,
            &|| Ok(()),
        );
        sink.push(ImageCompletion {
            scope: request.scope,
            key: request.key,
            result,
        });
        true
    }
}
struct Page {
    reader: Entity<ReaderView>,
}
impl gpui::Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_col().child(
            div()
                .id("silent-page-scroll")
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_y_scroll()
                .child(self.reader.clone()),
        )
    }
}
fn check_pending(cx: &mut TestAppContext, flow: bool, only_images: bool) {
    setup(cx);
    let provider = Arc::new(Delayed::default());
    let service = provider.clone();
    let changed = Rc::new(Cell::new(0usize));
    let notices = changed.clone();
    let mut body = if only_images {
        String::new()
    } else {
        "Before\n\n".into()
    };
    for i in 0..4 {
        body.push_str(&format!(
            "![quiet{i}](https://images.invalid/delay{i}.{})\n\n",
            if i % 2 == 0 { "svg" } else { "png" }
        ));
    }
    if !only_images {
        body.push_str("After\n\n");
        for i in 0..60 {
            body.push_str(&format!("Following paragraph {i}.\n\n"));
        }
    }
    let (host, cx) = cx.add_window_view(move |window, cx| {
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
            reader.set_image_limits(
                crate::reader::ImageLimits {
                    max_in_flight: 2,
                    ..Default::default()
                },
                cx,
            );
            reader.set_image_provider(Some(service as Arc<dyn ImageProvider>), cx);
            reader.on_layout_changed(Rc::new(move |_, app| {
                notices.set(notices.get() + 1);
                let _ = weak.update(app, |_, cx| cx.notify());
            }));
            reader
        });
        Page { reader }
    });
    let reader = host.read_with(cx, |host, _| host.reader.clone());
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(900.0), px(500.0)));
    if only_images {
        for _ in 0..12 {
            cx.run_until_parked();
            cx.background_executor
                .advance_clock(Duration::from_millis(25));
        }
    } else {
        frames(cx);
    }
    assert_eq!(
        provider.pending.lock().unwrap().len(),
        2,
        "collapsed images still dispatch, bounded by in-flight slots"
    );
    let base = if only_images { 0 } else { 1 };
    for i in base..base + 4 {
        // Loading and queued pictures draw no element, so neither the picture
        // selector nor its frame may exist before the bytes land.
        let picture: &'static str = Box::leak(format!("markdown-picture-{i}").into_boxed_str());
        let frame: &'static str = Box::leak(format!("markdown-picture-frame-{i}").into_boxed_str());
        assert!(
            cx.debug_bounds(picture).is_none() && cx.debug_bounds(frame).is_none(),
            "a pending picture reserves no panel, label or box"
        );
    }
    assert!(
        reader.read_with(cx, |reader, cx| reader
            .markdown()
            .unwrap()
            .read(cx)
            .document_text()
            .contains("quiet0")),
        "alt remains available to copy"
    );
    let mut anchor = None;
    let before = if only_images {
        None
    } else {
        Some(
            cx.debug_bounds("markdown-paragraph-5")
                .expect("following text must be visible before images load"),
        )
    };
    if !only_images && !flow {
        reader.update_in(cx, |reader, _, cx| {
            reader.markdown().unwrap().update(cx, |doc, cx| {
                doc.list_state().scroll_to(gpui::ListOffset {
                    item_ix: 5,
                    offset_in_item: px(0.0),
                });
                cx.notify();
            });
        });
        frames(cx);
        anchor = Some(reader.read_with(cx, |reader, cx| {
            reader.markdown().unwrap().read(cx).top_block()
        }));
    }
    for _ in 0..8 {
        provider.complete_one();
        frames(cx);
    }
    for i in 0..4 {
        assert_eq!(
            reader.read_with(cx, |reader, _| reader.image_state(&external_key(&format!(
                "https://images.invalid/delay{i}.{}",
                if i % 2 == 0 { "svg" } else { "png" }
            )))),
            Some("ready".into())
        );
    }
    if let Some(anchor) = anchor {
        assert_eq!(
            reader.read_with(cx, |reader, cx| reader
                .markdown()
                .unwrap()
                .read(cx)
                .top_block()),
            anchor,
            "completion preserves the virtual anchor"
        );
    }
    if flow && !only_images {
        let after = cx
            .debug_bounds("markdown-paragraph-5")
            .expect("following paragraph after growth");
        assert!(
            after.top() > before.unwrap().top() + px(140.0),
            "Flow host receives actual image heights"
        );
        assert!(
            changed.get() > 0,
            "Flow height changes notify the existing host interface"
        );
    }
    assert!(!reader.read_with(cx, |reader, _| reader.has_pending_images()));
}
#[gpui::test]
fn quality_silent_pending_virtual_loads_and_preserves_anchor(cx: &mut TestAppContext) {
    check_pending(cx, false, false);
}
#[gpui::test]
fn quality_silent_pending_flow_notifies_the_scroll_host(cx: &mut TestAppContext) {
    check_pending(cx, true, false);
}
#[gpui::test]
fn quality_density_change_retains_size_and_requests_only_higher_svg_density(
    cx: &mut TestAppContext,
) {
    setup(cx);
    let provider = Arc::new(Delayed::default());
    let service = provider.clone();
    let (reader, cx) = cx.add_window_view(move |window, cx| {
        let mut reader = ReaderView::new(
            crate::content::PreparedBody::Utf8 {
                text: "![card](https://images.invalid/card.svg)".into(),
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
        reader.set_image_provider(Some(service as Arc<dyn ImageProvider>), cx);
        reader
    });
    cx.update(|window, _| window.activate_window());
    frames(cx);
    assert!(provider.complete_one());
    frames(cx);
    let original = cx
        .debug_bounds("markdown-picture-frame-0")
        .expect("ready picture")
        .size;
    let old_scope = reader.read_with(cx, |reader, _| reader.image_scope());
    cx.simulate_scale_factor_change(2.5);
    frames(cx);
    assert_eq!(
        cx.debug_bounds("markdown-picture-frame-0")
            .expect("old image stays visible while upgrading")
            .size,
        original
    );
    assert_ne!(
        reader.read_with(cx, |reader, _| reader.image_scope()),
        old_scope
    );
    assert_eq!(
        provider
            .options
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .device_scale(),
        2.5
    );
    assert!(provider.complete_one());
    frames(cx);
    assert_eq!(
        cx.debug_bounds("markdown-picture-frame-0").unwrap().size,
        original
    );
    let count = provider.options.lock().unwrap().len();
    cx.simulate_scale_factor_change(1.0);
    frames(cx);
    cx.update(|window, _| window.set_rem_size(px(24.0)));
    frames(cx);
    assert_eq!(
        provider.options.lock().unwrap().len(),
        count,
        "moving to a lower-density screen or changing text rem keeps the existing crisp grid"
    );
    assert_eq!(
        cx.debug_bounds("markdown-picture-frame-0").unwrap().size,
        original,
        "text zoom must not double-scale author pixel dimensions"
    );
}

#[gpui::test]
fn quality_pending_badges_claim_neither_width_nor_flex_gap(cx: &mut TestAppContext) {
    setup(cx);
    let provider = Arc::new(Delayed::default());
    let service = provider.clone();
    let (reader, cx) = cx.add_window_view(move |window, cx| {
        let mut reader = ReaderView::new(
            crate::content::PreparedBody::Utf8 {
                text: (0..3)
                    .map(|i| format!("![badge{i}](https://images.invalid/badge{i}.svg)"))
                    .collect::<Vec<_>>()
                    .join(" "),
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
        reader.set_image_provider(Some(service as Arc<dyn ImageProvider>), cx);
        reader
    });
    cx.update(|window, _| window.activate_window());
    frames(cx);
    assert_eq!(provider.pending.lock().unwrap().len(), 3);
    assert!(provider.complete_one());
    frames(cx);
    provider.pending.lock().unwrap().swap(0, 1);
    assert!(provider.complete_one());
    frames(cx);
    let first = cx
        .debug_bounds("markdown-inline-picture-0-0")
        .expect("first ready badge");
    let last = cx
        .debug_bounds("markdown-inline-picture-0-2")
        .expect("last ready badge");
    assert!(
        (last.left() - first.right() - px(4.0)).abs() < px(1.0),
        "a pending middle badge must not claim an extra gap: {first:?}, {last:?}"
    );
    assert!(cx.debug_bounds("markdown-inline-picture-0-1").is_none());
    assert!(reader.read_with(cx, |reader, _| reader.has_pending_images()));
}

#[gpui::test]
fn quality_only_pending_images_dispatch_without_an_unrelated_redraw(cx: &mut TestAppContext) {
    check_pending(cx, true, true);
}
