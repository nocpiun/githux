//! Reusable offline transport and isolated account-runtime scenarios shared by preview and tests.
use crate::{
    app::accounts::runtime::AccountRuntime,
    auth::{CredentialRef, CredentialStore, InMemoryCredentialStore, token::Pat},
    data::AccountService,
    domain::{AccountIdentity, AccountKey, HostId},
    github::{GitHubClient, GitHubConfig},
    network::{
        executor::BackgroundExecutor,
        transport::{HttpRequest, HttpResponse, Transport, TransportError},
    },
    storage::accounts::{AccountDatabase, AccountDatabaseConfig, Directory, SavedAccount},
};
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Normal,
    Expired,
    Missing,
    Pending,
    Network,
    Slow,
}
impl Scenario {
    pub const ALL: [Self; 6] = [
        Self::Normal,
        Self::Expired,
        Self::Missing,
        Self::Pending,
        Self::Network,
        Self::Slow,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Expired => "Expired",
            Self::Missing => "Missing credential",
            Self::Pending => "Restored",
            Self::Network => "Network failure",
            Self::Slow => "Slow / cancel",
        }
    }
}

pub struct SettingsTransport {
    pub requests: AtomicUsize,
    pub scenario: Scenario,
}
impl Transport for SettingsTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: crate::network::cancel::CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            self.requests.fetch_add(1, Ordering::SeqCst);
            if self.scenario == Scenario::Slow {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(TransportError::Cancelled),
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
            if self.scenario == Scenario::Network {
                return Err(TransportError::Network {
                    message: "fixture connection unavailable".into(),
                });
            }
            let path = reqwest::Url::parse(&request.url).map_err(|_| TransportError::Network {
                message: "fixture URL invalid".into(),
            })?;
            if path.path() == "/user/repos" {
                return Ok(response(200, "[]".into()));
            }
            if path.path() != "/user" {
                return Ok(response(
                    599,
                    r#"{"message":"Unmodelled settings fixture request"}"#.into(),
                ));
            }
            // Only deterministic, explicitly synthetic values are recognized. Never echo input.
            let identity = request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .and_then(|(_, value)| match value.as_str() {
                    "Bearer preview-alex" => Some((1, "alex-demo", "Alex Morgan")),
                    "Bearer preview-sam" => Some((2, "sam-demo", "Sam Lee")),
                    "Bearer preview-jordan" => Some((3, "jordan-demo", "Jordan Park")),
                    _ => None,
                });
            let Some((id, login, name)) = identity else {
                return Ok(response(401, "{}".into()));
            };
            if self.scenario == Scenario::Expired && id == 1 {
                return Ok(response(401, "{}".into()));
            }
            Ok(response(
                200,
                serde_json::json!({"id":id,"login":login,"name":name}).to_string(),
            ))
        })
    }
}
fn response(status: u16, body: String) -> HttpResponse {
    HttpResponse {
        status,
        body,
        headers: vec![],
    }
}

pub struct Fixture {
    pub runtime: Arc<AccountRuntime>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub transport: Arc<SettingsTransport>,
    pub path: std::path::PathBuf,
}
impl Fixture {
    /// Run on a background thread, before publishing the runtime to GPUI.
    pub fn open(scenario: Scenario) -> Self {
        let directory =
            std::env::temp_dir().join(format!("githux-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("fixture directory");
        let config = AccountDatabaseConfig::at(directory.join("accounts.sqlite3"));
        let store = Arc::new(InMemoryCredentialStore::new());
        let mut saved = Directory::default();
        for (id, login, display, token) in [
            (1, "alex-demo", "Alex Morgan", "preview-alex"),
            (2, "sam-demo", "Sam Lee", "preview-sam"),
        ] {
            let key = AccountKey::new(id, HostId::github());
            let reference = CredentialRef::new(HostId::github(), id);
            if scenario != Scenario::Missing || id != 1 {
                store
                    .save(&reference, &Pat::from_str(token))
                    .expect("fixture credential");
            }
            saved.accounts.push(SavedAccount {
                identity: AccountIdentity {
                    key,
                    login: login.into(),
                    display_name: Some(display.into()),
                    avatar_url: None,
                    html_url: None,
                },
                credential: reference,
            });
        }
        saved.active = Some(AccountKey::new(1, HostId::github()));
        let mut database = AccountDatabase::open(config.clone()).expect("fixture schema");
        database.commit(&saved, None, None).expect("fixture seed");
        database.close();
        let executor = Arc::new(BackgroundExecutor::new(4));
        let transport = Arc::new(SettingsTransport {
            requests: AtomicUsize::new(0),
            scenario,
        });
        let client = Arc::new(GitHubClient::new(
            GitHubConfig::default(),
            transport.clone(),
        ));
        let host = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture host");
        let accounts = host
            .block_on(AccountService::open(
                config,
                store,
                client,
                executor.clone(),
            ))
            .expect("fixture account service");
        if scenario != Scenario::Pending {
            for id in 1..=2 {
                let _ = host.block_on(accounts.revalidate(AccountKey::new(id, HostId::github())));
            }
        }
        Self {
            runtime: Arc::new(AccountRuntime::new(accounts, executor)),
            transport,
            path: directory,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_is_reusable_and_unknown_requests_are_development_errors() {
        let transport = SettingsTransport {
            requests: AtomicUsize::new(0),
            scenario: Scenario::Normal,
        };
        let host = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        host.block_on(async {
            for _ in 0..3 {
                let request = HttpRequest::get("https://api.github.com/user")
                    .with_header("Authorization", "Bearer preview-sam");
                let response = transport
                    .execute(request, Default::default())
                    .await
                    .unwrap();
                assert_eq!(response.status, 200);
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["id"],
                    2
                );
            }
            assert_eq!(
                transport
                    .execute(
                        HttpRequest::get("https://api.github.com/unmodelled"),
                        Default::default()
                    )
                    .await
                    .unwrap()
                    .status,
                599
            );
        });
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime.shutdown();
        let path = self.path.clone();
        std::thread::spawn(move || {
            crate::app::accounts::wait_for_shutdown();
            let _ = std::fs::remove_dir_all(path);
        });
    }
}
