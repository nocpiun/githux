//! Offline state-machine tests for the repository pull request list.

use super::*;
use crate::auth::{InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::domain::error::GitHubError;
use crate::domain::repository::{
    Completeness, DynamicRef, IncompleteReason, RepositoryIdentity, RepositoryLocator,
};
use crate::domain::repository_pulls::{
    PullFilter, PullListPage, PullListQuery, PullPageCursor, PullState, PullSummary,
};
use crate::domain::{AccountKey, HostId};
use crate::github::GitHubClient;
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{MockTransport, Transport, json_response};
use crate::storage::accounts::AccountDatabaseConfig;
use serde_json::json;
use std::sync::Arc;

fn identity(owner: &str, name: &str, id: u64) -> RepositoryIdentity {
    RepositoryIdentity {
        id,
        locator: RepositoryLocator::new(owner, name).unwrap(),
    }
}

fn summary_with(
    id: u64,
    number: u64,
    state: PullState,
    title: &str,
    author: Option<&str>,
    updated_at: &str,
) -> PullSummary {
    PullSummary {
        id,
        number,
        title: title.into(),
        state,
        author: author.map(str::to_owned),
        head: DynamicRef::new("feature/x").unwrap(),
        base: DynamicRef::new("main").unwrap(),
        created_at: "2026-11-01T00:00:00Z".into(),
        updated_at: updated_at.into(),
    }
}

fn summary(id: u64, number: u64, state: PullState) -> PullSummary {
    summary_with(
        id,
        number,
        state,
        &format!("Change {number}"),
        Some("octocat"),
        "2026-11-20T00:00:00Z",
    )
}

fn page(
    query: &PullListQuery,
    page: u32,
    pulls: Vec<PullSummary>,
    next: Option<u32>,
) -> PullListPage {
    PullListPage {
        query: query.clone(),
        page,
        pulls,
        next: next.map(|next| PullPageCursor::new(1024, query.clone(), next, 30)),
        completeness: Completeness::Complete,
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    transport: Arc<MockTransport>,
    accounts: Arc<AccountService>,
}

impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let transport = Arc::new(MockTransport::new());
        let accounts = Arc::new(
            AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(
                    Default::default(),
                    transport.clone() as Arc<dyn Transport>,
                )),
                Arc::new(BackgroundExecutor::new(2)),
            )
            .await
            .unwrap(),
        );
        let fixture = Self {
            _temp: temp,
            transport,
            accounts,
        };
        fixture.login(1, "account-a").await;
        fixture
    }

    async fn login(&self, id: u64, login: &str) {
        self.transport
            .enqueue(Ok(json_response(
                200,
                &json!({"id": id, "login": login}).to_string(),
            )))
            .await;
        self.accounts
            .login(Pat::from_str("synthetic-offline-token"))
            .await
            .unwrap();
    }

    fn model(&self, owner: &str, name: &str, filter: PullFilter) -> PullListModel {
        self.model_for(identity(owner, name, 1024), filter)
    }

    fn model_for(&self, identity: RepositoryIdentity, filter: PullFilter) -> PullListModel {
        let session = self
            .accounts
            .repo_service()
            .pull_list_session(identity)
            .unwrap();
        PullListModel::new(session, filter)
    }
}

fn account(id: u64) -> AccountKey {
    AccountKey::new(id, HostId::github())
}

