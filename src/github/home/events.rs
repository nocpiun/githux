//! `GET /users/{login}/received_events`.
//!
//! The adapter parses every received event into the minimal typed payload and
//! publishes the **source list** unchanged: selecting retained activities,
//! resolving merged-pull-request authors and applying the followed-account
//! constraint all happen in [`crate::domain::home::build_feed`], so one policy
//! applies to every data path. Technical events (push, issues, comments,
//! releases, forks, membership, public, branch/tag create-or-delete, unknown
//! types) never become cards there.
//!
//! Source shape note: `received_events` merges activity from followed users and
//! watched repositories, so it is **not** a followed-users-only feed. That is why
//! the relationship filter is applied explicitly downstream instead of assuming
//! every event actor is followed.
//!
//! The adapter surfaces `ETag`, `X-Poll-Interval` and `304 Not Modified` so the
//! caller can schedule its own polling; it never polls in the background, never
//! uses web cookies and never calls an internal endpoint. Unknown types keep just
//! the type name, so a new payload shape cannot grow without bound.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::home::{
    FeedActor, FeedAuthor, FeedEvent, FeedEventType, FeedEventsRead, FeedFetch, FeedPartialReason,
    FeedPayload, FeedReadiness, FeedRepository, FeedSourceCursor, FeedSourcePage, HomeError, Login,
};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::github::headers::{parse_link_next, parse_sso_header};
use crate::network::cancel::CancellationToken;
use crate::network::transport::TransportError;
use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

/// The Events timeline only holds the most recent 300 events.
const EVENTS_WINDOW_LIMIT: usize = 300;

/// Defensive budgets for the feed adapter, bounded by the Events window.
/// `max_events` counts parsed source events, exactly as read: selecting retained
/// activities downstream must not trigger extra fetching to fill a page.
#[derive(Debug, Clone)]
pub struct FeedLimits {
    pub max_pages: usize,
    pub max_events: usize,
    pub response_bytes: usize,
    pub per_page: u32,
}

impl Default for FeedLimits {
    fn default() -> Self {
        Self {
            max_pages: 3,
            max_events: EVENTS_WINDOW_LIMIT,
            response_bytes: 4 * 1024 * 1024,
            per_page: 100,
        }
    }
}

