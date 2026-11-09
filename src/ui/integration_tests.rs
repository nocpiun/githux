//! Real Shell composition regressions; synthetic service transport, no window or network.
use super::*;
use crate::app::runtime::{Backend, ExitGate};
use crate::auth::memory::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::github::client::{GitHubClient, GitHubConfig};
use crate::network::{
    executor::BackgroundExecutor,
    transport::{HttpResponse, MockTransport},
};
use crate::storage::accounts::AccountDatabaseConfig;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::sync::Arc;

#[test]
fn settings_then_search_can_mount_a_new_page_and_shutdown_settles() {
    let directory = tempfile::tempdir().unwrap();
    let gate = ExitGate::default();
    let setup = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = setup.block_on(async {
        let transport = Arc::new(MockTransport::new());
        for body in [r#"{"id":1,"login":"user1"}"#, "[]"] {
            transport
                .enqueue(Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: body.into(),
                }))
                .await;
        }
        let executor = Arc::new(BackgroundExecutor::new(4));
        let service = AccountService::open(
            AccountDatabaseConfig::at(directory.path().join("accounts.sqlite3")),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(GitHubConfig::default(), transport)),
            executor.clone(),
        )
        .await
        .unwrap();
        Backend::new(service, executor, gate.clone())
    });
    install_runtime(RuntimeHandle::from_backend(backend));
    let mut app = Headless::start(EngineConfig::new(Root::of::<Shell>())).unwrap();
    app.first_render().unwrap();
    app.wait_for("accounts-token-input").unwrap();
    app.type_text("accounts-token-input", "synthetic-integration")
        .unwrap();
    app.press("accounts-submit").unwrap();
    app.wait_for_gone("accounts-token-input", nsh_testing::WAIT)
        .unwrap();
    app.press("shell-nav-Settings").unwrap();
    app.wait_for("saved-account-1").unwrap();
    app.press("shell-search").unwrap();
    app.wait_for("shell-palette-query").unwrap();
    app.type_text("shell-palette-query", "Notifications")
        .unwrap();
    app.press("label:Notifications").unwrap();
    app.wait_for_gone("shell-palette-query", nsh_testing::WAIT)
        .unwrap();
    app.wait_for_gone("saved-account-1", nsh_testing::WAIT)
        .unwrap();
    // A real notification surface (success or transport error) must replace Settings.
    app.wait_for("notifications-view").unwrap();
    drop(app);
    assert!(
        gate.wait(Duration::from_secs(5)),
        "Shell drop must release its runtime"
    );
}
