//! Focused tests for the account surface. Controller cases are deterministic and offline; headless
//! cases drive the real NSH element tree and a real service backend over a mock transport.

#[cfg(test)]
mod controller_cases {
    use crate::app::runtime::{Completion, Operation, Snapshot, Startup, Success};
    use crate::domain::account::AuthorizationState;
    use crate::domain::{AccountIdentity, AccountKey, AccountSummary, AuthEpoch, HostId};
    use crate::ui::accounts::controller::{AccountsController, Effect, Mode, Tone, empty_snapshot};

    fn key(id: u64) -> AccountKey {
        AccountKey::new(id, HostId::github())
    }

    fn summary(id: u64) -> AccountSummary {
        AccountSummary {
            identity: AccountIdentity {
                key: key(id),
                login: format!("user{id}"),
                display_name: Some(format!("User {id}")),
                avatar_url: None,
                html_url: None,
            },
            epoch: AuthEpoch(id),
            authorization: AuthorizationState::Verified,
        }
    }

    fn snapshot(accounts: Vec<AccountSummary>, active: Option<AccountKey>) -> Snapshot {
        Snapshot {
            accounts,
            active,
            ..empty_snapshot()
        }
    }

    #[test]
    fn authorization_distinguishes_saved_missing_rejected_and_connectivity() {
        use crate::domain::error::GitHubError;
        use crate::ui::accounts::controller::authorization_state;
        let states = [
            AuthorizationState::PendingVerification,
            AuthorizationState::MissingCredential,
            AuthorizationState::VerificationFailed(GitHubError::Unauthorized),
            AuthorizationState::VerificationFailed(GitHubError::Network {
                message: "offline".into(),
            }),
        ];
        let labels: Vec<_> = states.iter().map(|s| authorization_state(s).0).collect();
        assert!(labels[0].contains("pending"));
        assert!(labels[1].contains("missing"));
        assert!(labels[2].contains("expired or rejected"));
        assert!(labels[3].contains("connect"));
        assert!(states.iter().all(|s| authorization_state(s).1));
        assert!(!authorization_state(&AuthorizationState::Verified).1);
    }

    #[test]
    fn empty_submission_reports_an_error_and_queues_nothing() {
        let mut controller = AccountsController::new();
        assert!(!controller.submit("   "));
        assert!(controller.take_effects().is_empty());
        let status = controller.presentation().status.expect("error status");
        assert_eq!(status.tone, Tone::Error);
        assert!(status.text.contains("Paste a Classic PAT"));
    }

    #[test]
    fn submission_moves_the_pat_out_of_the_field_and_builds_one_command() {
        let mut controller = AccountsController::new();
        assert!(controller.submit("ghp_example"));
        assert_eq!(controller.input_text(), "");
        let effects = controller.take_effects();
        assert_eq!(effects.len(), 1);
        assert!(matches!(
            effects.into_iter().next(),
            Some(Effect::Submit(crate::app::runtime::Command::Add(_)))
        ));
        assert!(controller.presentation().busy);
    }

    #[test]
    fn a_second_submission_before_poll_is_refused_and_the_field_stays_empty() {
        let mut controller = AccountsController::new();
        assert!(controller.submit("ghp_first"));
        assert!(controller.take_effects().len() == 1);
        assert!(!controller.submit("ghp_second"));
        assert!(controller.take_effects().is_empty());
    }

    #[test]
    fn replacement_targets_the_chosen_account_not_the_active_one() {
        let mut controller = AccountsController::new();
        controller.ingest(&snapshot(vec![summary(1), summary(2)], Some(key(1))));
        controller.start_replace(key(2));
        assert_eq!(controller.presentation().mode, Mode::Replace(key(2)));
        assert!(controller.submit("ghp_replace"));
        let effects = controller.take_effects();
        assert!(matches!(
            effects.into_iter().next(),
            Some(Effect::Submit(crate::app::runtime::Command::Replace { account, .. })) if account == key(2)
        ));
    }