#[derive(Deserialize)]
struct RawEvent {
    id: String,
    #[serde(rename = "type")]
    event_type: Option<String>,
    actor: RawActor,
    repo: RawRepo,
    #[serde(default)]
    payload: Value,
    #[serde(default)]
    public: bool,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct RawActor {
    id: u64,
    login: String,
    #[serde(default)]
    display_login: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct RawRepo {
    id: u64,
    name: String,
    #[serde(default)]
    url: String,
}

/// Fetch exactly one page of received events, returning the parsed source events
/// and the validated next-page cursor (when the server offered one).
///
/// The next cursor is `None` only when the server exposed no `rel="next"` link or
/// the timeline hit the 300-event window; an untrusted/invalid/cycling link is a
/// typed partial reason and stops this batch. A failed page is a hard error: the
/// caller retries the same cursor, it does not advance.
pub async fn fetch_received_events_page(
    client: &GitHubClient,
    token: &Pat,
    login: &Login,
    cursor: FeedSourceCursor,
    limits: &FeedLimits,
    cancel: CancellationToken,
) -> Result<FeedSourcePage, HomeError> {
    validate_limits(limits)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    let url = feed_url(client, login, limits.per_page, cursor.page)?;
    let response = client
        .transport()
        .execute_bounded(
            client.build_get(url.as_str(), token),
            cancel.clone(),
            limits.response_bytes,
        )
        .await
        .map_err(|error| match error {
            TransportError::ResponseTooLarge { limit } => HomeError::LimitExceeded {
                resource: "events response bytes",
                limit,
            },
            other => classify_transport(other).into(),
        })?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    // Defense for custom transports that do not enforce the receive bound.
    if response.body.len() > limits.response_bytes {
        return Err(HomeError::LimitExceeded {
            resource: "events response bytes",
            limit: limits.response_bytes,
        });
    }
    let mut reasons = Vec::new();
    if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
        // Partial SSO still carries events; preserve the reason, do not drop them.
        reasons.push(FeedPartialReason::SsoPartial { organizations });
    }
    if let Some(error) = classify_status(&response) {
        return Err(error.into());
    }
    let raw_events: Vec<RawEvent> =
        serde_json::from_str(&response.body).map_err(|error| GitHubError::Parse {
            message: error.to_string(),
        })?;
    let etag = response.header("etag").map(str::to_string);
    let poll_interval_seconds = response
        .header("x-poll-interval")
        .and_then(|value| value.parse().ok());
    let mut events = Vec::with_capacity(raw_events.len().min(limits.max_events));
    for raw in raw_events {
        if events.len() >= limits.max_events {
            // The source budget was hit on this page: stop and report truncation,
            // and do not expose a continuation cursor that would bypass the limit.
            reasons.push(FeedPartialReason::EntryLimit);
            return Ok(FeedSourcePage {
                events,
                next: None,
                reasons,
                etag,
                poll_interval_seconds,
            });
        }
        events.push(convert_event(raw)?);
    }
    // Enforce the page budget before honoring a next link: a caller-supplied
    // `max_pages` is a bound on the whole incremental walk, not a soft hint.
    if cursor.page as usize >= limits.max_pages {
        reasons.push(FeedPartialReason::PageLimit);
        return Ok(FeedSourcePage {
            events,
            next: None,
            reasons,
            etag,
            poll_interval_seconds,
        });
    }
    let next = match next_page(client, response.header("link"), cursor.page)? {
        NextPage::None => None,
        NextPage::Page(next) => Some(FeedSourceCursor { page: next }),
        NextPage::Untrusted => {
            reasons.push(FeedPartialReason::UntrustedPagination);
            None
        }
        NextPage::Invalid => {
            reasons.push(FeedPartialReason::InvalidPagination);
            None
        }
        NextPage::Cycle => {
            reasons.push(FeedPartialReason::PaginationCycle);
            None
        }
    };
    Ok(FeedSourcePage {
        events,
        next,
        reasons,
        etag,
        poll_interval_seconds,
    })
}

/// Fetch events received by `login`.
///
/// The returned events are the **source** list in server order; the product
/// policy is applied by [`crate::domain::home::build_feed`]. When `if_none_match`
/// is supplied it is sent only on the first page. A `304` yields
/// [`FeedFetch::NotModified`] with the current ETag and poll hint. A normal empty
/// source is `Complete` with zero events, not an error.
pub async fn fetch_received_events(
    client: &GitHubClient,
    token: &Pat,
    login: &Login,
    if_none_match: Option<&str>,
    limits: &FeedLimits,
    cancel: CancellationToken,
) -> Result<FeedFetch, HomeError> {
    validate_limits(limits)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }

    let mut page: u32 = 1;
    let mut events: Vec<FeedEvent> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut reasons: Vec<FeedPartialReason> = Vec::new();
    let mut etag: Option<String> = None;
    let mut poll_interval_seconds: Option<u64> = None;
    let mut pages_fetched = 0usize;
    // Budgets count **parsed source events**, not retained activities: the Feed
    // policy removes non-activities, and a filtered list must never trigger extra
    // requests to fill a page. This is also why the retention filter below cannot
    // be confused with truncation.
    let mut parsed_events = 0usize;

