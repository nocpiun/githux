//! Measure the two rendered header boxes, including the reader action and path.

use gpui::{Entity, TestAppContext, VisualTestContext, px};

use super::{Fixture, activate, frames, normal, open, settle};
use super::{ReaderStatus, RepositoryNavigation, RepositoryPage, Selection};
use crate::repository_modules::fixture::Scene;

fn aligned_headers(cx: &mut VisualTestContext) -> gpui::Bounds<gpui::Pixels> {
    frames(cx);
    let tree = cx.debug_bounds("repository-tree-header").unwrap();
    let reader = cx.debug_bounds("repository-reader-header").unwrap();
    assert_eq!(tree.origin.y, reader.origin.y, "headers start together");
    assert_eq!(tree.bottom(), reader.bottom(), "bottom borders share a y");
    assert_eq!(tree.size.height, reader.size.height);
    for selector in ["repository-commit", "repository-selected-path"] {
        let content = cx.debug_bounds(selector).unwrap();
        assert!(
            content.top() >= reader.top(),
            "{selector} starts inside header"
        );
        assert!(
            content.bottom() <= reader.bottom(),
            "{selector} fits header"
        );
    }
    reader
}

fn select_source(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext) {
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(page, cx, |page| page.tree("src").is_some());
    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
}

#[gpui::test]
fn code_header_borders_align_with_readme_source_long_path_and_scaled_text(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    for rem_size in [16.0, 20.0] {
        cx.update(|window, _| window.set_rem_size(px(rem_size)));
        for scale in [1.0, 1.25, 1.5, 2.0] {
            cx.update(|window, _| window.set_scale_factor(scale));
            for width in [1240.0, 820.0, 560.0] {
                cx.simulate_resize(gpui::size(px(width), px(720.0)));
                aligned_headers(cx);
            }
        }
    }
    select_source(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    let reader = aligned_headers(cx);
    let action = cx.debug_bounds("repository-back-to-readme").unwrap();
    assert!(action.top() >= reader.top() && action.bottom() <= reader.bottom());

    // Layout-only stress: keep the real source reader/action mounted and replace
    // its displayed selection with a long unbroken repository-relative path.
    page.update(cx, |page, cx| {
        page.selection = Selection::File(format!("docs/{}/source.rs", "long-segment".repeat(30)));
        cx.notify();
    });
    let reader = aligned_headers(cx);
    let path = cx.debug_bounds("repository-selected-path").unwrap();
    assert!(path.left() >= reader.left() && path.right() <= reader.right());
}

#[gpui::test]
fn code_header_borders_stay_aligned_through_loading_failure_and_retry(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::FileFailure);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    aligned_headers(cx);
    select_source(&page, cx);
    assert_eq!(
        page.read_with(cx, |page, _| page.reader_status().clone()),
        ReaderStatus::Loading
    );
    aligned_headers(cx);
    settle(&page, cx, |page| {
        matches!(page.reader_status(), ReaderStatus::ContentFailed(_))
    });
    assert!(cx.debug_bounds("repository-content-error").is_some());
    aligned_headers(cx);
    page.update(cx, |page, cx| page.retry_reader(cx));
    aligned_headers(cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    aligned_headers(cx);
    page.update(cx, |page, cx| page.show_readme(cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    aligned_headers(cx);
    assert!(fixture.transport.mismatches().is_empty());
}
