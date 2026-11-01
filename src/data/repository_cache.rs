//! Persistent, account-bound repository observations. The online session stays
//! unchanged; this adapter adds explicit snapshot provenance and offline reads.
//! All storage work runs on Tokio's blocking pool, never on the caller/UI thread.

use super::repository::{RepositorySession, Scoped};
use crate::content::{PreparedContent, prepare_content};
use crate::domain::blob::{BlobBody, BlobIdentity, BlobLimits, BlobRead};
use crate::domain::cache::{CacheCompleteness, CacheMetadata, CacheMiss, CacheSnapshot};
use crate::domain::error::GitHubError;
use crate::domain::repo::{RepoOwner, RepoVisibility};
use crate::domain::repository::*;
use crate::github::{self, RepositoryLimits};
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{
    CacheKey, CacheRead, CacheScope, CacheStore, ObjectKind, Revalidation,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 1;
const DTO: &str = "githux-repository-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryCacheMode {
    OnlinePreferred,
    OfflineOnly,
    ForceRefresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryCacheOrigin {
    Network,
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryCacheProvenance {
    pub origin: RepositoryCacheOrigin,
    pub fetched_at_ms: i64,
    pub verified_at_ms: i64,
    pub completeness: CacheCompleteness,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepositoryCacheError {
    #[error(transparent)]
    Online(#[from] RepositoryError),
    #[error("cached repository value is unavailable: {0:?}")]
    Miss(CacheMiss),
    #[error("cache authorization gate denies access")]
    AccessDenied,
    #[error("repository cache storage failed")]
    Storage,
}

impl From<GitHubError> for RepositoryCacheError {
    fn from(error: GitHubError) -> Self {
        Self::Online(error.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedRepositoryRead<T> {
    pub value: T,
    pub provenance: RepositoryCacheProvenance,
}

#[derive(Clone)]
pub struct CachedRepositorySession {
    session: RepositorySession,
    cache: CacheStore,
    scope: CacheScope,
}

impl std::fmt::Debug for CachedRepositorySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedRepositorySession")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl CachedRepositorySession {
    pub fn new(
        session: RepositorySession,
        cache: CacheStore,
        scope: CacheScope,
    ) -> Result<Self, RepositoryError> {
        if scope.account() != &session.context().auth().account {
            return Err(RepositoryError::InvalidInput {
                field: "cache scope account",
            });
        }
        Ok(Self {
            session,
            cache,
            scope,
        })
    }

    pub fn context(&self) -> &super::RepositoryReadContext {
        self.session.context()
    }

    pub fn verify(&self) -> bool {
        self.session.verify()
    }

    pub fn inner(&self) -> &RepositorySession {
        &self.session
    }

    pub async fn repository(
        &self,
        repository: &RepositoryIdentity,
        limits: &RepositoryLimits,
        mode: RepositoryCacheMode,
        cancel: CancellationToken,
    ) -> Result<Scoped<CachedRepositoryRead<RepositoryDetails>>, RepositoryCacheError> {
        let key = mutable_key(
            repository.id(),
            "repository-details",
            "canonical-repository",
        )?;
        if mode == RepositoryCacheMode::OfflineOnly {
            let repository = repository.clone();
            let cached = self
                .offline(&key, cancel, move |value| {
                    decode_details(value, &repository)
                })
                .await?;
            return self.scoped(cached);
        }
        let prior = if mode == RepositoryCacheMode::OnlinePreferred {
            let repository = repository.clone();
            self.online_prior(&key, cancel.clone(), move |value| {
                decode_details(value, &repository)
            })
            .await?
        } else {
            None
        };
        let validator = prior
            .as_ref()
            .and_then(|value| validators(&value.payload.metadata));
        let permit = self
            .before_network(&key, repository.id(), cancel.clone())
            .await?;
        let result = self
            .session
            .read_with_metadata(
                cancel.clone(),
                validator,
                async move |pat, validator, cancel| {
                    github::repository::fetch_repository_with_metadata(
                        self.session.client(),
                        &pat,
                        repository.locator(),
                        limits,
                        validator.as_ref(),
                        cancel,
                    )
                    .await
                },
            )
            .await;
        if let Ok(scoped) = &result
            && let Some(details) = &scoped.value.0
            && details.identity != *repository
        {
            return Err(RepositoryError::InvalidResponse {
                message: "repository identity differs from the authorized request".into(),
            }
            .into());
        }
        let result = self
            .finish_network(
                result,
                key,
                permit,
                prior,
                cancel.clone(),
                encode_details,
                completeness_repository,
            )
            .await;
        retry_unvalidated_304(result, mode, || {
            Box::pin(self.repository(
                repository,
                limits,
                RepositoryCacheMode::ForceRefresh,
                cancel,
            ))
        })
        .await
    }

    pub async fn branches(
        &self,
        repository: &RepositoryIdentity,
        limits: &RepositoryLimits,
        mode: RepositoryCacheMode,
        cancel: CancellationToken,
    ) -> Result<Scoped<CachedRepositoryRead<BranchesOutcome>>, RepositoryCacheError> {
        let key = mutable_key(
            repository.id(),
            "branches",
            &format!(
                "all-pages-v1;pages={};entries={};response={}",
                limits.branch_pages, limits.branch_entries, limits.response_bytes,
            ),
        )?;
        if mode == RepositoryCacheMode::OfflineOnly {
            let repository = repository.clone();
            let cached = self
                .offline(&key, cancel, move |value| {
                    decode_branches(value, &repository)
                })
                .await?;
            return self.scoped(cached);
        }
        let prior = if mode == RepositoryCacheMode::OnlinePreferred {
            let repository = repository.clone();
            self.online_prior(&key, cancel.clone(), move |value| {
                decode_branches(value, &repository)
            })
            .await?
        } else {
            None
        };
        let permit = self
            .before_network(&key, repository.id(), cancel.clone())
            .await?;
        let result = self
            .session
            .read_with_metadata(cancel.clone(), None, async move |pat, _, cancel| {
                github::branches::fetch_branches_with_metadata(
                    self.session.client(),
                    &pat,
                    repository,
                    limits,
                    cancel,
                )
                .await
                .map(|(outcome, metadata)| (Some(outcome), metadata))
            })
            .await;
        self.finish_network(
            result,
            key,
            permit,
            prior,
            cancel,
            move |outcome| encode_branches(outcome, repository),
            completeness_branches,
        )
        .await
    }

    pub async fn revision(
        &self,
        repository: &RepositoryIdentity,
        revision: RevisionSpec,
        limits: &RepositoryLimits,
        mode: RepositoryCacheMode,
        cancel: CancellationToken,
    ) -> Result<Scoped<CachedRepositoryRead<RepoSnapshot>>, RepositoryCacheError> {
        let (key, fixed) = match &revision {
            RevisionSpec::Ref(reference) => (
                mutable_key(repository.id(), "revision-ref", reference.as_str())?,
                false,
            ),
            RevisionSpec::Commit(sha) => {
                (fixed_key(repository.id(), ObjectKind::Commit, sha)?, true)
            }
        };
        if fixed && mode != RepositoryCacheMode::ForceRefresh {
            let repository = repository.clone();
            let revision = revision.clone();
            if let Some(cached) = self
                .prior(&key, cancel.clone(), move |value| {
                    decode_snapshot(value, &repository, &revision)
                })
                .await?
            {
                return self.scoped(CachedRepositoryRead {
                    value: cached.value.clone(),
                    provenance: provenance(
                        cached.payload.metadata.clone(),
                        RepositoryCacheOrigin::Snapshot,
                    ),
                });
            }
        }
        if mode == RepositoryCacheMode::OfflineOnly {
            let repository = repository.clone();
            let revision = revision.clone();
            let cached = self
                .offline(&key, cancel, move |value| {
                    decode_snapshot(value, &repository, &revision)
                })
                .await?;
            return self.scoped(cached);
        }
        let prior = if !fixed && mode == RepositoryCacheMode::OnlinePreferred {
            let repository = repository.clone();
            let revision = revision.clone();
            self.online_prior(&key, cancel.clone(), move |value| {
                decode_snapshot(value, &repository, &revision)
            })
            .await?
        } else {
            None
        };
        let validator = prior
            .as_ref()
            .and_then(|value| validators(&value.payload.metadata));
        let permit = self
            .before_network(&key, repository.id(), cancel.clone())
            .await?;
        let network_revision = revision.clone();
        let result = self
            .session
            .read_with_metadata(
                cancel.clone(),
                validator,
                async move |pat, validator, cancel| {
                    github::revision::resolve_revision_with_metadata(
                        self.session.client(),
                        &pat,
                        repository,
                        network_revision,
                        limits,
                        validator.as_ref(),
                        cancel,
                    )
                    .await
                },
            )
            .await;
        let result = self
            .finish_network(
                result,
                key,
                permit,
                prior,
                cancel.clone(),
                move |snapshot| encode_snapshot(snapshot, repository),
                |_| CacheCompleteness::Complete,
            )
            .await;
        retry_unvalidated_304(result, mode, || {
            Box::pin(self.revision(
                repository,
                revision,
                limits,
                RepositoryCacheMode::ForceRefresh,
                cancel,
            ))
        })
        .await
    }

    pub async fn directory(
        &self,
        directory: &SnapshotDirectory,
        limits: &RepositoryLimits,
        mode: RepositoryCacheMode,
        cancel: CancellationToken,
    ) -> Result<Scoped<CachedRepositoryRead<DirectoryTree>>, RepositoryCacheError> {
        let key = CacheKey::fixed(
            directory.repository().id(),
            true,
            ObjectKind::Tree,
            directory.tree().as_str(),
            &format!("tree-v1;entry-limit={}", limits.tree_entries),
            VERSION,
        )
        .map_err(|_| RepositoryError::InvalidInput { field: "cache key" })?;
        if mode != RepositoryCacheMode::ForceRefresh {
            let directory = directory.clone();
            if let Some(cached) = self
                .prior(&key, cancel.clone(), move |value| {
                    decode_tree(value, &directory)
                })
                .await?
            {
                return self.scoped(CachedRepositoryRead {
                    value: cached.value.clone(),
                    provenance: provenance(
                        cached.payload.metadata.clone(),
                        RepositoryCacheOrigin::Snapshot,
                    ),
                });
            }
        }
        if mode == RepositoryCacheMode::OfflineOnly {
            let directory = directory.clone();
            let cached = self
                .offline(&key, cancel, move |value| decode_tree(value, &directory))
                .await?;
            return self.scoped(cached);
        }
        let permit = self
            .before_network(&key, directory.repository().id(), cancel.clone())
            .await?;
        let result = self
            .session
            .read_with_metadata(cancel.clone(), None, async move |pat, _, cancel| {
                github::tree::fetch_directory_with_metadata(
                    self.session.client(),
                    &pat,
                    directory,
                    limits,
                    cancel,
                )
                .await
                .map(|(tree, metadata)| (Some(tree), metadata))
            })
            .await;
        self.finish_network(
            result,
            key,
            permit,
            None,
            cancel,
            move |tree| encode_tree(tree, directory),
            completeness_tree,
        )
        .await
    }

    pub async fn readme(
        &self,
        snapshot: &RepoSnapshot,
        limits: &RepositoryLimits,
        mode: RepositoryCacheMode,
        cancel: CancellationToken,
    ) -> Result<Scoped<CachedRepositoryRead<ReadmeDiscovery>>, RepositoryCacheError> {
        // Mutable identity keeps Found/Absent separate from a Git Commit DTO;
        // the request representation still includes the immutable commit SHA.
        let key = mutable_key(
            snapshot.repository().id(),
            "readme-discovery",
            snapshot.commit().as_str(),
        )?;
        if mode != RepositoryCacheMode::ForceRefresh {
            let snapshot = snapshot.clone();
            if let Some(cached) = self
                .prior(&key, cancel.clone(), move |value| {
                    decode_readme(value, &snapshot)
                })
                .await?
            {
                return self.scoped(CachedRepositoryRead {
                    value: cached.value.clone(),
                    provenance: provenance(
                        cached.payload.metadata.clone(),
                        RepositoryCacheOrigin::Snapshot,
                    ),
                });
            }
        }
        if mode == RepositoryCacheMode::OfflineOnly {
            let snapshot = snapshot.clone();
            let cached = self
                .offline(&key, cancel, move |value| decode_readme(value, &snapshot))
                .await?;
            return self.scoped(cached);
        }
        let permit = self
            .before_network(&key, snapshot.repository().id(), cancel.clone())
            .await?;
        let result = self
            .session
            .read_with_metadata(cancel.clone(), None, async move |pat, _, cancel| {
                github::readme::discover_readme_with_metadata(
                    self.session.client(),
                    &pat,
                    snapshot,
                    limits,
                    cancel,
                )
                .await
                .map(|(readme, metadata)| (Some(readme), metadata))
            })
            .await;
        self.finish_network(
            result,
            key,
            permit,
            None,
            cancel,
            move |readme| encode_readme(readme, snapshot),
            |_| CacheCompleteness::Complete,
        )
        .await
    }

    pub async fn blob(
        &self,
        identity: &BlobIdentity,
        limits: &BlobLimits,
        mode: RepositoryCacheMode,
        cancel: CancellationToken,
    ) -> Result<Scoped<CachedRepositoryRead<BlobRead>>, RepositoryCacheError> {
        let key = fixed_key(
            identity.repository().id(),
            ObjectKind::Blob,
            identity.blob(),
        )?;
        if mode != RepositoryCacheMode::ForceRefresh {
            let identity = identity.clone();
            let limits = *limits;
            if let Some(cached) = self
                .prior(&key, cancel.clone(), move |value| {
                    decode_blob(value, &identity, &limits)
                })
                .await?
            {
                return self.scoped(CachedRepositoryRead {
                    value: cached.value.clone(),
                    provenance: provenance(
                        cached.payload.metadata.clone(),
                        RepositoryCacheOrigin::Snapshot,
                    ),
                });
            }
        }
        if mode == RepositoryCacheMode::OfflineOnly {
            let identity = identity.clone();
            let limits = *limits;
            let cached = self
                .offline(&key, cancel, move |value| {
                    decode_blob(value, &identity, &limits)
                })
                .await?;
            return self.scoped(cached);
        }
        let prior = if mode == RepositoryCacheMode::OnlinePreferred {
            let identity = identity.clone();
            let limits = *limits;
            self.online_prior(&key, cancel.clone(), move |value| {
                decode_blob(value, &identity, &limits)
            })
            .await?
        } else {
            None
        };
        let validator = prior
            .as_ref()
            .and_then(|value| validators(&value.payload.metadata));
        let mut permit = self
            .before_network(&key, identity.repository().id(), cancel.clone())
            .await?;
        let result = self
            .session
            .read_with_metadata(
                cancel.clone(),
                validator,
                async move |pat, validator, cancel| {
                    github::blob::fetch_blob_with_metadata(
                        self.session.client(),
                        &pat,
                        identity,
                        limits,
                        validator.as_ref(),
                        cancel,
                    )
                    .await
                },
            )
            .await;
        if let Ok(scoped) = &result
            && let Some(blob) = &scoped.value.0
            && matches!(blob.body(), BlobBody::TooLarge { .. })
        {
            let metadata = &scoped.value.1;
            self.confirm(&mut permit, metadata).await?;
            if !metadata_for(metadata, CacheCompleteness::Complete).permits_storage() {
                self.store(
                    permit.ticket,
                    &[],
                    metadata_for(metadata, CacheCompleteness::Complete),
                    cancel,
                )
                .await?;
            }
            return self.scoped(CachedRepositoryRead {
                value: blob.clone(),
                provenance: provenance(
                    metadata_for(metadata, CacheCompleteness::Complete),
                    RepositoryCacheOrigin::Network,
                ),
            });
        }
        let result = self
            .finish_network(
                result,
                key,
                permit,
                prior,
                cancel.clone(),
                encode_blob,
                |_| CacheCompleteness::Complete,
            )
            .await;
        retry_unvalidated_304(result, mode, || {
            Box::pin(self.blob(identity, limits, RepositoryCacheMode::ForceRefresh, cancel))
        })
        .await
    }

    pub async fn content(
        &self,
        identity: &BlobIdentity,
        limits: &BlobLimits,
        mode: RepositoryCacheMode,
        cancel: CancellationToken,
    ) -> Result<Scoped<CachedRepositoryRead<PreparedContent>>, RepositoryCacheError> {
        let blob = self.blob(identity, limits, mode, cancel.clone()).await?;
        let service = self.session.service().clone();
        let context = self.session.context().clone();
        let budget = limits.decoded_bytes;
        let prepared =
            tokio::task::spawn_blocking(move || prepare_content(blob.value.value, budget));
        let prepared = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(GitHubError::Cancelled.into()),
            prepared = prepared => prepared.map_err(|_| RepositoryError::GitHub(
                GitHubError::CredentialStore { message: "content preparation worker closed".into() },
            )),
        };
        if !service.is_current(&context) {
            return Err(GitHubError::StaleContext.into());
        }
        Ok(Scoped {
            context,
            value: CachedRepositoryRead {
                value: prepared?,
                provenance: blob.value.provenance,
            },
        })
    }

    fn scoped<T>(
        &self,
        value: CachedRepositoryRead<T>,
    ) -> Result<Scoped<CachedRepositoryRead<T>>, RepositoryCacheError> {
        self.check()?;
        Ok(Scoped {
            context: self.session.context().clone(),
            value,
        })
    }

    fn check(&self) -> Result<(), RepositoryCacheError> {
        if self.session.verify() {
            Ok(())
        } else {
            Err(GitHubError::StaleContext.into())
        }
    }

    async fn online_prior<T>(
        &self,
        key: &CacheKey,
        cancel: CancellationToken,
        decode: impl Fn(&CacheSnapshot) -> Result<T, RepositoryCacheError> + Send + 'static,
    ) -> Result<Option<Arc<Prior<T>>>, RepositoryCacheError>
    where
        T: Send + Sync + Clone + 'static,
    {
        match self.prior(key, cancel, decode).await {
            // A quarantined old value cannot supply bytes or validators, but
            // the current online session may establish fresh repository proof.
            Err(RepositoryCacheError::AccessDenied) => Ok(None),
            result => result,
        }
    }

    async fn prior<T>(
        &self,
        key: &CacheKey,
        cancel: CancellationToken,
        decode: impl Fn(&CacheSnapshot) -> Result<T, RepositoryCacheError> + Send + 'static,
    ) -> Result<Option<Arc<Prior<T>>>, RepositoryCacheError>
    where
        T: Send + Sync + Clone + 'static,
    {
        match self.load(key, cancel, decode).await? {
            CachedValue::Hit(payload, value) => Ok(Some(Arc::new(Prior { payload, value }))),
            CachedValue::Miss(_) => Ok(None),
        }
    }

    async fn offline<T>(
        &self,
        key: &CacheKey,
        cancel: CancellationToken,
        decode: impl Fn(&CacheSnapshot) -> Result<T, RepositoryCacheError> + Send + Sync + 'static,
    ) -> Result<CachedRepositoryRead<T>, RepositoryCacheError>
    where
        T: Send + Sync + Clone + 'static,
    {
        match self.load(key, cancel, decode).await? {
            CachedValue::Hit(payload, value) => Ok(CachedRepositoryRead {
                value,
                provenance: provenance(payload.metadata, RepositoryCacheOrigin::Snapshot),
            }),
            CachedValue::Miss(reason) => Err(RepositoryCacheError::Miss(reason)),
        }
    }

    async fn load<T>(
        &self,
        key: &CacheKey,
        cancel: CancellationToken,
        decode: impl Fn(&CacheSnapshot) -> Result<T, RepositoryCacheError> + Send + 'static,
    ) -> Result<CachedValue<T>, RepositoryCacheError>
    where
        T: Send + Sync + Clone + 'static,
    {
        self.check()?;
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let cache = self.cache.clone();
        let scope = self.scope.clone();
        let key = key.clone();
        let worker = tokio::task::spawn_blocking(move || {
            match cache.read(&scope, &key).map_err(cache_error)? {
                CacheRead::Miss(reason) => Ok(CachedValue::Miss(reason)),
                CacheRead::Hit(lease) => match lease.snapshot().map_err(cache_error)? {
                    None => Ok(CachedValue::Miss(CacheMiss::NotCached)),
                    Some(payload) => match decode(&payload) {
                        Ok(value) => Ok(CachedValue::Hit(payload, value)),
                        Err(RepositoryCacheError::Miss(reason)) => Ok(CachedValue::Miss(reason)),
                        Err(_) => Ok(CachedValue::Miss(CacheMiss::Corrupt)),
                    },
                },
            }
        });
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(GitHubError::Cancelled.into()),
            result = worker => result.map_err(|_| RepositoryCacheError::Storage)?,
        };
        self.check()?;
        result
    }

    async fn before_network(
        &self,
        key: &CacheKey,
        repository: u64,
        cancel: CancellationToken,
    ) -> Result<WritePermit, RepositoryCacheError> {
        self.check()?;
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let cache = self.cache.clone();
        let scope = self.scope.clone();
        let key = key.clone();
        let worker = tokio::task::spawn_blocking(move || {
            // A quarantined repository cannot receive a normal write ticket
            // until this request has proved access. Ordinary tickets predate IO.
            let proof = cache
                .begin_revalidation(&scope, Some(repository))
                .map_err(cache_error)?;
            let ticket = match cache.begin_write(&scope, &key) {
                Ok(ticket) => Some(ticket),
                Err(crate::storage::cache::CacheError::AccessDenied) => None,
                Err(error) => return Err(cache_error(error)),
            };
            Ok(WritePermit {
                proof: Some(proof),
                ticket,
                key,
            })
        });
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(GitHubError::Cancelled.into()),
            result = worker => result.map_err(|_| RepositoryCacheError::Storage)?,
        };
        self.check()?;
        result
    }

    async fn confirm(
        &self,
        permit: &mut WritePermit,
        metadata: &RepositoryResponseMetadata,
    ) -> Result<(), RepositoryCacheError> {
        self.check()?;
        // Locally rejected oversized blobs have no repository response and do
        // not unlock a quarantined fixed-SHA cache.
        if !metadata.network_observed {
            return Ok(());
        }
        let proof = permit.proof.take();
        let needs_ticket = permit.ticket.is_none();
        let cache = self.cache.clone();
        let scope = self.scope.clone();
        let key = permit.key.clone();
        let ticket = tokio::task::spawn_blocking(move || {
            if let Some(proof) = proof {
                cache.permission_confirmed(proof).map_err(cache_error)?;
            }
            if needs_ticket {
                cache
                    .begin_write(&scope, &key)
                    .map(Some)
                    .map_err(cache_error)
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|_| RepositoryCacheError::Storage)??;
        self.check()?;
        if permit.ticket.is_none() {
            permit.ticket = ticket;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_network<T>(
        &self,
        result: Result<Scoped<(Option<T>, RepositoryResponseMetadata)>, RepositoryError>,
        _key: CacheKey,
        mut permit: WritePermit,
        prior: Option<Arc<Prior<T>>>,
        cancel: CancellationToken,
        encode: impl Fn(&T) -> Result<Vec<u8>, RepositoryCacheError>,
        completeness: impl Fn(&T) -> CacheCompleteness,
    ) -> Result<Scoped<CachedRepositoryRead<T>>, RepositoryCacheError>
    where
        T: Send + Sync + Clone + 'static,
    {
        let scoped = match result {
            Ok(scoped) => scoped,
            Err(error) if offline_fallback(&error) => {
                if let Some(prior) = prior {
                    // The earlier lease has ended. Reload and snapshot again so
                    // revocation/PAT isolation during the request wins over the
                    // copied fallback bytes.
                    let current = self
                        .offline(&_key, cancel, move |_| Ok(prior.value.clone()))
                        .await?;
                    return self.scoped(current);
                }
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        };
        self.check()?;
        let (value, metadata) = scoped.value;
        if let Some(value) = value {
            self.confirm(&mut permit, &metadata).await?;
            let bytes = encode(&value)?;
            let complete = completeness(&value);
            let cache_metadata = metadata_for(&metadata, complete);
            self.store(permit.ticket, &bytes, cache_metadata.clone(), cancel)
                .await?;
            return self.scoped(CachedRepositoryRead {
                value,
                provenance: provenance(cache_metadata, RepositoryCacheOrigin::Network),
            });
        }

        // 304 can only reuse bytes whose original validator matches this exact
        // representation. Otherwise issue an unconditional request immediately.
        if let Some(prior) = prior
            && validators_match(&prior.payload.metadata, &metadata)
        {
            self.confirm(&mut permit, &metadata).await?;
            let mut refreshed =
                metadata_for(&metadata, prior.payload.metadata.completeness.clone());
            refreshed.fetched_at_ms = prior.payload.metadata.fetched_at_ms;
            self.store(
                permit.ticket,
                &prior.payload.bytes,
                refreshed.clone(),
                cancel.clone(),
            )
            .await?;
            return self.scoped(CachedRepositoryRead {
                value: prior.value.clone(),
                provenance: provenance(refreshed, RepositoryCacheOrigin::Network),
            });
        }

        Err(RepositoryError::GitHub(GitHubError::UnexpectedStatus { status: 304 }).into())
    }

    async fn store(
        &self,
        ticket: Option<crate::storage::cache::CacheWrite>,
        bytes: &[u8],
        metadata: CacheMetadata,
        cancel: CancellationToken,
    ) -> Result<(), RepositoryCacheError> {
        let Some(ticket) = ticket else {
            // Local TooLarge has no response bytes and no permission proof. It
            // remains a valid online result but must not write/unlock cache.
            self.check()?;
            return Ok(());
        };
        self.check()?;
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let cache = self.cache.clone();
        let bytes = bytes.to_vec();
        let worker = tokio::task::spawn_blocking(move || cache.put(ticket, &bytes, metadata));
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(GitHubError::Cancelled.into()),
            result = worker => result.map_err(|_| RepositoryCacheError::Storage)?,
        };
        self.check()?;
        result.map_err(cache_error)?;
        Ok(())
    }
}

struct Prior<T> {
    payload: CacheSnapshot,
    value: T,
}
enum CachedValue<T> {
    Hit(CacheSnapshot, T),
    Miss(CacheMiss),
}
struct WritePermit {
    proof: Option<Revalidation>,
    ticket: Option<crate::storage::cache::CacheWrite>,
    key: CacheKey,
}

async fn retry_unvalidated_304<T, F, Fut>(
    result: Result<T, RepositoryCacheError>,
    mode: RepositoryCacheMode,
    retry: F,
) -> Result<T, RepositoryCacheError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, RepositoryCacheError>>,
{
    if mode != RepositoryCacheMode::ForceRefresh
        && matches!(
            result,
            Err(RepositoryCacheError::Online(RepositoryError::GitHub(
                GitHubError::UnexpectedStatus { status: 304 }
            )))
        )
    {
        retry().await
    } else {
        result
    }
}

fn cache_error(error: crate::storage::cache::CacheError) -> RepositoryCacheError {
    match error {
        crate::storage::cache::CacheError::AccessDenied => RepositoryCacheError::AccessDenied,
        crate::storage::cache::CacheError::Stale => GitHubError::StaleContext.into(),
        _ => RepositoryCacheError::Storage,
    }
}

fn offline_fallback(error: &RepositoryError) -> bool {
    match error {
        RepositoryError::GitHub(
            GitHubError::Network { .. }
            | GitHubError::Timeout { .. }
            | GitHubError::RateLimited { .. },
        ) => true,
        RepositoryError::GitHub(GitHubError::UnexpectedStatus { status }) => *status >= 500,
        _ => false,
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn metadata_for(
    response: &RepositoryResponseMetadata,
    completeness: CacheCompleteness,
) -> CacheMetadata {
    let now = now();
    CacheMetadata {
        representation_version: VERSION,
        fetched_at_ms: now,
        verified_at_ms: now,
        completeness,
        etag: response.etag.clone(),
        last_modified: response.last_modified.clone(),
        cache_control: response.cache_control.clone(),
    }
}
fn provenance(metadata: CacheMetadata, origin: RepositoryCacheOrigin) -> RepositoryCacheProvenance {
    RepositoryCacheProvenance {
        origin,
        fetched_at_ms: metadata.fetched_at_ms,
        verified_at_ms: metadata.verified_at_ms,
        completeness: metadata.completeness,
    }
}
fn validators(metadata: &CacheMetadata) -> Option<RepositoryResponseMetadata> {
    if metadata.etag.is_none() && metadata.last_modified.is_none() {
        return None;
    }
    Some(RepositoryResponseMetadata {
        etag: metadata.etag.clone(),
        last_modified: metadata.last_modified.clone(),
        cache_control: None,
        network_observed: true,
    })
}
fn validators_match(prior: &CacheMetadata, response: &RepositoryResponseMetadata) -> bool {
    (prior.etag.is_some() || prior.last_modified.is_some())
        && prior.etag == response.etag
        && prior.last_modified == response.last_modified
}

fn mutable_key(
    repository: u64,
    resource: &str,
    request: &str,
) -> Result<CacheKey, RepositoryError> {
    CacheKey::mutable(Some(repository), true, resource, request, DTO, VERSION)
        .map_err(|_| RepositoryError::InvalidInput { field: "cache key" })
}
fn fixed_key(
    repository: u64,
    kind: ObjectKind,
    sha: &ObjectSha,
) -> Result<CacheKey, RepositoryError> {
    CacheKey::fixed(repository, true, kind, sha.as_str(), DTO, VERSION)
        .map_err(|_| RepositoryError::InvalidInput { field: "cache key" })
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    repository_id: u64,
    value: T,
}
fn decode_envelope<T: for<'de> Deserialize<'de>>(
    snapshot: &CacheSnapshot,
    repository: u64,
) -> Result<T, RepositoryCacheError> {
    if snapshot.metadata.representation_version != VERSION {
        return Err(RepositoryCacheError::Miss(CacheMiss::RepresentationVersion));
    }
    let envelope: Envelope<T> = serde_json::from_slice(&snapshot.bytes)
        .map_err(|_| RepositoryCacheError::Miss(CacheMiss::Corrupt))?;
    if envelope.version != VERSION || envelope.repository_id != repository {
        return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt));
    }
    Ok(envelope.value)
}

#[derive(Serialize, Deserialize)]
struct DetailsDto {
    owner_id: u64,
    owner_login: String,
    owner_avatar: Option<String>,
    name: String,
    description: Option<String>,
    visibility: String,
    default_branch: Option<String>,
    fork: bool,
    archived: bool,
    stars: u64,
    forks: u64,
    open_issues_and_prs: u64,
    subscribers: Option<u64>,
    size_kb: u64,
    html_url: String,
    clone_url: Option<String>,
    pushed_at: Option<String>,
}
fn encode_details(value: &RepositoryDetails) -> Result<Vec<u8>, RepositoryCacheError> {
    let repository_id = value.identity.id();
    let value = DetailsDto {
        owner_id: value.owner.id,
        owner_login: value.owner.login.clone(),
        owner_avatar: value.owner.avatar_url.clone(),
        name: value.identity.locator().name().to_owned(),
        description: value.description.clone(),
        visibility: match value.visibility {
            RepoVisibility::Public => "public",
            RepoVisibility::Internal => "internal",
            RepoVisibility::Private => "private",
        }
        .into(),
        default_branch: value
            .default_branch
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        fork: value.fork,
        archived: value.archived,
        stars: value.stars,
        forks: value.forks,
        open_issues_and_prs: value.open_issues_and_prs,
        subscribers: value.subscribers,
        size_kb: value.size_kb,
        html_url: value.html_url.clone(),
        clone_url: value.clone_url.clone(),
        pushed_at: value.pushed_at.clone(),
    };
    serde_json::to_vec(&Envelope {
        version: VERSION,
        repository_id,
        value,
    })
    .map_err(|_| RepositoryCacheError::Storage)
}
fn decode_details(
    snapshot: &CacheSnapshot,
    repository: &RepositoryIdentity,
) -> Result<RepositoryDetails, RepositoryCacheError> {
    // Repository id is checked after reading the small outer envelope below.
    let envelope: Envelope<DetailsDto> = serde_json::from_slice(&snapshot.bytes)
        .map_err(|_| RepositoryCacheError::Miss(CacheMiss::Corrupt))?;
    if envelope.version != VERSION {
        return Err(RepositoryCacheError::Miss(CacheMiss::RepresentationVersion));
    }
    if envelope.repository_id != repository.id() {
        return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt));
    }
    let raw = envelope.value;
    let visibility = match raw.visibility.as_str() {
        "public" => RepoVisibility::Public,
        "internal" => RepoVisibility::Internal,
        "private" => RepoVisibility::Private,
        _ => return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt)),
    };
    Ok(RepositoryDetails {
        identity: RepositoryIdentity {
            id: envelope.repository_id,
            locator: RepositoryLocator::new(raw.owner_login.clone(), raw.name)?,
        },
        owner: RepoOwner {
            id: raw.owner_id,
            login: raw.owner_login,
            avatar_url: raw.owner_avatar,
        },
        description: raw.description,
        visibility,
        default_branch: raw.default_branch.map(DynamicRef::new).transpose()?,
        fork: raw.fork,
        archived: raw.archived,
        stars: raw.stars,
        forks: raw.forks,
        open_issues_and_prs: raw.open_issues_and_prs,
        subscribers: raw.subscribers,
        size_kb: raw.size_kb,
        html_url: raw.html_url,
        clone_url: raw.clone_url,
        pushed_at: raw.pushed_at,
    })
}

#[derive(Serialize, Deserialize)]
struct BranchesDto {
    branches: Vec<BranchDto>,
    complete: bool,
}
#[derive(Serialize, Deserialize)]
struct BranchDto {
    name: String,
    commit: String,
    protected: bool,
}
fn encode_branches(
    value: &BranchesOutcome,
    repository: &RepositoryIdentity,
) -> Result<Vec<u8>, RepositoryCacheError> {
    if &value.repository != repository {
        return Err(RepositoryError::InvalidInput {
            field: "branches repository",
        }
        .into());
    }
    let branches = value
        .branches
        .iter()
        .map(|branch| BranchDto {
            name: branch.name.as_str().to_owned(),
            commit: branch.observed_commit.as_str().to_owned(),
            protected: branch.protected,
        })
        .collect();
    serde_json::to_vec(&Envelope {
        version: VERSION,
        repository_id: repository.id(),
        value: BranchesDto {
            branches,
            complete: value.completeness == Completeness::Complete,
        },
    })
    .map_err(|_| RepositoryCacheError::Storage)
}
fn decode_branches(
    value: &CacheSnapshot,
    repository: &RepositoryIdentity,
) -> Result<BranchesOutcome, RepositoryCacheError> {
    let raw: BranchesDto = decode_envelope(value, repository.id())?;
    let complete = matches!(value.metadata.completeness, CacheCompleteness::Complete);
    if raw.complete != complete {
        return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt));
    }
    let branches = raw
        .branches
        .into_iter()
        .map(|branch| {
            Ok(Branch {
                name: DynamicRef::new(branch.name)?,
                observed_commit: ObjectSha::new(branch.commit)?,
                protected: branch.protected,
            })
        })
        .collect::<Result<Vec<_>, RepositoryCacheError>>()?;
    let completeness = if complete {
        Completeness::Complete
    } else {
        Completeness::Partial {
            reasons: partial_reasons(&value.metadata),
        }
    };
    Ok(BranchesOutcome {
        repository: repository.clone(),
        branches,
        completeness,
    })
}

#[derive(Serialize, Deserialize)]
struct SignatureDto {
    name: Option<String>,
    email: Option<String>,
    date: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct SnapshotDto {
    commit: String,
    root_tree: String,
    message: String,
    author: Option<SignatureDto>,
    committer: Option<SignatureDto>,
}
fn encode_snapshot(
    value: &RepoSnapshot,
    repository: &RepositoryIdentity,
) -> Result<Vec<u8>, RepositoryCacheError> {
    if value.repository() != repository {
        return Err(RepositoryError::InvalidInput {
            field: "snapshot repository",
        }
        .into());
    }
    let signature = |value: Option<&GitSignature>| {
        value.map(|value| SignatureDto {
            name: value.name.clone(),
            email: value.email.clone(),
            date: value.date.clone(),
        })
    };
    serde_json::to_vec(&Envelope {
        version: VERSION,
        repository_id: repository.id(),
        value: SnapshotDto {
            commit: value.commit().as_str().to_owned(),
            root_tree: value.root_tree().as_str().to_owned(),
            message: value.message().to_owned(),
            author: signature(value.author()),
            committer: signature(value.committer()),
        },
    })
    .map_err(|_| RepositoryCacheError::Storage)
}
fn decode_snapshot(
    value: &CacheSnapshot,
    repository: &RepositoryIdentity,
    revision: &RevisionSpec,
) -> Result<RepoSnapshot, RepositoryCacheError> {
    let raw: SnapshotDto = decode_envelope(value, repository.id())?;
    let commit = ObjectSha::new(raw.commit)?;
    if let RevisionSpec::Commit(expected) = revision
        && expected != &commit
    {
        return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt));
    }
    let signature = |value: Option<SignatureDto>| {
        value.map(|value| GitSignature {
            name: value.name,
            email: value.email,
            date: value.date,
        })
    };
    Ok(RepoSnapshot {
        repository: repository.clone(),
        requested: revision.clone(),
        commit,
        root_tree: ObjectSha::new(raw.root_tree)?,
        message: raw.message,
        author: signature(raw.author),
        committer: signature(raw.committer),
    })
}

