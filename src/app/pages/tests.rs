//! Integration coverage for the real product Shell mounting the real page hosts.
//!
//! The account controller, Shell, page components and online read services are the production
//! ones. Only external boundaries are injected: a temporary SQLite directory, an in-memory
//! credential store and a deterministic transport.

#[path = "pr_files_integration_tests.rs"]
mod pr_files_integration_tests;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{AppContext, Entity, TestAppContext, VisualContext, VisualTestContext};

use crate::app::AccountsHandle;
use crate::app::accounts::Accounts;
use crate::app::accounts::runtime::AccountRuntime;
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::domain::{AccountKey, HostId};
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::ui::Shell;
use crate::workspace::Route;

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        crate::platform::fonts::register(cx).expect("test fonts");
        Shell::bind_keys(cx);
    });
}

fn json(status: u16, body: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: body.to_string(),
    }
}

const PR_FIXTURE_BODY: &str = "## [Conversation fixture](docs/design.md#decisions)\n\nStream the index without buffering the whole file.\n\n![Index diagram](index.svg)";

fn graphql_ok(data: serde_json::Value) -> HttpResponse {
    json(200, serde_json::json!({"data": data}))
}

/// The Shell/account tests route every request by path and body. This is not a one-shot queue:
/// the same Home, repository or profile endpoint can be read again in A→B→A scenarios.
#[derive(Default)]
struct PageTransport {
    reads: Mutex<Vec<String>>,
    writes: Mutex<Vec<String>>,
    starred: Mutex<bool>,
    long_pr_body: bool,
    pr_comments_status: std::sync::atomic::AtomicU16,
}

impl PageTransport {
    fn seen(&self, needle: &str) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.contains(needle))
            .count()
    }

    fn wrote(&self, needle: &str) -> usize {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.contains(needle))
            .count()
    }
}