    loop {
        if page as usize > limits.max_pages {
            reasons.push(FeedPartialReason::PageLimit);
            break;
        }

        let url = feed_url(client, login, limits.per_page, page)?;
        let mut request = client.build_get(url.as_str(), token);
        // The ETag describes the whole feed list, so it is only meaningful on
        // the first request; sending it on page 2+ could suppress a page the
        // caller still needs.
        if page == 1
            && let Some(value) = if_none_match
        {
            request = request.with_header("If-None-Match", value);
        }

        let response = match client
            .transport()
            .execute_bounded(request, cancel.clone(), limits.response_bytes)
            .await
        {
            Ok(response) => response,
            Err(TransportError::ResponseTooLarge { limit }) => {
                if pages_fetched == 0 {
                    return Err(HomeError::LimitExceeded {
                        resource: "events response bytes",
                        limit,
                    });
                }
                reasons.push(FeedPartialReason::ResponseLimit);
                break;
            }
            Err(error) => {
                let error = classify_transport(error);
                if matches!(error, GitHubError::Cancelled) {
                    return Err(error.into());
                }
                if pages_fetched == 0 {
                    return Err(error.into());
                }
                reasons.push(FeedPartialReason::PageFailed {
                    message: error.to_string(),
                });
                break;
            }
        };
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }

        if etag.is_none() {
            etag = response
                .header("etag")
                .map(str::to_string)
                // A 304 may omit the ETag; the caller's validator is still valid.
                .or_else(|| if_none_match.map(str::to_string));
        }
        if poll_interval_seconds.is_none() {
            poll_interval_seconds = response
                .header("x-poll-interval")
                .and_then(|value| value.parse().ok());
        }

        if response.status == 304 {
            if pages_fetched == 0 {
                return Ok(FeedFetch::NotModified {
                    etag,
                    poll_interval_seconds,
                });
            }
            // A 304 after earlier pages returned data is unexpected; keep what
            // we have and report it as partial rather than silently complete.
            reasons.push(FeedPartialReason::PageFailed {
                message: "unexpected 304 on a later page".to_string(),
            });
            break;
        }

        // Defense for custom transports that do not enforce the receive bound.
        if response.body.len() > limits.response_bytes {
            if pages_fetched == 0 {
                return Err(HomeError::LimitExceeded {
                    resource: "events response bytes",
                    limit: limits.response_bytes,
                });
            }
            reasons.push(FeedPartialReason::ResponseLimit);
            break;
        }
        if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
            reasons.push(FeedPartialReason::SsoPartial { organizations });
        }
        if let Some(error) = classify_status(&response) {
            if pages_fetched == 0 {
                return Err(error.into());
            }
            reasons.push(FeedPartialReason::PageFailed {
                message: error.to_string(),
            });
            break;
        }

        let raw_events: Vec<RawEvent> = match serde_json::from_str(&response.body) {
            Ok(raw_events) => raw_events,
            Err(error) => {
                if pages_fetched == 0 {
                    return Err(GitHubError::Parse {
                        message: error.to_string(),
                    }
                    .into());
                }
                reasons.push(FeedPartialReason::ParseFailed {
                    message: error.to_string(),
                });
                break;
            }
        };
        pages_fetched += 1;

        let mut truncated = false;
        for raw in raw_events {
            if parsed_events >= limits.max_events {
                truncated = true;
                break;
            }
            parsed_events += 1;
            let event = convert_event(raw)?;
            // Identity de-duplication uses the real event id across pages, so a
            // retained activity keeps its source identity.
            if seen.insert(event.id.clone()) {
                events.push(event);
            }
        }
        if truncated {
            reasons.push(FeedPartialReason::EntryLimit);
            break;
        }
        match next_page(client, response.header("link"), page)? {
            NextPage::None => break,
            NextPage::Page(next) => page = next,
            NextPage::Untrusted => {
                reasons.push(FeedPartialReason::UntrustedPagination);
                break;
            }
            NextPage::Invalid => {
                reasons.push(FeedPartialReason::InvalidPagination);
                break;
            }
            NextPage::Cycle => {
                reasons.push(FeedPartialReason::PaginationCycle);
                break;
            }
        }
    }

    let readiness = FeedReadiness::from_reasons(reasons);
    // A partial pagination must not hand back a reusable validator: a caller
    // that sent it back would get `304 NotModified` on page 1 and never retry
    // the missing page. The poll hint is still safe to return.
    let etag = match &readiness {
        FeedReadiness::Partial { .. } => None,
        FeedReadiness::Complete => etag,
    };
    Ok(FeedFetch::Events(FeedEventsRead {
        events,
        etag,
        poll_interval_seconds,
        readiness,
    }))
}

