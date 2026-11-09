//! Standalone preview of README picture loading in the Native Shell reader.
//!
//! Run with `cargo run --example reader_images` on Windows. The preview
//! assembles the **real** pipeline:
//!
//! ```text
//! AccountService → RepoService::session → RepositorySession
//!   → RepositoryImageService (fixed commit → root tree → directory chain → blob)
//!   → bounded decode (content::images)
//!   → ImageBridge → NSH picture element
//! ```
//!
//! Every external boundary is deterministic and injected:
//!
//! * `src/bin_support/reader_images_fixture.rs` answers the authenticated GitHub
//!   reads by request identity and serves real, generated pictures, so nothing
//!   here depends on a network succeeding;
//! * a temporary SQLite directory plus an in-memory credential store hold one
//!   synthetic PAT, so no real credential is read;
//! * public HTTPS pictures are answered from memory, never from a real host.
//!
//! The toolbar is development scaffolding, not part of the product Shell. What
//! a production page does instead: create one `ImageBridge` per document
//! generation with the repository's verified `RepositoryImageService`, call
//! `handle.poll()` from the page's UI drain, and retire the handle when the
//! captured account capability changes.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

pub use githux::{auth, content, data, diagnostics, domain, github, network, storage};

#[path = "../src/bin_support/reader_images_modules.rs"]
mod reader_images_modules;
#[allow(unused_imports)]
use reader_images_modules::{fixture, reader};

use crate::content::images::ImageProvider;
use crate::reader::{ImageBridge, ImageLimits, ReaderDocument, ReaderHandle, dark_tokens, reader};
use nsh_core::prelude::*;
use std::sync::Arc;
use std::time::Duration;

#[component]
pub fn Preview() -> impl IntoElement {
    let pipeline = fixture::Pipeline::new(fixture::Scene::Healthy);
    let opened = pipeline.open(fixture::Scene::Healthy, ImageLimits::default());
    // Keep the account/executor/temp directory alive for the whole preview.
    let _pipeline = stored(pipeline);
    let document = ReaderDocument::from_prepared(opened.content, opened.document.kind());
    let provider = opened.provider.clone() as Arc<dyn ImageProvider>;
    let action = signal(String::from("Ready"));
    let record = action;
    let handle = ReaderHandle::new(
        document,
        ImageBridge::new(Some(provider), Arc::new(|| true), ImageLimits::default()),
        opened.layout,
        move |event| record.set(format!("{event:?}")),
    );
    let poll = handle.clone();
    spawn(async move {
        loop {
            timer(Duration::from_millis(30)).await;
            poll.poll();
        }
    });
    let end = handle.clone();
    let retire = handle.clone();
    view()
        .flex_col()
        .size_full()
        .p(px(12.0))
        .child(
            view()
                .flex_row()
                .gap(px(12.0))
                .child(text("End").on_press(move || end.scroll_to_end()))
                .child(text("Retire").on_press(move || retire.retire())),
        )
        .child(
            view()
                .test_id("preview-action")
                .child(text(move || action.get()).text_size(px(11.0))),
        )
        .child(view().flex_1().min_h(px(0.0)).child(reader(handle)))
}

#[cfg(windows)]
fn main() -> impl std::process::Termination {
    use nsh_platform_windows as platform;
    platform::set_ui_sets(vec![nsh_base::registry()]);
    let root = nsh_core::element::Root::of::<Preview>().tokens(dark_tokens());
    platform::run_app(
        platform::AppSource::Rust(root),
        platform::AppOptions {
            title: "Githux reader pictures preview".into(),
            app_id: "dev.githux.reader-images-preview".into(),
            size: (960, 760),
            ..Default::default()
        },
    )
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This native preview targets Windows; the image bridge tests run headlessly.");
}
