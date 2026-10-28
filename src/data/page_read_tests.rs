//! End-to-end through AccountService: synthetic credentials, mock transport.
use super::account_tests::{config, key};
use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::content::PreparedBody;
use crate::domain::home::{DateBound, FeedFetch, HomeError, UpdatedWindow};
use crate::domain::profile::{ContributionRange, ProfileBasic, ProfileError, Showcase};
use crate::domain::repository::*;
use crate::domain::repository_extras::{
    ContributorsState, ExtrasCompleteness, RepositoryExtrasError,
};
use crate::github::home::{FeedLimits, HomeSearchLimits};
use crate::github::profile::ProfileLimits;
use crate::github::repository_extras::RepositoryExtrasLimits;
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ROOT: &str = "1111111111111111111111111111111111111111";
const BLOB: &str = "3333333333333333333333333333333333333333";
fn locator() -> RepositoryLocator {
    RepositoryLocator::new("octo", "repo").unwrap()
}
fn identity() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 42,
        locator: locator(),
    }
}
fn profile(login: &str) -> ProfileBasic {
    ProfileBasic {
        id: 1,
        login: login.into(),
        name: None,
        bio: None,
        avatar_url: None,
        profile_url: None,
        company: None,
        location: None,
        website_url: None,
        followers: None,
        following: None,
        public_repos: None,
        public_gists: None,
        created_at: None,
    }
}
fn details(login: &str, visibility: &str, branch: Option<&str>) -> Value {
    json!({"id":42,"name":login,"owner":{"id":1,"login":login},"visibility":visibility,
        "private":visibility != "public","default_branch":branch,"fork":false,
        "archived":false,"stargazers_count":0,"forks_count":0,"open_issues_count":0,
        "size":1,"html_url":format!("https://github.com/{login}/{login}")})
}
fn extras_details() -> Value {
    json!({"id":42,"name":"repo","owner":{"id":1,"login":"octo"},"private":false,
        "default_branch":"main","fork":false,"archived":false,"stargazers_count":0,
        "forks_count":0,"open_issues_count":0,"size":1,"html_url":"https://github.com/octo/repo"})
}
fn snapshot() -> Value {
    json!({"sha":COMMIT,"commit":{"message":"profile","tree":{"sha":ROOT},"author":null,"committer":null}})
}
fn readme() -> Value {
    json!({"type":"file","path":"README.md","sha":BLOB,"size":6})
}

type Step = (
    HttpResponse,
    Option<tokio::sync::oneshot::Receiver<()>>,
    Option<tokio::sync::oneshot::Sender<()>>,
);
#[derive(Default)]
struct Controlled {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<HttpRequest>>,
}
impl Controlled {
    fn push(&self, status: u16, body: Value) {
        self.push_response(json_response(status, &body.to_string()));
    }
    fn push_response(&self, response: HttpResponse) {
        self.steps.lock().unwrap().push_back((response, None, None));
    }
    fn block(
        &self,
        status: u16,
        body: Value,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        self.steps.lock().unwrap().push_back((
            json_response(status, &body.to_string()),
            Some(gate),
            Some(entered),
        ));
        (ready, release)
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Transport for Controlled {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            let (response, gate, entered) = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted response");
            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            if let Some(gate) = gate {
                gate.await.expect("release request");
            }
            Ok(response)
        })
    }
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max {
                return Err(TransportError::ResponseTooLarge { limit: max });
            }
            Ok(response)
        })
    }
}
async fn ready(signal: tokio::sync::oneshot::Receiver<()>) {
    tokio::time::timeout(Duration::from_secs(5), signal)
        .await
        .expect("request entered")
        .unwrap();
}
async fn service(
    temp: &tempfile::TempDir,
    transport: Arc<Controlled>,
    concurrency: usize,
) -> Arc<AccountService> {
    Arc::new(
        AccountService::open(
            config(temp),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(Default::default(), transport)),
            Arc::new(BackgroundExecutor::new(concurrency)),
        )
        .await
        .unwrap(),
    )
}
async fn login(svc: &AccountService, transport: &Controlled, id: u64, login: &str, token: &str) {
    transport.push(200, json!({"id":id,"login":login}));
    svc.login(Pat::from_str(token)).await.unwrap();
}
fn auth(request: &HttpRequest) -> &str {
    request
        .headers
        .iter()
        .find(|(name, _)| name == "Authorization")
        .map(|(_, value)| value.as_str())
        .unwrap()
}