#[derive(Serialize, Deserialize)]
struct TreeDto {
    commit: String,
    tree: String,
    path: Option<String>,
    truncated: Option<bool>,
    entries: Vec<TreeEntryDto>,
    complete: bool,
}
#[derive(Serialize, Deserialize)]
struct TreeEntryDto {
    path: String,
    sha: String,
    size: Option<u64>,
    kind: String,
    executable: bool,
    mode: Option<String>,
}
fn encode_tree(
    value: &DirectoryTree,
    directory: &SnapshotDirectory,
) -> Result<Vec<u8>, RepositoryCacheError> {
    if &value.directory != directory {
        return Err(RepositoryError::InvalidInput {
            field: "tree directory",
        }
        .into());
    }
    let entries = value
        .entries
        .iter()
        .map(|entry| {
            let (kind, executable, mode) = match &entry.kind {
                TreeEntryKind::File { executable } => ("file", *executable, None),
                TreeEntryKind::Directory => ("directory", false, None),
                TreeEntryKind::Symlink => ("symlink", false, None),
                TreeEntryKind::Submodule => ("submodule", false, None),
                TreeEntryKind::Unknown { git_type, mode } => {
                    (git_type.as_str(), false, Some(mode.clone()))
                }
            };
            TreeEntryDto {
                path: entry.path.as_str().to_owned(),
                sha: entry.sha.as_str().to_owned(),
                size: entry.size,
                kind: kind.to_owned(),
                executable,
                mode,
            }
        })
        .collect();
    serde_json::to_vec(&Envelope {
        version: VERSION,
        repository_id: directory.repository().id(),
        value: TreeDto {
            commit: directory.commit().as_str().to_owned(),
            tree: directory.tree().as_str().to_owned(),
            path: directory.path().map(|value| value.as_str().to_owned()),
            truncated: value.truncated,
            entries,
            complete: value.completeness == Completeness::Complete,
        },
    })
    .map_err(|_| RepositoryCacheError::Storage)
}
fn decode_tree(
    value: &CacheSnapshot,
    directory: &SnapshotDirectory,
) -> Result<DirectoryTree, RepositoryCacheError> {
    let raw: TreeDto = decode_envelope(value, directory.repository().id())?;
    // A tree SHA identifies the same immutable payload across commits/paths;
    // navigation context comes from the requested SnapshotDirectory.
    if raw.complete != matches!(value.metadata.completeness, CacheCompleteness::Complete) {
        return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt));
    }
    let mut paths = std::collections::HashSet::new();
    let entries = raw
        .entries
        .into_iter()
        .map(|entry| {
            let path = RepositoryPath::new(entry.path)?;
            if path.as_str().contains('/') || !paths.insert(path.as_str().to_owned()) {
                return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt));
            }
            let kind = match (entry.kind.as_str(), entry.executable, entry.mode) {
                ("file", executable, None) => TreeEntryKind::File { executable },
                ("directory", false, None) => TreeEntryKind::Directory,
                ("symlink", false, None) => TreeEntryKind::Symlink,
                ("submodule", false, None) => TreeEntryKind::Submodule,
                (git_type, false, Some(mode)) => TreeEntryKind::Unknown {
                    git_type: git_type.to_owned(),
                    mode,
                },
                _ => return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt)),
            };
            Ok(TreeEntry {
                path,
                sha: ObjectSha::new(entry.sha)?,
                size: entry.size,
                kind,
            })
        })
        .collect::<Result<Vec<_>, RepositoryCacheError>>()?;
    let completeness = if raw.complete {
        Completeness::Complete
    } else {
        Completeness::Partial {
            reasons: partial_reasons(&value.metadata),
        }
    };
    Ok(DirectoryTree {
        directory: directory.clone(),
        entries,
        truncated: raw.truncated,
        completeness,
    })
}

