//! Persistent page snapshots composed with the existing authenticated adapters.
//! Formal Shell assembly is deliberately a separate task.
use super::{PageReadSession, ProfileReadme, RepositoryReadContext, Scoped};
use crate::{
    domain::{
        cache::{CacheCompleteness, CacheMetadata, CacheMiss},
        error::GitHubError,
    },
    github::{
        client::GitHubClient,
        home::cache::{Exchange, PageTransport},
    },
    network::cancel::CancellationToken,
    storage::cache::{CacheError, CacheKey, CacheRead, CacheScope, CacheStore},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[cfg(test)]
mod authorization_tests;
#[cfg(test)]
mod capacity_tests;
#[cfg(test)]
mod description_tests;
#[cfg(test)]
mod graphql_feed_tests;
#[cfg(test)]
mod localhost_tests;
#[cfg(test)]
mod readme_tests;
mod reads;
#[cfg(test)]
mod revocation_tests;
#[cfg(test)]
mod tests;

const VERSION: u32 = 1;
#[derive(Debug, Clone, Copy)]
pub enum PageReadMode {
    Online,
    Offline,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOrigin {
    Network,
    HistoricalSnapshot,
}
#[derive(Debug)]
pub enum PageValue<T> {
    Value {
        value: T,
        origin: PageOrigin,
        metadata: CacheMetadata,
    },
    Miss(CacheMiss),
}
#[derive(Debug)]
pub enum PageCacheError<E> {
    Read(E),
    Cache(CacheError),
    Context(GitHubError),
}
#[derive(Serialize, Deserialize)]
struct SnapshotV1 {
    version: u32,
    exchanges: Vec<Exchange>,
}

#[derive(Clone)]
pub struct CachedPageReadSession {
    session: PageReadSession,
    store: CacheStore,
    scope: CacheScope,
}
impl CachedPageReadSession {
    /// The caller supplies the app's store clone and saved-account scope.
    pub fn new(
        session: PageReadSession,
        store: CacheStore,
        scope: CacheScope,
    ) -> Result<Self, CacheError> {
        if scope.account() != &session.context().auth().account || !session.verify() {
            return Err(CacheError::AccessDenied);
        }
        Ok(Self {
            session,
            store,
            scope,
        })
    }
    pub fn context(&self) -> &RepositoryReadContext {
        self.session.context()
    }
    #[cfg(test)]
    pub(crate) fn scope(&self) -> &CacheScope {
        &self.scope
    }
    pub fn verify(&self) -> bool {
        self.session.verify()
    }
    fn guard<E>(&self, cancel: &CancellationToken) -> Result<(), PageCacheError<E>> {
        if !self.verify() {
            return Err(PageCacheError::Context(GitHubError::StaleContext));
        }
        if cancel.is_cancelled() {
            return Err(PageCacheError::Context(GitHubError::Cancelled));
        }
        Ok(())
    }
    fn key(
        &self,
        repository: Option<u64>,
        resource: &str,
        parameters: serde_json::Value,
        partial: bool,
    ) -> Result<CacheKey, CacheError> {
        let config = self.session.page_client().config();
        let request = serde_json::json!({"login":self.session.login().as_str(), "parameters":parameters, "base":config.base_url, "accept":config.accept, "api":config.api_version, "partial":partial}).to_string();
        CacheKey::mutable(
            repository,
            true,
            resource,
            &request,
            "page-response-set-v1",
            VERSION,
        )
    }
    async fn blocking<T: Send + 'static, E>(
        &self,
        work: impl FnOnce(CacheStore, CacheScope) -> Result<T, CacheError> + Send + 'static,
    ) -> Result<T, PageCacheError<E>> {
        let store = self.store.clone();
        let scope = self.scope.clone();
        let session = self.session.clone();
        let result = tokio::task::spawn_blocking(move || {
            session
                .cache_access(|| work(store, scope))
                .map_err(|_| CacheError::Stale)?
        })
        .await;
        if !self.verify() {
            return Err(PageCacheError::Context(GitHubError::StaleContext));
        }
        result
            .map_err(|_| PageCacheError::Cache(CacheError::Io))?
            .map_err(PageCacheError::Cache)
    }
    #[allow(clippy::too_many_arguments)]
    async fn read<T, E, F, Fut>(
        &self,
        mode: PageReadMode,
        repository: Option<u64>,
        resource: &str,
        parameters: serde_json::Value,
        cancel: CancellationToken,
        read: F,
        completeness: impl FnOnce(&T) -> CacheCompleteness,
    ) -> Result<Scoped<PageValue<T>>, PageCacheError<E>>
    where
        F: FnOnce(PageReadSession) -> Fut,
        Fut: Future<Output = Result<Scoped<T>, E>>,
    {
        self.guard(&cancel)?;
        let key = self
            .key(repository, resource, parameters.clone(), false)
            .map_err(PageCacheError::Cache)?;
        let partial = self
            .key(repository, resource, parameters, true)
            .map_err(PageCacheError::Cache)?;
        let (transport, cached, tickets, lease) = match mode {
            PageReadMode::Offline => {
                let loaded = self
                    .blocking(move |store, scope| {
                        let mut missing = CacheMiss::NotCached;
                        for key in [key, partial] {
                            match store.read(&scope, &key)? {
                                CacheRead::Miss(reason) => {
                                    if reason != CacheMiss::NotCached {
                                        missing = reason;
                                    }
                                }
                                CacheRead::Hit(lease) => {
                                    if let Some(snapshot) = lease.snapshot()? {
                                        match serde_json::from_slice::<SnapshotV1>(&snapshot.bytes)
                                        {
                                            Ok(dto) if dto.version == VERSION => {
                                                return Ok(Ok((dto, snapshot.metadata, lease)));
                                            }
                                            Ok(_) => missing = CacheMiss::RepresentationVersion,
                                            Err(_) => missing = CacheMiss::Corrupt,
                                        }
                                    }
                                }
                            }
                        }
                        Ok(Err(missing))
                    })
                    .await?;
                self.guard(&cancel)?;
                let (dto, metadata, lease) = match loaded {
                    Ok(value) => value,
                    Err(reason) => {
                        return Ok(Scoped {
                            context: self.context().clone(),
                            value: PageValue::Miss(reason),
                        });
                    }
                };
                (
                    Arc::new(PageTransport::replay(dto.exchanges)),
                    Some(metadata),
                    None,
                    Some(lease),
                )
            }
            PageReadMode::Online => {
                let tickets = self
                    .blocking(move |store, scope| {
                        Ok((
                            store.begin_write(&scope, &key)?,
                            store.begin_write(&scope, &partial)?,
                        ))
                    })
                    .await?;
                (
                    Arc::new(PageTransport::recording(
                        self.session.page_client().transport().clone(),
                    )),
                    None,
                    Some(tickets),
                    None,
                )
            }
        };
        let client = Arc::new(GitHubClient::new(
            self.session.page_client().config().clone(),
            transport.clone(),
        ));
        let result = read(self.session.with_page_client(client)).await;
        if !self.verify() {
            return Err(PageCacheError::Context(GitHubError::StaleContext));
        }
        if transport.rejected() {
            self.blocking(move |store, scope| {
                store.access_event(
                    &scope,
                    crate::storage::cache::AccessEvent::CredentialRejected,
                )
            })
            .await?;
            return Err(PageCacheError::Context(GitHubError::Unauthorized));
        }
        if let Some(lease) = lease {
            let valid = self
                .blocking(move |_, _| Ok(lease.snapshot()?.is_some()))
                .await?;
            if !valid {
                return Err(PageCacheError::Cache(CacheError::AccessDenied));
            }
        }
        let stamp = now();
        // no-store invalidates both the complete and partial slots even when a
        // later dependency fails. Tickets prevent an older response deleting new data.
        if transport.no_store()
            && let Some((complete, partial)) = tickets
        {
            let metadata = metadata(stamp, CacheCompleteness::Complete, true);
            self.blocking(move |store, _| {
                store.put(complete, &[], metadata.clone())?;
                store.put(partial, &[], metadata)?;
                Ok(())
            })
            .await?;
            let value = result.map_err(PageCacheError::Read)?.value;
            self.guard(&cancel)?;
            Ok(Scoped {
                context: self.context().clone(),
                value: PageValue::Value {
                    metadata: self::metadata(stamp, completeness(&value), true),
                    value,
                    origin: PageOrigin::Network,
                },
            })
        } else {
            self.guard(&cancel)?;
            let value = result.map_err(PageCacheError::Read)?.value;
            let observed = completeness(&value);
            let mut metadata = cached.unwrap_or_else(|| metadata(stamp, observed.clone(), false));
            // Replay uses the caller's current Feed deduplication set. A newly
            // exposed unresolved event must never inherit Complete provenance.
            if observed != CacheCompleteness::Complete {
                metadata.completeness = observed.clone();
            }
            let origin = if tickets.is_some() {
                PageOrigin::Network
            } else {
                PageOrigin::HistoricalSnapshot
            };
            if let Some((complete, partial)) = tickets {
                let ticket = if observed == CacheCompleteness::Complete {
                    complete
                } else {
                    partial
                };
                let exchanges = transport.exchanges();
                let stored_metadata = metadata.clone();
                self.blocking(move |store, _| {
                    let bytes = serde_json::to_vec(&SnapshotV1 {
                        version: VERSION,
                        exchanges,
                    })
                    .map_err(|_| CacheError::Corrupt)?;
                    match store.put(ticket, &bytes, stored_metadata) {
                        Ok(_) | Err(CacheError::TooLarge | CacheError::Capacity) => Ok(()),
                        Err(error) => Err(error),
                    }
                })
                .await?;
            }
            self.guard(&cancel)?;
            Ok(Scoped {
                context: self.context().clone(),
                value: PageValue::Value {
                    value,
                    origin,
                    metadata,
                },
            })
        }
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn metadata(stamp: i64, completeness: CacheCompleteness, no_store: bool) -> CacheMetadata {
    CacheMetadata {
        representation_version: VERSION,
        fetched_at_ms: stamp,
        verified_at_ms: stamp,
        completeness,
        etag: None,
        last_modified: None,
        cache_control: no_store.then(|| "no-store".into()),
    }
}
fn partial(complete: bool, reason: &str) -> CacheCompleteness {
    if complete {
        CacheCompleteness::Complete
    } else {
        CacheCompleteness::Partial {
            reason: reason.into(),
        }
    }
}