#[tokio::test]
async fn open_and_closed_lists_keep_draft_merged_and_closed_without_merge_distinct() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    let request = model.begin(PullRequestMode::Replace).expect("initial read");
    assert_eq!(request.filter(), PullFilter::Open);
    assert_eq!(
        model.finish(
            &request,
            Ok(page(
                &model.query(),
                1,
                vec![
                    summary(1, 11, PullState::Open),
                    summary(2, 12, PullState::Draft),
                ],
                Some(2),
            )),
        ),
        PullFinish::Applied
    );
    assert_eq!(model.rows().len(), 2);
    assert!(model.next().is_some());
    assert!(model.loaded());

    let request = model
        .select_filter(PullFilter::Closed)
        .expect("a different filter reloads");
    assert_eq!(model.rows().len(), 0);
    assert_eq!(model.filter(), PullFilter::Closed);
    assert_eq!(
        model.finish(
            &request,
            Ok(page(
                &model.query(),
                1,
                vec![
                    summary(3, 13, PullState::Merged),
                    summary(4, 14, PullState::Closed),
                ],
                None,
            )),
        ),
        PullFinish::Applied
    );
    assert_eq!(
        model
            .rows()
            .iter()
            .map(|pull| pull.state)
            .collect::<Vec<_>>(),
        vec![PullState::Merged, PullState::Closed]
    );
    assert!(model.next().is_none());
    assert!(model.select_filter(PullFilter::Closed).is_none());
}

#[tokio::test]
async fn a_request_executes_through_the_account_bound_session_and_publishes() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    fixture
        .transport
        .enqueue(Ok(json_response(
            200,
            &json!([{
                "id": 7,
                "number": 7,
                "title": "Change 7",
                "state": "open",
                "draft": false,
                "merged_at": null,
                "user": {"login": "octocat"},
                "head": {"ref": "feature/x"},
                "base": {"ref": "main"},
                "created_at": "2026-11-01T00:00:00Z",
                "updated_at": "2026-11-20T00:00:00Z"
            }])
            .to_string(),
        )))
        .await;
    let request = model.begin(PullRequestMode::Replace).unwrap();
    let result = request.execute().await;
    assert!(result.is_ok());
    assert_eq!(model.finish(&request, result), PullFinish::Applied);
    assert_eq!(model.rows().len(), 1);
    assert_eq!(model.rows()[0].number, 7);
    let recorded = fixture.transport.recorded_requests().await;
    let url = reqwest::Url::parse(&recorded.last().unwrap().url).unwrap();
    assert_eq!(url.path(), "/repos/octo/repo/pulls");
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned()),
        Some("open".to_string())
    );
}

#[tokio::test]
async fn an_append_failure_keeps_the_loaded_pages_and_retries_the_same_cursor() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    let request = model.begin(PullRequestMode::Replace).unwrap();
    model.finish(
        &request,
        Ok(page(
            &model.query(),
            1,
            vec![
                summary(1, 11, PullState::Open),
                summary(2, 12, PullState::Open),
            ],
            Some(2),
        )),
    );
    let request = model.begin(PullRequestMode::Append).expect("next page");
    assert_eq!(request.cursor().map(PullPageCursor::page), Some(2));
    assert_eq!(
        model.finish(
            &request,
            Err(PullListError::Read(RepositoryError::GitHub(
                GitHubError::Network {
                    message: "offline".into(),
                },
            ))),
        ),
        PullFinish::Applied
    );
    assert_eq!(model.rows().len(), 2);
    assert!(model.error().is_some());
    assert_eq!(model.failed_mode(), Some(PullRequestMode::Append));
    assert_eq!(model.next().map(PullPageCursor::page), Some(2));

    let retry = model.retry().expect("retry the failed page");
    assert_eq!(retry.mode(), PullRequestMode::Append);
    assert_eq!(retry.cursor().map(PullPageCursor::page), Some(2));
    assert_eq!(
        model.finish(
            &retry,
            Ok(page(
                &model.query(),
                2,
                vec![summary(3, 13, PullState::Open)],
                None,
            )),
        ),
        PullFinish::Applied
    );
    assert_eq!(model.rows().len(), 3);
    assert!(model.error().is_none());
    assert!(model.next().is_none());
}