impl Transport for PageTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: crate::network::cancel::CancellationToken,
        max_body_bytes: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max_body_bytes {
                return Err(TransportError::ResponseTooLarge {
                    limit: max_body_bytes,
                });
            }
            Ok(response)
        })
    }

    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: crate::network::cancel::CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.reads.lock().unwrap().push(request.url.clone());

            let url = request.url.as_str();
            if url.ends_with("/user/starred/alice/demo") {
                return match request.method {
                    crate::network::transport::HttpMethod::Get => Ok(json(
                        if *self.starred.lock().unwrap() {
                            204
                        } else {
                            404
                        },
                        serde_json::json!({}),
                    )),
                    crate::network::transport::HttpMethod::Put => {
                        *self.starred.lock().unwrap() = true;
                        Ok(json(204, serde_json::json!({})))
                    }
                    crate::network::transport::HttpMethod::Delete => {
                        *self.starred.lock().unwrap() = false;
                        Ok(json(204, serde_json::json!({})))
                    }
                    _ => Ok(json(599, serde_json::json!({}))),
                };
            }
            let body = request.body.clone().unwrap_or_default();
            let lower = url.to_ascii_lowercase();
            if url.ends_with("/user") {
                return Ok(json(200, serde_json::json!({"id": 1, "login": "alice"})));
            }
            if url.contains("/user/repos") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 10, "name": "demo", "full_name": "alice/demo",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "description": "Synthetic repository",
                        "default_branch": "main"
                    }, {
                        "id": 11, "name": "other", "full_name": "alice/other",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "default_branch": "main"
                    }]),
                ));
            }
            // Notifications: a two-thread unread inbox over the account-scoped endpoint. Done and
            // Unsubscribe keep their real REST semantics (204 for Done, ignored=true for the
            // subscription step).
            // Notification subjects are opaque server data: the fixture uses the official REST
            // shape with https://github.com/... URLs, which must stay non-navigating, plus the
            // matched API shapes for issue/PR rows the product is allowed to open.
            if url.starts_with("https://api.github.com/notifications") {
                match request.method {
                    crate::network::transport::HttpMethod::Delete => {
                        self.writes.lock().unwrap().push(format!("DELETE {url}"));
                        return Ok(HttpResponse {
                            status: 204,
                            headers: vec![("x-poll-interval".into(), "1".into())],
                            body: String::new(),
                        });
                    }
                    crate::network::transport::HttpMethod::Put => {
                        self.writes.lock().unwrap().push(format!("PUT {url}"));
                        return Ok(json(200, serde_json::json!({ "ignored": true })));
                    }
                    crate::network::transport::HttpMethod::Get => {}
                    _ => return Ok(json(599, serde_json::json!({}))),
                }
                return Ok(HttpResponse {
                    status: 200,
                    headers: vec![
                        ("x-poll-interval".into(), "1".into()),
                        (
                            "last-modified".into(),
                            "Thu, 01 Nov 2026 12:00:00 GMT".into(),
                        ),
                    ],
                    body: serde_json::json!([
                        {
                            "id": "9001", "unread": true, "reason": "subscribed",
                            "updated_at": "2026-11-03T10:00:00Z", "last_read_at": null,
                            "subject": {
                                "title": "Fix the login flow", "type": "Issue",
                                "url": "https://api.github.com/repos/alice/demo/issues/42",
                                "latest_comment_url": null
                            },
                            "repository": {
                                "id": 10, "name": "demo",
                                "owner": {"login": "alice"},
                                "url": "https://api.github.com/repos/alice/demo"
                            }
                        },
                        {
                            "id": "9002", "unread": true, "reason": "review_requested",
                            "updated_at": "2026-11-03T09:00:00Z", "last_read_at": null,
                            "subject": {
                                "title": "Update the pull request", "type": "PullRequest",
                                "url": "https://api.github.com/repos/alice/demo/pulls/42",
                                "latest_comment_url": null
                            },
                            "repository": {
                                "id": 10, "name": "demo",
                                "owner": {"login": "alice"},
                                "url": "https://api.github.com/repos/alice/demo"
                            }
                        },
                        {
                            "id": "9003", "unread": true, "reason": "mention",
                            "updated_at": "2026-11-03T08:00:00Z", "last_read_at": null,
                            "subject": {
                                "title": "Other repository issue with the same number", "type": "Issue",
                                "url": "https://api.github.com/repos/alice/other/issues/42",
                                "latest_comment_url": null
                            },
                            "repository": {
                                "id": 11, "name": "other",
                                "owner": {"login": "alice"},
                                "url": "https://api.github.com/repos/alice/other"
                            }
                        },
                        {
                            "id": "9004", "unread": true, "reason": "subscribed",
                            "updated_at": "2026-11-03T07:00:00Z", "last_read_at": null,
                            "subject": {
                                "title": "Opaque HTML subject URL", "type": "Issue",
                                "url": "https://github.com/alice/demo/issues/43",
                                "latest_comment_url": null
                            },
                            "repository": {
                                "id": 10, "name": "demo",
                                "owner": {"login": "alice"},
                                "url": "https://api.github.com/repos/alice/demo"
                            }
                        }
                    ])
                    .to_string(),
                });
            }
            if url.ends_with("/repos/alice/demo/pulls/5/files?per_page=30&page=1") {
                return Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: include_str!("../../github/pr_files/fixtures/changed_files.json")
                        .to_owned(),
                });
            }
            if url.ends_with("/repos/alice/demo/pulls/21/files?per_page=30&page=1") {
                return Ok(json(200, serde_json::json!([])));
            }
            if url.contains("/repos/alice/demo/issues/5/comments") {
                let status = self
                    .pr_comments_status
                    .load(std::sync::atomic::Ordering::Relaxed);
                if status != 0 {
                    return Ok(json(status, serde_json::json!({})));
                }
                let second = url.contains("&page=2");
                let mut page = json(
                    200,
                    serde_json::json!([{
                        "id": if second { 502 } else { 501 }, "user": {"id": 2, "login": "bob"},
                        "body": if second { "Second comment **page**" } else { "First comment **page**\n\n![Index diagram](index.svg)" },
                        "created_at": "2026-10-28T11:00:00Z", "updated_at": "2026-10-28T11:00:00Z"
                    }]),
                );
                if !second {
                    page.headers.push(("link".into(), "<https://api.github.com/repos/alice/demo/issues/5/comments?per_page=30&page=2>; rel=\"next\"".into()));
                }
                return Ok(page);
            }
            if url.contains("/repos/alice/demo/issues/7/comments") {
                return Ok(json(200, serde_json::json!([])));
            }
            if let Ok(repo_url) = reqwest::Url::parse(url)
                && repo_url.path() == "/repos/alice/demo/issues"
            {
                let page = repo_url
                    .query_pairs()
                    .find(|(key, _)| key == "page")
                    .map(|(_, value)| value.into_owned());
                let row = |number| {
                    serde_json::json!({
                        "number": number,"title":format!("Issue #{number}"),"state":"open","updated_at":"2026-10-28T11:00:00Z"
                    })
                };
                let (rows, next) = match page.as_deref() {
                    Some("1") => ((30..60).rev().map(row).collect::<Vec<_>>(), Some(2)),
                    Some("2") => (
                        vec![serde_json::json!({"number":29,"pull_request":{}})],
                        Some(3),
                    ),
                    Some("3") => (vec![row(21)], None),
                    _ => return Ok(json(599, serde_json::json!({}))),
                };
                let mut response = json(200, serde_json::json!(rows));
                if let Some(next) = next {
                    response.headers.push(("link".into(),format!("<https://api.github.com/repos/alice/demo/issues?state=open&sort=created&direction=desc&per_page=30&page={next}>; rel=\"next\"")));
                }
                return Ok(response);
            }
            if url.contains("/repos/alice/demo/issues/42/comments") {
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 401,
                        "user": { "id": 2, "login": "bob", "avatar_url": null },
                        "body": "Notification issue comment body.",
                        "created_at": "2026-10-28T11:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z",
                        "author_association": "CONTRIBUTOR"
                    }]),
                ));
            }
            if url.ends_with("/repos/alice/demo/issues/42") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10042,
                        "number": 42,
                        "title": "Notification issue forty two",
                        "state": "open",
                        "user": { "id": 1, "login": "alice", "avatar_url": null },
                        "body": "Issue body reached from a notification click.",
                        "comments": 1,
                        "labels": [{ "name": "bug" }],
                        "created_at": "2026-10-28T10:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z"
                    }),
                ));
            }
            if url.contains("/repos/alice/other/issues/42/comments") {
                return Ok(json(200, serde_json::json!([])));
            }
            if url.ends_with("/repos/alice/other/issues/42") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10043,
                        "number": 42,
                        "title": "Other repository issue forty two",
                        "state": "open",
                        "user": { "id": 1, "login": "alice", "avatar_url": null },
                        "body": "Same number, different repository.",
                        "comments": 0,
                        "labels": [],
                        "created_at": "2026-10-28T09:00:00Z",
                        "updated_at": "2026-10-28T09:30:00Z"
                    }),
                ));
            }
            if url.contains("/repos/alice/demo/issues/21/comments") {
                let page = reqwest::Url::parse(url)
                    .ok()
                    .and_then(|parsed| {
                        parsed
                            .query_pairs()
                            .find(|(k, _)| k == "page")
                            .and_then(|(_, v)| v.parse::<u32>().ok())
                    })
                    .unwrap_or(1);
                if page == 1 {
                    return Ok(HttpResponse {
                        status: 200,
                        headers: vec![
                            ("content-type".into(), "application/json".into()),
                            (
                                "link".into(),
                                r#"<https://api.github.com/repos/alice/demo/issues/21/comments?per_page=30&page=2>; rel="next""#.into(),
                            ),
                        ],
                        body: serde_json::json!([{
                            "id": 301,
                            "user": { "id": 2, "login": "bob", "avatar_url": null },
                            "body": "Looks good to me!",
                            "created_at": "2026-10-28T11:00:00Z",
                            "updated_at": "2026-10-28T11:00:00Z",
                            "author_association": "CONTRIBUTOR"
                        }])
                        .to_string(),
                    });
                }
                return Ok(json(
                    200,
                    serde_json::json!([{
                        "id": 302,
                        "user": { "id": 3, "login": "carol", "avatar_url": null },
                        "body": "Second page comment verified!",
                        "created_at": "2026-10-28T12:00:00Z",
                        "updated_at": "2026-10-28T12:00:00Z",
                        "author_association": "MEMBER"
                    }]),
                ));
            }
            if url.ends_with("/repos/alice/demo/issues/404") {
                return Ok(json(404, serde_json::json!({ "message": "Not Found" })));
            }
            if url.ends_with("/repos/alice/demo/issues/403") {
                return Ok(json(403, serde_json::json!({ "message": "Forbidden" })));
            }
            if url.ends_with("/repos/alice/demo/issues/21") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10001,
                        "number": 21,
                        "title": "Synthetic issue",
                        "state": "open",
                        "user": { "id": 1, "login": "alice", "avatar_url": null },
                        "body": "Detailed **Markdown** description for demo #21.",
                        "comments": 1,
                        "labels": [{ "name": "bug" }],
                        "created_at": "2026-10-28T10:00:00Z",
                        "updated_at": "2026-10-28T11:00:00Z"
                    }),
                ));
            }
            if url.contains("/repos/alice/other/issues/21/comments") {
                return Ok(json(200, serde_json::json!([])));
            }
            if url.ends_with("/repos/alice/other/issues/21") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10002,
                        "number": 21,
                        "title": "Other repository issue #21",
                        "state": "closed",
                        "state_reason": "completed",
                        "user": { "id": 1, "login": "alice", "avatar_url": null },
                        "body": null,
                        "comments": 0,
                        "labels": [],
                        "created_at": "2026-10-28T09:00:00Z",
                        "updated_at": "2026-10-28T09:30:00Z"
                    }),
                ));
            }
            if url.contains("/search/issues") {
                // Global work lists use `is:issue`/`is:pr`; Home uses `involves:`. Decode the
                // query so the two product surfaces never share a response shape.
                let query = reqwest::Url::parse(url)
                    .ok()
                    .and_then(|parsed| {
                        parsed
                            .query_pairs()
                            .find(|(key, _)| key == "q")
                            .map(|(_, value)| value.into_owned())
                    })
                    .unwrap_or_default();
                if query.starts_with("is:issue") || query.starts_with("is:pr") {
                    let pr = query.starts_with("is:pr");
                    let item = if pr {
                        serde_json::json!({
                            "id": 20001, "number": 7, "title": "Synthetic pull request",
                            "state": "open", "updated_at": "2026-10-28T10:00:00Z",
                            "repository_url": "https://api.github.com/repos/alice/demo",
                            "pull_request": {"merged_at": null}
                        })
                    } else {
                        serde_json::json!({
                            "id": 10001, "number": 21, "title": "Synthetic issue",
                            "state": "open", "updated_at": "2026-10-28T10:00:00Z",
                            "repository_url": "https://api.github.com/repos/alice/demo",
                            "pull_request": null
                        })
                    };
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "total_count": 1, "incomplete_results": false, "items": [item]
                        }),
                    ));
                }
                // One open item and one closed item: the confirmed Home rule must
                // return only the open one even if a non-conforming page leaks a
                // closed row.
                return Ok(json(
                    200,
                    serde_json::json!({
                        "total_count": 2, "incomplete_results": false,
                        "items": [{
                            "id": 900, "number": 12, "title": "Synthetic involved issue",
                            "html_url": "https://github.com/alice/demo/issues/12",
                            "state": "open", "updated_at": "2026-10-28T10:00:00Z",
                            "repository_url": "https://api.github.com/repos/alice/demo",
                            "pull_request": null
                        }, {
                            "id": 901, "number": 13, "title": "Synthetic closed issue",
                            "html_url": "https://github.com/alice/demo/issues/13",
                            "state": "closed", "updated_at": "2026-10-28T09:00:00Z",
                            "repository_url": "https://api.github.com/repos/alice/demo",
                            "pull_request": null
                        }]
                    }),
                ));
            }
            if lower.contains("/users/catol") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 22, "login": "carol", "name": "Carol", "avatar_url": null,
                        "bio": "Another user", "followers": 2, "following": 1
                    }),
                ));
            }
            // The active account follows bob and carol, not dave.
            if url.contains("/user/following") {
                return Ok(json(
                    200,
                    serde_json::json!([
                        {"id": 7, "login": "bob", "avatar_url": null},
                        {"id": 8, "login": "carol", "avatar_url": null}
                    ]),
                ));
            }
            // Real detail-endpoint shape for the merged contribution: the author
            // lives here, not in the slim event payload.
            if lower.contains("/repos/alice/demo/pulls/5") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 555, "number": 5, "state": "closed", "merged": true,
                        "title": "Synthetic merged pull request",
                        "html_url": "https://github.com/alice/demo/pull/5",
                        "user": {"id": 8, "login": "carol", "avatar_url": null},
                        "body": if self.long_pr_body {
                            "A detailed pull request description that exceeds the Feed preview limit. ".repeat(8)
                        } else {
                            "Fixes critical issue with bounded memory allocation.".into()
                        }
                    }),
                ));
            }
            if url.contains("/received_events") {
                // Live shapes: a star by a followed account, a push (never a card),
                // a star by an unfollowed account, and a live `action=merged` event
                // whose slim pull_request carries no author. Page 2 serves one more
                // followed star so the append path can be exercised.
                let page_two = url.contains("page=2");
                let events = if page_two {
                    serde_json::json!([{
                        "id": "evt-5", "type": "WatchEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T06:00:00Z"
                    }])
                } else {
                    serde_json::json!([{
                        "id": "evt-1", "type": "WatchEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T09:00:00Z"
                    }, {
                        "id": "evt-2", "type": "PushEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"ref": "refs/heads/main", "before": "a", "head": "b", "size": 1},
                        "created_at": "2026-10-28T08:00:00Z"
                    }, {
                        "id": "evt-3", "type": "WatchEvent",
                        "actor": {"id": 9, "login": "dave", "avatar_url": null},
                        "repo": {"id": 11, "name": "alice/other", "url": "https://api.github.com/repos/alice/other"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T07:30:00Z"
                    }, {
                        "id": "evt-4", "type": "PullRequestEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {
                            "action": "merged", "number": 5,
                            "pull_request": {
                                "url": "https://api.github.com/repos/alice/demo/pulls/5",
                                "id": 555, "number": 5,
                                "head": {"ref": "feature", "sha": "aaaa"},
                                "base": {"ref": "main", "sha": "bbbb"}
                            }
                        },
                        "created_at": "2026-10-28T07:00:00Z"
                    }])
                };
                let mut headers = vec![("x-poll-interval".into(), "1".into())];
                if !page_two {
                    headers.push((
                        "link".into(),
                        "<https://api.github.com/users/alice/received_events?per_page=100&page=2>; rel=\"next\""
                            .to_string(),
                    ));
                }
                return Ok(HttpResponse {
                    status: 200,
                    headers,
                    body: events.to_string(),
                });
            }
            if url.ends_with("/graphql") {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(
                    request.body.as_deref().unwrap_or_default(),
                ) && payload["query"]
                    .as_str()
                    .is_some_and(|query| query.starts_with("query PRConversation"))
                {
                    let vars = &payload["variables"];
                    let number = vars["number"].as_u64().unwrap();
                    // #42 is the notification-driven PR fixture; its body differs from the
                    // Issue #42 body so the test proves the PR conversation itself was read.
                    let body = match number {
                        5 => PR_FIXTURE_BODY,
                        21 => "PR **description** for demo #21.",
                        42 => "Pull request body reached from a notification click.",
                        _ => "",
                    };
                    return Ok(graphql_ok(serde_json::json!({"repository": {
                        "nameWithOwner": format!("{}/{}", vars["owner"].as_str().unwrap(), vars["name"].as_str().unwrap()),
                        "pullRequest": {"number": number, "title": "Stream the index",
                            "state": if number == 5 { "MERGED" } else { "OPEN" }, "isDraft": number == 7,
                            "author": {"login": "bob"}, "body": body,
                            "baseRefName": "main", "headRefName": "feature/stream", "headRef": null,
                            "headRepository": {"nameWithOwner": "bob/demo"}}
                    }})));
                }
                // Home/Profile tests can distinguish the profile query from repository counts by
                // the fixed variables/fields in the body.
                if body.contains("Pinned") || body.contains("itemShowcase") {
                    return Ok(graphql_ok(serde_json::json!({
                        "user": {
                            "pinnedItems": {"totalCount": 0, "pageInfo": {"hasNextPage": false}, "nodes": []},
                            "itemShowcase": {"hasPinnedItems": false, "items": {"totalCount": 0, "pageInfo": {"hasNextPage": false}, "nodes": []}},
                            "contributionsCollection": null
                        }
                    })));
                }
                if body.contains("contributionsCollection") {
                    return Ok(graphql_ok(serde_json::json!({
                        "user": {"contributionsCollection": {"contributionCalendar": {
                            "totalContributions": 3,
                            "weeks": [{"contributionDays": [{"date": "2026-01-01", "contributionCount": 3, "contributionLevel": "FIRST_QUARTILE", "color": "#123456"}]}],
                            "months": []
                        }}}
                    })));
                }
                return Ok(graphql_ok(serde_json::json!({
                    "repository": {"databaseId": 10, "issues": {"totalCount": 4}, "pullRequests": {"totalCount": 2}}
                })));
            }
            if lower.contains("/users/carol") {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 22, "login": "carol", "name": "Carol", "avatar_url": null,
                        "bio": "Another user", "followers": 2, "following": 1
                    }),
                ));
            }
            if self.long_pr_body {
                const COMMIT: &str = "2222222222222222222222222222222222222222";
                const TREE: &str = "3333333333333333333333333333333333333333";
                const BLOB: &str = "5c7e77fdc11e53409b877f790a77782f75d92ea9";
                if url.ends_with("/repos/alice/demo/commits/main")
                    || url.ends_with(&format!("/repos/alice/demo/commits/{COMMIT}"))
                {
                    return Ok(json(
                        200,
                        serde_json::json!({"sha": COMMIT, "commit": {"message": "Index diagram fixture", "tree": {"sha": TREE}}}),
                    ));
                }
                if url.ends_with(&format!("/repos/alice/demo/git/trees/{TREE}")) {
                    return Ok(json(
                        200,
                        serde_json::json!({"sha": TREE, "truncated": false, "tree": [
                            {"path": "index.svg", "type": "blob", "mode": "100644", "size": 111, "sha": BLOB}
                        ]}),
                    ));
                }
                if url.ends_with(&format!("/repos/alice/demo/git/blobs/{BLOB}")) {
                    return Ok(json(
                        200,
                        serde_json::json!({"sha": BLOB, "size": 111, "encoding": "base64", "content": "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIzMiIgaGVpZ2h0PSIyNCI+PHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjI0IiBmaWxsPSJibHVlIi8+PC9zdmc+"}),
                    ));
                }
            }
            if url == "https://api.github.com/repos/alice/demo" {
                return Ok(json(
                    200,
                    serde_json::json!({
                        "id": 10, "name": "demo", "full_name": "alice/demo",
                        "owner": {"id": 1, "login": "alice", "avatar_url": null},
                        "visibility": "public", "private": false, "default_branch": "main",
                        "fork": false, "archived": false,
                        "stargazers_count": if *self.starred.lock().unwrap() { 1 } else { 0 },
                        "forks_count": 0, "open_issues_count": 0, "size": 1,
                        "description": "Synthetic repository", "html_url": "https://github.com/alice/demo"
                    }),
                ));
            }
            Ok(json(599, serde_json::json!({})))
        })
    }
}

