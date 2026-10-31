//! End-to-end through AccountService: synthetic credentials, mock transport.
mod feed_refresh_tests;
use super::account_tests::{config, key};
use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::content::PreparedBody;
use crate::domain::home::{DateBound, FeedPartialReason, FeedReadiness, HomeError, UpdatedWindow};
use crate::domain::profile::{ContributionRange, ProfileBasic, ProfileError, Showcase};
use crate::domain::repository::*;
use crate::domain::repository_extras::{
    ContributorsState, ExtrasCompleteness, RepositoryExtrasError,
};
use crate::github::home::{FeedLimits, FollowingLimits, HomeSearchLimits, PullRequestDetailLimits};
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
fn root_tree(truncated: bool) -> Value {
    json!({"sha":ROOT,"truncated":truncated,"tree":[
        {"path":"docs","type":"tree","mode":"040000","sha":BLOB},
        {"path":"README.md","type":"blob","mode":"100644","sha":BLOB,"size":6}
    ]})
}

/// The Feed read needs explicit auxiliary budgets; tests use small, fixed ones so
/// a misuse shows up as a request-count assertion rather than a silent default.
fn feed_args(author_lookups: usize) -> (FollowingLimits, PullRequestDetailLimits, usize) {
    (
        FollowingLimits::default(),
        PullRequestDetailLimits::default(),
        author_lookups,
    )
}

async fn feed_call(
    session: &PageReadSession,
    limits: &FeedLimits,
    cancel: CancellationToken,
) -> Result<crate::data::Scoped<crate::domain::home::FeedOutcome>, HomeError> {
    let (following, author, lookups) = feed_args(4);
    session
        .feed(limits, &following, &author, lookups, cancel)
        .await
}

type Step = (
    HttpResponse,
    Option<tokio::sync::oneshot::Receiver<()>>,
    Option<tokio::sync::oneshot::Sender<()>>,
);
#[derive(Default)]
struct Controlled {
    steps: Mutex<VecDeque<Step>>,
    routed_steps: Mutex<std::collections::HashMap<String, Step>>,
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