fn validate_limits(limits: &FeedLimits) -> Result<(), HomeError> {
    if limits.per_page == 0 || limits.per_page > 100 {
        return Err(HomeError::InvalidInput { field: "per_page" });
    }
    if limits.max_pages == 0 {
        return Err(HomeError::InvalidInput { field: "max_pages" });
    }
    if limits.max_events == 0 {
        return Err(HomeError::InvalidInput {
            field: "max_events",
        });
    }
    Ok(())
}

fn feed_url(
    client: &GitHubClient,
    login: &Login,
    per_page: u32,
    page: u32,
) -> Result<Url, HomeError> {
    // `login` is validated to alphanumerics and hyphens, so it cannot escape the
    // path segment. The URL is still origin-checked through the client.
    let base = format!(
        "{}users/{}/received_events",
        client.config().base_url,
        login.as_str()
    );
    let mut url = Url::parse(&base).map_err(|_| HomeError::InvalidInput {
        field: "API base URL",
    })?;
    url.query_pairs_mut()
        .append_pair("per_page", &per_page.to_string())
        .append_pair("page", &page.to_string());
    if !client.url_is_trusted(url.as_str()) {
        return Err(HomeError::InvalidInput {
            field: "API origin",
        });
    }
    Ok(url)
}

fn convert_event(raw: RawEvent) -> Result<FeedEvent, HomeError> {
    let type_name = raw.event_type.unwrap_or_default();
    let (event_type, payload) = convert_payload(&type_name, &raw.payload);
    Ok(FeedEvent {
        id: raw.id,
        event_type,
        actor: FeedActor {
            id: raw.actor.id,
            login: raw.actor.login,
            display_login: raw.actor.display_login,
            avatar_url: raw.actor.avatar_url,
        },
        repository: FeedRepository {
            id: raw.repo.id,
            name: raw.repo.name,
            url: raw.repo.url,
        },
        created_at: raw.created_at,
        public: raw.public,
        payload,
    })
}