#[tokio::test]
async fn home_and_feed_use_captured_login_credentials_bounds_and_no_validators() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 2, "other", "other-token").await;
    login(&svc, &transport, 1, "authoritative", "account-token").await;
    let session = svc.page_session().unwrap();
    let window = UpdatedWindow::new(Some(DateBound::new("2026-10-01").unwrap()), None).unwrap();
    transport.push(
        200,
        json!({"total_count":0,"incomplete_results":false,"items":[]}),
    );
    assert_eq!(
        session
            .home(
                Some(window),
                &HomeSearchLimits {
                    per_page: 42,
                    ..Default::default()
                },
                Default::default()
            )
            .await
            .unwrap()
            .value
            .total_count,
        0
    );
    transport.push_response(HttpResponse {
        status: 200,
        headers: vec![
            ("etag".into(), "\"feed\"".into()),
            ("x-poll-interval".into(), "60".into()),
        ],
        body: "[]".into(),
    });
    let FeedFetch::Events(feed) = session
        .feed(
            &FeedLimits {
                per_page: 23,
                ..Default::default()
            },
            Default::default(),
        )
        .await
        .unwrap()
        .value
    else {
        panic!("events")
    };
    assert_eq!(feed.etag.as_deref(), Some("\"feed\""));
    assert_eq!(feed.poll_interval_seconds, Some(60));
    let requests = transport.requests.lock().unwrap();
    assert!(
        requests[2]
            .url
            .contains("q=involves%3Aauthoritative+updated%3A%3E%3D2026-10-01")
    );
    assert!(
        requests[2].url.contains("sort=updated")
            && requests[2].url.contains("order=desc")
            && requests[2].url.contains("per_page=42")
    );
    assert!(
        requests[3]
            .url
            .ends_with("/users/authoritative/received_events?per_page=23&page=1")
    );
    for request in &requests[2..4] {
        assert_eq!(auth(request), "Bearer account-token");
    }
    assert!(
        !requests[3]
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("if-none-match"))
    );
}

#[tokio::test]
async fn profile_target_does_not_replace_authorizing_account_and_graphql_partial_survives() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 3).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    transport.push(200, json!({"id":99,"login":"Target","name":"Target Name"}));
    assert_eq!(
        session
            .profile("Target", Default::default())
            .await
            .unwrap()
            .value
            .login,
        "Target"
    );
    transport.push(200, json!({"data":{"user":{"pinnedItems":{"totalCount":0,"pageInfo":{"hasNextPage":false},"nodes":[]},"itemShowcase":{"hasPinnedItems":false,"items":{"pageInfo":{"hasNextPage":false},"nodes":[]}}}},"errors":[{"type":"FORBIDDEN","path":["user","private"]}]}));
    let pinned = session
        .profile_pinned("Target", &ProfileLimits::default(), Default::default())
        .await
        .unwrap()
        .value;
    assert_eq!(pinned.errors.len(), 1);
    assert!(matches!(pinned.showcase, Showcase::PopularRepositories(_)));
    transport.push(200, json!({"data":{"user":{"contributionsCollection":{"contributionCalendar":{"totalContributions":0,"weeks":[],"months":[]}}}}}));
    let range = ContributionRange::new("2026-01-01", "2026-01-31").unwrap();
    session
        .contribution_calendar(
            "Target",
            &range,
            &ProfileLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    let requests = transport.requests.lock().unwrap();
    assert!(requests[1].url.ends_with("/users/Target"));
    for request in &requests[1..4] {
        assert_eq!(auth(request), "Bearer viewer-token");
    }
    for request in &requests[2..4] {
        assert!(request.body.as_ref().unwrap().contains("Target"));
    }
    assert!(
        requests[3]
            .body
            .as_ref()
            .unwrap()
            .contains("2026-01-01T00:00:00Z")
    );
}

