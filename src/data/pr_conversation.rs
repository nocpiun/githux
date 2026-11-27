//! Durable, account-bound reading snapshots for the PR Conversation route.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::HashSet, sync::Arc, time::SystemTime};

use super::{AccountCache, RepositorySession, Scoped};
use crate::domain::{
    cache::{CacheCompleteness, CacheMetadata},
    error::GitHubError,
    issue_detail::{
        IssueComment, IssueCommentsCursor, IssueCommentsPage, IssueDetailError, IssueLocator,
    },
    pr_conversation::{Conversation, ConversationReading, Target, access_denied},
};
use crate::github::{
    GitHubClient,
    home::cache::PageTransport,
    issue_detail::{self, IssueDetailLimits},
    pr_conversation,
};
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{
    CacheAuthorization, CacheKey, CacheRead, CacheWrite, Revalidation, WriteOutcome,
};

const VERSION: u32 = 1;
// Bound restart work independently of the live, user-driven pagination.
const MAX_REPLAY_PAGES: u32 = 10;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReadingScope {
    pub repository: u64,
    generation: [u8; 16],
}

pub(crate) type CommentCollection = [u8; 16];

#[derive(Serialize, Deserialize)]
struct CommentsSnapshot {
    collection: CommentCollection,
    comments: Vec<IssueComment>,
    has_next: bool,
}

pub(crate) struct CachedConversation {
    pub reading: ConversationReading,
    pub scope: ReadingScope,
    pub authorization: CacheAuthorization,
    pub comments: Option<CachedComments>,
}

pub(crate) struct CachedComments {
    pub comments: Vec<IssueComment>,
    pub partial: bool,
}

pub(crate) struct Refresh<T, E> {
    pub result: Result<T, E>,
    pub no_store: bool,
}

pub(crate) struct LiveConversation {
    pub conversation: Conversation,
    pub repository: Option<u64>,
    pub cache_scope: Option<ReadingScope>,
}

pub(crate) struct LiveComments {
    pub page: Scoped<IssueCommentsPage>,
    pub collection: Option<CommentCollection>,
}

#[derive(Clone)]
pub(crate) struct ConversationSession {
    session: RepositorySession,
    target: Target,
    locator: IssueLocator,
    cache: Option<AccountCache>,
}

struct BodyWrite {
    cache: AccountCache,
    route: CacheWrite,
    previous: Option<ReadingScope>,
    body: Option<CacheWrite>,
    proof: Option<Revalidation>,
    authorization: Option<CacheAuthorization>,
}

impl ConversationSession {
    pub fn new(session: RepositorySession, target: Target, locator: IssueLocator) -> Self {
        let cache = session.service().cache(session.context());
        Self {
            session,
            target,
            locator,
            cache,
        }
    }

    pub fn verify(&self) -> bool {
        self.session.verify()
    }

    fn key(
        &self,
        resource: &str,
        scope: Option<&ReadingScope>,
        page: impl Serialize,
    ) -> Option<CacheKey> {
        let config = self.session.client().config();
        let parameters = serde_json::json!([
            self.locator.normalized_key(),
            config.base_url,
            config.accept,
            config.api_version,
            scope.map(|scope| scope.generation),
            page,
        ]);
        CacheKey::mutable(
            scope.map(|scope| scope.repository),
            true,
            resource,
            &parameters.to_string(),
            "pr-conversation-v1",
            VERSION,
        )
        .ok()
    }

    fn route_key(&self) -> Option<CacheKey> {
        self.key("pr-conversation-route", None, ())
    }

    fn body_key(&self, scope: &ReadingScope) -> Option<CacheKey> {
        self.key("pr-conversation-body", Some(scope), ())
    }

    fn comments_key(
        &self,
        scope: &ReadingScope,
        per_page: u32,
        page: u32,
        collection: Option<CommentCollection>,
    ) -> Option<CacheKey> {
        self.key(
            "pr-conversation-comments",
            Some(scope),
            (per_page, page, collection),
        )
    }