fn convert_payload(type_name: &str, payload: &Value) -> (FeedEventType, FeedPayload) {
    match type_name {
        "PushEvent" => (
            FeedEventType::Push,
            FeedPayload::Push {
                git_ref: string_at(payload, &["ref"]),
                before: string_at(payload, &["before"]),
                head: string_at(payload, &["head"]),
                size: u64_at(payload, &["size"]),
            },
        ),
        "PullRequestEvent" => (
            FeedEventType::PullRequest,
            FeedPayload::PullRequest {
                action: action(payload),
                number: u64_at(payload, &["number"]),
                title: string_at(payload, &["pull_request", "title"]),
                html_url: string_at(payload, &["pull_request", "html_url"]),
                state: string_at(payload, &["pull_request", "state"]),
                merged: bool_at(payload, &["pull_request", "merged"]),
                // The real contributor, from the authoritative payload, so a
                // maintainer's merge click is never credited as authorship and
                // no extra pull-request request is needed.
                author: author_at(payload, &["pull_request", "user"]),
            },
        ),
        "IssuesEvent" => (
            FeedEventType::Issues,
            FeedPayload::Issues {
                action: action(payload),
                number: u64_at(payload, &["issue", "number"]),
                title: string_at(payload, &["issue", "title"]),
                html_url: string_at(payload, &["issue", "html_url"]),
                state: string_at(payload, &["issue", "state"]),
            },
        ),
        "IssueCommentEvent" => (
            FeedEventType::IssueComment,
            FeedPayload::IssueComment {
                action: action(payload),
                issue_number: u64_at(payload, &["issue", "number"]),
                comment_url: string_at(payload, &["comment", "html_url"]),
            },
        ),
        "WatchEvent" => (
            FeedEventType::Watch,
            FeedPayload::Watch {
                action: action(payload),
            },
        ),
        "CreateEvent" => (
            FeedEventType::Create,
            FeedPayload::Create {
                ref_type: string_at(payload, &["ref_type"]),
                git_ref: string_at(payload, &["ref"]),
            },
        ),
        "DeleteEvent" => (
            FeedEventType::Delete,
            FeedPayload::Delete {
                ref_type: string_at(payload, &["ref_type"]),
                git_ref: string_at(payload, &["ref"]),
            },
        ),
        "ForkEvent" => (
            FeedEventType::Fork,
            FeedPayload::Fork {
                forkee_full_name: string_at(payload, &["forkee", "full_name"]),
            },
        ),
        "ReleaseEvent" => (
            FeedEventType::Release,
            FeedPayload::Release {
                action: action(payload),
                tag_name: string_at(payload, &["release", "tag_name"]),
                name: string_at(payload, &["release", "name"]),
                html_url: string_at(payload, &["release", "html_url"]),
            },
        ),
        "PublicEvent" => (FeedEventType::Public, FeedPayload::Public),
        "MemberEvent" => (
            FeedEventType::Member,
            FeedPayload::Member {
                action: action(payload),
                member_login: string_at(payload, &["member", "login"]),
            },
        ),
        // Unknown types are compatible but keep no payload, so a future shape
        // cannot be stored without bound.
        other => (
            FeedEventType::Unknown(other.to_string()),
            FeedPayload::Unknown,
        ),
    }
}

fn action(payload: &Value) -> String {
    string_at(payload, &["action"]).unwrap_or_default()
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(str::to_string)
}

fn u64_at(value: &Value, path: &[&str]) -> Option<u64> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_u64()
}

fn bool_at(value: &Value, path: &[&str]) -> Option<bool> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_bool()
}

/// Read a `{id, login, avatar_url}` user object. A user without both a numeric
/// id and a non-empty login is not usable attribution evidence, so it is treated
/// as absent rather than as a partial author.
fn author_at(value: &Value, path: &[&str]) -> Option<FeedAuthor> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    let id = current.get("id")?.as_u64()?;
    let login = current.get("login")?.as_str()?.trim();
    if login.is_empty() {
        return None;
    }
    Some(FeedAuthor {
        id,
        login: login.to_string(),
        avatar_url: current
            .get("avatar_url")
            .and_then(Value::as_str)
            .filter(|url| !url.trim().is_empty())
            .map(str::to_string),
    })
}

enum NextPage {
    None,
    Page(u32),
    Untrusted,
    Invalid,
    Cycle,
}

/// Validate the `Link: rel="next"` target and return only its page number. The
/// request is rebuilt from our own endpoint/query, so a stale or hostile link
/// cannot change the feed identity.
fn next_page(
    client: &GitHubClient,
    link: Option<&str>,
    current: u32,
) -> Result<NextPage, HomeError> {
    let Some(target) = parse_link_next(link) else {
        return Ok(NextPage::None);
    };
    if !client.url_is_trusted(&target) {
        return Ok(NextPage::Untrusted);
    }
    let Ok(url) = Url::parse(&target) else {
        return Ok(NextPage::Invalid);
    };
    if !url
        .path()
        .trim_end_matches('/')
        .ends_with("/received_events")
    {
        return Ok(NextPage::Invalid);
    }
    let page = url
        .query_pairs()
        .find(|(key, _)| key.as_ref() == "page")
        .and_then(|(_, value)| value.parse::<u32>().ok());
    let Some(page) = page else {
        return Ok(NextPage::Invalid);
    };
    if page <= current {
        return Ok(NextPage::Cycle);
    }
    Ok(NextPage::Page(page))
}

#[cfg(test)]
mod tests;
