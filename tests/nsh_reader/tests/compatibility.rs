use githux_nsh::{
    content::{PreparedBody, UnsupportedContent, images::*},
    preview::reader as fixture,
    reader::{
        images::{ImageBridge, Picture},
        *,
    },
};
use nsh_core::{element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[test]
fn prepared_states_and_existing_html_semantics() {
    for (body, label) in [
        (PreparedBody::Empty, "文件为空"),
        (
            PreparedBody::Unsupported(UnsupportedContent::NonUtf8),
            "无法作为文本显示",
        ),
        (
            PreparedBody::TooLarge {
                size: 3000000,
                limit: MAX_TEXT_BYTES,
            },
            "超出读取上限",
        ),
        (
            PreparedBody::Utf8 {
                text: "a".repeat(MAX_TEXT_BYTES + 1),
                has_bom: false,
            },
            "超出读取上限",
        ),
    ] {
        assert_eq!(
            ReaderDocument::prepare(fixture::context(), body, DocumentKind::Markdown).body,
            ReaderBody::Status(label)
        );
    }
    let doc = fixture::document(fixture::README);
    let ReaderBody::Markdown(doc) = &doc.body else {
        panic!()
    };
    let plain = doc.plain_text();
    assert!(!plain.contains("secret-hidden"));
    for text in [
        "中文 README",
        "原生 HTML",
        "表头\t值",
        "fn main()",
        "最后一段",
    ] {
        assert!(plain.contains(text), "missing {text}: {plain}");
    }
    assert!(
        matches!(links::resolve("../guide.md#目标",&fixture::context()),LinkTarget::Repository(RepositoryLink {path,commit,..}) if path=="guide.md" && commit==fixture::context().commit)
    );
}
#[derive(Default)]
struct Provider {
    requests: Mutex<Vec<(ImageRequest, ImageSink)>>,
    cancelled: Mutex<Vec<ImageScope>>,
}
impl ImageProvider for Provider {
    fn start(&self, request: ImageRequest, sink: ImageSink) {
        self.requests.lock().unwrap().push((request, sink));
    }
    fn cancel(&self, scope: ImageScope) {
        self.cancelled.lock().unwrap().push(scope);
    }
}
fn picture() -> DecodedImage {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    decode_image(png.into_inner(), &ImageLimits::default()).unwrap()
}
fn finish(provider: &Provider, index: usize) {
    let requests = provider.requests.lock().unwrap();
    let (request, sink) = &requests[index];
    sink.push(ImageCompletion {
        scope: request.scope,
        key: request.key.clone(),
        result: Ok(picture()),
    });
}
#[test]
fn images_refuse_dangerous_sources_and_never_revive_retired_capabilities() {
    let provider = Arc::new(Provider::default());
    let valid = Arc::new(AtomicBool::new(true));
    let guard = valid.clone();
    let mut board = ImageBridge::new(
        Some(provider.clone()),
        Arc::new(move || guard.load(Ordering::Acquire)),
        ImageLimits::default(),
    );
    for raw in [
        "file:///private.png",
        "data:image/png;base64,abc",
        "//host/a",
        "http://host/a",
        "\\\\server\\a",
        "#frag",
    ] {
        assert!(matches!(
            board.picture(raw, &fixture::context()),
            Picture::Failed(ImageFailure::Refused(_))
        ));
    }
    board.poll();
    assert!(provider.requests.lock().unwrap().is_empty());
    assert_eq!(
        board.picture("images/a.png", &fixture::context()),
        Picture::Pending
    );
    board.poll();
    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            matches!(&requests[0].0.target,ImageTarget::Repository {path,commit,..} if path=="docs/images/a.png" && commit==&fixture::context().commit)
        );
    }
    finish(&provider, 0);
    assert!(board.poll());
    assert!(
        matches!(board.picture("images/a.png",&fixture::context()),Picture::Ready {source,..} if source.starts_with("data:image/png;base64,"))
    );
    board.picture("images/late.png", &fixture::context());
    board.poll();
    valid.store(false, Ordering::Release);
    assert!(board.poll());
    valid.store(true, Ordering::Release);
    finish(&provider, 1);
    board.poll();
    assert_eq!(
        board.picture("images/a.png", &fixture::context()),
        Picture::Failed(ImageFailure::StaleContext)
    );
    assert!(!provider.cancelled.lock().unwrap().is_empty());
}
#[test]
fn hiding_advances_scope_and_rejects_late_results_without_cancelling_other_readers() {
    let provider = Arc::new(Provider::default());
    let mut first = ImageBridge::new(
        Some(provider.clone()),
        Arc::new(|| true),
        ImageLimits::default(),
    );
    let mut second = ImageBridge::new(
        Some(provider.clone()),
        Arc::new(|| true),
        ImageLimits::default(),
    );
    assert_ne!(first.scope().reader, second.scope().reader);
    first.picture("a.png", &fixture::context());
    first.poll();
    let old = first.scope();
    first.suspend();
    finish(&provider, 0);
    first.poll();
    assert_ne!(old, first.scope());
    assert_eq!(
        first.picture("a.png", &fixture::context()),
        Picture::Pending
    );
    second.picture("a.png", &fixture::context());
    second.poll();
    assert_eq!(provider.cancelled.lock().unwrap().as_slice(), &[old]);
}
#[test]
fn native_document_mounts_html_tables_code_and_all_images_are_overridden() {
    let mut app = Headless::start(EngineConfig::new(Root::of::<fixture::Preview>())).unwrap();
    app.wait_for("最后一段。").unwrap();
    assert!(!app.screen().shows("secret-hidden"));
    assert!(app.screen().shows("local or unsupported source"));
    assert!(app.screen().shows("no image service attached"));
    assert!(
        app.screen()
            .in_selectable_view("第一段可跨块选择。")
            .unwrap()
    );
    assert!(app.screen().in_selectable_view("最后一段。").unwrap());
    assert!(app.screen().opened_urls().is_empty());
    app.press("Reset").unwrap();
    app.wait_for("最后一段。").unwrap();
    app.press("Source").unwrap();
    app.wait_for("reader-source").unwrap();
    // The native read-only Editor draws line numbers and keeps the text unchanged on editing keys.
    assert!(app.screen().count("text:/^[0-9]+$/").unwrap() >= 2);
    app.type_text("reader-source", "X").unwrap();
    app.press_key("backspace").unwrap();
    app.press_key("enter").unwrap();
    assert!(app.screen().shows("fn main() {"));
    app.press("Reset").unwrap();
    app.wait_for("最后一段。").unwrap();
    app.press("Retire").unwrap();
    app.wait_for("内容已撤权").unwrap();
    assert!(!app.screen().shows("最后一段。"));
}

#[component]
fn Flow() -> impl IntoElement {
    let h = ReaderHandle::new(
        fixture::document(&("长文 中文 paragraph\n\n".repeat(150) + "FLOW END")),
        fixture::bridge(),
        ReaderLayout::Flow,
        |_| {},
    );
    scroll()
        .test_id("profile-scroll")
        .w(px(600.0))
        .h(px(300.0))
        .child(
            view()
                .flex_col()
                .child(reader(h))
                .child(text("PINNED AFTER README")),
        )
}
#[test]
fn flow_has_real_natural_height_and_only_the_page_scrolls() {
    let mut app = Headless::start(EngineConfig::new(Root::of::<Flow>())).unwrap();
    app.wait_for("FLOW END").unwrap();
    assert_eq!(app.screen().count("id:reader-scroll").unwrap(), 0);
    // The native host measures actual glyph height; headless proves the complete mounted flow.
    app.scroll("id:profile-scroll", px(100000.0)).unwrap();
    assert!(app.screen().shows("PINNED AFTER README"));
}