#[tokio::test]
async fn append_requires_a_cursor_and_a_replacement_supersedes_it() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    assert!(model.begin(PullRequestMode::Append).is_none());

    let first = model.begin(PullRequestMode::Replace).unwrap();
    assert!(model.begin(PullRequestMode::Replace).is_none());
    assert!(model.begin(PullRequestMode::Append).is_none());
    model.finish(
        &first,
        Ok(page(
            &model.query(),
            1,
            vec![summary(1, 11, PullState::Open)],
            Some(2),
        )),
    );
    let append = model.begin(PullRequestMode::Append).unwrap();
    let replacement = model
        .begin(PullRequestMode::Replace)
        .expect("refresh may supersede an append");
    assert_eq!(
        model.finish(&append, Ok(page(&model.query(), 2, vec![], None)),),
        PullFinish::Rejected
    );
    assert_eq!(model.rows().len(), 1);
    assert_eq!(model.busy(), Some(PullRequestMode::Replace));
    assert_eq!(
        model.finish(
            &replacement,
            Ok(page(
                &model.query(),
                1,
                vec![summary(2, 12, PullState::Open)],
                None,
            )),
        ),
        PullFinish::Applied
    );
    assert_eq!(model.rows().len(), 1);
    assert_eq!(model.rows()[0].number, 12);
}

#[tokio::test]
async fn an_empty_page_is_success_and_a_later_failure_is_not_emptiness() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Closed);
    assert!(model.retry().is_none());
    let request = model.begin(PullRequestMode::Replace).unwrap();
    model.finish(&request, Ok(page(&model.query(), 1, vec![], None)));
    assert!(model.loaded());
    assert!(model.is_empty_result());
    assert!(!model.is_partial());
    assert!(model.error().is_none());

    // An SSO-partial empty page is not evidence that the repository has no
    // pull requests, so it must not claim the real empty state.
    let request = model.begin(PullRequestMode::Replace).unwrap();
    let mut partial = page(&model.query(), 1, vec![], None);
    partial.completeness = Completeness::Partial {
        reasons: vec![IncompleteReason::SsoPartial {
            organizations: vec!["acme".into()],
        }],
    };
    model.finish(&request, Ok(partial));
    assert!(!model.is_empty_result());
    assert!(model.is_partial());
    assert!(model.error().is_none());

    // A transient failure after a confirmed empty page is still not "empty".
    let request = model.begin(PullRequestMode::Replace).unwrap();
    model.finish(
        &request,
        Err(PullListError::Read(RepositoryError::GitHub(
            GitHubError::Network {
                message: "offline".into(),
            },
        ))),
    );
    assert!(!model.is_empty_result());
    assert!(model.error().is_some());
    assert!(model.loaded());
    assert_eq!(model.rows().len(), 0);
}

#[tokio::test]
async fn stale_account_responses_never_publish_rows() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    let request = model.begin(PullRequestMode::Replace).unwrap();
    // A → B → A: the selection epoch changes, so the request is still stale.
    fixture.login(2, "account-b").await;
    fixture.accounts.set_active(account(1)).await.unwrap();
    assert!(!model.verify());
    assert_eq!(
        model.finish(
            &request,
            Ok(page(
                &model.query(),
                1,
                vec![summary(1, 11, PullState::Open)],
                None,
            )),
        ),
        PullFinish::Rejected
    );
    assert!(model.rows().is_empty());
    assert_eq!(model.busy(), None);

    // A fresh model for the new authorization never accepts the old account's reply.
    let mut rebound = fixture.model("octo", "repo", PullFilter::Open);
    let request = rebound.begin(PullRequestMode::Replace).unwrap();
    fixture.login(3, "account-c").await;
    assert_eq!(
        rebound.finish(
            &request,
            Err(PullListError::Read(RepositoryError::GitHub(
                GitHubError::Unauthorized,
            ))),
        ),
        PullFinish::Rejected
    );
    assert!(rebound.error().is_none());
}