fn open_shell(
    cx: &mut TestAppContext,
    transport: Arc<PageTransport>,
) -> (Entity<Shell>, Entity<Accounts>, &mut VisualTestContext) {
    setup(cx);
    let temp = tempfile::tempdir().unwrap();
    let executor = Arc::new(BackgroundExecutor::new(4));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let accounts = runtime
        .block_on(async {
            AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    transport as Arc<dyn Transport>,
                )),
                executor.clone(),
            )
            .await
        })
        .expect("open account service");
    // Keep the temporary directory alive for the window's lifetime.
    std::mem::forget(temp);
    let runtime = AccountRuntime::new(accounts, executor);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let controller = cx.new(|cx| Accounts::new(Some(Arc::new(runtime)), window, cx));
        cx.set_global(AccountsHandle(controller));
        Shell::new(window, cx)
    });
    let controller = cx.read(AccountsHandle::get).expect("installed controller");
    (view, controller, cx)
}

fn pump(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    controller.update(cx, |accounts, cx| accounts.drain_for_test(cx));
    cx.run_until_parked();
}

fn wait_until(
    controller: &Entity<Accounts>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&Accounts) -> bool,
) {
    for _ in 0..600 {
        pump(controller, cx);
        std::thread::sleep(Duration::from_millis(2));
        if controller.read_with(cx, |accounts, _| done(accounts)) {
            return;
        }
    }
    panic!("page host did not settle");
}