    fn block_url(
        &self,
        url: String,
        status: u16,
        body: Value,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        self.routed_steps.lock().unwrap().insert(
            url,
            (
                json_response(status, &body.to_string()),
                Some(gate),
                Some(entered),
            ),
        );
        (ready, release)
    }
}
impl Transport for Controlled {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let routed = self.routed_steps.lock().unwrap().remove(&request.url);
            self.requests.lock().unwrap().push(request);
            let (response, gate, entered) = routed.unwrap_or_else(|| {
                self.steps
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("scripted response")
            });
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
    // Feed now walks three reads in a fixed order: source events, followed
    // accounts, then one author lookup per distinct merged pull request. With no
    // merged events in the source, no detail request is made.
    transport.push_response(HttpResponse {
        status: 200,
        headers: vec![
            ("etag".into(), "\"feed\"".into()),
            ("x-poll-interval".into(), "60".into()),
        ],
        body: "[]".into(),
    });
    transport.push(200, json!([]));
    let feed = feed_call(
        &session,
        &FeedLimits {
            per_page: 23,
            ..Default::default()
        },
        Default::default(),
    )
    .await
    .unwrap()
    .value;
    assert_eq!(feed.etag.as_deref(), Some("\"feed\""));
    assert_eq!(feed.poll_interval_seconds, Some(60));
    assert!(feed.activities.is_empty());
    // A complete source plus a complete following list and no merges is a normal
    // empty Feed, not a partial read.
    assert_eq!(feed.readiness, FeedReadiness::Complete);
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 5, "home, events, following");
    assert!(
        requests[2]
            .url
            .contains("q=involves%3Aauthoritative+is%3Aopen+updated%3A%3E%3D2026-10-01"),
        "the confirmed Home query keeps involves + is:open and the explicit window: {}",
        requests[2].url
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
    assert!(
        requests[4]
            .url
            .ends_with("/user/following?per_page=100&page=1"),
        "the followed-account read targets the authenticated endpoint: {}",
        requests[4].url
    );
    for request in &requests[2..5] {
        assert_eq!(
            auth(request),
            "Bearer account-token",
            "every Feed read uses the captured account credential"
        );
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

/// End-to-end through the **real adapters**: live-shaped received events, a real
/// `/user/following` page set and a real pull-request detail response produce the
/// visible Feed. This is the regression for the failures found against live data:
/// the old classifier accepted none of the 17 real `action=merged` events (they
/// carry no author), and nothing checked the followed relationship, so unfollowed
/// subjects appeared on 8 of 13 stars.
#[tokio::test]
async fn real_adapter_shapes_produce_only_followed_contribution_star_and_create_cards() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    // 1. Source events: a live `action=merged` (no embedded author, no merged
    //    flag), a star by a followed account, a star by an unfollowed account, a
    //    branch creation, and a repository creation by a followed account.
    transport.push(
        200,
        json!([
            {
                "id": "evt-merged",
                "type": "PullRequestEvent",
                "actor": {"id": 10, "login": "merging-maintainer", "avatar_url": null},
                "repo": {"id": 99, "name": "acme/widgets", "url": "https://api.github.com/repos/acme/widgets"},
                "payload": {
                    "action": "merged",
                    "number": 42,
                    "pull_request": {
                        "url": "https://api.github.com/repos/acme/widgets/pulls/42",
                        "id": 424242u64,
                        "number": 42,
                        "head": {"ref": "feat/x", "sha": "aaaa"},
                        "base": {"ref": "main", "sha": "bbbb"}
                    }
                },
                "created_at": "2026-10-30T12:00:41Z"
            },
            {
                "id": "evt-star-followed",
                "type": "WatchEvent",
                "actor": {"id": 11, "login": "followed-friend", "avatar_url": null},
                "repo": {"id": 99, "name": "acme/widgets", "url": "https://api.github.com/repos/acme/widgets"},
                "payload": {"action": "started"},
                "created_at": "2026-10-30T11:00:00Z"
            },
            {
                "id": "evt-star-unfollowed",
                "type": "WatchEvent",
                "actor": {"id": 12, "login": "unfollowed-stranger", "avatar_url": null},
                "repo": {"id": 98, "name": "acme/other", "url": "https://api.github.com/repos/acme/other"},
                "payload": {"action": "started"},
                "created_at": "2026-10-30T10:00:00Z"
            },
            {
                "id": "evt-branch",
                "type": "CreateEvent",
                "actor": {"id": 11, "login": "followed-friend", "avatar_url": null},
                "repo": {"id": 99, "name": "acme/widgets", "url": "https://api.github.com/repos/acme/widgets"},
                "payload": {"ref_type": "branch", "ref": "feature"},
                "created_at": "2026-10-30T09:30:00Z"
            },
            {
                "id": "evt-create-repo",
                "type": "CreateEvent",
                "actor": {"id": 11, "login": "followed-friend", "avatar_url": null},
                "repo": {"id": 97, "name": "followed-friend/new-project", "url": "https://api.github.com/repos/followed-friend/new-project"},
                "payload": {"ref_type": "repository", "ref": null},
                "created_at": "2026-10-30T09:00:00Z"
            }
        ]),
    );
    // 2. Followed accounts: the contributor author, the starring account and the
    //    repository-creating account, but not the merging maintainer and not the
    //    unfollowed stargazer.
    transport.push(
        200,
        json!([
            {"id": 31, "login": "real-contributor", "avatar_url": null},
            {"id": 11, "login": "followed-friend", "avatar_url": null}
        ]),
    );
    // 3. The merged pull request's detail response supplies the author.
    transport.push(
        200,
        json!({
            "id": 424242u64,
            "number": 42,
            "state": "closed",
            "merged": true,
            "merged_at": "2026-10-30T12:00:41Z",
            "title": "Real merged contribution",
            "html_url": "https://github.com/acme/widgets/pull/42",
            "user": {"id": 31, "login": "real-contributor", "avatar_url": null}
        }),
    );

    let feed = feed_call(&session, &Default::default(), Default::default())
        .await
        .expect("feed")
        .value;

    let summary: Vec<(String, String, String)> = feed
        .activities
        .iter()
        .map(|activity| {
            (
                format!("{:?}", activity.kind),
                activity.subject.login.clone(),
                activity.event.id.clone(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "PullRequestMerged".to_string(),
                "real-contributor".to_string(),
                "evt-merged".to_string()
            ),
            (
                "RepositoryStarred".to_string(),
                "followed-friend".to_string(),
                "evt-star-followed".to_string()
            ),
            (
                "RepositoryCreated".to_string(),
                "followed-friend".to_string(),
                "evt-create-repo".to_string()
            ),
        ],
        "only followed subjects, in source order, crediting the PR author"
    );
    // The merged card carries the detail title and number.
    let merged = &feed.activities[0];
    assert_eq!(
        merged.subject.login, "real-contributor",
        "the author, not the merging maintainer"
    );
    assert_ne!(merged.subject.login, merged.event.actor.login);
    assert_eq!(
        merged
            .pull_request
            .as_ref()
            .and_then(|target| target.number),
        Some(42)
    );
    assert_eq!(
        merged
            .pull_request
            .as_ref()
            .and_then(|target| target.title.as_deref()),
        Some("Real merged contribution")
    );
    assert_eq!(feed.readiness, FeedReadiness::Complete);

    let requests = transport.requests.lock().unwrap();
    // login + events + following + one detail read. Five source events with one
    // merged PR must not fan out further.
    assert_eq!(
        requests.len(),
        4,
        "{:?}",
        requests.iter().map(|r| &r.url).collect::<Vec<_>>()
    );
    assert!(requests[1].url.contains("/users/viewer/received_events"));
    assert!(requests[2].url.contains("/user/following"));
    assert_eq!(
        requests[3].url, "https://api.github.com/repos/acme/widgets/pulls/42",
        "the detail URL is rebuilt from the parsed event, not taken from the payload"
    );
    for request in &requests[1..] {
        assert_eq!(auth(request), "Bearer viewer-token");
    }
}

/// A merged contribution whose author is followed but whose detail read fails must
/// be withheld and reported, never silently dropped and never credited to the
/// merger.
#[tokio::test]
async fn a_failed_author_read_withholds_the_contribution_and_reports_partial() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    transport.push(
        200,
        json!([{
            "id": "evt-merged",
            "type": "PullRequestEvent",
            "actor": {"id": 10, "login": "merging-maintainer", "avatar_url": null},
            "repo": {"id": 99, "name": "acme/widgets", "url": "https://api.github.com/repos/acme/widgets"},
            "payload": {
                "action": "merged",
                "number": 42,
                "pull_request": {
                    "url": "https://api.github.com/repos/acme/widgets/pulls/42",
                    "id": 424242u64, "number": 42,
                    "head": {"ref": "x", "sha": "a"}, "base": {"ref": "main", "sha": "b"}
                }
            },
            "created_at": "2026-10-30T12:00:41Z"
        }]),
    );
    transport.push(200, json!([{"id": 10, "login": "merging-maintainer"}]));
    transport.push_response(HttpResponse {
        status: 503,
        headers: vec![],
        body: "{}".to_string(),
    });

    let feed = feed_call(&session, &Default::default(), Default::default())
        .await
        .expect("feed returns a partial outcome")
        .value;
    assert!(feed.activities.is_empty());
    assert!(
        matches!(
            feed.readiness,
            FeedReadiness::Partial { ref reasons }
                if reasons.iter().any(|reason| matches!(
                    reason,
                    FeedPartialReason::AuthorUnresolved { count: 1 }
                ))
        ),
        "an unresolved author must be reported, got {:?}",
        feed.readiness
    );
}

/// A partial or failed followed-account read must never certify membership. Here
/// the second `/user/following` page fails, so the subject on it cannot be
/// confirmed even though the same login stars something.
#[tokio::test]
async fn incomplete_following_read_withholds_unconfirmed_subjects() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    transport.push(
        200,
        json!([{
            "id": "evt-star",
            "type": "WatchEvent",
            "actor": {"id": 12, "login": "second-page-friend", "avatar_url": null},
            "repo": {"id": 99, "name": "acme/widgets", "url": "https://api.github.com/repos/acme/widgets"},
            "payload": {"action": "started"},
            "created_at": "2026-10-30T11:00:00Z"
        }]),
    );
    transport.push_response(HttpResponse {
        status: 200,
        headers: vec![(
            "link".to_string(),
            r#"<https://api.github.com/user/following?per_page=100&page=2>; rel="next""#
                .to_string(),
        )],
        body: json!([{"id": 11, "login": "first-page-friend"}]).to_string(),
    });
    transport.push(503, json!({}));

    let feed = feed_call(&session, &Default::default(), Default::default())
        .await
        .expect("feed returns a partial outcome")
        .value;
    assert!(
        feed.activities.is_empty(),
        "an unconfirmed subject must not be shown: {:?}",
        feed.activities
    );
    assert!(matches!(
        feed.readiness,
        FeedReadiness::Partial { ref reasons }
            if reasons.iter().any(|reason| matches!(
                reason,
                FeedPartialReason::FollowingIncomplete { .. }
            ))
    ));
}