#[tokio::test]
async fn extras_preserve_distinct_models_metadata_and_share_one_account() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 3).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let repo = svc.repo_service().session().unwrap();
    transport.push(200, extras_details());
    let repository = repo
        .repository(locator(), &Default::default(), Default::default())
        .await
        .unwrap()
        .value
        .identity;
    let session = svc.page_session().unwrap();
    transport.push_response(HttpResponse {
        status: 200,
        headers: vec![("etag".into(), "r1".into())],
        body: json!([{"id":1,"tag_name":"v1","draft":false,"prerelease":false}]).to_string(),
    });
    let releases = session
        .releases(
            &repository,
            &RepositoryExtrasLimits {
                per_page: 17,
                ..Default::default()
            },
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(releases.completeness, ExtrasCompleteness::Complete);
    assert_eq!(releases.page_metadata[0].etag.as_deref(), Some("r1"));
    transport.push_response(HttpResponse {
        status: 404,
        headers: vec![("etag".into(), "latest".into())],
        body: "{}".into(),
    });
    assert!(
        session
            .latest_release(&repository, &Default::default(), Default::default())
            .await
            .unwrap()
            .value
            .release
            .is_none()
    );
    transport.push(204, json!(null));
    let contributors = session
        .contributors(&repository, &Default::default(), Default::default())
        .await
        .unwrap()
        .value;
    assert_eq!(contributors.state, ContributorsState::EmptyRepository);
    transport.push_response(HttpResponse {
        status: 200,
        headers: vec![("last-modified".into(), "date".into())],
        body: json!({"Rust":75,"TOML":25}).to_string(),
    });
    let languages = session
        .languages(&repository, &Default::default(), Default::default())
        .await
        .unwrap()
        .value;
    assert_eq!(languages.total_bytes, 100);
    assert_eq!(languages.metadata.last_modified.as_deref(), Some("date"));
    let requests = transport.requests.lock().unwrap();
    assert!(requests[2].url.contains("per_page=17"));
    for request in &requests[2..6] {
        assert_eq!(auth(request), "Bearer viewer-token");
        assert!(
            !request
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("if-none-match")
                    || name.eq_ignore_ascii_case("if-modified-since"))
        );
    }
}

#[tokio::test]
async fn unexpected_304_and_rate_limit_remain_errors_not_empty_values() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    transport.push_response(HttpResponse {
        status: 304,
        headers: vec![],
        body: String::new(),
    });
    assert!(matches!(
        session
            .feed(&Default::default(), Default::default())
            .await
            .unwrap_err(),
        HomeError::InvalidResponse { .. }
    ));
    transport.push(200, extras_details());
    let repository = svc
        .repo_service()
        .session()
        .unwrap()
        .repository(locator(), &Default::default(), Default::default())
        .await
        .unwrap()
        .value
        .identity;
    transport.push(304, json!({}));
    assert!(matches!(
        session
            .languages(&repository, &Default::default(), Default::default())
            .await
            .unwrap_err(),
        RepositoryExtrasError::GitHub(GitHubError::UnexpectedStatus { status: 304 })
    ));
    transport.push_response(HttpResponse {
        status: 403,
        headers: vec![("x-ratelimit-remaining".into(), "0".into())],
        body: "{}".into(),
    });
    assert!(matches!(
        session
            .releases(&repository, &Default::default(), Default::default())
            .await
            .unwrap_err(),
        RepositoryExtrasError::GitHub(GitHubError::RateLimited { .. })
    ));
}