fn login(controller: &Entity<Accounts>, cx: &mut VisualTestContext) {
    let input = controller.read_with(cx, |accounts, _| accounts.token_input().clone());
    input.update(cx, |input, cx| input.set_text("synthetic-page-token", cx));
    cx.update_window_entity(controller, |accounts, window, cx| {
        accounts.submit(window, cx)
    });
    wait_until(controller, cx, |accounts| {
        accounts.snapshot().accounts.len() == 1
    });
}

#[gpui::test]
fn home_refresh_and_navigation_actions_do_not_reenter_the_host(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)) {
            break;
        }
    }
    assert!(view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)));
    // A real refresh action goes HomeView -> HomePage through a deferred dispatch. It must not
    // panic, and it must start exactly one new request for that column.
    let before = transport.seen("/search/issues");
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.refresh_home_for_test(crate::ui::home::Column::Home, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(transport.seen("/search/issues"), before + 1);
}

#[gpui::test]
fn repository_contributor_navigation_is_deferred_and_opens_a_profile(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    // Emit the exact typed action a real contributor control produces while the repository host
    // is on the update stack. It must be dispatched after that update, then reach Shell's shared
    // open-or-activate path without panicking.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate_contributor_for_test("carol", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::UserProfile {
            host: HostId::github(),
            login: "carol".to_string(),
        }
    );
}

#[gpui::test]
fn production_shell_composes_real_star_and_keeps_fork_disabled(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, cx| shell
            .active_repository_mutations_enabled(cx)),
        Some(true),
        "the formal Shell injects the real Star service"
    );
    for _ in 0..400 {
        pump(&controller, cx);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if transport.seen("/user/starred/alice/demo") > 0
            && view.read_with(cx, |shell, cx| {
                shell.active_repository_star_ready_for_test(cx) == Some(true)
            })
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // The request reaching transport is earlier than the page's light result
    // drain. Let the initial status publish before pressing the actual button.
    for _ in 0..20 {
        pump(&controller, cx);
        cx.update_window_entity(&view, |shell, window, cx| {
            shell.drain_repository_star_for_test(window, cx)
        });
        cx.update(|window, _| window.refresh());
        std::thread::sleep(Duration::from_millis(5));
    }
    let star = cx
        .debug_bounds("repository-star")
        .expect("formal Star button");
    cx.simulate_click(star.center(), gpui::Modifiers::none());
    for _ in 0..400 {
        pump(&controller, cx);
        cx.update_window_entity(&view, |shell, window, cx| {
            shell.drain_repository_star_for_test(window, cx)
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if transport.seen("/user/starred/alice/demo") >= 2
            && transport.seen("/repos/alice/demo") >= 2
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        *transport.starred.lock().unwrap(),
        "PUT reached the local server"
    );
    assert!(
        transport.seen("/repos/alice/demo") >= 2,
        "the formal page refreshed its authoritative summary"
    );
    let fork = cx.debug_bounds("repository-fork").expect("Fork button");
    cx.simulate_click(fork.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(transport.seen("/forks"), 0, "Fork remains disabled");
}

#[gpui::test]
fn own_contributor_link_activates_the_existing_profile_tab(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    // Open the signed-in user's own profile from the sidebar first.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Profile, window, cx);
    });
    cx.run_until_parked();
    let own = view.read_with(cx, |shell, _| shell.active_tab_id());
    let tabs = view.read_with(cx, |shell, _| shell.tab_count());
    // Then follow the same user's contributor link from a repository page.
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate_contributor_for_test("alice", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Profile,
        "the signed-in user's contributor link resolves to the own-profile route"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        own,
        "it activates the already-open profile tab"
    );
    assert_eq!(
        view.read_with(cx, |shell, _| shell.tab_count()),
        tabs + 1,
        "only the repository tab was added; no second profile page"
    );
}

#[gpui::test]
fn feed_refresh_is_reenabled_when_its_server_poll_interval_expires(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)) {
            break;
        }
    }
    // The synthetic Feed response requests a one-second poll interval. Refresh stays disabled
    // while that interval runs, then the host restores the control without issuing a read.
    assert!(!view.read_with(cx, |shell, cx| shell.feed_refresh_available_for_test(cx)));
    // The first page also exposes a next page, and the short feed is underfilled, so the
    // legitimate end-of-scroll append settles once before we measure the cooldown.
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if transport.seen("/received_events") >= 2 {
            break;
        }
    }
    let feed_reads = transport.seen("/received_events");
    // Drive the same expiry boundary the real timer reaches, without depending on the test
    // scheduler's wall clock or introducing a real sleep.
    view.update(cx, |shell, cx| shell.expire_feed_cooldown_for_test(cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |shell, cx| shell.feed_refresh_available_for_test(cx)));
    assert_eq!(
        transport.seen("/received_events"),
        feed_reads,
        "cooldown expiry only restores refresh; it must not poll"
    );
}