    pub async fn cached(
        &self,
        limits: IssueDetailLimits,
        cancel: CancellationToken,
    ) -> Option<CachedConversation> {
        let cache = self.cache.clone()?;
        let session = self.clone();
        let token = cancel.clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            let route_auth = cache.store().authorization(cache.scope(), None)?;
            let scope: ReadingScope = load(&cache, &session.route_key()?)?;
            let authorization = cache
                .store()
                .authorization(cache.scope(), Some(scope.repository))?;
            let reading = load(&cache, &session.body_key(&scope)?)?;
            let mut cached_comments = None;
            let mut collection = None;
            let mut seen = HashSet::new();
            for page in 1..=MAX_REPLAY_PAGES {
                if token.is_cancelled() || !authorization.is_current() || !route_auth.is_current() {
                    return None;
                }
                let key =
                    session.comments_key(&scope, limits.comments_per_page, page, collection)?;
                let Some(snapshot) = load::<CommentsSnapshot>(&cache, &key) else {
                    break;
                };
                if collection.is_some_and(|id| id != snapshot.collection) {
                    break;
                }
                collection = Some(snapshot.collection);
                let comments = cached_comments.get_or_insert_with(|| CachedComments {
                    comments: Vec::new(),
                    partial: true,
                });
                comments.comments.extend(
                    snapshot
                        .comments
                        .into_iter()
                        .filter(|comment| seen.insert(comment.id)),
                );
                comments.partial = snapshot.has_next;
                if !snapshot.has_next {
                    break;
                }
            }
            route_auth.is_current().then_some(CachedConversation {
                reading,
                scope,
                authorization,
                comments: cached_comments,
            })
        })
        .await
        .ok()
        .flatten()?;
        (self.verify() && !cancel.is_cancelled() && snapshot.authorization.is_current())
            .then_some(snapshot)
    }

    async fn body_write(&self) -> Option<BodyWrite> {
        let cache = self.cache.clone()?;
        let session = self.clone();
        tokio::task::spawn_blocking(move || {
            let route_key = session.route_key()?;
            let previous = load::<ReadingScope>(&cache, &route_key);
            let route = cache.store().begin_write(cache.scope(), &route_key).ok()?;
            let body = previous.as_ref().and_then(|scope| {
                cache
                    .store()
                    .begin_write(cache.scope(), &session.body_key(scope)?)
                    .ok()
            });
            let proof = previous.as_ref().and_then(|scope| {
                session
                    .session
                    .service()
                    .begin_revalidation(session.session.context(), Some(scope.repository))
                    .ok()
            });
            let authorization = previous.as_ref().and_then(|scope| {
                cache
                    .store()
                    .authorization(cache.scope(), Some(scope.repository))
            });
            Some(BodyWrite {
                cache,
                route,
                previous,
                body,
                proof,
                authorization,
            })
        })
        .await
        .ok()
        .flatten()
    }

    pub async fn refresh(
        &self,
        cancel: CancellationToken,
    ) -> Refresh<LiveConversation, GitHubError> {
        let write = self.body_write().await;
        let authorization = write.as_ref().and_then(|write| write.authorization.clone());
        let (client, transport) = self.read_client();
        let result = self
            .session
            .read(cancel.clone(), |pat, cancel| async move {
                pr_conversation::fetch(&client, &pat, self.target.clone(), cancel).await
            })
            .await;
        let no_store = transport
            .as_ref()
            .is_some_and(|transport| transport.no_store());
        let result = match result {
            Ok(scoped) => {
                let (repository, reading) = scoped.value;
                let cache_scope = if let Some(write) = write {
                    self.finish_body_write(write, repository, &reading, no_store, &cancel)
                        .await
                } else {
                    None
                };
                Ok(LiveConversation {
                    conversation: Conversation {
                        target: self.target.clone(),
                        reading,
                    },
                    repository,
                    cache_scope,
                })
            }
            Err(error) => {
                if matches!(error, GitHubError::Unauthorized) {
                    let _ = self
                        .session
                        .service()
                        .credential_rejected(self.session.context())
                        .await;
                } else if (no_store || access_denied(&error))
                    && let Some(write) = write
                {
                    self.forget_route(write, cancel.clone()).await;
                }
                Err(error)
            }
        };
        let result = result.and_then(|live| {
            if authorization
                .as_ref()
                .is_some_and(|stamp| !stamp.is_current())
            {
                return Err(GitHubError::StaleContext);
            }
            Ok(live)
        });
        Refresh {
            result: self.check(&cancel).and(result),
            no_store,
        }
    }

    async fn finish_body_write(
        &self,
        mut write: BodyWrite,
        repository: Option<u64>,
        reading: &ConversationReading,
        no_store: bool,
        cancel: &CancellationToken,
    ) -> Option<ReadingScope> {
        if no_store || repository.is_none() {
            self.forget_route(write, cancel.clone()).await;
            return None;
        }
        self.check(cancel).ok()?;
        let repository = repository?;
        let previous = write
            .previous
            .filter(|scope| scope.repository == repository);
        if previous.is_some()
            && let Some(proof) = write.proof.take()
        {
            self.session
                .service()
                .permission_confirmed(self.session.context(), proof)
                .await
                .ok()?;
        }
        let session = self.clone();
        let reading = reading.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            session.check(&cancel).ok()?;
            let cache = &write.cache;
            let (scope, ticket) = if let Some(scope) = previous {
                (scope, write.body?)
            } else {
                // New route bindings have a unique body key. If the pre-request route
                // ticket has gone stale, the unreferenced body cannot be read on restart.
                let scope = ReadingScope {
                    repository,
                    generation: uuid::Uuid::new_v4().into_bytes(),
                };
                let ticket = cache
                    .store()
                    .begin_write(cache.scope(), &session.body_key(&scope)?)
                    .ok()?;
                (scope, ticket)
            };
            save(cache, ticket, &reading, CacheCompleteness::Complete)?;
            save(cache, write.route, &scope, CacheCompleteness::Complete)?;
            Some(scope)
        })
        .await
        .ok()
        .flatten()
    }

    async fn forget_route(&self, write: BodyWrite, cancel: CancellationToken) {
        let session = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if session.check(&cancel).is_ok() {
                discard(&write.cache, write.route);
                if let Some(ticket) = write.body {
                    discard(&write.cache, ticket);
                }
            }
        })
        .await;
    }

    pub async fn refresh_comments(
        &self,
        scope: Option<ReadingScope>,
        collection: Option<CommentCollection>,
        cursor: Option<IssueCommentsCursor>,
        limits: IssueDetailLimits,
        cancel: CancellationToken,
        persist: bool,
    ) -> Refresh<LiveComments, IssueDetailError> {
        let page = cursor.as_ref().map_or(1, IssueCommentsCursor::page);
        let write = if let (Some(cache), Some(scope)) = (self.cache.clone(), scope.as_ref()) {
            let route_key = self.route_key();
            let page_key = (page == 1 || (page <= MAX_REPLAY_PAGES && collection.is_some()))
                .then(|| self.comments_key(scope, limits.comments_per_page, page, collection))
                .flatten();
            let scope = scope.clone();
            tokio::task::spawn_blocking(move || {
                let route_key = route_key?;
                if load::<ReadingScope>(&cache, &route_key).as_ref() != Some(&scope) {
                    return None;
                }
                let route = cache.store().begin_write(cache.scope(), &route_key).ok()?;
                let ticket =
                    page_key.and_then(|key| cache.store().begin_write(cache.scope(), &key).ok());
                Some((cache, route, ticket))
            })
            .await
            .ok()
            .flatten()
        } else {
            None
        };
        let (client, transport) = self.read_client();
        let result = self
            .session
            .read(cancel.clone(), |pat, cancel| async move {
                issue_detail::fetch_comments_page(
                    &client,
                    &pat,
                    &self.locator,
                    cursor.as_ref(),
                    &limits,
                    cancel,
                )
                .await
            })
            .await;
        let no_store = transport
            .as_ref()
            .is_some_and(|transport| transport.no_store());
        let denied =
            matches!(&result, Err(error) if error.is_missing() || error.is_auth_or_permission());
        if matches!(
            &result,
            Err(IssueDetailError::GitHub(GitHubError::Unauthorized))
        ) {
            let _ = self
                .session
                .service()
                .credential_rejected(self.session.context())
                .await;
        }
        let collection = if page == 1 {
            Some(uuid::Uuid::new_v4().into_bytes())
        } else {
            collection
        };
        if let Some((cache, route, ticket)) = write {
            let snapshot = result.as_ref().ok().and_then(|reply| {
                Some(CommentsSnapshot {
                    collection: collection?,
                    comments: reply.value.value.comments.clone(),
                    has_next: reply.value.value.next.is_some(),
                })
            });
            let session = self.clone();
            let token = cancel.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if session.check(&token).is_err() {
                    return;
                }
                if denied {
                    discard(&cache, route);
                }
                if let Some(ticket) = ticket {
                    if denied || no_store {
                        discard(&cache, ticket);
                    } else if persist && let Some(snapshot) = snapshot {
                        let completeness = if snapshot.has_next {
                            CacheCompleteness::Partial {
                                reason: "more comments available".into(),
                            }
                        } else {
                            CacheCompleteness::Complete
                        };
                        save(&cache, ticket, &snapshot, completeness);
                    }
                }
            })
            .await;
        }
        Refresh {
            result: self
                .check(&cancel)
                .map_err(IssueDetailError::from)
                .and(result)
                .map(|reply| LiveComments {
                    page: Scoped {
                        context: reply.context,
                        value: reply.value.value,
                    },
                    collection,
                }),
            no_store,
        }
    }

    fn check(&self, cancel: &CancellationToken) -> Result<(), GitHubError> {
        if !self.verify() {
            return Err(GitHubError::StaleContext);
        }
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled);
        }
        Ok(())
    }

    fn read_client(&self) -> (Arc<GitHubClient>, Option<Arc<PageTransport>>) {
        let client = self.session.client();
        if self.cache.is_none() {
            return (client.clone(), None);
        }
        // Observe cache directives even when the adapter rejects the HTTP/GraphQL response.
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

fn load<T: DeserializeOwned>(cache: &AccountCache, key: &CacheKey) -> Option<T> {
    let CacheRead::Hit(lease) = cache.store().read(cache.scope(), key).ok()? else {
        return None;
    };
    serde_json::from_slice(&lease.snapshot().ok().flatten()?.bytes).ok()
}

fn metadata(completeness: CacheCompleteness, no_store: bool) -> CacheMetadata {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64);
    CacheMetadata {
        representation_version: VERSION,
        fetched_at_ms: now,
        verified_at_ms: now,
        completeness,
        etag: None,
        last_modified: None,
        cache_control: no_store.then(|| "no-store".into()),
    }
}

fn save(
    cache: &AccountCache,
    ticket: CacheWrite,
    snapshot: &impl Serialize,
    completeness: CacheCompleteness,
) -> Option<()> {
    let bytes = serde_json::to_vec(snapshot).ok()?;
    let outcome = cache
        .store()
        .put(ticket, &bytes, metadata(completeness, false))
        .ok()?;
    matches!(outcome, WriteOutcome::Stored).then_some(())
}

fn discard(cache: &AccountCache, ticket: CacheWrite) {
    let _ = cache
        .store()
        .put(ticket, &[], metadata(CacheCompleteness::Complete, true));
}
