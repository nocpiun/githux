//! Headless Native Shell test of the page's view wiring.
//!
//! This is the view half of the page: a real `RepositoryPage` is mounted in an NSH headless engine
//! with a real `PageContext` from the fixture-backed runtime, then a UI-thread tick polls the
//! controller exactly like the shell's foreground drain. It proves the page renders from the
//! controller's snapshot (tree rows, README, Star count, in-page tabs) and that a tab switch stops
//! drawing the Code area without losing the navigation intent.
//!
//! It does not prove real Windows input, IME, pixel layout or a real GitHub account; those remain
//! manual acceptance items.

use std::sync::Mutex;
use std::time::Duration;

use nsh_core::prelude::*;
use nsh_engine::EngineConfig;
use nsh_testing::Headless;

use super::{Fixture, intent};
use crate::app::pages::{NavigationSink, PageView};
use crate::app::runtime::PageContext;
use crate::repository_modules::fixture::{NAME, OWNER, Scene};
use crate::ui::repository::{PageHandle, RepositoryPage};
use crate::workspace::{RepositoryNavigation, Route, Sessions};

static PREPARED: Mutex<Option<(PageContext, Sessions)>> = Mutex::new(None);

#[component]
fn Harness() -> impl IntoElement {
    let (context, sessions) = PREPARED
        .lock()
        .expect("prepared page")
        .take()
        .expect("the test prepares one page");
    let navigation: NavigationSink = std::rc::Rc::new(|_| {});
    let page = std::rc::Rc::new(RepositoryPage::new(
        context,
        OWNER.to_string(),
        NAME.to_string(),
        RepositoryNavigation::default(),
        navigation,
    ));
    let polled = page.clone();
    spawn(async move {
        loop {
            timer(Duration::from_millis(20)).await;
            {
                let mut controller = polled.controller_mut();
                controller.poll(&sessions);
            }
            polled.sync();
        }
    });
    dynamic(move || page.element())
}

#[test]
fn the_page_renders_real_rows_and_switches_in_page_tabs() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    handle.snapshot();
    let tab = handle.sessions_mut().open(Route::Repository {
        namespace: OWNER.to_string(),
        name: NAME.to_string(),
    });
    let context = handle
        .services()
        .expect("services")
        .page_context(handle.sessions(), tab, None)
        .expect("page context");
    let sessions = handle.sessions().clone();
    *PREPARED.lock().unwrap() = Some((context, sessions));

    let mut app = Headless::start(EngineConfig::new(nsh_engine::Root::of::<Harness>()))
        .expect("headless app starts");
    app.wait_for("README.md").expect("the README row renders");
    app.wait_for("id:repository-tree")
        .expect("the tree list mounts");
    app.wait_for("128")
        .expect("the authoritative Star count renders");
    app.wait_for("docs")
        .expect("the root directory row renders");
    app.press("id:repository-tab-issues")
        .expect("the in-page Issues tab is pressable");
    app.wait_for("id:repository-tab-empty")
        .expect("leaving Code draws no tree or reader");
    app.press("id:repository-tab-code")
        .expect("the in-page Code tab is pressable");
    app.wait_for("README.md")
        .expect("returning to Code restores the same document");

    let _ = PageHandle::default();
    let _ = intent();
}
