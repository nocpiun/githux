//! Reader behavior: link and image boundaries, prepared states, Markdown and
//! HTML semantics, the native mount (source/status/Flow/bounded), selection
//! scope, document replacement, retirement and long-document windowing.
//!
//! The assembly mirrors the application: the real `src/ui/reader` module tree
//! over the real `content`/`domain` business modules. Headless NSH runs the
//! real engine; native mouse drag/Ctrl+C/IME remain owner-verified on a real
//! Windows window (see `docs/nsh-reader.md`).

#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "support/reader_modules.rs"]
mod reader_modules;

#[allow(unused_imports)]
use reader_modules::reader;

use crate::content::images::ImageLimits;
use crate::content::{PreparedBody, UnsupportedContent};
use crate::reader::images::ImageBridge;
use crate::reader::{
    DocumentKind, LinkTarget, MAX_TEXT_BYTES, ReaderBody, ReaderDocument, ReaderHandle,
    ReaderLayout, ResourceContext, fixtures, links, reader,
};
use nsh_core::{element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;
use std::sync::Arc;
use std::time::Duration;

fn context() -> ResourceContext {
    ResourceContext::new(
        fixtures::OWNER,
        fixtures::NAME,
        fixtures::COMMIT,
        fixtures::PATH,
    )
}

fn resolve(url: &str) -> LinkTarget {
    links::resolve(url, &context())
}

fn repository_path(target: LinkTarget) -> String {
    match target {
        LinkTarget::Repository(link) => link.path,
        other => panic!("expected a repository link, got {other:?}"),
    }
}

fn bridge() -> ImageBridge {
    ImageBridge::new(None, Arc::new(|| true), ImageLimits::default())
}

fn document(body: PreparedBody, kind: DocumentKind) -> Arc<ReaderDocument> {
    ReaderDocument::prepare(context(), body, kind)
}

// ---- pure link and document-kind boundaries ----

#[test]
fn relative_links_keep_repository_commit_and_directory() {
    let target = resolve("images/diagram.png");
    let LinkTarget::Repository(link) = target else {
        panic!("expected a repository link, got {target:?}");
    };
    assert_eq!(link.owner, fixtures::OWNER);
    assert_eq!(link.name, fixtures::NAME);
    assert_eq!(link.commit, fixtures::COMMIT);
    assert_eq!(link.path, "docs/images/diagram.png");
}

#[test]
fn parent_segments_climb_but_stop_at_the_root() {
    assert_eq!(repository_path(resolve("../LICENSE")), "LICENSE");
    assert_eq!(repository_path(resolve("../../../../LICENSE")), "LICENSE");
    assert_eq!(repository_path(resolve("/src/main.rs")), "src/main.rs");
    assert_eq!(repository_path(resolve("./guide.md")), "docs/guide.md");
    assert_eq!(
        repository_path(resolve("guide.md?plain=1")),
        "docs/guide.md"
    );
    assert_eq!(
        repository_path(resolve("images/a%20b.png")),
        "docs/images/a b.png"
    );
}

#[test]
fn a_fragment_is_reported_but_kept_with_the_file() {
    let target = resolve("guide.md#setup");
    let LinkTarget::Repository(link) = target else {
        panic!("expected a repository link, got {target:?}");
    };
    assert_eq!(link.path, "docs/guide.md");
    assert_eq!(link.anchor.as_deref(), Some("setup"));
    assert!(matches!(resolve("#setup"), LinkTarget::Anchor { .. }));
}

#[test]
fn external_links_are_only_http_and_https() {
    assert!(matches!(
        resolve("https://example.com/x"),
        LinkTarget::External { .. }
    ));
    assert!(matches!(
        resolve("http://example.com"),
        LinkTarget::External { .. }
    ));
}

#[test]
fn dangerous_schemes_are_refused() {
    for url in [
        "javascript:alert(1)",
        "JavaScript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "file:///C:/secret.txt",
        "vbscript:msgbox(1)",
        "//example.com/x",
        "",
        "   ",
        "images\\a.png",
        "images/%zz.png",
    ] {
        assert!(
            matches!(resolve(url), LinkTarget::Refused { .. }),
            "{url} must be refused"
        );
    }
}

#[test]
fn document_kind_follows_the_path() {
    assert_eq!(DocumentKind::for_path("README.md"), DocumentKind::Markdown);
    assert_eq!(DocumentKind::for_path("README"), DocumentKind::Markdown);
    assert_eq!(
        DocumentKind::for_path("docs/guide.markdown"),
        DocumentKind::Markdown
    );
    assert_eq!(DocumentKind::for_path("README.mdx"), DocumentKind::Markdown);
    assert_eq!(
        DocumentKind::for_path("src/main.rs"),
        DocumentKind::Source {
            language: Some("rs".into())
        }
    );
}

#[test]
fn prepared_states_map_to_honest_statuses() {
    for (body, expected) in [
        (PreparedBody::Empty, "This file is empty."),
        (
            PreparedBody::Unsupported(UnsupportedContent::NonUtf8),
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
        (
            PreparedBody::Utf8 {
                text: String::new(),
                has_bom: false,
            },
            "This file is empty.",
        ),
    ] {
        assert_eq!(
            document(body, DocumentKind::Markdown).body,
            ReaderBody::Status(expected)
        );
    }
}

#[test]
fn markdown_and_html_keep_gfm_tables_comments_code_and_safe_links() {
    let prepared = document(fixtures::markdown(), DocumentKind::Markdown);
    let ReaderBody::Markdown(model) = &prepared.body else {
        panic!("expected Markdown, got {:?}", prepared.body);
    };
    let plain = model.plain_text();
    for expected in [
        "Githux reader 阅读器",
        "first item with 中文和 emoji 🚀",
        "A quote that keeps the reader honest.",
        "fn main()",
        "Column\tMeaning",
        "a dangerous link",
    ] {
        assert!(plain.contains(expected), "missing {expected:?}: {plain}");
    }
    assert!(
        !plain.contains("inert text"),
        "a script's body must never be visible: {plain}"
    );
    // The relative link resolves against the fixed commit, never a branch.
    assert!(matches!(
        resolve("guide.md#setup"),
        LinkTarget::Repository(ref link)
            if link.commit == fixtures::COMMIT && link.path == "docs/guide.md"
    ));

    let prepared = document(fixtures::html(), DocumentKind::Markdown);
    let ReaderBody::Markdown(model) = &prepared.body else {
        panic!("expected Markdown, got {:?}", prepared.body);
    };
    let plain = model.plain_text();
    for expected in [
        "HTML 混排",
        "普通段落，带 加粗、斜体",
        "内部段落与 Ctrl 键。",
        "列\t含义",
        "第一项",
        "尾部文本。",
    ] {
        assert!(plain.contains(expected), "missing {expected:?}: {plain}");
    }
    assert!(
        !plain.contains("hidden cell"),
        "a commented-out table must never reach the model: {plain}"
    );
    assert!(
        !plain.contains("never runs"),
        "a script must never reach the model: {plain}"
    );
    assert!(
        plain.contains("<div align=\"center\">"),
        "HTML inside inline code stays verbatim: {plain}"
    );
}

// ---- headless mounts ----

fn launch<T>() -> Headless
where
    T: nsh_core::element::Component,
    T::Props: Default,
{
    Headless::start(EngineConfig::new(Root::of::<T>())).expect("headless engine")
}

fn mount(
    prepared: PreparedBody,
    kind: DocumentKind,
    layout: ReaderLayout,
) -> (ReaderHandle, impl IntoElement) {
    let action = signal(String::from("none"));
    let record = action;
    let handle = ReaderHandle::new(document(prepared, kind), bridge(), layout, move |event| {
        record.set(format!("{event:?}"));
    });
    let poll = handle.clone();
    spawn(async move {
        loop {
            timer(Duration::from_millis(20)).await;
            poll.poll();
        }
    });
    (handle.clone(), view().size_full().child(reader(handle)))
}

#[component]
fn MarkdownPreview() -> impl IntoElement {
    let (_handle, page) = mount(
        fixtures::markdown(),
        DocumentKind::Markdown,
        ReaderLayout::Repository,
    );
    page
}

#[component]
fn HtmlPreview() -> impl IntoElement {
    let (_handle, page) = mount(
        fixtures::html(),
        DocumentKind::Markdown,
        ReaderLayout::Repository,
    );
    page
}

#[component]
fn SourcePreview() -> impl IntoElement {
    let (_handle, page) = mount(
        fixtures::source(),
        DocumentKind::Source {
            language: Some("rust".into()),
        },
        ReaderLayout::Repository,
    );
    page
}

#[component]
fn EmptyPreview() -> impl IntoElement {
    let (_handle, page) = mount(
        fixtures::empty(),
        DocumentKind::Markdown,
        ReaderLayout::Repository,
    );
    page
}

#[component]
fn UnsupportedPreview() -> impl IntoElement {
    let (_handle, page) = mount(
        fixtures::unsupported(),
        DocumentKind::Markdown,
        ReaderLayout::Repository,
    );
    page
}

#[component]
fn TooLargePreview() -> impl IntoElement {
    let (_handle, page) = mount(
        fixtures::too_large(),
        DocumentKind::Markdown,
        ReaderLayout::Repository,
    );
    page
}

/// A bounded long document: more blocks than the reader keeps mounted, so the
/// virtual list is used.
#[component]
fn LongPreview() -> impl IntoElement {
    let mut body = String::new();
    for section in 0..300 {
        body.push_str(&format!(
            "## Section {section}\n\nA paragraph with 中文 and 🚀 that pads the document. `code` and [a link](guide.md).\n\n"
        ));
    }
    body.push_str("VIRTUAL END\n");
    let (handle, page) = mount(
        PreparedBody::Utf8 {
            text: body,
            has_bom: false,
        },
        DocumentKind::Markdown,
        ReaderLayout::Repository,
    );
    let end = handle.clone();
    view()
        .flex_col()
        .size_full()
        .child(text("LongEnd").on_press(move || end.scroll_to_end()))
        .child(view().flex_1().min_h(px(0.0)).child(page))
}

/// A Profile flow: natural height inside the page's own scroll container.
#[component]
fn FlowPreview() -> impl IntoElement {
    let body = format!("{}FLOW END", "长文 中文 paragraph.\n\n".repeat(120));
    let (_handle, page) = mount(
        PreparedBody::Utf8 {
            text: body,
            has_bom: false,
        },
        DocumentKind::Markdown,
        ReaderLayout::Flow,
    );
    scroll()
        .test_id("profile-scroll")
        .w(px(600.0))
        .h(px(300.0))
        .child(
            view()
                .flex_col()
                .child(page)
                .child(text("PINNED AFTER README")),
        )
}

/// Buttons drive the same handle API the host uses: link reports, replacement,
/// retirement.
#[component]
fn ActionsPreview() -> impl IntoElement {
    let action = signal(String::from("none"));
    let record = action;
    let handle = ReaderHandle::new(
        document(fixtures::markdown(), DocumentKind::Markdown),
        bridge(),
        ReaderLayout::Repository,
        move |event| record.set(format!("{event:?}")),
    );
    let poll = handle.clone();
    spawn(async move {
        loop {
            timer(Duration::from_millis(20)).await;
            poll.poll();
        }
    });
    let relative = handle.clone();
    let dangerous = handle.clone();
    let anchor = handle.clone();
    let source = handle.clone();
    let reset = handle.clone();
    let retire = handle.clone();
    let end = handle.clone();
    let reset_document = document(fixtures::markdown(), DocumentKind::Markdown);
    let source_kind = DocumentKind::Source {
        language: Some("rust".into()),
    };
    view()
        .flex_col()
        .size_full()
        .child(
            view()
                .flex_row()
                .gap(px(8.0))
                .child(text("Relative").on_press(move || relative.handle_link("guide.md#setup")))
                .child(
                    text("Dangerous")
                        .on_press(move || dangerous.handle_link("javascript:alert(1)")),
                )
                .child(text("Anchor").on_press(move || anchor.handle_link("#setup")))
                .child(text("End").on_press(move || end.scroll_to_end()))
                .child(text("Source").on_press({
                    let source = source.clone();
                    let kind = source_kind.clone();
                    move || {
                        source.replace(document(fixtures::source(), kind.clone()), bridge());
                    }
                }))
                .child(text("Reset").on_press({
                    let reset = reset.clone();
                    let body = reset_document.clone();
                    move || reset.replace(body.clone(), bridge())
                }))
                .child(text("Retire").on_press(move || retire.retire())),
        )
        .child(
            view()
                .test_id("reader-action")
                .child(text(move || action.get())),
        )
        .child(view().flex_1().min_h(px(0.0)).child(reader(handle)))
}

/// Every Markdown picture route: one refused local address, one relative
/// address with no provider attached.
#[component]
fn PicturePreview() -> impl IntoElement {
    let (_handle, page) = mount(
        PreparedBody::Utf8 {
            text: "![local](file:///C:/private.png)\n\n![relative](images/chart.png)\n".into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
        ReaderLayout::Repository,
    );
    page
}

#[test]
fn markdown_mounts_blocks_inside_one_selectable_view() {
    let mut app = launch::<MarkdownPreview>();
    app.wait_for("Githux reader 阅读器").unwrap();
    assert!(app.screen().shows("first item with 中文和 emoji 🚀"));
    assert!(app.screen().shows("Column"));
    assert!(app.screen().count("text:/fn main/").unwrap() >= 1);
    assert!(app.screen().shows("a dangerous link"));
    assert!(!app.screen().shows("inert text"));
    // Cross-block selection: every text block is in the same selectable area.
    assert!(
        app.screen()
            .in_selectable_view("Githux reader 阅读器")
            .unwrap()
    );
    assert!(
        app.screen()
            .in_selectable_view("text:/A quote that keeps/")
            .unwrap()
    );
    assert_eq!(app.screen().count("id:reader-document").unwrap(), 1);
}

#[test]
fn html_mounts_supported_markup_and_hides_comments_and_scripts() {
    let mut app = launch::<HtmlPreview>();
    app.wait_for("尾部文本。").unwrap();
    assert!(app.screen().shows("HTML 混排"));
    assert!(app.screen().count("text:/内部段落与/").unwrap() >= 1);
    assert!(app.screen().shows("第一项"));
    assert!(!app.screen().shows("hidden cell"));
    assert!(!app.screen().shows("never runs"));
    assert!(app.screen().count("text:/<div align/").unwrap() >= 1);
    assert!(app.screen().in_selectable_view("尾部文本。").unwrap());
}

#[test]
fn empty_unsupported_and_too_large_mount_their_status() {
    let mut app = launch::<EmptyPreview>();
    app.wait_for("This file is empty.").unwrap();
    assert_eq!(app.screen().count("id:reader-status").unwrap(), 1);
    let mut app = launch::<UnsupportedPreview>();
    app.wait_for("This file cannot be shown as text.").unwrap();
    let mut app = launch::<TooLargePreview>();
    app.wait_for("This file is too large to show.").unwrap();
}

#[test]
fn source_is_read_only_with_line_numbers() {
    let mut app = launch::<SourcePreview>();
    app.wait_for("reader-source").unwrap();
    assert!(app.screen().count("text:/fn greet/").unwrap() >= 1);
    assert!(
        app.screen().count("text:/^[0-9]+$/").unwrap() >= 2,
        "the native editor draws line numbers"
    );
    app.type_text("reader-source", "X").unwrap();
    app.press_key("backspace").unwrap();
    app.press_key("enter").unwrap();
    assert!(app.screen().count("text:/fn greet/").unwrap() >= 1);
    assert!(!app.screen().shows("XX"));
}

#[test]
fn flow_has_real_natural_height_and_only_the_page_scrolls() {
    let mut app = launch::<FlowPreview>();
    app.wait_for("FLOW END").unwrap();
    assert_eq!(app.screen().count("id:reader-scroll").unwrap(), 0);
    app.scroll("id:profile-scroll", px(100000.0)).unwrap();
    assert!(app.screen().shows("PINNED AFTER README"));
}

#[test]
fn long_documents_mount_only_the_rows_near_the_viewport_and_reach_the_end() {
    let mut app = launch::<LongPreview>();
    app.wait_for("Section 0").unwrap();
    // Far rows are not mounted at all...
    assert_eq!(app.screen().count("id:reader-block-0").unwrap(), 1);
    assert_eq!(app.screen().count("id:reader-block-1000").unwrap(), 0);
    // ...and the end command reaches the real tail, which only exists once the
    // virtual list mounts it.
    app.press("LongEnd").unwrap();
    app.wait_for("VIRTUAL END").unwrap();
}

#[test]
fn pressing_a_relative_link_reports_a_repository_target_and_opens_nothing() {
    let mut app = launch::<ActionsPreview>();
    app.wait_for("Relative").unwrap();
    app.press("Relative").unwrap();
    let action = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.contains("Repository"))
        .map(str::to_owned)
        .expect("the action is recorded");
    assert!(action.contains("docs/guide.md"), "{action}");
    assert!(action.contains(fixtures::COMMIT), "{action}");
    assert!(app.screen().opened_urls().is_empty());
}

#[test]
fn a_dangerous_link_is_refused_without_opening_anything() {
    let mut app = launch::<ActionsPreview>();
    app.wait_for("Dangerous").unwrap();
    app.press("Dangerous").unwrap();
    let action = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.contains("Refused"))
        .map(str::to_owned)
        .expect("the refusal is recorded");
    assert!(action.contains("javascript:alert(1)"), "{action}");
    assert!(app.screen().opened_urls().is_empty());
}

#[test]
fn an_anchor_link_is_reported_and_jumps_inside_the_bounded_reader() {
    let mut app = launch::<ActionsPreview>();
    app.wait_for("Anchor").unwrap();
    app.press("Anchor").unwrap();
    let action = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.contains("Link(Anchor"))
        .map(str::to_owned)
        .expect("the anchor target is recorded");
    assert!(action.contains("setup"), "{action}");
    assert!(app.screen().opened_urls().is_empty());
}

