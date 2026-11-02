//! Request lifecycle shared by native preview, product view and deterministic tests.
use crate::data::work_items::WorkItemSession;
use crate::domain::work_item::*;
use crate::github::work_items::WorkItemLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Replace,
    Append,
}
#[derive(Clone)]
pub struct Request {
    pub generation: u64,
    pub query: WorkItemQuery,
    pub cursor: Option<WorkItemCursor>,
    pub mode: RequestMode,
    pub session: WorkItemSession,
    pub cancel: CancellationToken,
    source: CancellationTokenSource,
}
impl Request {
    pub async fn execute(&self, limits: &WorkItemLimits) -> Result<WorkItemPage, WorkItemError> {
        self.session
            .page(
                &self.query,
                self.cursor.as_ref(),
                limits,
                self.cancel.clone(),
            )
            .await
            .map(|scoped| scoped.value)
    }
}
pub struct Controller {
    pub query: WorkItemQuery,
    pub rows: Vec<WorkItem>,
    pub next: Option<WorkItemCursor>,
    pub partial: Vec<PartialReason>,
    pub error: Option<WorkItemError>,
    pub failed_mode: Option<RequestMode>,
    pub loaded: bool,
    pub total_count: Option<u64>,
    pub busy: Option<RequestMode>,
    session: WorkItemSession,
    generation: u64,
    flight: Option<Request>,
}
impl Controller {
    pub fn new(session: WorkItemSession, kind: WorkItemKind) -> Self {
        Self {
            query: session.query(kind, Relation::Assigned, StateFilter::Open),
            rows: Vec::new(),
            next: None,
            partial: Vec::new(),
            error: None,
            failed_mode: None,
            loaded: false,
            total_count: None,
            busy: None,
            session,
            generation: 0,
            flight: None,
        }
    }
    pub fn context(&self) -> &crate::data::RepositoryReadContext {
        self.session.context()
    }
    pub fn current(&self) -> bool {
        self.session.verify()
    }
    pub fn rebind(&mut self, session: WorkItemSession) {
        self.cancel();
        let kind = self.query.kind;
        let generation = self.generation;
        *self = Self::new(session, kind);
        self.generation = generation;
    }
    pub fn filter(
        &mut self,
        kind: WorkItemKind,
        relation: Relation,
        state: StateFilter,
    ) -> Option<Request> {
        let query = self.session.query(kind, relation, state);
        if self.query == query {
            return None;
        }
        self.cancel();
        self.query = query;
        self.rows.clear();
        self.next = None;
        self.partial.clear();
        self.error = None;
        self.failed_mode = None;
        self.loaded = false;
        self.total_count = None;
        self.begin(RequestMode::Replace)
    }
    /// Refresh may supersede an append; repeated same-mode in-flight actions
    /// are ignored. A refresh retains the last snapshot until success.
    pub fn begin(&mut self, mode: RequestMode) -> Option<Request> {
        if !self.current()
            || self.busy == Some(mode)
            || (mode == RequestMode::Append && (self.busy.is_some() || self.next.is_none()))
        {
            return None;
        }
        self.cancel();
        let (cancel, source) = CancellationToken::new();
        let request = Request {
            generation: self.generation,
            query: self.query.clone(),
            cursor: if mode == RequestMode::Append {
                self.next.clone()
            } else {
                None
            },
            mode,
            session: self.session.clone(),
            cancel,
            source,
        };
        self.busy = Some(mode);
        self.error = None;
        self.failed_mode = None;
        self.flight = Some(request.clone());
        Some(request)
    }
    pub fn cancel(&mut self) {
        if let Some(request) = self.flight.take() {
            request.source.cancel();
        }
        self.generation = self
            .generation
            .checked_add(1)
            .expect("request generation overflow");
        self.busy = None;
    }
    pub fn finish(
        &mut self,
        request: &Request,
        result: Result<WorkItemPage, WorkItemError>,
    ) -> bool {
        if !request.session.verify()
            || !self.current()
            || request.cancel.is_cancelled()
            || request.session.context() != self.session.context()
            || request.generation != self.generation
            || request.query != self.query
            || self.flight.as_ref().is_none_or(|flight| {
                flight.generation != request.generation
                    || flight.cursor != request.cursor
                    || flight.mode != request.mode
            })
        {
            return false;
        }
        self.flight = None;
        self.busy = None;
        match result {
            Err(error) => {
                self.failed_mode = Some(request.mode);
                self.error = Some(error);
            }
            Ok(page) => {
                let expected = request.cursor.as_ref().map_or(1, WorkItemCursor::page);
                if page.query != request.query || page.page != expected {
                    self.failed_mode = Some(request.mode);
                    self.error = Some(WorkItemError::InvalidResponse {
                        message: "page identity differs from request".into(),
                    });
                    return true;
                }
                if request.mode == RequestMode::Replace {
                    self.rows.clear();
                    self.partial.clear();
                }
                let mut seen: std::collections::HashSet<_> =
                    self.rows.iter().map(|row| row.id).collect();
                self.rows
                    .extend(page.items.into_iter().filter(|item| seen.insert(item.id)));
                for reason in page.partial {
                    if !self.partial.contains(&reason) {
                        self.partial.push(reason);
                    }
                }
                self.next = page.next;
                self.total_count = Some(page.total_count);
                self.loaded = true;
                self.error = None;
                self.failed_mode = None;
            }
        }
        true
    }
    pub fn is_empty(&self) -> bool {
        self.current()
            && self.loaded
            && self.rows.is_empty()
            && self.partial.is_empty()
            && self.next.is_none()
            && self.error.is_none()
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.cancel();
    }
}
