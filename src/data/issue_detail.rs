//! Account-scoped single-Issue and paginated-comments data session, reusing
//! [`RepositorySession`]'s credential, auth epoch, concurrency semaphore and cancellation path,
//! plus optional persistent snapshots via [`AccountCache`].

#[cfg(test)]
#[path = "issue_detail_tests.rs"]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{AccountCache, RepoService, RepositoryReadContext, RepositorySession, Scoped};
use crate::domain::cache::{CacheCompleteness, CacheMetadata};
use crate::domain::error::GitHubError;
use crate::domain::issue_detail::{
    IssueAuthor, IssueComment, IssueCommentsCursor, IssueCommentsPage, IssueDetail,
    IssueDetailError, IssueLabel, IssueLocator, IssueState, IssueStateReason,
};
use crate::domain::repository::{RepositoryError, RepositoryLocator};
use crate::github::{
    GitHubClient,
    home::cache::PageTransport,
    issue_detail::{self, IssueDetailLimits},
};
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{
    CacheAuthorization, CacheError, CacheKey, CacheRead, CacheWrite, WriteOutcome,
};

const SNAPSHOT_VERSION: u32 = 1;
// Replay has both a page limit and a byte budget. Live pages still use the normal per-request limits.
const MAX_REPLAY_PAGES: u32 = 10;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct IssueSnapshotDto {
    id: u64,
    number: u64,
    owner: String,
    name: String,
    title: String,
    state: IssueState,
    state_reason: Option<IssueStateReason>,
    author: Option<IssueAuthor>,
    body: Option<String>,
    comments_count: u64,
    labels: Vec<IssueLabel>,
    created_at: String,
    updated_at: String,
    closed_at: Option<String>,
    html_url: Option<String>,
}

impl IssueSnapshotDto {
    fn from_detail(detail: &IssueDetail) -> Self {
        Self {
            id: detail.id,
            number: detail.number,
            owner: detail.repository.owner().to_string(),
            name: detail.repository.name().to_string(),
            title: detail.title.clone(),
            state: detail.state.clone(),
            state_reason: detail.state_reason.clone(),
            author: detail.author.clone(),
            body: detail.body.clone(),
            comments_count: detail.comments_count,
            labels: detail.labels.clone(),
            created_at: detail.created_at.clone(),
            updated_at: detail.updated_at.clone(),
            closed_at: detail.closed_at.clone(),
            html_url: detail.html_url.clone(),
        }
    }

