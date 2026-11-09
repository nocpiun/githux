//! Offline native Settings preview. Real services + temporary SQLite + memory credentials, with a
//! repeatable transport keyed by the requested resource and synthetic PAT. Never opens user data.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

#[cfg(windows)]
mod preview {
    use githux::{
        app::{
            pages::PageNavigation,
            runtime::{Backend, ExitGate, RuntimeHandle},
        },
        auth::{credential::CredentialStore, memory::InMemoryCredentialStore},
        data::AccountService,
        github::client::{GitHubClient, GitHubConfig},
        network::{
            cancel::CancellationToken,
            executor::BackgroundExecutor,
            transport::{HttpRequest, HttpResponse, Transport, TransportError, json_response},
        },
        storage::accounts::AccountDatabaseConfig,
        ui::{foundation, settings::SettingsView},
    };
    use nsh_core::prelude::*;
    use std::{
        cell::RefCell,
        future::Future,
        pin::Pin,
        rc::Rc,
        sync::{Arc, Mutex, OnceLock},
        time::Duration,
    };

    static BACKEND: OnceLock<Mutex<Option<Backend>>> = OnceLock::new();

    /// Every /user and /user/repos request can be repeated. Unknown inputs return a real 401;
    /// unmodelled endpoints return 599 to expose a development error, not a product absence.
    struct OfflineTransport;
    impl Transport for OfflineTransport {
        fn execute<'a>(
            &'a self,
            request: HttpRequest,
            cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
        {
            Box::pin(async move {
                if cancel.is_cancelled() {
                    return Err(TransportError::Cancelled);
                }
                let token = request
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                    .map(|(_, value)| value.as_str())
                    .unwrap_or("");
                let id = match token {
                    "Bearer preview-alex" => 1,
                    "Bearer preview-sam" => 2,
                    "Bearer preview-jordan" => 3,
                    _ => return Ok(json_response(401, r#"{"message":"Bad credentials"}"#)),
                };
                let path =
                    reqwest::Url::parse(&request.url).map_err(|_| TransportError::Network {
                        message: "invalid fixture URL".into(),
                    })?;
                Ok(match path.path() {
                    "/user" => json_response(
                        200,
                        &format!(r#"{{"id":{id},"login":"user{id}","name":"Preview {id}"}}"#),
                    ),
                    "/user/repos" => json_response(
                        200,
                        &format!(
                            r#"[{{"id":{id},"name":"demo","full_name":"user{id}/demo","owner":{{"id":{id},"login":"user{id}"}},"visibility":"public"}}]"#
                        ),
                    ),
                    _ => json_response(599, r#"{"message":"Unmodelled preview request"}"#),
                })
            })
        }
    }

    #[component]
    fn SettingsExample() -> impl IntoElement {
        let runtime = Rc::new(RefCell::new(RuntimeHandle::from_backend(
            BACKEND
                .get()
                .expect("preview backend")
                .lock()
                .expect("backend slot")
                .take()
                .expect("one mount"),
        )));
        let settings = Rc::new(RefCell::new(
            SettingsView::new(Rc::new(|nav| {
                if let PageNavigation::External(url) = nav {
                    let _answer = nsh_core::element::open_url(&url);
                }
            }))
            .with_fixture_categories(5),
        ));
        let node = settings.borrow().element();
        let (polling, handle) = (settings.clone(), runtime.clone());
        let _timer = set_interval(Duration::from_millis(50), move || {
            polling.borrow_mut().poll(&mut handle.borrow_mut());
        });
        cleanup(move || {
            settings.borrow_mut().retire();
            runtime.borrow_mut().shutdown();
        });
        view().size_full().child(node)
    }

    pub fn run() -> std::process::ExitCode {
        use nsh_platform_windows::{AppOptions, AppSource, run_app};
        // Setup occurs before the UI event loop. The TempDir survives until the exit gate settles.
        let directory = tempfile::tempdir().expect("preview directory");
        let gate = ExitGate::default();
        let executor = Arc::new(BackgroundExecutor::new(4));
        let credentials = Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>;
        let client = Arc::new(GitHubClient::new(
            GitHubConfig::default(),
            Arc::new(OfflineTransport),
        ));
        let setup = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("setup runtime");
        let accounts = setup.block_on(async {
            let accounts = AccountService::open(
                AccountDatabaseConfig::at(directory.path().join("accounts.sqlite3")),
                credentials,
                client,
                executor.clone(),
            )
            .await
            .expect("temporary accounts");
            accounts
                .login(githux::auth::token::Pat::from_str("preview-alex"))
                .await
                .expect("fixture login");
            accounts
                .login(githux::auth::token::Pat::from_str("preview-sam"))
                .await
                .expect("fixture login");
            accounts
        });
        BACKEND
            .set(Mutex::new(Some(Backend::new(
                accounts,
                executor,
                gate.clone(),
            ))))
            .ok()
            .expect("one backend");
        nsh_platform_windows::set_ui_sets(vec![nsh_base::registry()]);
        let result = run_app(
            AppSource::Rust(
                foundation::fonts::install(nsh_engine::Root::of::<SettingsExample>())
                    .tokens(foundation::theme::tokens())
                    .app_id("dev.githux.settings-preview"),
            ),
            AppOptions {
                title: "Githux · Settings preview · preview-alex / preview-sam / preview-jordan"
                    .into(),
                size: (1000, 800),
                ..Default::default()
            },
        );
        if !gate.wait(Duration::from_secs(10)) {
            return std::process::ExitCode::FAILURE;
        }
        result
    }
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    preview::run()
}
#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("The NSH Settings window currently targets Windows.");
    std::process::ExitCode::FAILURE
}
