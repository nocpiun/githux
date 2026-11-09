//! Offline Native Shell regression tests for the production Notifications host.
//!
//! Real runtime (`RuntimeHandle` over real `AccountService`/SQLite/`CacheStore`), the real inbox
//! controller and page host, a repeatable mock transport and an NSH headless root. Only the
//! transport, the temporary directories and the synthetic PAT are fixtures; the private notifier
//! payloads are synthetic. No GitHub request, real PAT or real account directory is used.
//!
//! `docs/native-shell-inplace.md` fixes the final module paths; this standalone target maps the
//! real source files onto them (the integration author owns `src/app/pages/mod.rs`, so the minimal
//! documented `PageView` contract lives in `src/bin_support/nsh_pages_pages.rs`).
#![allow(dead_code, unused_imports)]

#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;

#[path = "../src/nsh/foundation/mod.rs"]
pub mod foundation_impl;
#[path = "../src/ui/global_work_items/mod.rs"]
pub mod global_work_items_impl;
#[path = "../src/ui/notifications/mod.rs"]
pub mod notifications_impl;
#[path = "../src/bin_support/nsh_pages_pages.rs"]
pub mod pages_impl;
#[path = "../src/ui/relative_time.rs"]
pub mod relative_time_impl;
#[path = "../src/nsh/runtime/mod.rs"]
pub mod runtime_impl;

#[path = "../src/bin_support/notifications_fixture.rs"]
mod fixture;
#[path = "../src/bin_support/nsh_pages_harness.rs"]
mod harness;

pub mod app {
    pub use crate::pages_impl as pages;
    pub use crate::runtime_impl as runtime;
}
pub mod ui {
    pub use crate::foundation_impl as foundation;
    pub use crate::global_work_items_impl as global_work_items;
    pub use crate::notifications_impl as notifications;
    pub use crate::relative_time_impl as relative_time;
}
pub mod workspace {
    pub use crate::runtime_impl::sessions::{
        PageIdentity, RepositoryNavigation, Route, RouteKey, Session, Sessions, TabId, Workspace,
    };
}

use std::sync::Arc;
use std::time::Duration;

use fixture::{Fixture, Scene};
use harness::{FixtureBackend, Kind};
use network::transport::HttpMethod;
use nsh_testing::Headless;

fn setup(scene: Scene) -> (Arc<Fixture>, FixtureBackend, Headless) {
    let fixture = Arc::new(Fixture::new(scene));
    let mut backend = FixtureBackend::build(
        fixture.clone() as Arc<dyn network::transport::Transport>,
        "ghp_synthetic-notifications",
        Kind::Notifications,
    );
    // The fixture's rows are relative to its captured wall clock.
    backend.mount.now = Some(fixture::PREVIEW_NOW);
    let app = backend.start().expect("headless app starts");
    (fixture, backend, app)
}

fn wait_ready(app: &mut Headless) {
    app.wait_for("id:harness-status").expect("harness mounts");
    app.wait_for("text:ready")
        .expect("page mounts on real services");
}

fn requests(fixture: &Fixture) -> Vec<network::transport::HttpRequest> {
    fixture.requests()
}

fn page_gets(fixture: &Fixture) -> Vec<String> {
    requests(fixture)
        .into_iter()
        .filter(|request| {
            request.method == HttpMethod::Get && request.url.contains("/notifications")
        })
        .map(|request| request.url)
        .collect()
}

fn row_titles(app: &Headless) -> usize {
    app.screen().count("id:notification-row").unwrap_or(0)
}