    fn into_detail(self) -> Option<IssueDetail> {
        let repository =
            crate::domain::repository::RepositoryLocator::new(self.owner, self.name).ok()?;
        Some(IssueDetail {
            id: self.id,
            number: self.number,
            repository,
            title: self.title,
            state: self.state,
            state_reason: self.state_reason,
            author: self.author,
            body: self.body,
            comments_count: self.comments_count,
            labels: self.labels,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
            html_url: self.html_url,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReadingScope {
    pub(crate) repository: u64,
    binding: [u8; 16],
}

pub(crate) struct CachedReading {
    pub issue: IssueDetail,
    pub comments: Option<SavedComments>,
    pub scope: ReadingScope,
    pub verified_at_ms: i64,
    pub authorization: CacheAuthorization,
}

#[derive(Serialize, Deserialize)]
struct ReadingSnapshot {
    issue: IssueSnapshotDto,
    comments: Option<SavedComments>,
    verified_at_ms: i64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SavedComments {
    collection: [u8; 16],
    last_page: u32,
    per_page: u32,
    pub partial: bool,
    pub comments: Vec<IssueComment>,
}

pub(crate) struct Refresh<T> {
    pub result: Result<T, IssueDetailError>,
    pub no_store: bool,
}

#[derive(Debug)]
pub(crate) struct LiveIssue {
    pub issue: IssueDetail,
    pub repository: Option<u64>,
    pub scope: Option<ReadingScope>,
    pub authorization: Option<CacheAuthorization>,
}

#[derive(Debug)]
pub(crate) struct LiveComments {
    pub page: Scoped<IssueCommentsPage>,
    pub collection: Option<[u8; 16]>,
}

/// Account-bound session for reading a single Issue and its paginated comments.
#[derive(Clone)]
pub struct IssueDetailSession {
    repository: RepositorySession,
    cache: Option<AccountCache>,
}

impl RepoService {
    pub fn issue_detail_session(&self) -> Result<IssueDetailSession, IssueDetailError> {
        let repository = self.session().map_err(|_| GitHubError::StaleContext)?;
        let cache = self
            .cache(repository.context())
            .filter(|cache| cache.scope().account() == &repository.context().auth().account);
        Ok(IssueDetailSession { repository, cache })
    }
}

struct BodyWrite {
    route: CacheWrite,
    old_scope: Option<ReadingScope>,
    old_reading: Option<CacheWrite>,
    authorization: Option<CacheAuthorization>,
}

impl IssueDetailSession {
    pub fn context(&self) -> &RepositoryReadContext {
        self.repository.context()
    }

    pub fn verify(&self) -> bool {
        self.repository.verify()
    }

    fn check(&self, cancel: &CancellationToken) -> Result<(), IssueDetailError> {
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        Ok(())
    }

    fn key(&self, locator: &IssueLocator, scope: Option<&ReadingScope>) -> Option<CacheKey> {
        let config = self.repository.client().config();
        let request = serde_json::json!([
            locator.normalized_key(),
            config.base_url,
            config.accept,
            config.api_version,
            scope.map(|scope| scope.binding),
        ]);
        CacheKey::mutable(
            scope.map(|scope| scope.repository),
            true,
            if scope.is_some() {
                "issue-reading"
            } else {
                "issue-reading-route"
            },
            &request.to_string(),
            "githux-issue-reading-v1",
            SNAPSHOT_VERSION,
        )
        .ok()
    }

    pub(crate) async fn cached(
        &self,
        locator: &IssueLocator,
        limits: &IssueDetailLimits,
        cancel: &CancellationToken,
    ) -> Option<CachedReading> {
        self.check(cancel).ok()?;
        let cache = self.cache.clone()?;
        let session = self.clone();
        let locator = locator.clone();
        let per_page = limits.comments_per_page;
        let cached = tokio::task::spawn_blocking(move || {
            let route_auth = cache.store().authorization(cache.scope(), None)?;
            let scope: ReadingScope = load(&cache, &session.key(&locator, None)?)?;
            let authorization = cache
                .store()
                .authorization(cache.scope(), Some(scope.repository))?;
            let snapshot: ReadingSnapshot = load(&cache, &session.key(&locator, Some(&scope))?)?;
            let issue = snapshot.issue.into_detail()?;
            if !route_auth.is_current()
                || issue.number != locator.number()
                || !same_repository(&issue.repository, locator.repository())
            {
                return None;
            }
            Some(CachedReading {
                issue,
                comments: snapshot
                    .comments
                    .filter(|comments| comments.per_page == per_page),
                scope,
                verified_at_ms: snapshot.verified_at_ms,
                authorization,
            })
        })
        .await
        .ok()
        .flatten()?;
        self.check(cancel).ok()?;
        cached.authorization.is_current().then_some(cached)
    }

    async fn prepare(
        &self,
        locator: &IssueLocator,
        cancel: &CancellationToken,
    ) -> Option<BodyWrite> {
        let locator = locator.clone();
        self.cache_commit(cancel, move |session, cache| {
            let route_key = session.key(&locator, None)?;
            let route = cache.store().begin_write(cache.scope(), &route_key).ok()?;
            let old_scope = load::<ReadingScope>(&cache, &route_key);
            let authorization = old_scope.as_ref().and_then(|scope| {
                cache
                    .store()
                    .authorization(cache.scope(), Some(scope.repository))
            });
            let old_reading = old_scope.as_ref().and_then(|scope| {
                cache
                    .store()
                    .begin_write(cache.scope(), &session.key(&locator, Some(scope))?)
                    .ok()
            });
            Some(BodyWrite {
                route,
                old_scope,
                old_reading,
                authorization,
            })
        })
        .await
        .flatten()
    }

    /// Serialize the final context check and cache effect with account selection.
    /// Network reads and proof requests run outside the account commit lock.
    async fn cache_commit<T: Send + 'static>(
        &self,
        cancel: &CancellationToken,
        work: impl FnOnce(&Self, AccountCache) -> T + Send + 'static,
    ) -> Option<T> {
        let cache = self.cache.clone()?;
        let session = self.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = session.repository.service().credential_commit.lock().ok()?;
            session.check(&cancel).ok()?;
            Some(work(&session, cache))
        })
        .await
        .ok()
        .flatten()
    }

    async fn discard_reading(&self, cancel: &CancellationToken, write: BodyWrite) {
        self.cache_commit(cancel, move |_, cache| {
            discard(&cache, write.route);
            if let Some(ticket) = write.old_reading {
                discard(&cache, ticket);
            }
        })
        .await;
    }

    pub(crate) async fn refresh_issue(
        &self,
        locator: &IssueLocator,
        limits: &IssueDetailLimits,
        cancel: CancellationToken,
    ) -> Refresh<LiveIssue> {
        let write = self.prepare(locator, &cancel).await;
        let (client, transport) = self.read_client();
        let response = self
            .repository
            .read(cancel.clone(), |pat, cancel| async move {
                issue_detail::fetch_issue(&client, &pat, locator, limits, cancel).await
            })
            .await;
        let no_store = response.as_ref().is_ok_and(|reply| reply.value.no_store)
            || transport.is_some_and(|transport| transport.no_store());
        self.reject_credential(&response).await;
        let result = self
            .finish_issue(locator, write, response, no_store, &cancel)
            .await;
        Refresh { result, no_store }
    }

    async fn finish_issue(
        &self,
        locator: &IssueLocator,
        write: Option<BodyWrite>,
        response: Result<Scoped<issue_detail::IssueResponse<IssueDetail>>, IssueDetailError>,
        no_store: bool,
        cancel: &CancellationToken,
    ) -> Result<LiveIssue, IssueDetailError> {
        self.check(cancel)?;
        let Some((cache, mut write)) = self.cache.clone().zip(write) else {
            return response.map(|reply| LiveIssue {
                issue: reply.value.value,
                repository: None,
                scope: None,
                authorization: None,
            });
        };
        if write
            .authorization
            .as_ref()
            .is_some_and(|auth| !auth.is_current())
        {
            return Err(GitHubError::StaleContext.into());
        }
        if no_store || matches!(&response, Err(error) if error.invalidates_reading()) {
            self.discard_reading(cancel, write).await;
            self.check(cancel)?;
            return response.map(|reply| LiveIssue {
                issue: reply.value.value,
                repository: None,
                scope: None,
                authorization: None,
            });
        }
        let issue = response?.value.value;
        let (repository, identity_no_store) = self.confirm_repository(locator, cancel).await?;
        self.check(cancel)?;
        if write
            .authorization
            .as_ref()
            .is_some_and(|auth| !auth.is_current())
        {
            return Err(GitHubError::StaleContext.into());
        }
        let Some(repository_id) = repository else {
            if identity_no_store {
                self.discard_reading(cancel, write).await;
            }
            self.check(cancel)?;
            return Ok(LiveIssue {
                issue,
                repository,
                scope: None,
                authorization: None,
            });
        };
        let authorization = cache.store().authorization(cache.scope(), repository);
        let old_scope = write.old_scope.take();
        let old_ticket = write.old_reading.take();
        let repository_changed = old_scope
            .as_ref()
            .is_some_and(|scope| scope.repository != repository_id);
        let (scope, ticket) =
            if !repository_changed && let Some((scope, ticket)) = old_scope.zip(old_ticket) {
                (scope, Some(ticket))
            } else {
                let scope = ReadingScope {
                    repository: repository_id,
                    binding: uuid::Uuid::new_v4().into_bytes(),
                };
                let key = self.key(locator, Some(&scope));
                let ticket = self.new_ticket(locator, repository_id, key, cancel).await?;
                (scope, ticket)
            };
        let key = self.key(locator, Some(&scope));
        let dto = IssueSnapshotDto::from_detail(&issue);
        let auth = authorization.clone();
        let scope = self
            .cache_commit(cancel, move |_, cache| {
                if !auth.as_ref()?.is_current() {
                    return None;
                }
                let comments =
                    load::<ReadingSnapshot>(&cache, &key?).and_then(|saved| saved.comments);
                let mut snapshot = ReadingSnapshot {
                    issue: dto,
                    comments,
                    verified_at_ms: now_ms(),
                };
                let bytes = encode(&snapshot).or_else(|| {
                    snapshot.comments = None;
                    encode(&snapshot)
                });
                let stored = ticket.zip(bytes).is_some_and(|(ticket, bytes)| {
                    store(
                        &cache,
                        ticket,
                        &bytes,
                        completeness(snapshot.comments.as_ref()),
                        false,
                    )
                });
                if !stored {
                    if repository_changed {
                        discard(&cache, write.route);
                    }
                    return None;
                }
                save(&cache, write.route, &scope, CacheCompleteness::Complete).then_some(scope)
            })
            .await
            .flatten();
        self.check(cancel)?;
        if authorization
            .as_ref()
            .is_some_and(|auth| !auth.is_current())
        {
            return Err(GitHubError::StaleContext.into());
        }
        Ok(LiveIssue {
            issue,
            repository,
            scope,
            authorization,
        })
    }

    async fn confirm_repository(
        &self,
        locator: &IssueLocator,
        cancel: &CancellationToken,
    ) -> Result<(Option<u64>, bool), IssueDetailError> {
        let (client, transport) = self.read_client();
        let client = &client;
        let response = self
            .repository
            .read(cancel.clone(), |pat, cancel| async move {
                issue_detail::fetch_repository_identity(client, &pat, locator, cancel).await
            })
            .await;
        let no_store = transport.is_some_and(|transport| transport.no_store());
        match response {
            Ok(reply)
                if !no_store
                    && reply.value.id() > 0
                    && same_repository(reply.value.locator(), locator.repository()) =>
            {
                Ok((Some(reply.value.id()), false))
            }
            Err(RepositoryError::GitHub(GitHubError::Unauthorized)) => {
                let error = Err(GitHubError::Unauthorized.into());
                self.reject_credential::<()>(&error).await;
                error.map(|_| (None, no_store))
            }
            _ => Ok((None, no_store)),
        }
    }

    async fn new_ticket(
        &self,
        locator: &IssueLocator,
        repository: u64,
        key: Option<CacheKey>,
        cancel: &CancellationToken,
    ) -> Result<Option<CacheWrite>, IssueDetailError> {
        let Some(key) = key else { return Ok(None) };
        let begin = || {
            let key = key.clone();
            self.cache_commit(cancel, move |_, cache| {
                cache.store().begin_write(cache.scope(), &key)
            })
        };
        match begin().await {
            Some(Ok(ticket)) => return Ok(Some(ticket)),
            Some(Err(CacheError::AccessDenied)) => {}
            _ => return Ok(None),
        }
        // Discovering the repo ID does not lift a quarantine. Establish a proof before
        // asking GitHub to confirm access again with the current fixed credentials.
        let session = self.repository.clone();
        let proof = tokio::task::spawn_blocking(move || {
            session
                .service()
                .begin_revalidation(session.context(), Some(repository))
        })
        .await;
        let Ok(Ok(proof)) = proof else {
            return Ok(None);
        };
        self.check(cancel)?;
        if self.confirm_repository(locator, cancel).await?.0 != Some(repository) {
            return Ok(None);
        }
        self.check(cancel)?;
        if self
            .repository
            .service()
            .permission_confirmed(self.context(), proof)
            .await
            .is_err()
        {
            return Ok(None);
        }
        Ok(begin().await.and_then(Result::ok))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn refresh_comments(
        &self,
        locator: &IssueLocator,
        scope: Option<&ReadingScope>,
        collection: Option<[u8; 16]>,
        cursor: Option<&IssueCommentsCursor>,
        limits: &IssueDetailLimits,
        cancel: CancellationToken,
        persist: bool,
    ) -> Refresh<LiveComments> {
        let page = cursor.map_or(1, IssueCommentsCursor::page);
        let route = self.key(locator, None);
        let requested_scope = scope.cloned();
        let target = locator.clone();
        let work = self
            .cache_commit(&cancel, move |session, cache| {
                let scope = load::<ReadingScope>(&cache, &route?)?;
                if requested_scope
                    .as_ref()
                    .is_some_and(|requested| requested != &scope)
                {
                    return None;
                }
                let authorization = cache
                    .store()
                    .authorization(cache.scope(), Some(scope.repository))?;
                let key = session.key(&target, Some(&scope))?;
                let ticket = cache.store().begin_write(cache.scope(), &key).ok()?;
                Some((
                    authorization,
                    key,
                    ticket,
                    persist && requested_scope.is_some(),
                ))
            })
            .await
            .flatten();
        let (client, transport) = self.read_client();
        let response = self
            .repository
            .read(cancel.clone(), |pat, cancel| async move {
                issue_detail::fetch_comments_page(&client, &pat, locator, cursor, limits, cancel)
                    .await
            })
            .await;
        let no_store = response.as_ref().is_ok_and(|reply| reply.value.no_store)
            || transport.is_some_and(|transport| transport.no_store());
        self.reject_credential(&response).await;
        let mut collection = if page == 1 {
            Some(uuid::Uuid::new_v4().into_bytes())
        } else {
            collection
        };
        let result = self.check(&cancel).and(response);
        if let Some((authorization, key, ticket, persist)) = work {
            let invalidate = matches!(&result, Err(error) if error.invalidates_reading());
            let response = result.as_ref().ok().map(|reply| reply.value.value.clone());
            let per_page = limits.comments_per_page;
            let stamp = authorization.clone();
            let persisted = self
                .cache_commit(&cancel, move |_, cache| {
                    if !stamp.is_current() {
                        return false;
                    }
                    if invalidate {
                        discard(&cache, ticket);
                        return false;
                    }
                    let Some(mut snapshot) = load::<ReadingSnapshot>(&cache, &key) else {
                        return false;
                    };
                    if no_store {
                        snapshot.comments = None;
                        save(
                            &cache,
                            ticket,
                            &snapshot,
                            CacheCompleteness::Partial {
                                reason: "comments are not saved".into(),
                            },
                        );
                        return false;
                    }
                    let Some(response) = response.filter(|_| persist && page <= MAX_REPLAY_PAGES)
                    else {
                        return false;
                    };
                    let Some(id) = collection else { return false };
                    if page == 1 {
                        snapshot.comments = Some(SavedComments {
                            collection: id,
                            last_page: 1,
                            per_page,
                            partial: response.next.is_some(),
                            comments: response.comments,
                        });
                    } else {
                        let Some(saved) = snapshot.comments.as_mut().filter(|saved| {
                            saved.collection == id
                                && saved.last_page + 1 == page
                                && saved.partial
                                && saved.per_page == per_page
                        }) else {
                            return false;
                        };
                        let mut seen: HashSet<_> =
                            saved.comments.iter().map(|comment| comment.id).collect();
                        saved.comments.extend(
                            response
                                .comments
                                .into_iter()
                                .filter(|comment| seen.insert(comment.id)),
                        );
                        saved.last_page = page;
                        saved.partial = response.next.is_some();
                    }
                    let Some(bytes) = encode(&snapshot) else {
                        if page == 1 {
                            snapshot.comments = None;
                            save(&cache, ticket, &snapshot, completeness(None));
                        }
                        return false;
                    };
                    store(
                        &cache,
                        ticket,
                        &bytes,
                        completeness(snapshot.comments.as_ref()),
                        false,
                    )
                })
                .await
                .unwrap_or(false);
            if !authorization.is_current() {
                return Refresh {
                    result: Err(GitHubError::StaleContext.into()),
                    no_store,
                };
            }
            if !persisted && result.is_ok() {
                collection = None;
            }
        }
        Refresh {
            result: self.check(&cancel).and(result).map(|reply| LiveComments {
                page: Scoped {
                    context: reply.context,
                    value: reply.value.value,
                },
                collection,
            }),
            no_store,
        }
    }

    async fn reject_credential<T>(&self, response: &Result<T, IssueDetailError>) {
        if matches!(
            response,
            Err(IssueDetailError::GitHub(GitHubError::Unauthorized))
        ) {
            let _ = self
                .repository
                .service()
                .credential_rejected(self.context())
                .await;
        }
    }

    fn read_client(&self) -> (Arc<GitHubClient>, Option<Arc<PageTransport>>) {
        let client = self.repository.client();
        if self.cache.is_none() {
            return (client.clone(), None);
        }
        let transport = Arc::new(PageTransport::recording(client.transport().clone()));
        (
            Arc::new(GitHubClient::new(
                client.config().clone(),
                transport.clone(),
            )),
            Some(transport),
        )
    }
}

fn same_repository(left: &RepositoryLocator, right: &RepositoryLocator) -> bool {
    left.owner().eq_ignore_ascii_case(right.owner())
        && left.name().eq_ignore_ascii_case(right.name())
}

fn load<T: DeserializeOwned>(cache: &AccountCache, key: &CacheKey) -> Option<T> {
    let CacheRead::Hit(lease) = cache.store().read(cache.scope(), key).ok()? else {
        return None;
    };
    let hit = lease.snapshot().ok().flatten()?;
    if hit.bytes.len() > MAX_SNAPSHOT_BYTES {
        return None;
    }
    serde_json::from_slice(&hit.bytes).ok()
}

fn completeness(comments: Option<&SavedComments>) -> CacheCompleteness {
    match comments {
        Some(comments) if !comments.partial => CacheCompleteness::Complete,
        _ => CacheCompleteness::Partial {
            reason: "comments have not been fully read".into(),
        },
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn encode(value: &impl Serialize) -> Option<Vec<u8>> {
    let bytes = serde_json::to_vec(value).ok()?;
    (bytes.len() <= MAX_SNAPSHOT_BYTES).then_some(bytes)
}

fn save(
    cache: &AccountCache,
    ticket: CacheWrite,
    value: &impl Serialize,
    completeness: CacheCompleteness,
) -> bool {
    encode(value).is_some_and(|bytes| store(cache, ticket, &bytes, completeness, false))
}

fn discard(cache: &AccountCache, ticket: CacheWrite) {
    store(cache, ticket, &[], CacheCompleteness::Complete, true);
}

fn store(
    cache: &AccountCache,
    ticket: CacheWrite,
    bytes: &[u8],
    completeness: CacheCompleteness,
    no_store: bool,
) -> bool {
    let now = now_ms();
    let metadata = CacheMetadata {
        representation_version: SNAPSHOT_VERSION,
        fetched_at_ms: now,
        verified_at_ms: now,
        completeness,
        etag: None,
        last_modified: None,
        cache_control: no_store.then(|| "no-store".into()),
    };
    matches!(
        cache.store().put(ticket, bytes, metadata),
        Ok(WriteOutcome::Stored)
    )
}
