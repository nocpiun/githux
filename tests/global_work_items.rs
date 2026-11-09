//! Offline Native Shell regression tests for the production global Issues / Pull Requests host.
//!
//! Real runtime (`RuntimeHandle` over real `AccountService`/SQLite/`CacheStore`), the real search
//! controller and page host, a repeatable mock transport and an NSH headless root. Only the
//! transport, the temporary directories and the synthetic PAT are fixtures. No GitHub request,
//! real PAT or real account directory is used.
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

#[path = "../src/bin_support/work_items_fixture.rs"]
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

use fixture::{FixtureTransport, Scene};
use harness::{FixtureBackend, Kind};
use nsh_testing::Headless;

/// The work-items fixture's RFC3339 stamps are relative to this preview clock.
const NOW: u64 = 1_793_620_800;

fn setup(scene: Scene, kind: Kind) -> (Arc<FixtureTransport>, FixtureBackend, Headless) {
    let transport = Arc::new(FixtureTransport::new(scene));
    let mut backend = FixtureBackend::build(
        transport.clone() as Arc<dyn network::transport::Transport>,
        "ghp_synthetic-work-items",
        kind,
    );
    backend.mount.now = Some(NOW);
    let app = backend.start().expect("headless app starts");
    (transport, backend, app)
}

fn wait_ready(app: &mut Headless) {
    app.wait_for("id:harness-status").expect("harness mounts");
    app.wait_for("text:ready")
        .expect("page mounts on real services");
}

fn searches(transport: &FixtureTransport) -> Vec<String> {
    transport
        .requests()
        .into_iter()
        .filter(|url| url.contains("/search/issues"))
        .collect()
}

#[test]
fn issues_load_real_rows_with_home_relative_time_and_open_the_internal_repository() {
    let (transport, _backend, mut app) = setup(Scene::Mixed, Kind::Issues);
    wait_ready(&mut app);
    app.wait_for("text:Issues").expect("page header");
    app.wait_for("text:Assigned Issue work item 1")
        .expect("rows come from the real search adapter");
    // Relative copy is the shared Home formatter; the exact value stays in the tooltip.
    app.wait_for("text:3 minutes ago")
        .expect("relative metadata line");
    app.wait_for("text:Yesterday")
        .expect("Home's accepted Yesterday wording is preserved");
    let requests = searches(&transport);
    assert_eq!(requests.len(), 1, "one page-one search: {requests:?}");
    for part in [
        "is%3Aissue",
        "assignee%3Atest-user",
        "is%3Aopen",
        "sort=updated",
        "order=desc",
    ] {
        assert!(requests[0].contains(part), "{} lacks {part}", requests[0]);
    }
    // The repository link opens the internal route, not a browser target.
    app.press("id:work-repository >> nth=0")
        .expect("repository link is pressable");
    app.wait_for("text:open:acme/a-very-long-repository-name-for-narrow-layouts")
        .expect("the host routes the repository internally");
    assert!(app.screen().opened_urls().is_empty());
}

#[test]
fn pull_requests_use_the_pr_query() {
    let (transport, _backend, mut app) = setup(Scene::Mixed, Kind::PullRequests);
    wait_ready(&mut app);
    app.wait_for("text:Pull Requests").expect("page header");
    app.wait_for("text:Assigned PR work item 1")
        .expect("PR rows render");
    let requests = searches(&transport);
    assert!(requests[0].contains("is%3Apr"), "{}", requests[0]);
}

#[test]
fn relation_and_state_filters_requery_and_clear_old_rows() {
    let (transport, _backend, mut app) = setup(Scene::Mixed, Kind::Issues);
    wait_ready(&mut app);
    app.wait_for("text:Assigned Issue work item 1")
        .expect("initial rows");
    // Relation: Assigned -> Created.
    app.press("label:Created")
        .expect("the Created filter is pressable");
    app.wait_for("text:Created Issue work item 1")
        .expect("the new relation's rows replace the old ones");
    let created = searches(&transport);
    assert!(
        created.iter().any(|url| url.contains("author%3Atest-user")),
        "{created:?}"
    );
    // State: Open -> Closed.
    app.press("label:Closed")
        .expect("the Closed filter is pressable");
    // Only the filter control itself still says Open; every row states Closed.
    app.wait_until(
        "the closed query's rows",
        Duration::from_secs(5),
        |screen| {
            let texts = screen.texts();
            texts.iter().filter(|text| **text == "Open").count() == 1 && texts.contains(&"Closed")
        },
    )
    .expect("closed rows show their state instead of Open");
    let closed = searches(&transport);
    assert!(
        closed.iter().any(|url| url.contains("is%3Aclosed")),
        "{closed:?}"
    );
}