/// A failed `/user/following` first page is a hard error, never a complete empty
/// Feed: "could not read" and "follows nobody" are different answers.
#[tokio::test]
async fn a_failed_following_read_is_an_error_not_an_empty_feed() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    transport.push(200, json!([]));
    transport.push(401, json!({"message": "Bad credentials"}));

    let error = feed_call(&session, &Default::default(), Default::default())
        .await
        .expect_err("a failed relationship read must fail the Feed");
    assert!(matches!(
        error,
        HomeError::GitHub(GitHubError::Unauthorized)
    ));
}

/// Repeated refinement: two source events target the same merged pull request, so
/// exactly one detail read is made and both dedupe to one card identity source.
#[tokio::test]
async fn duplicate_merged_events_share_one_author_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    let merged = |id: &str| {
        json!({
            "id": id,
            "type": "PullRequestEvent",
            "actor": {"id": 10, "login": "maintainer", "avatar_url": null},
            "repo": {"id": 99, "name": "acme/widgets", "url": "https://api.github.com/repos/acme/widgets"},
            "payload": {
                "action": "merged",
                "number": 42,
                "pull_request": {
                    "url": "https://api.github.com/repos/acme/widgets/pulls/42",
                    "id": 424242u64, "number": 42,
                    "head": {"ref": "x", "sha": "a"}, "base": {"ref": "main", "sha": "b"}
                }
            },
            "created_at": "2026-10-30T12:00:41Z"
        })
    };
    transport.push(200, json!([merged("evt-1"), merged("evt-2")]));
    transport.push(200, json!([{"id": 31, "login": "real-contributor"}]));
    transport.push(
        200,
        json!({
            "id": 424242u64, "number": 42, "state": "closed", "merged": true,
            "title": "Real merged contribution",
            "user": {"id": 31, "login": "real-contributor"}
        }),
    );

    let feed = feed_call(&session, &Default::default(), Default::default())
        .await
        .expect("feed")
        .value;
    assert_eq!(feed.activities.len(), 2, "both events keep their identity");
    assert_eq!(
        transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.url.contains("/pulls/42"))
            .count(),
        1,
        "one detail read per distinct pull request"
    );
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
    let error = feed_call(&session, &Default::default(), Default::default())
        .await
        .unwrap_err();
    // No validator is sent, so an unexpected 304 is a genuine unexpected status,
    // not a NotModified replay; it is never a successful empty Feed.
    assert!(
        matches!(
            error,
            HomeError::GitHub(GitHubError::UnexpectedStatus { status: 304 })
        ),
        "{error:?}"
    );
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
    transport.push(200, root_tree(false));
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
            .ends_with(&format!("/repos/Target/Target/git/trees/{ROOT}"))
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
    for (tree, missing) in [
        (json!({"sha":ROOT,"truncated":false,"tree":[]}), true),
        (
            json!({"sha":ROOT,"truncated":false,"tree":[{"path":"file","type":"blob","mode":"100644","sha":BLOB}]}),
            true,
        ),
        (json!({"sha":ROOT,"truncated":true,"tree":[]}), false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let svc = service(&temp, transport.clone(), 2).await;
        login(&svc, &transport, 1, "viewer", "token").await;
        transport.push(200, details("Target", "public", Some("main")));
        transport.push(200, snapshot());
        transport.push(200, tree);
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
        assert_eq!(matches!(result, ProfileReadme::Missing { .. }), missing);
        assert_eq!(
            matches!(result, ProfileReadme::Unavailable { .. }),
            !missing
        );
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
                _ => root_tree(false),
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
        feed_call(&session, &Default::default(), Default::default())
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
        let task = tokio::spawn(async move {
            feed_call(&session, &Default::default(), Default::default()).await
        });
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
        let task =
            tokio::spawn(async move { feed_call(&session, &Default::default(), cancel).await });
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

#[tokio::test]
async fn repository_open_counts_reject_in_flight_success_and_error_after_account_aba() {
    for status in [200, 403] {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(Controlled::default());
        let accounts = service(&temp, transport.clone(), 4).await;
        login(&accounts, &transport, 1, "alice", "synthetic-a").await;
        let session = accounts.page_session().unwrap();
        let repo = identity();
        let (entered,release)=transport.block(status,json!({"data":{"repository":{"databaseId":42,"issues":{"totalCount":3},"pullRequests":{"totalCount":1}}}}));
        let pending = tokio::spawn(async move {
            session
                .repository_counts(&repo, CancellationToken::default())
                .await
        });
        ready(entered).await;
        login(&accounts, &transport, 2, "bob", "synthetic-b").await;
        accounts.set_active(key(1)).await.unwrap();
        release.send(()).unwrap();
        assert!(matches!(
            pending.await.unwrap(),
            Err(crate::domain::repository_counts::CountsError::Read(
                ProfileError::GitHub(GitHubError::StaleContext)
            ))
        ));
        accounts.close().await.unwrap();
    }
}