#[derive(Serialize, Deserialize)]
struct ReadmeDto {
    state: String,
    path: Option<String>,
    blob: Option<String>,
    size: Option<u64>,
}
fn encode_readme(
    value: &ReadmeDiscovery,
    snapshot: &RepoSnapshot,
) -> Result<Vec<u8>, RepositoryCacheError> {
    let (state, path, blob, size) = match value {
        ReadmeDiscovery::Found(value)
            if value.repository == *snapshot.repository() && value.commit == *snapshot.commit() =>
        {
            (
                "found",
                Some(value.path.as_str().to_owned()),
                Some(value.blob.as_str().to_owned()),
                Some(value.size),
            )
        }
        ReadmeDiscovery::NotFoundOrInaccessible { repository, commit }
            if repository == snapshot.repository() && commit == snapshot.commit() =>
        {
            ("uncertain", None, None, None)
        }
        ReadmeDiscovery::AbsentInEmptyTree { repository, commit }
            if repository == snapshot.repository() && commit == snapshot.commit() =>
        {
            ("absent-empty", None, None, None)
        }
        _ => {
            return Err(RepositoryError::InvalidInput {
                field: "README snapshot",
            }
            .into());
        }
    };
    serde_json::to_vec(&Envelope {
        version: VERSION,
        repository_id: snapshot.repository().id(),
        value: ReadmeDto {
            state: state.to_owned(),
            path,
            blob,
            size,
        },
    })
    .map_err(|_| RepositoryCacheError::Storage)
}
fn decode_readme(
    value: &CacheSnapshot,
    snapshot: &RepoSnapshot,
) -> Result<ReadmeDiscovery, RepositoryCacheError> {
    let raw: ReadmeDto = decode_envelope(value, snapshot.repository().id())?;
    match (raw.state.as_str(), raw.path, raw.blob, raw.size) {
        ("found", Some(path), Some(blob), Some(size)) => {
            Ok(ReadmeDiscovery::Found(ReadmeDescriptor {
                repository: snapshot.repository().clone(),
                commit: snapshot.commit().clone(),
                path: RepositoryPath::new(path)?,
                blob: ObjectSha::new(blob)?,
                size,
            }))
        }
        ("uncertain", None, None, None) => Ok(ReadmeDiscovery::NotFoundOrInaccessible {
            repository: snapshot.repository().clone(),
            commit: snapshot.commit().clone(),
        }),
        ("absent-empty", None, None, None) => Ok(ReadmeDiscovery::AbsentInEmptyTree {
            repository: snapshot.repository().clone(),
            commit: snapshot.commit().clone(),
        }),
        _ => Err(RepositoryCacheError::Miss(CacheMiss::Corrupt)),
    }
}