#[gpui::test]
fn the_shell_mounts_real_home_and_reads_its_two_columns(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    let _ = (&view, &controller);
    // Home/Feed use the real page-data service under the settled account. A synthetic transport
    // answers both; mounting uses the next frame, so pump until the real Home view is present.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if cx.debug_bounds("home-view").is_some() {
            break;
        }
        if let Some(page_unavailable) = cx.debug_bounds("page-service-unavailable") {
            panic!(
                "Shell kept the fallback page after login: {page_unavailable:?}; route={:?}",
                view.read_with(cx, |shell, _| shell.route())
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let dumped = controller.read_with(cx, |accounts, _| {
        (
            accounts.services().is_some(),
            accounts.snapshot().active.clone(),
            format!("{:?}", accounts.snapshot().repositories.preview),
        )
    });
    if cx.debug_bounds("home-view").is_none() {
        panic!("home not mounted: {dumped:?}");
    }
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if transport.seen("/search/issues") >= 1
            && transport.seen("/received_events") >= 1
            && view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if transport.seen("/search/issues") == 0 && transport.seen("/search/issues?") == 0 {
        panic!(
            "no search read; reads={:?} requests={:?}",
            transport.reads.lock().unwrap(),
            view.read_with(cx, |shell, cx| shell.home_debug_reads(cx))
        );
    }
    assert!(
        transport.seen("/search/issues") >= 1 || transport.seen("/search/issues?") >= 1,
        "Home requests its account-scoped involves query"
    );
    assert!(
        transport.seen("/received_events") >= 1,
        "Feed requests the received-events endpoint"
    );
    assert!(
        transport.seen("/user/following") >= 1,
        "Feed reads the followed-account set for the membership constraint"
    );
    assert!(view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)));
    // Through the real adapters: Home keeps only the open item; Feed keeps the
    // followed star and the merged contribution (author resolved from the detail
    // endpoint), dropping the push and the unfollowed account's star.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("home-item-row-0").is_some(),
        "the open Home item is visible"
    );
    assert!(
        cx.debug_bounds("home-item-row-1").is_none(),
        "the closed Home item must not have a row"
    );
    assert!(
        cx.debug_bounds("feed-event-row-0").is_some(),
        "the followed star is visible"
    );
    assert!(
        cx.debug_bounds("feed-event-row-1").is_some(),
        "the merged contribution is visible"
    );
    assert!(
        cx.debug_bounds("feed-pr-body-1").is_some(),
        "the merged contribution renders its extracted body preview"
    );
    assert!(
        cx.debug_bounds("feed-event-row-2").is_none(),
        "the push event and the unfollowed star must not have rows"
    );
    // Drain enrichment for the starred repository description.
    for _ in 0..100 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if cx.debug_bounds("feed-repo-description-0").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        cx.debug_bounds("feed-repo-description-0").is_some(),
        "the starred repository displays its enriched description"
    );
}

#[gpui::test]
fn sidebar_repository_opens_the_real_repository_page_and_keeps_it_on_activation(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let repository = view.read_with(cx, |shell, _| shell.repositories()[0].clone());
    assert!(controller.read_with(cx, |accounts, _| accounts.services().is_some()));
    let before = transport.seen("/repos/alice/demo") + transport.seen("/branches");
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&repository), window, cx);
    });
    cx.run_until_parked();
    // A real repository page was mounted for the route: the tab and page exist even when the
    // synthetic transport only models the summary and not every git endpoint.
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::repository(&repository)
    );
    assert!(cx.debug_bounds("repository-page").is_some());
    let _ = before;
}

#[gpui::test]
fn a_profile_route_canonicalizes_only_after_server_identity_arrives(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(
            Route::UserProfile {
                host: HostId::github(),
                login: "CATOL".to_string(),
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    // The route spelling is only a request target; the assertion below proves no stable numeric
    // ProfileTarget was invented before the server answered.
    assert_eq!(
        cx.update(|_, cx| { view.read_with(cx, |shell, cx| shell.profile_target_for_test(cx)) }),
        None
    );
    // The route keeps the requested (unverified) spelling; only a server response may
    // canonicalize it. No hash/login/0 is treated as a stable numeric ProfileTarget.
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::UserProfile {
            host: HostId::github(),
            login: "CATOL".to_string()
        }
    );

    // Drive the real Profile host: the synthetic transport identifies id 22 and canonical login
    // `carol`, so pins/contributions/README can then be requested for that real target.
    for _ in 0..400 {
        view.update(cx, |shell, cx| {
            shell.mount_active_page_for_test_windowless(cx)
        });
        cx.update_window_entity(&view, |shell, window, cx| {
            shell.drain_profile_for_test(window, cx)
        });
        cx.run_until_parked();
        if transport.seen("/users/") > 0 {
            cx.update_window_entity(&view, |shell, window, cx| {
                shell.drain_profile_for_test(window, cx)
            });
        }
        if view.read_with(cx, |shell, _| shell.route())
            == (Route::UserProfile {
                host: HostId::github(),
                login: "carol".to_string(),
            })
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(30));
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.drain_profile_for_test(window, cx)
    });
    assert!(transport.seen("/users/") >= 1);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::UserProfile {
            host: HostId::github(),
            login: "carol".to_string()
        },
        "canonical server login replaces the requested spelling"
    );
}

#[gpui::test]
fn closing_one_repository_tab_does_not_stop_or_clear_the_other(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    let first = view.read_with(cx, |shell, _| shell.active_tab_id());
    view.update(cx, |shell, cx| {
        shell.select_repository(first, "main", Some("README.md"));
        cx.notify();
    });
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[1]), window, cx);
    });
    cx.run_until_parked();
    let second = view.read_with(cx, |shell, _| shell.active_tab_id());
    view.update(cx, |shell, cx| {
        shell.select_repository(second, "release", Some("src/main.rs"));
        cx.notify();
    });
    assert_ne!(first, second);

    // Activation keeps each mounted page entity; it does not rebuild content for ordinary tab
    // switches. Only the active page owns current interaction state.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.close_tab(first, window, cx)
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |shell, _| shell.tab_ids().contains(&first)));
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), second);
    assert_eq!(
        view.read_with(cx, |shell, _| shell
            .repository_state(second)
            .and_then(|state| state.file.clone())),
        Some("src/main.rs".to_string()),
        "closing a different tab leaves this page's navigation intact"
    );
}

#[gpui::test]
fn same_resource_activation_and_reorder_keep_one_page_instance(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    let id = view.read_with(cx, |shell, _| shell.active_tab_id());
    view.update(cx, |shell, cx| {
        shell.select_repository(id, "main", Some("README.md"));
        cx.notify();
    });
    let before = view.read_with(cx, |shell, _| shell.repository_state(id).cloned());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), id);
    view.update(cx, |shell, cx| {
        assert!(shell.workspace().reorder(id, 0));
        cx.notify();
    });
    assert_eq!(view.read_with(cx, |shell, _| shell.active_tab_id()), id);
    assert_eq!(
        view.read_with(cx, |shell, _| shell.repository_state(id).cloned()),
        before
    );
}

#[gpui::test]
fn authorization_change_drops_page_hosts_without_reusing_their_old_context(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    let rows = view.read_with(cx, |shell, _| shell.repositories().to_vec());
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::repository(&rows[0]), window, cx);
    });
    cx.run_until_parked();
    let tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert!(view.read_with(cx, |shell, cx| {
        shell.mounted_repository_context_for_test(tab, cx).is_some()
    }));
    // A real revalidation advances the account epoch even though the same account stays active.
    controller.update(cx, |accounts, cx| {
        accounts.revalidate(AccountKey::new(1, HostId::github()), cx);
    });
    wait_until(&controller, cx, |accounts| {
        !accounts.snapshot().accounts.is_empty()
    });
    for _ in 0..100 {
        pump(&controller, cx);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if view.read_with(cx, |shell, cx| {
            shell.mounted_repository_context_for_test(tab, cx).is_none()
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        view.read_with(cx, |shell, cx| shell
            .mounted_repository_context_for_test(tab, cx)
            .is_none()),
        "the old authorization-bound page host must be dropped"
    );
}

#[gpui::test]
fn compact_retry_state_is_readable_and_retries_once(cx: &mut TestAppContext) {
    use crate::ui::compact_status;

    cx.update(ely_core::init_for_tests);
    let (view, cx) = cx.add_window_view(|_window, _cx| {
        use gpui::{App, Context, IntoElement, Render, Window, div, prelude::*};
        struct Probe {
            retries: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let retries = self.retries.clone();
                let failed = compact_status::failed(
                    "probe-failed",
                    "Failed to load docs",
                    "rate limited; try again later",
                    Some(move |_: &mut Window, _: &mut App| retries.set(retries.get() + 1)),
                    None,
                    cx,
                );
                let loading = compact_status::loading("probe-loading", "Loading docs…", cx);
                let empty = compact_status::empty("No files", cx);
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(failed)
                    .child(loading)
                    .child(empty)
            }
        }
        Probe {
            retries: std::rc::Rc::new(std::cell::Cell::new(0)),
        }
    });
    cx.run_until_parked();
    let failed = cx
        .debug_bounds("compact-failed-Failed to load docs")
        .unwrap();
    let loading = cx.debug_bounds("compact-loading-Loading docs…").unwrap();
    assert!(failed.size.height > gpui::px(0.0));
    assert!(loading.size.height > gpui::px(0.0));
    assert!(cx.debug_bounds("compact-empty-No files").is_some());
    // One real click on the warning mark is one retry. The test entity owns the count; the
    // production pages route the same callback through their existing single-resource retry.
    cx.simulate_click(failed.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |probe, _| probe.retries.get()),
        1,
        "a warning mark carries exactly one retry"
    );
}