#[tokio::test]
async fn profile_readme_uses_canonical_public_target_and_fixed_commit() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 3).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    transport.push(200, details("Target", "public", Some("main")));
    transport.push(200, snapshot());
    transport.push(200, readme());
    transport.push(
        200,
        json!({"sha":BLOB,"size":6,"encoding":"base64","content":"SGVsbG8h"}),
    );
    let result = session
        .profile_readme(
            &profile("Target"),
            &Default::default(),
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap();
    let ProfileReadme::Prepared {
        repository,
        commit,
        content,
    } = result.value
    else {
        panic!("prepared")
    };
    assert_eq!(repository.locator().owner(), "Target");
    assert_eq!(commit.as_str(), COMMIT);
    assert_eq!(content.identity.commit().as_str(), COMMIT);
    assert!(matches!(content.body, PreparedBody::Utf8 { ref text, .. } if text == "Hello!"));
    let requests = transport.requests.lock().unwrap();
    assert!(requests[1].url.ends_with("/repos/Target/Target"));
    assert!(
        requests[2]
            .url
            .ends_with("/repos/Target/Target/commits/main")
    );
    assert!(
        requests[3]
            .url
            .ends_with(&format!("/repos/Target/Target/readme?ref={COMMIT}"))
    );
    assert!(
        requests[4]
            .url
            .ends_with(&format!("/repos/Target/Target/git/blobs/{BLOB}"))
    );
    assert!(
        requests[1..]
            .iter()
            .all(|r| auth(r) == "Bearer viewer-token")
    );
}

#[tokio::test]
async fn profile_readme_distinguishes_private_no_branch_missing_uncertain_and_failures() {
    for (visibility, branch, expected) in [
        ("private", Some("main"), 0),
        ("internal", Some("main"), 0),
        ("public", None, 1),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let svc = service(&temp, transport.clone(), 2).await;
        login(&svc, &transport, 1, "viewer", "token").await;
        transport.push(200, details("Target", visibility, branch));
        let result = svc
            .page_session()
            .unwrap()
            .profile_readme(
                &profile("Target"),
                &Default::default(),
                &Default::default(),
                Default::default(),
            )
            .await
            .unwrap()
            .value;
        let ProfileReadme::NotEligible { reason, .. } = result else {
            panic!("not eligible")
        };
        assert_eq!(
            reason == ProfileReadmeIneligible::NoDefaultBranch,
            expected == 1
        );
        assert_eq!(transport.count(), 2);
    }
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 1, "viewer", "token").await;
    let mut renamed = details("Target", "public", Some("main"));
    renamed["name"] = json!("renamed");
    transport.push(200, renamed);
    assert!(matches!(
        svc.page_session()
            .unwrap()
            .profile_readme(
                &profile("Target"),
                &Default::default(),
                &Default::default(),
                Default::default()
            )
            .await
            .unwrap()
            .value,
        ProfileReadme::NotEligible {
            reason: ProfileReadmeIneligible::NotPublicSameNameRepository,
            ..
        }
    ));
    for empty in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let svc = service(&temp, transport.clone(), 2).await;
        login(&svc, &transport, 1, "viewer", "token").await;
        transport.push(200, details("Target", "public", Some("main")));
        transport.push(200, snapshot());
        transport.push(404, json!({}));
        transport.push(200, json!({"sha":ROOT,"truncated":false,"tree":if empty { json!([]) } else { json!([{"path":"file","type":"blob","mode":"100644","sha":BLOB}]) }}));
        let result = svc
            .page_session()
            .unwrap()
            .profile_readme(
                &profile("Target"),
                &Default::default(),
                &Default::default(),
                Default::default(),
            )
            .await
            .unwrap()
            .value;
        assert_eq!(matches!(result, ProfileReadme::Missing { .. }), empty);
        assert_eq!(matches!(result, ProfileReadme::Unavailable { .. }), !empty);
    }
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 1, "viewer", "token").await;
    transport.push(403, json!({}));
    assert!(matches!(
        svc.page_session()
            .unwrap()
            .profile_readme(
                &profile("Target"),
                &Default::default(),
                &Default::default(),
                Default::default()
            )
            .await
            .unwrap_err(),
        RepositoryError::GitHub(GitHubError::Forbidden)
    ));
}