    #[test]
    fn zero_accounts_clears_the_saved_rows_and_selection() {
        let mut controller = AccountsController::new();
        assert!(controller.ingest(&snapshot(vec![summary(1)], Some(key(1)))));
        let presentation = controller.presentation();
        assert_eq!(presentation.accounts.len(), 1);
        assert_eq!(presentation.active, Some(key(1)));
        assert!(controller.ingest(&snapshot(Vec::new(), None)));
        let presentation = controller.presentation();
        assert!(presentation.accounts.is_empty());
        assert!(presentation.active.is_none());
    }

    #[test]
    fn a_completion_is_applied_once_and_does_not_overwrite_later_local_feedback() {
        let mut controller = AccountsController::new();
        let mut settled = snapshot(vec![summary(1)], Some(key(1)));
        settled.busy = None;
        settled.completion = Some(Completion::Finished {
            operation: Operation::Add,
            result: Ok(Success::Added),
        });
        assert!(controller.ingest(&settled));
        assert_eq!(
            controller.presentation().status.unwrap().tone,
            Tone::Success
        );
        controller.start_replace(key(1));
        assert_eq!(controller.presentation().status.unwrap().tone, Tone::Info);
        controller.ingest(&settled);
        assert_eq!(controller.presentation().status.unwrap().tone, Tone::Info);
    }

    #[test]
    fn removal_requires_confirmation_and_queues_remove_once_confirmed() {
        let mut controller = AccountsController::new();
        controller.ingest(&snapshot(vec![summary(1)], Some(key(1))));
        controller.ask_remove(key(1));
        assert_eq!(controller.presentation().confirm_remove, Some(key(1)));
        assert!(controller.take_effects().is_empty());
        controller.confirm_remove(key(1));
        let effects = controller.take_effects();
        assert!(matches!(
            effects.into_iter().next(),
            Some(Effect::Submit(crate::app::runtime::Command::Remove(account))) if account == key(1)
        ));
        assert!(controller.presentation().confirm_remove.is_none());
    }

    #[test]
    fn cancellation_is_queued_while_busy_and_resets_an_idle_form() {
        let mut controller = AccountsController::new();
        controller.submit("ghp_example");
        controller.take_effects();
        controller.cancel();
        assert!(matches!(
            controller.take_effects().into_iter().next(),
            Some(Effect::Cancel)
        ));

        let mut idle = AccountsController::new();
        idle.start_replace(key(9));
        idle.cancel();
        assert!(idle.take_effects().is_empty());
        assert_eq!(idle.presentation().mode, Mode::Add);
        assert!(idle.input_text().is_empty());
    }

    #[test]
    fn recovery_and_repository_retry_are_explicit_commands() {
        let mut controller = AccountsController::new();
        controller.recover();
        assert!(matches!(
            controller.take_effects().into_iter().next(),
            Some(Effect::Submit(crate::app::runtime::Command::Recover))
        ));
        controller.end_poll();
        controller.retry_repositories();
        assert!(matches!(
            controller.take_effects().into_iter().next(),
            Some(Effect::Submit(
                crate::app::runtime::Command::RetryRepositories
            ))
        ));
    }

    #[test]
    fn startup_failure_is_reported_without_inventing_an_empty_account_list() {
        let mut controller = AccountsController::new();
        let mut failed = empty_snapshot();
        failed.startup = Startup::Failed(crate::app::runtime::OpenError::HttpClient);
        controller.ingest(&failed);
        let presentation = controller.presentation();
        assert!(presentation.accounts.is_empty());
        assert_eq!(presentation.status, None);
        let (tone, text) =
            crate::ui::accounts::controller::startup_text(&presentation.startup).unwrap();
        assert_eq!(tone, Tone::Error);
        assert!(text.contains("HTTP client"));
    }
}

#[cfg(test)]
mod headless_cases {
    use crate::app::pages::NavigationSink;
    use crate::app::runtime::{Backend, ExitGate, RuntimeHandle};
    use crate::auth::credential::CredentialStore;
    use crate::auth::memory::InMemoryCredentialStore;
    use crate::data::AccountService;
    use crate::github::client::{GitHubClient, GitHubConfig};
    use crate::network::executor::BackgroundExecutor;
    use crate::network::transport::{MockTransport, json_response};
    use crate::storage::accounts::AccountDatabaseConfig;
    use crate::ui::accounts::AccountsView;
    use nsh_core::element::Root;
    use nsh_core::prelude::*;
    use nsh_core::{MountBatch, MountItem, Size};
    use nsh_engine::EngineConfig;
    use nsh_testing::Headless;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    static SLOT: OnceLock<Mutex<Option<Backend>>> = OnceLock::new();
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

