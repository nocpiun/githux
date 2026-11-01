//! Read-only global work lists. Query/cursor identities contain no credentials.
pub use crate::domain::home::HomePartialReason as PartialReason;
use crate::domain::home::{HomeError, Login};
pub type WorkItemError = HomeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkItemKind {
    Issue,
    PullRequest,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Relation {
    Assigned,
    Created,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateFilter {
    Open,
    Closed,
    All,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Open,
    Closed,
    Merged,
    Unknown,
}
impl ItemState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Closed => "Closed",
            Self::Merged => "Merged",
            Self::Unknown => "Unknown",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemQuery {
    pub kind: WorkItemKind,
    pub relation: Relation,
    pub state: StateFilter,
    login: Login,
}
impl WorkItemQuery {
    pub fn new(kind: WorkItemKind, login: Login) -> Self {
        Self {
            kind,
            relation: Relation::Assigned,
            state: StateFilter::Open,
            login,
        }
    }
    pub fn login(&self) -> &Login {
        &self.login
    }
    pub fn expression(&self) -> String {
        let kind = match self.kind {
            WorkItemKind::Issue => "issue",
            WorkItemKind::PullRequest => "pr",
        };
        let relation = match self.relation {
            Relation::Assigned => "assignee",
            Relation::Created => "author",
        };
        let state = match self.state {
            StateFilter::Open => " is:open",
            StateFilter::Closed => " is:closed",
            StateFilter::All => "",
        };
        format!("is:{kind} {relation}:{}{state}", self.login.as_str())
    }
}
/// A stable host + canonical locator, since Search does not return repo ids.
/// Never use a repository-local item number as a repository id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkRepository {
    pub host: String,
    pub owner: String,
    pub name: String,
}
impl WorkRepository {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItem {
    pub id: u64,
    pub kind: WorkItemKind,
    pub number: u64,
    pub title: String,
    pub repository: WorkRepository,
    pub state: ItemState,
    pub updated_at: String,
}
/// Constructed only by the adapter after validating a next link. Contains the
/// complete query identity; a subsequent page cannot adopt a different filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemCursor {
    pub(crate) query: WorkItemQuery,
    pub(crate) page: u32,
    pub(crate) per_page: u32,
    pub(crate) origin: String,
}
impl WorkItemCursor {
    pub fn query(&self) -> &WorkItemQuery {
        &self.query
    }
    pub fn page(&self) -> u32 {
        self.page
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemPage {
    pub query: WorkItemQuery,
    pub page: u32,
    pub items: Vec<WorkItem>,
    pub total_count: u64,
    pub next: Option<WorkItemCursor>,
    pub partial: Vec<PartialReason>,
}