#[tokio::test]
async fn stale_success_error_partial_and_aba_are_rejected_across_domains() {
    for change in ["switch", "aba", "replace", "remove-readd", "close"] {
        for status in [200, 401] {
            let temp = tempfile::tempdir().unwrap();
            let transport = Arc::new(Controlled::default());
            let svc = service(&temp, transport.clone(), 3).await;
            login(&svc, &transport, 2, "other", "other-token").await;
            login(&svc, &transport, 1, "viewer", "old-token").await;
            let session = svc.page_session().unwrap();
            let context = session.context().clone();
            let (entered, release) = transport.block(
                status,
                if status == 200 {
                    json!({"total_count":0,"items":[]})
                } else {
                    json!({})
                },
            );
            let task = tokio::spawn(async move {
                session
                    .home(None, &Default::default(), Default::default())
                    .await
            });
            ready(entered).await;
            match change {
                "switch" => svc.set_active(key(2)).await.unwrap(),
                "aba" => {
                    svc.set_active(key(2)).await.unwrap();
                    svc.set_active(key(1)).await.unwrap();
                }
                "replace" => {
                    transport.push(200, json!({"id":1,"login":"viewer"}));
                    svc.replace(key(1), Pat::from_str("new-token"))
                        .await
                        .unwrap();
                }
                "remove-readd" => {
                    svc.remove_saved(key(1)).await.unwrap();
                    login(&svc, &transport, 1, "viewer", "new-token").await;
                }
                "close" => svc.close().await.unwrap(),
                _ => unreachable!(),
            }
            release.send(()).unwrap();
            assert_eq!(
                task.await.unwrap().unwrap_err(),
                HomeError::GitHub(GitHubError::StaleContext),
                "{change} {status}"
            );
            assert!(!svc.repo_service().is_current(&context));
        }
    }
}

#[tokio::test]
async fn profile_readme_auth_change_between_steps_and_pinned_partial_error_are_stale() {
    for step in ["repository", "revision", "readme"] {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let svc = service(&temp, transport.clone(), 3).await;
        login(&svc, &transport, 2, "other", "other-token").await;
        login(&svc, &transport, 1, "viewer", "viewer-token").await;
        if step != "repository" {
            transport.push(200, details("Target", "public", Some("main")));
        }
        if step == "readme" {
            transport.push(200, snapshot());
        }
        let (entered, release) = transport.block(
            200,
            match step {
                "repository" => details("Target", "public", Some("main")),
                "revision" => snapshot(),
                _ => readme(),
            },
        );
        let session = svc.page_session().unwrap();
        let task = tokio::spawn(async move {
            session
                .profile_readme(
                    &profile("Target"),
                    &Default::default(),
                    &Default::default(),
                    Default::default(),
                )
                .await
        });
        ready(entered).await;
        svc.set_active(key(2)).await.unwrap();
        svc.set_active(key(1)).await.unwrap();
        release.send(()).unwrap();
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            RepositoryError::GitHub(GitHubError::StaleContext),
            "{step}"
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 2, "other", "other-token").await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    let (entered, release) = transport.block(200, json!({"data":{"user":{"pinnedItems":{"totalCount":0,"nodes":[]},"itemShowcase":{"hasPinnedItems":false,"items":{"nodes":[]}}}},"errors":[{"type":"FORBIDDEN"}]}));
    let task = tokio::spawn(async move {
        session
            .profile_pinned("Target", &Default::default(), Default::default())
            .await
    });
    ready(entered).await;
    svc.set_active(key(2)).await.unwrap();
    release.send(()).unwrap();
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        ProfileError::GitHub(GitHubError::StaleContext)
    );
}

#[tokio::test]
async fn cancellation_service_instance_mismatch_and_stale_pre_call_have_no_leakage() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let first = service(&a, transport.clone(), 2).await;
    let second = service(&b, transport.clone(), 2).await;
    login(&first, &transport, 1, "same", "first-token").await;
    login(&second, &transport, 1, "same", "second-token").await;
    let session = first.page_session().unwrap();
    assert_eq!(
        session.context().auth(),
        second.page_session().unwrap().context().auth()
    );
    assert!(!second.repo_service().is_current(session.context()));
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    assert_eq!(
        session
            .home(None, &Default::default(), cancel)
            .await
            .unwrap_err(),
        HomeError::GitHub(GitHubError::Cancelled)
    );
    assert_eq!(transport.count(), 2);
    second.set_active(key(1)).await.unwrap();
    login(&first, &transport, 2, "other", "other-token").await;
    assert_eq!(
        session
            .feed(&Default::default(), Default::default())
            .await
            .unwrap_err(),
        HomeError::GitHub(GitHubError::StaleContext)
    );
    assert_eq!(transport.count(), 3);
    assert!(!format!("{session:?}").contains("first-token"));
}

