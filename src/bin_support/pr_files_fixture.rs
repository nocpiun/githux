use crate::auth::{memory::InMemoryCredentialStore, token::Pat};
use crate::data::{AccountService, pr_files::FilesSession};
use crate::domain::repository::RepositoryLocator;
use crate::github::{GitHubClient, pr_files::Target};
use crate::network::{
    cancel::CancellationToken,
    executor::BackgroundExecutor,
    transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError},
};
use crate::storage::accounts::AccountDatabaseConfig;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const FILES: &str = include_str!("../github/pr_files/fixtures/changed_files.json");
pub const FIRST: u64 = 1;
pub const EMPTY: u64 = 2;

#[derive(Debug)]
pub struct FilesTransport {
    #[cfg(test)]
    pub paths: std::sync::Mutex<Vec<String>>,
    fail_next_page: AtomicBool,
}

impl FilesTransport {
    fn respond(&self, request: &HttpRequest) -> HttpResponse {
        let mut response = HttpResponse {
            status: 599,
            headers: Default::default(),
            body: "{}".into(),
        };
        let Ok(url) = reqwest::Url::parse(&request.url) else {
            return response;
        };
        #[cfg(test)]
        self.paths.lock().unwrap().push(url.as_str().to_owned());
        if request.method != HttpMethod::Get || url.host_str() != Some("api.github.com") {
            return response;
        }
        match (url.path(), url.query()) {
            ("/user", None) => {
                response.status = 200;
                let second = request.headers.iter().any(|(name, value)| {
                    name.eq_ignore_ascii_case("authorization")
                        && value.ends_with("fixture-account-b")
                });
                response.body = if second {
                    r#"{"id":8,"login":"fixture-b"}"#
                } else {
                    r#"{"id":7,"login":"fixture-a"}"#
                }
                .into();
            }
            ("/repos/example/project/pulls/1/files", Some("per_page=30&page=1")) => {
                response.status = 200;
                let mut files: Vec<serde_json::Value> = serde_json::from_str(FILES).unwrap();
                files.extend((files.len()..30).map(|index| {
                    serde_json::json!({
                        "filename": format!("docs/notes-{index}.md"), "status": "added",
                        "additions": 1, "deletions": 0, "changes": 1,
                        "patch": "@@ -0,0 +1 @@\n+Notes",
                    })
                }));
                response.body = serde_json::to_string(&files).unwrap();
                response.headers.push(("link".into(), "<https://api.github.com/repositories/17/pulls/1/files?per_page=30&page=2>; rel=\"next\"".into()));
            }
            ("/repos/example/project/pulls/1/files", Some("per_page=30&page=2")) => {
                if self.fail_next_page.swap(false, Ordering::AcqRel) {
                    response.status = 503;
                } else {
                    response.status = 200;
                    response.body = serde_json::json!([{
                        "filename": "docs/page-two.md", "status": "added",
                        "additions": 1, "deletions": 0, "changes": 1,
                        "patch": format!("@@ -0,0 +1 @@\n+{} end of line", "long text ".repeat(38)),
                    }])
                    .to_string();
                }
            }
            ("/repos/example/project/pulls/2/files", Some("per_page=30&page=1")) => {
                response.status = 200;
                response.body = "[]".into();
            }
            ("/repos/example/project/pulls/3/files", Some("per_page=30&page=100")) => {
                response.status = 200;
                let files = (0..30)
                    .map(|index| {
                        serde_json::json!({
                            "filename": format!("assets/file-{index}.bin"), "status": "modified",
                            "additions": 0, "deletions": 0, "changes": 0,
                        })
                    })
                    .collect::<Vec<_>>();
                response.body = serde_json::to_string(&files).unwrap();
            }
            ("/repos/example/project/pulls/4/files", Some("per_page=30&page=1")) => {
                response.status = 200;
                response.body = FILES.into();
                response.headers.push(("link".into(), "<https://api.github.com/repositories/17/pulls/2/files?per_page=30&page=2>; rel=\"next\"".into()));
            }
            _ => {}
        }
        response
    }
}

impl Transport for FilesTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > limit {
                return Err(TransportError::ResponseTooLarge { limit });
            }
            Ok(response)
        })
    }

    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            Ok(self.respond(&request))
        })
    }
}

pub struct Fixture {
    #[cfg(test)]
    pub transport: Arc<FilesTransport>,
    pub executor: Arc<BackgroundExecutor>,
    pub accounts: AccountService,
    _temp: tempfile::TempDir,
}

impl Fixture {
    pub async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(FilesTransport {
            #[cfg(test)]
            paths: std::sync::Mutex::new(Vec::new()),
            fail_next_page: AtomicBool::new(true),
        });
        let executor = Arc::new(BackgroundExecutor::new(4));
        let accounts = AccountService::open(
            AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(Default::default(), transport.clone())),
            executor.clone(),
        )
        .await
        .unwrap();
        accounts
            .login(Pat::from_str("fixture-account-a"))
            .await
            .unwrap();
        Self {
            #[cfg(test)]
            transport,
            executor,
            accounts,
            _temp: temp,
        }
    }

    pub fn session(&self, number: u64) -> FilesSession {
        FilesSession::new(
            self.accounts.repo_service().session().unwrap(),
            Target {
                repository: RepositoryLocator::new("example", "project").unwrap(),
                number: number.try_into().unwrap(),
            },
        )
    }
}
