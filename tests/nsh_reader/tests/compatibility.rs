//! Native Shell compatibility evidence for the in-place reader.
//!
//! Phase one shipped the reader as `src/nsh/reader` in a separate preview
//! package; this suite now mounts the same component from
//! `src/ui/reader` (the in-place location) with the same fixtures, so the
//! evidence carries over instead of being dropped with the old namespace.
//!
//! Headless NSH proves the mounted tree, selection scope, source editor and
//! lifecycle. Real mouse drag/Ctrl+C/IME remain owner-verified on a Windows
//! window (`tests/nsh_reader/src/main.rs` boots the matching native probe).

use githux::ui::reader;
pub use githux::{content, domain, ui};

use crate::content::PreparedBody;
use crate::reader::images::{ImageBridge, Picture};
use crate::reader::{
    DocumentKind, ImageFailure, ImageLimits, MAX_TEXT_BYTES, ReaderBody, ReaderDocument,
    ReaderHandle, ReaderLayout, fixtures, reader,
};
use nsh_core::{element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;
use std::sync::Arc;

#[test]
fn prepared_states_and_existing_html_semantics() {
    for (body, label) in [
        (PreparedBody::Empty, "This file is empty."),
        (
            PreparedBody::Unsupported(crate::content::UnsupportedContent::NonUtf8),
            "This file cannot be shown as text.",
        ),
        (
            PreparedBody::TooLarge {
                size: 3_000_000,
                limit: MAX_TEXT_BYTES,
            },
            "This file is too large to show.",
        ),
        (
            PreparedBody::Utf8 {
                text: "a".repeat(MAX_TEXT_BYTES + 1),
                has_bom: false,
            },
            "This file is too large to show.",
        ),
    ] {
        assert_eq!(
            ReaderDocument::prepare(fixtures::context(), body, DocumentKind::Markdown).body,
            ReaderBody::Status(label)
        );
    }
    let doc = ReaderDocument::prepare(
        fixtures::context(),
        PreparedBody::Utf8 {
            text: fixtures::NATIVE_README.to_string(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    );
    let ReaderBody::Markdown(model) = &doc.body else {
        panic!("expected Markdown, got {:?}", doc.body);
    };
    let plain = model.plain_text();
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
    assert!(matches!(
        crate::reader::links::resolve("../guide.md#目标", &fixtures::context()),
        crate::reader::LinkTarget::Repository(link)
            if link.path == "guide.md" && link.commit == fixtures::context().commit
    ));
}

#[test]
fn pictures_refuse_dangerous_sources_and_no_provider_is_honest() {
    let mut bridge = ImageBridge::new(None, Arc::new(|| true), ImageLimits::default());
    for raw in [
        "file:///private.png",
        "data:image/png;base64,abc",
        "//host/a",
        "http://host/a",
        "\\\\server\\a",
        "#frag",
    ] {
        assert!(matches!(
            bridge.picture(raw, &fixtures::context()),
            Picture::Failed(ImageFailure::Refused(_))
        ));
    }
    assert_eq!(
        bridge.picture("images/a.png", &fixtures::context()),
        Picture::Failed(ImageFailure::NoProvider)
    );
    assert!(!bridge.poll());
}

#[test]
fn native_document_mounts_html_tables_code_and_all_images_are_overridden() {
    let mut app = Headless::start(EngineConfig::new(
        Root::of::<fixtures::Preview>().tokens(reader::dark_tokens()),
    ))
    .unwrap();
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
    app.press("Source").unwrap();
    app.wait_for("reader-source").unwrap();
    // The native read-only Editor draws line numbers and keeps the text
    // unchanged on editing keys.
    assert!(app.screen().count("text:/^[0-9]+$/").unwrap() >= 2);
    app.type_text("reader-source", "X").unwrap();
    app.press_key("backspace").unwrap();
    app.press_key("enter").unwrap();
    assert!(app.screen().shows("fn main() {"));
    app.press("Reset").unwrap();
    app.wait_for("最后一段。").unwrap();
    app.press("Retire").unwrap();
    app.wait_for("Content is no longer available.").unwrap();
    assert!(!app.screen().shows("最后一段。"));
}

#[component]
fn Flow() -> impl IntoElement {
    let handle = ReaderHandle::new(
        fixtures::document(
            PreparedBody::Utf8 {
                text: "长文 中文 paragraph\n\n".repeat(150) + "FLOW END",
                has_bom: false,
            },
            DocumentKind::Markdown,
        ),
        fixtures::bridge(),
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
                .child(reader(handle))
                .child(text("PINNED AFTER README")),
        )
}

#[test]
fn flow_has_real_natural_height_and_only_the_page_scrolls() {
    let mut app = Headless::start(EngineConfig::new(Root::of::<Flow>())).unwrap();
    app.wait_for("FLOW END").unwrap();
    assert_eq!(app.screen().count("id:reader-scroll").unwrap(), 0);
    // The native host measures actual glyph height; headless proves the complete
    // mounted flow and that the page's own scroll reaches what follows it.
    app.scroll("id:profile-scroll", px(100000.0)).unwrap();
    assert!(app.screen().shows("PINNED AFTER README"));
}