#[tokio::test]
async fn returning_from_a_detail_keeps_filter_pages_and_scroll() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Closed);
    let request = model.begin(PullRequestMode::Replace).unwrap();
    model.finish(
        &request,
        Ok(page(
            &model.query(),
            1,
            vec![
                summary(1, 11, PullState::Merged),
                summary(2, 12, PullState::Closed),
            ],
            Some(2),
        )),
    );
    let request = model.begin(PullRequestMode::Append).unwrap();
    model.finish(
        &request,
        Ok(page(
            &model.query(),
            2,
            vec![summary(3, 13, PullState::Closed)],
            None,
        )),
    );
    let anchor = PullScrollAnchor {
        pull_id: 12,
        offset: 48.5,
    };
    model.remember_scroll(anchor);

    assert_eq!(model.filter(), PullFilter::Closed);
    assert_eq!(model.rows().len(), 3);
    assert_eq!(model.scroll(), Some(anchor));
    assert!(model.next().is_none());
    assert!(model.loaded());
    assert!(model.busy().is_none());
}

#[tokio::test]
async fn cross_repository_rows_and_foreign_pages_are_rejected() {
    let fixture = Fixture::new().await;
    let mut a = fixture.model_for(identity("octo", "a", 41), PullFilter::Open);
    let mut b = fixture.model_for(identity("octo", "b", 42), PullFilter::Open);
    assert_ne!(a.identity().id(), b.identity().id());

    let request = a.begin(PullRequestMode::Replace).unwrap();
    let foreign = page(&b.query(), 1, vec![summary(7, 7, PullState::Open)], None);
    assert_eq!(a.finish(&request, Ok(foreign)), PullFinish::Rejected);
    assert!(a.rows().is_empty());

    let request = a.begin(PullRequestMode::Replace).unwrap();
    a.finish(
        &request,
        Ok(page(
            &a.query(),
            1,
            vec![summary(7, 7, PullState::Open)],
            None,
        )),
    );
    let request = b.begin(PullRequestMode::Replace).unwrap();
    b.finish(
        &request,
        Ok(page(
            &b.query(),
            1,
            vec![summary(8, 7, PullState::Merged)],
            None,
        )),
    );
    // Same visible number, different repository and state: the anchors and rows
    // never move between the two models.
    assert_eq!(a.rows()[0].number, b.rows()[0].number);
    assert_eq!(a.rows()[0].state, PullState::Open);
    assert_eq!(b.rows()[0].state, PullState::Merged);
    assert_eq!(a.rows()[0].id, 7);
    assert_eq!(b.rows()[0].id, 8);
}

#[tokio::test]
async fn a_foreign_filter_payload_is_rejected() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    let request = model.begin(PullRequestMode::Replace).unwrap();
    let closed_query = PullListQuery::new(
        RepositoryLocator::new("octo", "repo").unwrap(),
        PullFilter::Closed,
    );
    assert_eq!(
        model.finish(&request, Ok(page(&closed_query, 1, vec![], None))),
        PullFinish::Rejected
    );
    assert!(model.rows().is_empty());
    assert!(model.error().is_none());
    assert_eq!(model.busy(), None);
}

#[tokio::test]
async fn dropping_the_model_aborts_its_in_flight_read() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    let request = model.begin(PullRequestMode::Replace).unwrap();
    assert!(!request.is_aborted());
    // A closed repository tab drops the model without finishing the read. The shared request
    // slot must be released instead of waiting for the transport timeout.
    drop(model);
    assert!(request.is_aborted());
}

#[tokio::test]
async fn a_rejected_credential_clears_rows_while_a_network_failure_keeps_them() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    let request = model.begin(PullRequestMode::Replace).unwrap();
    model.finish(
        &request,
        Ok(page(
            &model.query(),
            1,
            vec![
                summary(30, 30, PullState::Open),
                summary(29, 29, PullState::Open),
            ],
            Some(2),
        )),
    );
    // A transient failure keeps the loaded page and only retries the continuation.
    let request = model.begin(PullRequestMode::Append).unwrap();
    model.finish(
        &request,
        Err(PullListError::Read(RepositoryError::GitHub(
            GitHubError::Network {
                message: "offline".into(),
            },
        ))),
    );
    assert_eq!(model.rows().len(), 2);
    assert_eq!(model.failed_mode(), Some(PullRequestMode::Append));
    assert!(model.next().is_some());

    // A refused credential is not transient: the rows are authorization-bound and must go.
    let request = model.retry().unwrap();
    assert_eq!(
        model.finish(
            &request,
            Err(PullListError::Read(RepositoryError::GitHub(
                GitHubError::Unauthorized,
            ))),
        ),
        PullFinish::Applied
    );
    assert!(model.rows().is_empty(), "rejected rows are cleared");
    assert!(model.next().is_none());
    assert!(!model.loaded());
    assert!(model.error().is_some());
    assert_eq!(model.failed_mode(), Some(PullRequestMode::Replace));
    // The filter is a view intent and stays so the tab reloads the same state.
    assert_eq!(model.filter(), PullFilter::Open);
}