#[test]
fn replacing_source_and_document_keeps_the_reader_usable() {
    let mut app = launch::<ActionsPreview>();
    app.wait_for("Githux reader 阅读器").unwrap();
    app.press("Source").unwrap();
    app.wait_for("reader-source").unwrap();
    assert!(app.screen().count("text:/fn greet/").unwrap() >= 1);
    app.press("Reset").unwrap();
    app.wait_for("Githux reader 阅读器").unwrap();
    assert_eq!(app.screen().count("id:reader-source").unwrap(), 0);
}

#[test]
fn retirement_clears_the_body_and_reports_the_action() {
    let mut app = launch::<ActionsPreview>();
    app.wait_for("Githux reader 阅读器").unwrap();
    app.press("Retire").unwrap();
    app.wait_for("Content is no longer available.").unwrap();
    assert!(!app.screen().shows("Githux reader 阅读器"));
    let action = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.contains("AuthorizationRetired"))
        .map(str::to_owned)
        .expect("the retirement is reported");
    assert!(action.contains("AuthorizationRetired"));
}

#[test]
fn pictures_never_reach_a_loader_and_show_honest_placeholders() {
    let mut app = launch::<PicturePreview>();
    app.wait_for("local or unsupported source").unwrap();
    assert!(app.screen().shows("no image service attached"));
    assert!(app.screen().opened_urls().is_empty());
}