#[gpui::test]
fn feed_end_of_scroll_appends_the_next_page_through_the_real_shell_host(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    cx.run_until_parked();
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)) {
            break;
        }
    }
    assert!(view.read_with(cx, |shell, cx| shell.home_is_ready_for_test(cx)));
    // The first page produced the followed star and the merged contribution; the
    // push and the unfollowed star did not. The first page also exposes page 2.
    // A short first page automatically continues through the real host; no
    // synthetic LoadMore action or additional user scroll should be required.
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
        if cx.debug_bounds("feed-event-row-2").is_some() {
            break;
        }
    }
    assert!(
        transport.seen("/received_events?per_page=100&page=2")
            + transport.seen("/received_events?page=2")
            >= 1,
        "append fetched the second source page, not a fresh first page"
    );
    assert_eq!(
        transport.seen("/received_events"),
        2,
        "one first page and exactly one automatic continuation"
    );
    // The appended star's row is visible after the append; the first-page rows
    // remain, so the list grew in place rather than resetting.
    assert!(cx.debug_bounds("feed-event-row-0").is_some());
    assert!(cx.debug_bounds("feed-event-row-1").is_some());
    assert!(cx.debug_bounds("feed-event-row-2").is_some());
    // Page 2 exposed no next link, so no further append fires.
    let after = transport.seen("/received_events");
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.load_more_home_for_test(window, cx)
    });
    cx.run_until_parked();
    view.update(cx, |shell, cx| shell.drain_home_for_test(cx));
    assert_eq!(
        transport.seen("/received_events"),
        after,
        "true exhaustion must not issue another read"
    );
}

#[gpui::test]
fn feed_read_more_opens_and_reuses_the_internal_pr_tab(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport {
        long_pr_body: true,
        ..Default::default()
    });
    let (shell, controller, cx) = open_shell(cx, transport.clone());
    cx.simulate_resize(gpui::size(gpui::px(1400.0), gpui::px(1100.0)));
    login(&controller, cx);
    wait_until(&controller, cx, |accounts| {
        matches!(&accounts.snapshot().repositories.preview,
        crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2)
    });
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.home_mount_for_test(window, cx)
    });
    let mut first_tab = None;
    for _ in 0..2 {
        for _ in 0..400 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            shell.update(cx, |shell, cx| shell.drain_home_for_test(cx));
            if cx.debug_bounds("feed-pr-read-more-1").is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let target = cx
            .debug_bounds("feed-pr-read-more-1")
            .expect("long PR preview has Read more");
        cx.simulate_click(target.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.route()),
            Route::PullRequest {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 5,
            }
        );
        wait_for_bounds(cx, "pr-conversation-body");
        wait_for_bounds(cx, "markdown-heading-0");
        wait_for_bounds(cx, "pr-comment-501");
        shell.read_with(cx, |shell, cx| {
            let pr = shell
                .pull_request_summary_for_test(cx)
                .expect("mounted PR summary");
            assert_eq!(pr.body.as_deref(), Some(PR_FIXTURE_BODY));
            assert_eq!(pr.state, crate::domain::pr_conversation::State::Merged);
            assert_eq!(
                (&*pr.base_branch, &*pr.head_branch),
                ("main", "feature/stream")
            );
            assert_eq!(
                pr.source_branch,
                crate::domain::pr_conversation::SourceBranch::Deleted {
                    repository: "bob/demo".into()
                }
            );
        });
        let active = shell.update(cx, |shell, _| {
            assert_eq!(
                shell.workspace().sessions().len(),
                2,
                "Home plus one PR tab"
            );
            shell.workspace().active_id()
        });
        if let Some(first) = first_tab {
            assert_eq!(active, first);
        } else {
            first_tab = Some(active);
        }
        cx.update_window_entity(&shell, |shell, window, cx| {
            shell.navigate(Route::Home, window, cx)
        });
        cx.run_until_parked();
    }
    assert_eq!(transport.seen("/repos/alice/demo/issues/5/comments"), 1);
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.activate_tab_for_test(first_tab.unwrap(), window, cx);
    });
    wait_for_bounds(cx, "markdown-picture-frame-2");
    wait_for_bounds(cx, "markdown-picture-frame-1");
    let heading = cx.debug_bounds("markdown-heading-0").unwrap();
    cx.simulate_click(
        gpui::point(heading.left() + gpui::px(8.0), heading.center().y),
        gpui::Modifiers::none(),
    );
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/alice/demo/blob/HEAD/docs/design.md#decisions")
    );
    wait_for_bounds(cx, "pr-comments-load-more");
    let more = cx.debug_bounds("pr-comments-load-more").unwrap();
    cx.simulate_click(
        gpui::point(more.left() + gpui::px(35.0), more.center().y),
        gpui::Modifiers::none(),
    );
    wait_for_bounds(cx, "pr-comment-502");
    assert!(cx.debug_bounds("pr-comment-501").is_some());

    transport
        .pr_comments_status
        .store(503, std::sync::atomic::Ordering::Relaxed);
    wait_for_bounds(cx, "tab-refresh");
    let refresh = cx.debug_bounds("tab-refresh").unwrap();
    cx.simulate_click(refresh.center(), gpui::Modifiers::none());
    wait_for_bounds(cx, "pr-comments-retry");
    assert!(cx.debug_bounds("pr-conversation-body").is_some());
    assert!(cx.debug_bounds("pr-comment-502").is_some());
    assert!(cx.debug_bounds("pr-comments-empty").is_none());

    transport
        .pr_comments_status
        .store(403, std::sync::atomic::Ordering::Relaxed);
    let retry = cx.debug_bounds("pr-comments-retry").unwrap();
    cx.simulate_click(retry.center(), gpui::Modifiers::none());
    wait_for_bounds(cx, "compact-failed-Could not read this pull request");
    assert!(cx.debug_bounds("pr-conversation-body").is_none());
    assert!(cx.debug_bounds("pr-comment-501").is_none());

    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(
            Route::PullRequest {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 7,
            },
            window,
            cx,
        )
    });
    wait_for_bounds(cx, "pr-conversation-empty-body");
    wait_for_bounds(cx, "pr-comments-empty");
}

fn wait_for_bounds(cx: &mut VisualTestContext, selector: &'static str) {
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if cx.debug_bounds(selector).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("{selector} did not appear");
}

fn wait_for_gone(cx: &mut VisualTestContext, selector: &'static str) {
    for _ in 0..400 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if cx.debug_bounds(selector).is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("{selector} did not disappear");
}