fn encode_blob(value: &BlobRead) -> Result<Vec<u8>, RepositoryCacheError> {
    match value.body() {
        BlobBody::Bytes(bytes) => Ok(bytes.clone()),
        BlobBody::TooLarge { .. } => Err(RepositoryError::InvalidInput {
            field: "oversized blob cache",
        }
        .into()),
    }
}
fn decode_blob(
    value: &CacheSnapshot,
    identity: &BlobIdentity,
    limits: &BlobLimits,
) -> Result<BlobRead, RepositoryCacheError> {
    if let Some(size) = identity.size()
        && size != value.bytes.len() as u64
    {
        return Err(RepositoryCacheError::Miss(CacheMiss::Corrupt));
    }
    if value.bytes.len() > limits.decoded_bytes {
        return Ok(BlobRead {
            identity: identity.clone(),
            body: BlobBody::TooLarge {
                size: value.bytes.len() as u64,
                limit: limits.decoded_bytes,
            },
        });
    }
    // This is an ordinary byte payload, not a serialized internal handle. The
    // fixed cache key binds the blob SHA; identity binds commit/path here too.
    Ok(BlobRead {
        identity: identity.clone(),
        body: BlobBody::Bytes(value.bytes.clone()),
    })
}

fn partial_reasons(metadata: &CacheMetadata) -> Vec<IncompleteReason> {
    match &metadata.completeness {
        CacheCompleteness::Complete => Vec::new(),
        CacheCompleteness::Partial { reason } => reason
            .split(',')
            .filter_map(|reason| match reason {
                "failed" => Some(IncompleteReason::Failed(
                    RepositoryError::EmptyOrUnavailable,
                )),
                "page-limit" => Some(IncompleteReason::PageLimit),
                "entry-limit" => Some(IncompleteReason::EntryLimit),
                "response-limit" => Some(IncompleteReason::ResponseLimit),
                "untrusted-pagination" => Some(IncompleteReason::UntrustedPagination),
                "invalid-pagination" => Some(IncompleteReason::InvalidPagination),
                "pagination-cycle" => Some(IncompleteReason::PaginationCycle),
                "server-truncated" => Some(IncompleteReason::ServerTruncated),
                "unknown-entry-type" => Some(IncompleteReason::UnknownEntryType),
                "sso-partial" => Some(IncompleteReason::SsoPartial {
                    organizations: Vec::new(),
                }),
                _ => None,
            })
            .collect(),
    }
}
fn partial_reason_text(reasons: &[IncompleteReason]) -> String {
    reasons
        .iter()
        .map(|reason| match reason {
            IncompleteReason::Failed(_) => "failed",
            IncompleteReason::PageLimit => "page-limit",
            IncompleteReason::EntryLimit => "entry-limit",
            IncompleteReason::ResponseLimit => "response-limit",
            IncompleteReason::UntrustedPagination => "untrusted-pagination",
            IncompleteReason::InvalidPagination => "invalid-pagination",
            IncompleteReason::PaginationCycle => "pagination-cycle",
            IncompleteReason::SsoPartial { .. } => "sso-partial",
            IncompleteReason::ServerTruncated => "server-truncated",
            IncompleteReason::UnknownEntryType => "unknown-entry-type",
        })
        .collect::<Vec<_>>()
        .join(",")
}
fn completeness_branches(value: &BranchesOutcome) -> CacheCompleteness {
    match &value.completeness {
        Completeness::Complete => CacheCompleteness::Complete,
        Completeness::Partial { reasons } => CacheCompleteness::Partial {
            reason: partial_reason_text(reasons),
        },
    }
}
fn completeness_repository(_: &RepositoryDetails) -> CacheCompleteness {
    CacheCompleteness::Complete
}
fn completeness_tree(value: &DirectoryTree) -> CacheCompleteness {
    match &value.completeness {
        Completeness::Complete => CacheCompleteness::Complete,
        Completeness::Partial { reasons } => CacheCompleteness::Partial {
            reason: partial_reason_text(reasons),
        },
    }
}