#[test]
fn inbox_reads_real_rows_relative_time_and_opens_the_internal_repository() {
    let (fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("first fixture page renders through the real controller");
    // Relative copy comes from the shared Home formatter; the exact value stays in the tooltip.
    app.wait_for("text:2 hours ago")
        .expect("relative time is the shared formatter's");
    app.wait_for("text:Notification 5 — Improve keyboard focus")
        .expect("a later fixture row renders");
    app.wait_for("text:Time unavailable")
        .expect("an invalid timestamp is not shown as a successful age");
    // Read state is background + dot + weight, never a label.
    assert!(!app.screen().shows("Unread"));
    assert!(!app.screen().shows("Read"));
    let unread = app
        .screen()
        .span_style("text:Notification 2 — Release v2.0")
        .expect("row title");
    assert_eq!(unread.font_weight, 400, "fixture row 2 is read");
    // One page-one inbox request, with the documented query.
    let gets = page_gets(&fixture);
    assert_eq!(
        gets.len(),
        1,
        "mount reads the inbox exactly once: {gets:?}"
    );
    assert!(gets[0].contains("all=false"), "{}", gets[0]);
    assert!(gets[0].contains("participating=false"), "{}", gets[0]);
    // A repository link is internal navigation, not an external browser target.
    app.press("id:notification-repository >> nth=1")
        .expect("repository link is pressable");
    app.wait_for("text:open:synthetic-owner/a-deliberately-long-repository-name")
        .expect("the host routes the repository internally");
    assert!(app.screen().opened_urls().is_empty());
}

#[test]
fn read_and_unread_rows_differ_only_by_weight_and_dot() {
    let (_fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    // Fixture rows 1 and 3 (1-based) are read; rows whose `index % 3 == 1` are read.
    let read = app
        .screen()
        .span_style("text:Notification 2 — Release v2.0")
        .expect("read row title");
    let unread = app
        .screen()
        .span_style("text:Notification 3 — Build completed")
        .expect("unread row title");
    assert_eq!(read.font_weight, 400);
    assert_eq!(unread.font_weight, 500);
}

#[test]
fn done_is_a_real_delete_and_removes_the_row() {
    let (fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    let before = row_titles(&app);
    app.press("label:Done >> nth=1").expect("Done is pressable");
    app.wait_for_gone("text:Notification 2 — Release v2.0", Duration::from_secs(5))
        .expect("Done removes the row only after the server confirms it");
    assert!(
        requests(&fixture).iter().any(|request| {
            request.method == HttpMethod::Delete
                && request.url.ends_with("/notifications/threads/9001")
        }),
        "Done must be DELETE /notifications/threads/{{id}}"
    );
    assert_eq!(row_titles(&app) + 1, before);
}

#[test]
fn unsubscribe_keeps_the_confirmed_ignore_when_done_fails() {
    let (fixture, _backend, mut app) = setup(Scene::UnsubscribeStepTwoFailure);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    app.press("label:Unsubscribe >> nth=1")
        .expect("Unsubscribe is pressable");
    // The ignore succeeded, Done failed: the row stays, the warning offers retrying only Done.
    app.wait_for("id:notification-action-warning")
        .expect("the failure is a compact, retryable warning");
    assert!(app.screen().shows("Notification 2 — Release v2.0"));
    assert!(
        !app.screen()
            .enabled("label:Unsubscribe >> nth=1")
            .expect("unsubscribe button"),
        "the confirmed ignore must not be offered again"
    );
    assert!(
        app.screen()
            .enabled("label:Done >> nth=1")
            .expect("done button"),
        "the remaining Done step stays retryable"
    );
    let puts = requests(&fixture)
        .iter()
        .filter(|request| request.method == HttpMethod::Put)
        .count();
    assert_eq!(puts, 1, "exactly one subscription update");
    // Retrying Done finishes the row; no second subscription write is issued.
    app.press("id:notification-action-warning")
        .expect("warning retries the remaining step");
    app.wait_for_gone("text:Notification 2 — Release v2.0", Duration::from_secs(5))
        .expect("the retried Done removes the row");
    assert_eq!(
        requests(&fixture)
            .iter()
            .filter(|request| request.method == HttpMethod::Put)
            .count(),
        1
    );
}

#[test]
fn save_for_later_stays_disabled_and_sends_nothing() {
    let (fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    assert!(
        !app.screen()
            .enabled("label:Save for later (unavailable) >> nth=0")
            .expect("save entry exists"),
        "Save for later has no public API and stays disabled"
    );
    let before = requests(&fixture).len();
    let _ = app.press("label:Save for later (unavailable) >> nth=0");
    app.wait(Duration::from_millis(150)).expect("settle");
    assert_eq!(requests(&fixture).len(), before, "no hidden request");
}

#[test]
fn load_more_reads_the_next_real_page() {
    let (fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    let first_page = app.screen().texts().len();
    app.press("label:Load more notifications")
        .expect("Load more is pressable");
    app.wait_for("text:Notification 9 — Improve keyboard focus")
        .expect("the appended page's rows join the same list");
    let pages: Vec<String> = page_gets(&fixture)
        .into_iter()
        .filter(|url| url.contains("page=2"))
        .collect();
    assert_eq!(pages.len(), 1, "exactly one page-two request: {pages:?}");
    assert!(app.screen().texts().len() > first_page);
}

#[test]
fn first_failure_is_not_an_empty_inbox_and_retry_recovers() {
    let (fixture, _backend, mut app) = setup(Scene::FirstFailure);
    wait_ready(&mut app);
    app.wait_for("id:notifications-error")
        .expect("a genuine failure gets the compact warning");
    assert!(
        !app.screen().shows("No unread notifications"),
        "a failure must not become a valid empty inbox"
    );
    // The failed page still carried X-Poll-Interval: the retry is refused until the server
    // cooldown expires, then the same warning retries and recovers.
    app.press("id:notifications-error")
        .expect("the warning attempts the retry");
    app.wait(Duration::from_millis(1200)).expect("settle");
    app.press("id:notifications-error")
        .expect("the warning retries after the cooldown");
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("the retry loads the real page");
    assert_eq!(
        page_gets(&fixture)
            .into_iter()
            .filter(|url| url.contains("page=1"))
            .count(),
        2
    );
}

#[test]
fn confirmed_empty_inbox_is_its_own_state() {
    let (_fixture, _backend, mut app) = setup(Scene::Empty);
    wait_ready(&mut app);
    app.wait_for("text:No unread notifications")
        .expect("confirmed empty is stated once");
    assert!(
        !app.screen()
            .matches("id:notifications-error")
            .unwrap_or(false)
    );
    assert_eq!(row_titles(&app), 0);
}

#[test]
fn account_mutation_clears_the_displayed_inbox() {
    let (_fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    // Adding an account invalidates the binding at intent, before any commit is observed.
    app.press("label:Add second").expect("harness control");
    app.wait_for("text:Account changed; open Notifications again for the current account.")
        .expect("the previous account's rows are cleared");
    assert_eq!(row_titles(&app), 0);
    // Selecting the new account keeps the old rows from coming back.
    app.press("label:Select second").expect("harness control");
    app.wait(Duration::from_millis(300)).expect("settle");
    assert_eq!(row_titles(&app), 0);
    assert!(
        !app.screen()
            .matches("id:notification-repository")
            .unwrap_or(false)
    );
}

#[test]
fn account_revocation_clears_the_displayed_inbox_without_a_new_request() {
    let (fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    let before = page_gets(&fixture).len();
    app.press("label:Revoke").expect("harness control");
    app.wait_for("text:Account changed; open Notifications again for the current account.")
        .expect("one drain clears the displayed rows");
    assert_eq!(row_titles(&app), 0);
    assert_eq!(
        page_gets(&fixture).len(),
        before,
        "revocation is not an automatic retry loop"
    );
}

#[test]
fn a_hidden_cold_inbox_reads_nothing_and_resumes_when_shown() {
    let fixture = Arc::new(Fixture::new(Scene::Mixed));
    let mut backend = FixtureBackend::build(
        fixture.clone() as Arc<dyn network::transport::Transport>,
        "ghp_synthetic-notifications",
        Kind::Notifications,
    );
    backend.mount.now = Some(fixture::PREVIEW_NOW);
    backend.mount.start_inactive = true;
    let mut app = backend.start().expect("headless app starts");
    wait_ready(&mut app);
    app.wait(Duration::from_millis(400)).expect("settle");
    assert_eq!(row_titles(&app), 0, "a hidden cold page shows no rows");
    assert_eq!(
        page_gets(&fixture).len(),
        0,
        "a hidden page starts no read of its own"
    );
    app.press("label:Active").expect("the Shell shows it");
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("returning resumes the interrupted first read");
    assert_eq!(page_gets(&fixture).len(), 1);
}

#[test]
fn a_loaded_inbox_is_not_re_read_on_reactivation() {
    let (fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    let before = page_gets(&fixture).len();
    app.press("label:Inactive").expect("harness control");
    app.press("label:Active").expect("harness control");
    app.wait(Duration::from_millis(300)).expect("settle");
    assert_eq!(page_gets(&fixture).len(), before, "no re-read");
    assert!(row_titles(&app) > 0, "rows are retained");
}

#[test]
fn retiring_the_page_clears_its_rows_and_stops_work() {
    let (fixture, _backend, mut app) = setup(Scene::Mixed);
    wait_ready(&mut app);
    app.wait_for("text:Notification 2 — Release v2.0")
        .expect("rows render");
    let before = page_gets(&fixture).len();
    app.press("label:Retire").expect("harness control");
    app.wait(Duration::from_millis(300)).expect("settle");
    assert_eq!(row_titles(&app), 0);
    assert_eq!(page_gets(&fixture).len(), before);
}