#[gpui::test]
fn notifications_route_reads_routes_repositories_and_completes_done(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Notifications, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "notification-row-0");
    assert!(cx.debug_bounds("notification-row-3").is_some());
    assert!(transport.seen("/notifications") >= 1, "the inbox is read");
    let notifications_tab = view.read_with(cx, |shell, _| shell.active_tab_id());

    // A verified Issue subject opens the real Issue detail surface and reads that issue's body,
    // not just a route value: the fixture only serves #42 from the issue endpoints.
    let issue_title = cx
        .debug_bounds("notification-title-0")
        .expect("issue title");
    cx.simulate_click(issue_title.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Issue {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 42,
        }
    );
    wait_for_bounds(cx, "issue-detail-body");
    wait_for_bounds(cx, "issue-comment-401");
    assert!(transport.seen("/repos/alice/demo/issues/42") >= 1);
    let demo_issue_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(demo_issue_tab, notifications_tab);

    // Reopening the same notification reuses the same Issue tab instead of duplicating it.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Notifications, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "notification-title-0");
    let issue_title = cx
        .debug_bounds("notification-title-0")
        .expect("issue title");
    cx.simulate_click(issue_title.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        demo_issue_tab,
        "the same notification issue reuses its tab"
    );

    // A Pull Request notification with the same repository and number is a distinct typed
    // destination: PR conversation mounting belongs to the pr-conversation task.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Notifications, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "notification-title-1");
    let pull_title = cx
        .debug_bounds("notification-title-1")
        .expect("pull request title");
    cx.simulate_click(pull_title.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::PullRequest {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 42,
        }
    );
    // The PR conversation page (delivered by #109) reads its own body and comments.
    wait_for_bounds(cx, "pr-conversation-body");
    wait_for_bounds(cx, "pr-comment-401");
    view.read_with(cx, |shell, cx| {
        let pr = shell
            .pull_request_summary_for_test(cx)
            .expect("mounted PR summary");
        assert_eq!(
            pr.body.as_deref(),
            Some("Pull request body reached from a notification click."),
            "the notification PR must open its own real conversation body"
        );
    });
    let pull_request_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(pull_request_tab, demo_issue_tab);

    // The same issue number in another repository opens its own issue, proving the repository
    // identity is part of the target rather than only the number.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Notifications, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "notification-title-2");
    let other_title = cx
        .debug_bounds("notification-title-2")
        .expect("other repository issue title");
    cx.simulate_click(other_title.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Issue {
            namespace: "alice".into(),
            name: "other".into(),
            number: 42,
        }
    );
    wait_for_bounds(cx, "issue-detail-body");
    let other_issue_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(other_issue_tab, demo_issue_tab);
    assert_ne!(other_issue_tab, pull_request_tab);

    // An official-shape `github.com` subject URL is not a match: the title stays inert.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Notifications, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "notification-title-3");
    let opaque = cx
        .debug_bounds("notification-title-3")
        .expect("opaque subject title");
    cx.simulate_click(opaque.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Notifications,
        "an unreliable subject keeps the previous non-navigating behavior"
    );

    // The repository link is a real typed navigation into the application, never a browser hop,
    // and stays independent of the subject title on the same row.
    let repository_link = cx
        .debug_bounds("notification-repository-0")
        .expect("repository link");
    cx.simulate_click(repository_link.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Repository {
            namespace: "alice".into(),
            name: "demo".into(),
        }
    );

    // Returning reuses the retained Notifications tab and its already-loaded snapshot.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Notifications, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "notification-row-0");

    // Done is the real DELETE semantics; the confirmed thread leaves the inbox row set and never
    // fires a subject navigation.
    let done = cx
        .debug_bounds("notification-action-done-0")
        .expect("Done control");
    cx.simulate_click(done.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    wait_for_gone(cx, "notification-row-3");
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Notifications
    );
    assert!(transport.wrote("DELETE") >= 1);
    assert!(transport.wrote("notifications/threads/9001") >= 1);
}

#[gpui::test]
fn issues_and_pull_requests_read_distinct_lists_and_reuse_their_tabs(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Issues, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "work-item-10001");
    assert!(
        cx.debug_bounds("work-item-20001").is_none(),
        "Issues shows no PR rows"
    );
    let issues_tab = view.read_with(cx, |shell, _| shell.active_tab_id());

    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::PullRequests, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "work-item-20001");
    assert!(
        cx.debug_bounds("work-item-10001").is_none(),
        "PRs shows no Issue rows"
    );

    // Re-opening the same resource reuses the existing tab instead of duplicating it.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::PullRequests, window, cx);
    });
    cx.run_until_parked();
    let pr_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(issues_tab, pr_tab, "Issues and PRs are distinct sessions");
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        pr_tab,
        "re-opening Pull requests reuses the same tab"
    );
    assert!(transport.seen("/search/issues") >= 2);
}

#[gpui::test]
fn repository_issue_list_opens_real_detail_and_returns_to_the_same_scroll(cx: &mut TestAppContext) {
    let transport = Arc::new(PageTransport::default());
    let (shell, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(&controller, cx, |accounts| {
        matches!(&accounts.snapshot().repositories.preview,
        crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2)
    });
    let repository = Route::Repository {
        namespace: "alice".into(),
        name: "demo".into(),
    };
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.navigate(repository.clone(), window, cx)
    });
    let repo_tab = shell.read_with(cx, |shell, _| shell.active_tab_id());
    wait_for_bounds(cx, "repository-tab-issues");
    let tab = cx.debug_bounds("repository-tab-issues").unwrap();
    cx.simulate_click(tab.center(), gpui::Modifiers::none());
    let drain_until = |selector: &'static str, cx: &mut VisualTestContext| {
        for _ in 0..400 {
            cx.update_window_entity(&shell, |shell, window, cx| {
                shell.drain_all_pages_for_test(window, cx)
            });
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
            if cx.debug_bounds(selector).is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("{selector} did not appear");
    };
    drain_until("repository-issues-more", cx);
    let more = cx.debug_bounds("repository-issues-more").unwrap();
    cx.simulate_click(more.center(), gpui::Modifiers::none());
    for _ in 0..400 {
        cx.update_window_entity(&shell, |shell, window, cx| {
            shell.drain_all_pages_for_test(window, cx)
        });
        cx.update(|window, cx| window.simulate_next_frame(cx));
        if cx.debug_bounds("repository-issues-more").is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(transport.seen("/repos/alice/demo/issues?"), 3);
    let viewport = cx.debug_bounds("repository-issues-viewport").unwrap();
    cx.simulate_event(gpui::ScrollEvent {
        position: viewport.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-3000.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    });
    drain_until("repository-issue-title-21", cx);
    let title = cx.debug_bounds("repository-issue-title-21").unwrap();
    assert!(viewport.contains(&title.center()));
    cx.simulate_click(title.center(), gpui::Modifiers::none());
    wait_for_bounds(cx, "issue-detail-body");
    wait_for_bounds(cx, "issue-comment-301");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.route()),
        Route::Issue {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 21
        }
    );
    let detail = shell.read_with(cx, |shell, _| shell.active_tab_id());
    cx.update_window_entity(&shell, |shell, window, cx| {
        shell.close_tab(detail, window, cx)
    });
    wait_for_bounds(cx, "repository-issue-title-21");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.active_tab_id()),
        repo_tab
    );
    assert_eq!(cx.debug_bounds("repository-issue-title-21").unwrap(), title);
    assert_eq!(transport.seen("/repos/alice/demo/issues?"), 3);
}