#[tokio::test]
async fn a_later_complete_page_keeps_an_earlier_partial_warning_until_a_replacement() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    let request = model.begin(PullRequestMode::Replace).unwrap();
    let mut first = page(
        &model.query(),
        1,
        vec![summary(1, 11, PullState::Open)],
        Some(2),
    );
    first.completeness = Completeness::Partial {
        reasons: vec![IncompleteReason::SsoPartial {
            organizations: vec!["acme".into()],
        }],
    };
    assert_eq!(model.finish(&request, Ok(first)), PullFinish::Applied);
    assert!(model.is_partial());

    // A later page that is itself complete does not re-fetch the earlier rows, so the warning
    // must survive the append.
    let request = model.begin(PullRequestMode::Append).unwrap();
    assert_eq!(
        model.finish(
            &request,
            Ok(page(
                &model.query(),
                2,
                vec![summary(2, 12, PullState::Open)],
                None,
            )),
        ),
        PullFinish::Applied
    );
    assert!(model.is_partial(), "the earlier partial warning survives");
    assert_eq!(model.rows().len(), 2);

    // A replacement read the whole list again and clears it.
    let request = model.begin(PullRequestMode::Replace).unwrap();
    assert_eq!(
        model.finish(
            &request,
            Ok(page(
                &model.query(),
                1,
                vec![summary(1, 11, PullState::Open)],
                None,
            )),
        ),
        PullFinish::Applied
    );
    assert!(!model.is_partial());
}

#[tokio::test]
async fn rows_that_shift_between_pages_are_deduped_by_id() {
    let fixture = Fixture::new().await;
    let mut model = fixture.model("octo", "repo", PullFilter::Open);
    let request = model.begin(PullRequestMode::Replace).unwrap();
    model.finish(
        &request,
        Ok(page(
            &model.query(),
            1,
            vec![
                summary(30, 30, PullState::Open),
                summary(29, 29, PullState::Open),
            ],
            Some(2),
        )),
    );
    let request = model.begin(PullRequestMode::Append).unwrap();
    model.finish(
        &request,
        Ok(page(
            &model.query(),
            2,
            vec![
                summary(29, 29, PullState::Merged),
                summary(28, 28, PullState::Open),
            ],
            None,
        )),
    );
    let ids: Vec<_> = model.rows().iter().map(|pull| pull.id).collect();
    assert_eq!(ids, vec![30, 29, 28]);
    // The duplicate keeps its position while the newer page refreshes it.
    assert_eq!(model.rows()[1].state, PullState::Merged);
}

#[test]
fn row_display_fields_cover_state_author_title_and_update_time() {
    let draft = summary_with(1, 11, PullState::Draft, "", None, "1970-01-01T00:01:00Z");
    let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60);
    let row = draft.row(now);
    assert_eq!(row.title, "Untitled pull request");
    assert_eq!(row.state_label, "Draft");
    assert_eq!(row.author, None);
    assert_eq!(row.head, "feature/x");
    assert_eq!(row.base, "main");
    assert_eq!(row.updated, "Just now");
    assert_eq!(row.updated_tooltip, "1970-01-01T00:01:00Z");

    let merged = summary_with(
        2,
        12,
        PullState::Merged,
        "Merged change",
        Some("octocat"),
        "bad",
    );
    let row = merged.row(now);
    assert_eq!(row.title, "Merged change");
    assert_eq!(row.state_label, "Merged");
    assert_eq!(row.author.as_deref(), Some("octocat"));
    assert_eq!(row.updated, "Time unavailable");
}