struct PausedLoad {
    inner: InMemoryCredentialStore,
    gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    fail: bool,
}
impl CredentialStore for PausedLoad {
    fn save(&self, r: &CredentialRef, p: &Pat) -> Result<(), CredentialError> {
        self.inner.save(r, p)
    }
    fn delete(&self, r: &CredentialRef) -> Result<(), CredentialError> {
        self.inner.delete(r)
    }
    fn describe(&self) -> &'static str {
        "paused load"
    }
    fn load(&self, r: &CredentialRef) -> Result<Option<Pat>, CredentialError> {
        if let Some((entered, resume)) = self.gate.lock().unwrap().take() {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(!self.fail, "synthetic credential worker failure");
        }
        self.inner.load(r)
    }
}
async fn paused_service(
    temp: &tempfile::TempDir,
    transport: Arc<Controlled>,
    store: Arc<PausedLoad>,
) -> Arc<AccountService> {
    Arc::new(
        AccountService::open(
            config(temp),
            store,
            Arc::new(GitHubClient::new(Default::default(), transport)),
            Arc::new(BackgroundExecutor::new(1)),
        )
        .await
        .unwrap(),
    )
}
fn pause(
    store: &PausedLoad,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, resume) = std::sync::mpsc::channel();
    *store.gate.lock().unwrap() = Some((entered, resume));
    (ready, release)
}

#[tokio::test]
async fn slow_credentials_and_shared_permit_never_send_under_superseded_auth() {
    for boundary in ["load", "permit"] {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let store = Arc::new(PausedLoad {
            inner: Default::default(),
            gate: Mutex::new(None),
            fail: false,
        });
        let svc = paused_service(&temp, transport.clone(), store.clone()).await;
        login(&svc, &transport, 2, "other", "other-token").await;
        login(&svc, &transport, 1, "viewer", "viewer-token").await;
        let session = svc.page_session().unwrap();
        let hold = (boundary == "permit").then(|| svc.executor.semaphore().clone());
        let permit = match hold {
            Some(semaphore) => Some(semaphore.acquire_owned().await.unwrap()),
            None => None,
        };
        let release_load = if boundary == "load" {
            let (ready, release) = pause(&store);
            Some((ready, release))
        } else {
            None
        };
        let task =
            tokio::spawn(
                async move { session.feed(&Default::default(), Default::default()).await },
            );
        if let Some((signal, release)) = release_load {
            ready(signal).await;
            svc.set_active(key(2)).await.unwrap();
            svc.set_active(key(1)).await.unwrap();
            release.send(()).unwrap();
        } else {
            tokio::time::sleep(Duration::from_millis(80)).await;
            svc.set_active(key(2)).await.unwrap();
            svc.set_active(key(1)).await.unwrap();
        }
        drop(permit);
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            HomeError::GitHub(GitHubError::StaleContext),
            "{boundary}"
        );
        assert_eq!(transport.count(), 2, "{boundary}: no page request");
    }
}

#[tokio::test]
async fn cancelled_load_does_not_send_and_worker_failure_after_switch_is_stale() {
    for outcome in ["cancel", "panic"] {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let store = Arc::new(PausedLoad {
            inner: Default::default(),
            gate: Mutex::new(None),
            fail: outcome == "panic",
        });
        let svc = paused_service(&temp, transport.clone(), store.clone()).await;
        login(&svc, &transport, 1, "viewer", "viewer-token").await;
        let session = svc.page_session().unwrap();
        let (ready_signal, release) = pause(&store);
        let (cancel, source) = CancellationToken::new();
        let task = tokio::spawn(async move { session.feed(&Default::default(), cancel).await });
        ready(ready_signal).await;
        if outcome == "panic" {
            svc.close().await.unwrap();
        } else {
            source.cancel();
        }
        release.send(()).unwrap();
        let expected = if outcome == "panic" {
            GitHubError::StaleContext
        } else {
            GitHubError::Cancelled
        };
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            HomeError::GitHub(expected)
        );
        assert_eq!(transport.count(), 1);
    }
}
