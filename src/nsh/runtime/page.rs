//! Page controllers get real services and carry these tickets across every await.
use super::{
    backend::Backend,
    sessions::{PageIdentity, Sessions},
};
use crate::data::{
    AccountCache, AccountService, PageReadSession, RepoService, RepositoryReadContext,
    RepositorySession,
};
use crate::domain::repository::RepositoryError;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::network::executor::SpawnError;
use crate::storage::cache::CacheAuthorization;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A binding is invalidated on mutation INTENT, not only after its commit is observed.
#[derive(Clone)]
pub struct Services {
    pub(super) backend: Arc<Backend>,
    pub(super) binding: Arc<AtomicBool>,
}
impl Services {
    pub fn accounts(&self) -> &Arc<AccountService> {
        &self.backend.accounts
    }
    pub fn repositories(&self) -> RepoService {
        self.backend.accounts.repo_service()
    }
    pub fn spawn<T: Send + 'static>(
        &self,
        work: impl Future<Output = T> + Send + 'static,
    ) -> Result<tokio::task::JoinHandle<T>, SpawnError> {
        if !self.is_current() {
            return Err(SpawnError);
        }
        self.backend.spawn(work)
    }
    pub fn is_current(&self) -> bool {
        self.binding.load(Ordering::Acquire)
    }
    pub(super) fn invalidate(&self) {
        self.binding.store(false, Ordering::Release);
    }
    pub fn page_context(
        &self,
        sessions: &Sessions,
        tab: super::sessions::TabId,
        repository: Option<u64>,
    ) -> Result<PageContext, RepositoryError> {
        let identity = sessions.identity(tab).ok_or(RepositoryError::GitHub(
            crate::domain::error::GitHubError::StaleContext,
        ))?;
        let session = self.repositories().session()?;
        if identity.account.as_ref() != Some(&session.context().auth().account)
            || !self.is_current()
        {
            return Err(RepositoryError::GitHub(
                crate::domain::error::GitHubError::StaleContext,
            ));
        }
        let cache = self.repositories().cache(session.context());
        let authorization = cache
            .as_ref()
            .and_then(|c| c.store().authorization(c.scope(), repository));
        // With a binding, absent authorization is a denied token, NOT an online-only bypass.
        let denied = cache.is_some() && authorization.is_none();
        Ok(PageContext {
            services: self.clone(),
            identity,
            context: session.context().clone(),
            cache,
            authorization,
            denied,
        })
    }
}

#[derive(Clone)]
pub struct PageContext {
    services: Services,
    identity: PageIdentity,
    context: RepositoryReadContext,
    cache: Option<AccountCache>,
    authorization: std::option::Option<CacheAuthorization>,
    denied: bool,
}
impl PageContext {
    pub fn identity(&self) -> &PageIdentity {
        &self.identity
    }
    pub fn context(&self) -> &RepositoryReadContext {
        &self.context
    }
    pub fn services(&self) -> &Services {
        &self.services
    }
    pub fn cache(&self) -> Option<&AccountCache> {
        self.cache.as_ref()
    }
    /// These constructors cannot silently bind a later account to this page.
    pub fn repository_session(&self) -> Result<RepositorySession, RepositoryError> {
        let session = self.services.repositories().session()?;
        if self.services.repositories().is_current(&self.context)
            && session.context() == &self.context
        {
            Ok(session)
        } else {
            Err(RepositoryError::GitHub(
                crate::domain::error::GitHubError::StaleContext,
            ))
        }
    }
    pub fn page_session(&self) -> Result<PageReadSession, RepositoryError> {
        let session = self.services.repositories().page_session()?;
        if self.services.repositories().is_current(&self.context)
            && session.context() == &self.context
        {
            Ok(session)
        } else {
            Err(RepositoryError::GitHub(
                crate::domain::error::GitHubError::StaleContext,
            ))
        }
    }
    pub fn is_current(&self, sessions: &Sessions) -> bool {
        self.services.is_current()
            && sessions.is_current(&self.identity)
            && self.services.repositories().is_current(&self.context)
            && !self.denied
            && self
                .authorization
                .as_ref()
                .is_none_or(CacheAuthorization::is_current)
    }
}

/// One section's request. Its cancellation never aborts other page subscribers.
#[derive(Clone)]
pub struct PageTicket {
    context: PageContext,
    generation: u64,
    cancel: CancellationToken,
}
impl PageTicket {
    pub fn page(&self) -> &PageContext {
        &self.context
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }
    pub fn is_current(&self, sessions: &Sessions) -> bool {
        !self.cancel.is_cancelled() && self.context.is_current(sessions)
    }
}

/// Store Result<T, E> here when a section needs errors; success and error use identical gates.
/// The UI must call `value`/`clear_invalid` before binding signals on every drain, including drains
/// without replies. A cached bare clone displayed by the UI is subject to the same obligation.
pub struct PageSlot<T> {
    generation: u64,
    pending: Option<CancellationTokenSource>,
    displayed: Option<(PageTicket, T)>,
}
impl<T> Default for PageSlot<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            pending: None,
            displayed: None,
        }
    }
}
impl<T> PageSlot<T> {
    pub fn begin(&mut self, context: PageContext) -> PageTicket {
        if let Some(cancel) = self.pending.take() {
            cancel.cancel();
        }
        self.generation = self
            .generation
            .checked_add(1)
            .expect("page generation exhausted");
        let (cancel, source) = CancellationToken::new();
        self.pending = Some(source);
        PageTicket {
            context,
            generation: self.generation,
            cancel,
        }
    }
    pub fn publish(&mut self, ticket: PageTicket, value: T, sessions: &Sessions) -> bool {
        if ticket.generation != self.generation
            || self.pending.is_none()
            || !ticket.is_current(sessions)
        {
            return false;
        }
        self.pending = None;
        self.displayed = Some((ticket, value));
        true
    }
    /// Whether a displayed value is still valid under the current account/workspace/authorization.
    pub fn is_valid(&self, sessions: &Sessions) -> bool {
        self.displayed
            .as_ref()
            .is_some_and(|(ticket, _)| ticket.is_current(sessions))
    }
    pub fn clear_invalid(&mut self, sessions: &Sessions) -> bool {
        if self
            .displayed
            .as_ref()
            .is_some_and(|(ticket, _)| !ticket.is_current(sessions))
        {
            self.displayed = None;
            true
        } else {
            false
        }
    }
    pub fn value(&mut self, sessions: &Sessions) -> Option<&T> {
        self.clear_invalid(sessions);
        self.displayed.as_ref().map(|(_, value)| value)
    }
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.pending.take() {
            cancel.cancel();
        }
        self.generation = self
            .generation
            .checked_add(1)
            .expect("page generation exhausted");
    }
    pub fn clear(&mut self) {
        self.cancel();
        self.displayed = None;
    }
}
impl<T> Drop for PageSlot<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}