#[gpui::test]
fn global_issues_title_opens_reuses_and_isolates_issue_detail_while_repo_link_stays_separate(
    cx: &mut TestAppContext,
) {
    let transport = Arc::new(PageTransport::default());
    let (view, controller, cx) = open_shell(cx, transport.clone());
    login(&controller, cx);
    wait_until(
        &controller,
        cx,
        |accounts| matches!(&accounts.snapshot().repositories.preview, crate::ui::accounts::RepositoryPreview::Ready(outcome) if outcome.repositories.len() == 2),
    );

    // 1. Open Global Issues list.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Issues, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "work-title-10001");
    let issues_tab = view.read_with(cx, |shell, _| shell.active_tab_id());

    // 2. Click the issue title -> opens Route::Issue and renders body + comments.
    let title_bounds = cx.debug_bounds("work-title-10001").expect("issue title");
    cx.simulate_click(title_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    wait_for_bounds(cx, "issue-detail-body");
    wait_for_bounds(cx, "issue-comment-301");
    let demo_issue_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Issue {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 21,
        }
    );

    // 3. Re-opening the same issue from the Issues list reuses the existing detail tab.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Issues, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "work-title-10001");
    let title_bounds = cx.debug_bounds("work-title-10001").expect("issue title");
    cx.simulate_click(title_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        demo_issue_tab,
        "clicking the same issue title twice must reuse its tab"
    );

    // 4. Repository link on the same row still opens Route::Repository.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(Route::Issues, window, cx);
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "work-repo-10001");
    let repo_bounds = cx.debug_bounds("work-repo-10001").expect("repo link");
    cx.simulate_click(repo_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Repository {
            namespace: "alice".into(),
            name: "demo".into(),
        }
    );

    // 5. Same number in another repository and same repo+number as PR get distinct tabs.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(
            Route::Issue {
                namespace: "alice".into(),
                name: "other".into(),
                number: 21,
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "issue-detail-empty-body");
    wait_for_bounds(cx, "issue-detail-empty-comments");
    let other_issue_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(other_issue_tab, demo_issue_tab);

    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(
            Route::PullRequest {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 21,
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    let pr_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    assert_ne!(pr_tab, demo_issue_tab);
    assert_ne!(pr_tab, other_issue_tab);

    // 6. Trigger tab-refresh on active Issue detail tab.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(
            Route::Issue {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 21,
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    let before_issue_reads = transport.seen("/repos/alice/demo/issues/21");
    let refreshed = cx.update_window_entity(&view, |shell, window, cx| {
        shell.refresh_active_page(window, cx)
    });
    assert!(refreshed, "tab-refresh must be enabled on Route::Issue");
    cx.run_until_parked();
    wait_for_bounds(cx, "issue-detail-body");
    assert!(transport.seen("/repos/alice/demo/issues/21") > before_issue_reads);

    // 7. Closing the issue detail tab returns cleanly to the Issues list with its rows intact.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.workspace().close(demo_issue_tab, cx);
        shell.navigate(Route::Issues, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.active_tab_id()),
        issues_tab
    );
    wait_for_bounds(cx, "work-item-10001");
}

#[gpui::test]
fn cold_start_async_account_restore_opens_visible_issue_detail_once_paginates_returns_and_reopens(
    cx: &mut TestAppContext,
) {
    // Prepare an AccountService with an already-saved signed-in account (just like
    // `examples/issue_detail.rs` and cold-start with a saved account directory).
    let transport = Arc::new(PageTransport::default());
    let temp = tempfile::tempdir().unwrap();
    let executor = Arc::new(BackgroundExecutor::new(4));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let accounts_service = runtime.block_on(async {
        let service = AccountService::open(
            AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(
                GitHubConfig::default(),
                transport.clone() as Arc<dyn Transport>,
            )),
            executor.clone(),
        )
        .await
        .unwrap();
        service
            .login(crate::auth::token::Pat::new(
                "ghp_cold_start_token".to_string(),
            ))
            .await
            .unwrap();
        service
    });
    std::mem::forget(temp);
    let account_runtime = Arc::new(AccountRuntime::new(accounts_service, executor));

    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        crate::app::theme::install(cx);
        crate::ui::Shell::bind_keys(cx);
    });

    let (view, cx) = cx.add_window_view(|window, cx| {
        let controller = cx.new(|cx| Accounts::new(Some(account_runtime.clone()), window, cx));
        cx.set_global(AccountsHandle(controller.clone()));
        let shell = Shell::new(window, cx);

        // Product Shell itself still starts on Home until the preview observer reacts to the
        // async account restoration.
        assert_eq!(shell.route(), Route::Home);

        // Same one-shot event-driven bootstrap used by `examples/issue_detail.rs` (no sleep).
        let shell_weak = cx.weak_entity();
        let window_handle = window.window_handle();
        let mut opened = false;
        cx.observe(&controller, move |_, accounts, cx| {
            if opened || accounts.read(cx).snapshot().active.is_none() {
                return;
            }
            opened = true;
            let shell_weak = shell_weak.clone();
            let _ = window_handle.update(cx, move |_, window, cx| {
                window.defer(cx, move |window, cx| {
                    let _ = shell_weak.update(cx, |shell, cx| {
                        shell.navigate(Route::Issues, window, cx);
                        shell.navigate(
                            Route::Issue {
                                namespace: "alice".into(),
                                name: "demo".into(),
                                number: 21,
                            },
                            window,
                            cx,
                        );
                    });
                });
            });
        })
        .detach();
        shell
    });
    let controller = cx.read(AccountsHandle::get).expect("installed controller");

    // Wait for async account restore -> Shell::sync_accounts -> one-shot preview bootstrap.
    wait_until(&controller, cx, |accounts| {
        accounts.snapshot().active.is_some()
    });
    cx.run_until_parked();

    // 1. Landed directly on `alice/demo #21` with real visible body and page-1 comment.
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Issue {
            namespace: "alice".into(),
            name: "demo".into(),
            number: 21,
        }
    );
    wait_for_bounds(cx, "issue-detail-title");
    wait_for_bounds(cx, "issue-detail-body");
    wait_for_bounds(cx, "issue-comment-301");
    wait_for_bounds(cx, "issue-detail-load-more");

    // 2. Paginate comments via Load more button: page 2 appends without wiping body or page 1.
    let load_more = cx
        .debug_bounds("issue-detail-load-more")
        .expect("Load more comments button");
    cx.simulate_click(load_more.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    wait_for_bounds(cx, "issue-comment-302");
    assert!(cx.debug_bounds("issue-detail-body").is_some());
    assert!(cx.debug_bounds("issue-comment-301").is_some());

    // 3. Close the detail tab -> automatically activates left neighbour `Route::Issues`.
    let detail_tab = view.read_with(cx, |shell, _| shell.active_tab_id());
    cx.update_window_entity(&view, |shell, _, cx| {
        shell.workspace().close(detail_tab, cx);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |shell, _| shell.route()), Route::Issues);
    wait_for_bounds(cx, "work-title-10001");

    // A later Accounts notification must NOT re-trigger the one-shot bootstrap.
    controller.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |shell, _| shell.route()),
        Route::Issues,
        "one-shot bootstrap must not fire again after closing detail"
    );

    // 4. Reopen `alice/demo #21` by clicking its title in Global Issues.
    let title = cx.debug_bounds("work-title-10001").expect("issue title");
    cx.simulate_click(title.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    wait_for_bounds(cx, "issue-detail-body");
    wait_for_bounds(cx, "issue-comment-301");

    // 5. Missing (404) and permission denied (403) render distinct error states in Shell.
    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(
            Route::Issue {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 404,
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "issue-detail-error-missing");

    cx.update_window_entity(&view, |shell, window, cx| {
        shell.navigate(
            Route::Issue {
                namespace: "alice".into(),
                name: "demo".into(),
                number: 403,
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    wait_for_bounds(cx, "issue-detail-error-permission");
}