fn cached_reader(fixture: &Fixture) -> (tempfile::TempDir, CachedPageReadSession) {
    let directory = tempfile::tempdir().unwrap();
    let store = crate::storage::cache::CacheStore::open(crate::storage::cache::CacheConfig::at(
        directory.path().to_path_buf(),
    ))
    .unwrap();
    let session = fixture.accounts.page_session().unwrap();
    let scope = store
        .saved_account(session.context().auth().account.clone())
        .unwrap();
    (
        directory,
        CachedPageReadSession::new(session, store, scope).unwrap(),
    )
}

#[tokio::test]
async fn a_stored_first_page_paints_before_the_network_and_the_network_replaces_it() {
    let fixture = Fixture::new().await;
    let (_directory, cached) = cached_reader(&fixture);
    let mut model = fixture
        .model("octo", "repo", PullFilter::Open)
        .with_cache(Some(cached));
    let request = model.begin(PullRequestMode::Replace).unwrap();
    let cached_page = page(
        &model.query(),
        1,
        vec![
            summary(1, 11, PullState::Open),
            summary(2, 12, PullState::Draft),
        ],
        Some(2),
    );
    assert!(model.apply_cached(&request, vec![cached_page], None));
    assert!(model.is_historical());
    assert!(model.loaded());
    // The stored prefix stops where it was stored; a live continuation is never appended to
    // it. The list is marked partial whenever the stored prefix is not the whole list.
    assert!(model.is_partial());
    assert!(!model.is_empty_result());
    assert!(!model.is_cached_empty());
    assert_eq!(model.failed_mode(), None);
    assert!(model.begin(PullRequestMode::Append).is_none());

    // The network replacement owns the list in place: same filter, fresh rows, no marker.
    assert_eq!(
        model.finish(
            &request,
            Ok(page(
                &model.query(),
                1,
                vec![summary(3, 13, PullState::Open)],
                None,
            )),
        ),
        PullFinish::Applied
    );
    assert!(!model.is_historical());
    assert!(!model.is_partial());
    assert_eq!(model.rows().len(), 1);
    assert_eq!(model.rows()[0].number, 13);
}

#[tokio::test]
async fn a_late_cached_paint_cannot_overwrite_a_network_answer() {
    let fixture = Fixture::new().await;
    let (_directory, cached) = cached_reader(&fixture);
    let mut model = fixture
        .model("octo", "repo", PullFilter::Open)
        .with_cache(Some(cached));
    let request = model.begin(PullRequestMode::Replace).unwrap();
    model.finish(
        &request,
        Ok(page(
            &model.query(),
            1,
            vec![summary(5, 15, PullState::Open)],
            None,
        )),
    );
    assert!(!model.is_historical());
    // The network answer owns the list; the stored prefix arriving later is ignored.
    assert!(!model.apply_cached(
        &request,
        vec![page(
            &model.query(),
            1,
            vec![summary(1, 11, PullState::Open)],
            Some(2),
        )],
        None,
    ));
    assert_eq!(model.rows().len(), 1);
    assert_eq!(model.rows()[0].number, 15);
    assert!(!model.is_historical());
}