    fn slot() -> &'static Mutex<Option<Backend>> {
        SLOT.get_or_init(|| Mutex::new(None))
    }

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn backend(temp: &tempfile::TempDir, mock: Arc<MockTransport>) -> Backend {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let credentials = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
            let executor = Arc::new(BackgroundExecutor::new(4));
            let client = Arc::new(GitHubClient::new(GitHubConfig::default(), mock));
            let accounts = AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                credentials,
                client,
                executor.clone(),
            )
            .await
            .unwrap();
            Backend::new(accounts, executor, ExitGate::default())
        })
    }

    fn enqueue_user(mock: &MockTransport, id: u64) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(mock.enqueue(Ok(json_response(
            200,
            &format!(r#"{{"id":{id},"login":"user{id}"}}"#),
        ))));
    }

    fn enqueue_repositories(mock: &MockTransport, ids: &[u64]) {
        let repositories: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": id,
                    "name": format!("r{id}"),
                    "full_name": format!("user1/r{id}"),
                    "owner": {"id": 1, "login": "user1"},
                    "visibility": "public"
                })
            })
            .collect();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(mock.enqueue(Ok(json_response(
            200,
            &serde_json::json!(repositories).to_string(),
        ))));
    }

    struct Pair {
        view: AccountsView,
        runtime: RuntimeHandle,
    }

    impl Pair {
        fn new() -> Self {
            let backend = slot()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .expect("headless backend");
            let navigation: NavigationSink = Rc::new(|_| {});
            Self {
                view: AccountsView::new(navigation).with_repository_preview(),
                runtime: RuntimeHandle::from_backend(backend),
            }
        }
    }

    #[component]
    fn Harness() -> impl IntoElement {
        let pair = stored(Rc::new(RefCell::new(Pair::new())));
        let ticking = pair;
        let _timer = set_interval(Duration::from_millis(10), move || {
            ticking.with(|cell| {
                let mut guard = cell.borrow_mut();
                let Pair { view, runtime } = &mut *guard;
                view.poll(runtime);
            });
        });
        view()
            .size_full()
            .bg(nsh_base::theme().background)
            .child(pair.with(|cell| cell.borrow().view.element()))
            .child(
                nsh_base::button()
                    .on_press(move |_: &()| {
                        pair.with(|cell| cell.borrow_mut().view.retire());
                    })
                    .child(text("Retire test view")),
            )
    }

    fn start_raw() -> Headless {
        let config = EngineConfig {
            viewport: Size::new(px(900.0), px(800.0)),
            ..EngineConfig::new(Root::of::<Harness>())
        };
        Headless::start(config).expect("headless engine")
    }

    fn start() -> Headless {
        let mut app = start_raw();
        app.first_render().expect("first render");
        app
    }

    #[test]
    fn the_pat_field_is_masked_and_login_renders_the_authoritative_account() {
        let _serial = serial();
        let temp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockTransport::new());
        enqueue_user(&mock, 1);
        enqueue_repositories(&mock, &[1, 2]);
        *slot()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(backend(&temp, mock));
        let mut app = start_raw();

        let secure = Arc::new(Mutex::new(false));
        let seen = secure.clone();
        app.observe(move |_: &str, batch: &MountBatch| {
            for item in &batch.items {
                if let MountItem::Input { config, .. } = item
                    && config.secure
                {
                    *seen.lock().unwrap() = true;
                }
            }
        });
        app.first_render().expect("first render");
        app.wait_for("accounts-token-form").unwrap();
        app.wait_for("accounts-token-input").unwrap();
        assert!(
            *secure.lock().unwrap(),
            "the Classic PAT field must be a secure native input"
        );
        app.type_text("accounts-token-input", "ghp_fake").unwrap();
        app.press("accounts-submit").unwrap();
        app.wait_for("saved-account-1").unwrap();
        app.wait_for("accounts-repositories-ready").unwrap();
        assert!(app.screen().shows("@user1"));
        app.wait_for("text:/2 repositories/")
            .expect("the real repository read is bound into the account surface");
    }

    #[test]
    fn an_empty_submission_reports_the_error_without_queuing_a_command() {
        let _serial = serial();
        let temp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockTransport::new());
        *slot()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(backend(&temp, mock));
        let mut app = start();
        app.wait_for("accounts-token-input").unwrap();
        app.submit("accounts-token-input").unwrap();
        app.wait_for("label:Paste a Classic PAT before submitting.")
            .unwrap();
        assert!(!app.screen().shows("Verified this session"));
    }

    #[test]
    fn removing_the_only_account_clears_the_displayed_rows() {
        let _serial = serial();
        let temp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockTransport::new());
        enqueue_user(&mock, 1);
        enqueue_repositories(&mock, &[1]);
        *slot()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(backend(&temp, mock));
        let mut app = start();
        app.wait_for("accounts-token-input").unwrap();
        app.type_text("accounts-token-input", "ghp_fake").unwrap();
        app.press("accounts-submit").unwrap();
        app.wait_for("saved-account-1").unwrap();

        app.press("account-remove-1").unwrap();
        app.wait_for("accounts-operation-status").unwrap();
        app.press("account-remove-confirm-1").unwrap();
        app.wait_for_gone("saved-account-1", nsh_testing::WAIT)
            .unwrap();
        app.wait_for("accounts-empty").unwrap();
        assert!(!app.screen().shows("@user1"));
    }

    #[test]
    fn composition_confirmation_keeps_the_form_and_secret_reset_replaces_the_native_node() {
        use nsh_core::{EventKind, EventPayload, PayloadValue};
        let _serial = serial();
        let temp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockTransport::new());
        enqueue_user(&mock, 1);
        enqueue_repositories(&mock, &[1]);
        *slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(backend(&temp, mock));
        let mut app = start_raw();
        let input = Arc::new(Mutex::new(None));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let (seen, deleted) = (input.clone(), removed.clone());
        app.observe(move |_: &str, batch: &MountBatch| {
            for item in &batch.items {
                match item {
                    MountItem::Input { id, .. } => *seen.lock().unwrap() = Some(*id),
                    MountItem::Delete { id } => deleted.lock().unwrap().push(*id),
                    _ => {}
                }
            }
        });
        app.first_render().unwrap();
        app.wait_for("accounts-token-input").unwrap();
        app.type_text("accounts-token-input", "中文-preview")
            .unwrap();
        let id = input.lock().unwrap().expect("native input");
        app.engine().control().dispatch_event(
            id,
            EventKind::CompositionChange,
            EventPayload::Map(vec![
                ("composing", PayloadValue::Bool(true)),
                ("text", PayloadValue::Str("中文".into())),
            ]),
        );
        app.submit("accounts-token-input").unwrap();
        assert!(!app.screen().matches("saved-account-1").unwrap());
        assert!(app.screen().matches("accounts-token-form").unwrap());
        app.engine().control().dispatch_event(
            id,
            EventKind::CompositionChange,
            EventPayload::Map(vec![
                ("composing", PayloadValue::Bool(false)),
                ("text", PayloadValue::Str("".into())),
            ]),
        );
        app.submit("accounts-token-input").unwrap();
        app.wait_for("saved-account-1").unwrap();
        app.wait_for_gone("accounts-token-input", nsh_testing::WAIT)
            .unwrap();
        assert!(
            removed.lock().unwrap().contains(&id),
            "submission must destroy the native edit history"
        );
    }

    #[test]
    fn retirement_removes_already_displayed_account_content() {
        let _serial = serial();
        let temp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockTransport::new());
        enqueue_user(&mock, 1);
        enqueue_repositories(&mock, &[1]);
        *slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(backend(&temp, mock));
        let mut app = start();
        app.wait_for("accounts-token-input").unwrap();
        app.type_text("accounts-token-input", "ghp_fake").unwrap();
        app.submit("accounts-token-input").unwrap();
        app.wait_for("saved-account-1").unwrap();
        app.press("Retire test view").unwrap();
        app.wait_for_gone("saved-account-1", nsh_testing::WAIT)
            .unwrap();
        app.wait_for_gone("accounts-token-form", nsh_testing::WAIT)
            .unwrap();
        assert!(!app.screen().matches("accounts-title").unwrap());
    }
}