#[test]
fn load_more_appends_the_next_real_page() {
    let (transport, _backend, mut app) = setup(Scene::Mixed, Kind::Issues);
    wait_ready(&mut app);
    app.wait_for("text:Assigned Issue work item 1")
        .expect("initial rows");
    app.press("label:Load more results")
        .expect("Load more is pressable");
    app.wait_until("the second page", Duration::from_secs(5), |_| {
        searches(&transport)
            .iter()
            .any(|url| url.contains("page=2"))
    })
    .expect("page two is requested once");
    // One row of the fixture repeats the previous page's last id; dedup keeps 99 of 100.
    app.wait_for("text:rows:99")
        .expect("the appended rows join the same model without duplicates");
}

#[test]
fn append_failure_keeps_rows_and_retries_only_the_failed_page() {
    let (transport, _backend, mut app) = setup(Scene::AppendFailure, Kind::Issues);
    wait_ready(&mut app);
    app.wait_for("text:Assigned Issue work item 1")
        .expect("initial rows");
    app.wait_for("text:rows:50").expect("first page");
    app.press("label:Load more results")
        .expect("Load more is pressable");
    app.wait_for("id:work-error")
        .expect("a genuine page failure is a compact warning");
    assert!(
        app.screen().shows("rows:50"),
        "a failed append keeps the rows it already has"
    );
    assert!(
        !app.screen().shows("No results"),
        "a failure is not an empty list"
    );
    app.press("id:work-error").expect("the warning retries");
    app.wait_until("the retried page", Duration::from_secs(5), |_| {
        searches(&transport)
            .iter()
            .filter(|url| url.contains("page=2"))
            .count()
            == 2
    })
    .expect("the retry asks for the same page again");
    app.wait_for("text:rows:99")
        .expect("the retried page's rows are appended");
}

#[test]
fn confirmed_empty_result_is_not_a_failure() {
    let (_transport, _backend, mut app) = setup(Scene::Empty, Kind::Issues);
    wait_ready(&mut app);
    app.wait_for("text:No results")
        .expect("an empty result set is stated once");
    assert!(!app.screen().matches("id:work-error").unwrap_or(false));
}

#[test]
fn account_revocation_clears_the_displayed_list() {
    let (transport, _backend, mut app) = setup(Scene::Mixed, Kind::Issues);
    wait_ready(&mut app);
    app.wait_for("text:Assigned Issue work item 1")
        .expect("rows render");
    let before = searches(&transport).len();
    app.press("label:Revoke").expect("harness control");
    app.wait_for("text:Account changed; open this list again for the current account.")
        .expect("one drain clears the displayed rows");
    assert_eq!(
        app.screen().count("id:work-repository").unwrap_or(0),
        0,
        "no repository link from the previous authorization survives"
    );
    assert_eq!(
        searches(&transport).len(),
        before,
        "revocation is not an automatic retry loop"
    );
}

#[test]
fn retiring_the_list_clears_it_and_stops_work() {
    let (transport, _backend, mut app) = setup(Scene::Mixed, Kind::Issues);
    wait_ready(&mut app);
    app.wait_for("text:Assigned Issue work item 1")
        .expect("rows render");
    let before = searches(&transport).len();
    app.press("label:Retire").expect("harness control");
    app.wait(Duration::from_millis(300)).expect("settle");
    assert_eq!(app.screen().count("id:work-repository").unwrap_or(0), 0);
    assert_eq!(searches(&transport).len(), before);
}