#[tokio::test]
async fn a_stored_prefix_survives_a_transient_failure_and_a_refused_credential_clears_it() {
    let fixture = Fixture::new().await;
    let (_directory, cached) = cached_reader(&fixture);
    let mut model = fixture
        .model("octo", "repo", PullFilter::Open)
        .with_cache(Some(cached));
    let request = model.begin(PullRequestMode::Replace).unwrap();
    assert!(model.apply_cached(
        &request,
        vec![page(
            &model.query(),
            1,
            vec![summary(1, 11, PullState::Open)],
            Some(2),
        )],
        None,
    ));
    // A transient failure keeps the bounded prefix and offers the first page again.
    assert_eq!(
        model.finish(
            &request,
            Err(PullListError::Read(RepositoryError::GitHub(
                GitHubError::Network {
                    message: "offline".into(),
                },
            ))),
        ),
        PullFinish::Applied
    );
    assert_eq!(model.rows().len(), 1);
    assert!(model.is_historical());
    assert!(model.is_partial());
    assert_eq!(model.failed_mode(), Some(PullRequestMode::Replace));
    // Retrying the replacement keeps the cached rows until the answer arrives.
    let retry = model.retry().unwrap();
    assert_eq!(model.rows().len(), 1);
    assert!(!model.apply_cached(
        &retry,
        vec![page(
            &model.query(),
            1,
            vec![summary(3, 13, PullState::Open)],
            None,
        )],
        None,
    ));
    // A refused credential is not transient: the authorization-bound prefix must go.
    assert_eq!(
        model.finish(
            &retry,
            Err(PullListError::Read(RepositoryError::GitHub(
                GitHubError::Unauthorized,
            ))),
        ),
        PullFinish::Applied
    );
    assert!(model.rows().is_empty());
    assert!(!model.is_historical());
    assert!(!model.is_partial());
    assert!(!model.loaded());
    assert!(!model.is_empty_result());
    assert!(!model.is_cached_empty());
    assert_eq!(model.failed_mode(), Some(PullRequestMode::Replace));
}

#[tokio::test]
async fn a_stored_empty_page_is_historical_not_a_confirmed_empty_result() {
    let fixture = Fixture::new().await;
    let (_directory, cached) = cached_reader(&fixture);
    let mut model = fixture
        .model("octo", "repo", PullFilter::Closed)
        .with_cache(Some(cached));
    let request = model.begin(PullRequestMode::Replace).unwrap();
    // A page from another filter is never applied to this list.
    let mut foreign = page(
        &model.query(),
        1,
        vec![summary(1, 11, PullState::Open)],
        None,
    );
    foreign.query = PullListQuery::new(
        RepositoryLocator::new("octo", "repo").unwrap(),
        PullFilter::Open,
    );
    assert!(!model.apply_cached(&request, vec![foreign], None));
    assert!(model.apply_cached(&request, vec![page(&model.query(), 1, vec![], None)], None));
    assert!(model.is_cached_empty());
    assert!(!model.is_empty_result());
    // A bounded stored empty page has no continuation; it is only a historical statement.
    assert!(!model.is_partial());
    // A stored paint is only admitted by the replacement that issued its restore, while the
    // list is still empty. A refresh of already stored rows keeps them until the answer
    // arrives, so a late replay of the old request is also refused.
    model.cancel();
    let fresh = model.begin(PullRequestMode::Replace).unwrap();
    assert!(!model.apply_cached(
        &request,
        vec![page(
            &model.query(),
            1,
            vec![summary(2, 12, PullState::Open)],
            None
        )],
        None,
    ));
    assert!(!model.apply_cached(
        &fresh,
        vec![page(
            &model.query(),
            1,
            vec![summary(2, 12, PullState::Open)],
            None
        )],
        None,
    ));
    assert!(model.is_cached_empty());
}

#[tokio::test]
async fn a_verified_replacement_installs_its_own_generation_and_a_stored_paint_keeps_the_old_one() {
    let fixture = Fixture::new().await;
    let (_directory, cached) = cached_reader(&fixture);
    let mut model = fixture
        .model("octo", "repo", PullFilter::Open)
        .with_cache(Some(cached));
    // A replacement starts a generation; only a successful answer may install it.
    let request = model.begin(PullRequestMode::Replace).unwrap();
    let replacement = request.collection().expect("a persisted first page");
    assert_eq!(model.collection(), None);
    model.finish(
        &request,
        Ok(page(
            &model.query(),
            1,
            vec![summary(1, 11, PullState::Open)],
            Some(2),
        )),
    );
    assert_eq!(model.collection(), Some(replacement));
    // The next append belongs to that verified first page, not to some older epoch.
    let append = model.begin(PullRequestMode::Append).unwrap();
    assert_eq!(append.collection(), Some(replacement));
    model.cancel();

    // A stored paint carries the generation it was stored under, and the replacement's newer
    // generation never relabels it.
    let mut model = fixture
        .model("octo", "repo", PullFilter::Open)
        .with_cache(Some(cached_reader(&fixture).1));
    let request = model.begin(PullRequestMode::Replace).unwrap();
    let stored = crate::data::page_cache::new_collection_generation();
    assert_ne!(Some(stored), request.collection());
    assert!(model.apply_cached(
        &request,
        vec![page(
            &model.query(),
            1,
            vec![summary(2, 12, PullState::Open)],
            Some(2)
        )],
        Some(stored),
    ));
    assert_eq!(model.collection(), Some(stored));
    // Paging is not offered from a stored prefix; a replacement must re-read the first page.
    assert!(model.begin(PullRequestMode::Append).is_none());
}

#[tokio::test]
async fn a_duplicate_id_restored_from_two_pages_keeps_its_first_position_and_the_later_fields() {
    let fixture = Fixture::new().await;
    let (_directory, cached) = cached_reader(&fixture);
    let mut model = fixture
        .model("octo", "repo", PullFilter::Open)
        .with_cache(Some(cached));
    let request = model.begin(PullRequestMode::Replace).unwrap();
    // The same pull request id appears on page one and again on page two, where it has moved to
    // merged. An online append keeps the first position and takes the newer fields; a stored
    // restore must fold the same way instead of keeping the stale row.
    let first = page(
        &model.query(),
        1,
        vec![
            summary_with(
                7,
                11,
                PullState::Open,
                "Still open",
                Some("octocat"),
                "2026-11-01T00:00:00Z",
            ),
            summary_with(
                8,
                12,
                PullState::Open,
                "Second row",
                Some("octocat"),
                "2026-11-01T00:00:00Z",
            ),
        ],
        Some(2),
    );
    let second = page(
        &model.query(),
        2,
        vec![summary_with(
            7,
            11,
            PullState::Merged,
            "Merged now",
            Some("reviewer"),
            "2026-11-20T00:00:00Z",
        )],
        None,
    );
    assert!(model.apply_cached(&request, vec![first, second], None));
    let rows = model.rows();
    assert_eq!(rows.len(), 2, "the duplicate id is folded, not added twice");
    assert_eq!(rows[0].id, 7);
    assert_eq!(rows[0].number, 11, "the first position is kept");
    assert_eq!(
        rows[0].state,
        PullState::Merged,
        "the later page's fields win"
    );
    assert_eq!(rows[0].title, "Merged now");
    assert_eq!(rows[0].author.as_deref(), Some("reviewer"));
    assert_eq!(rows[1].number, 12);

    // The online append honours exactly the same fold.
    let mut online = fixture.model("octo", "repo", PullFilter::Open);
    let request = online.begin(PullRequestMode::Replace).unwrap();
    online.finish(
        &request,
        Ok(page(
            &online.query(),
            1,
            vec![
                summary_with(
                    7,
                    11,
                    PullState::Open,
                    "Still open",
                    Some("octocat"),
                    "2026-11-01T00:00:00Z",
                ),
                summary_with(
                    8,
                    12,
                    PullState::Open,
                    "Second row",
                    Some("octocat"),
                    "2026-11-01T00:00:00Z",
                ),
            ],
            Some(2),
        )),
    );
    let append = online.begin(PullRequestMode::Append).unwrap();
    online.finish(
        &append,
        Ok(page(
            &online.query(),
            2,
            vec![summary_with(
                7,
                11,
                PullState::Merged,
                "Merged now",
                Some("reviewer"),
                "2026-11-20T00:00:00Z",
            )],
            None,
        )),
    );
    assert_eq!(online.rows()[0].id, rows[0].id);
    assert_eq!(online.rows()[0].state, rows[0].state);
    assert_eq!(online.rows()[0].title, rows[0].title);
    assert_eq!(online.rows()[0].author, rows[0].author);
}
