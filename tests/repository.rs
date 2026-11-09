//! Integration target for the Native Shell repository page.
//!
//! The page's own suite lives beside the code (`src/ui/repository/tests.rs` and its submodules).
//! `cargo test` compiles integration targets but not example targets, so this file assembles the
//! same real service and page modules the product uses and activates the page's `#[cfg(test)]`
//! suite. The module tree mirrors `src/main.rs` + the Native Shell page contract:
//!
//! * `auth`/`content`/`data`/`domain`/`github`/`network`/`storage` are the unchanged business layer;
//! * `app::runtime` is the real runtime bridge, `workspace` re-exports its navigation memory;
//! * `app::pages` is the minimal page contract (`PageNavigation`/`NavigationSink`/`PageView`);
//! * `ui::foundation`, `ui::reader`, `ui::repository`, `ui::repository_overview` are the real page
//!   sources.
//!
//! This target requires the Native Shell dependencies the integrated root package uses; it is not
//! another application assembly and never ships. The shared fixture (`src/bin_support/
//! repository_fixture.rs`) is the one the preview and the suite browse.

#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;

pub mod app {
    #[path = "../../src/app/runtime/mod.rs"]
    mod runtime_impl;
    pub use runtime_impl as runtime;

    /// The integrator-owned page contract (`src/app/pages/mod.rs`), copied here so this target can
    /// compile the page without assembling every other page. Keep it in sync with that file.
    pub mod pages {
        use super::runtime::RuntimeHandle;
        use crate::workspace::{RepositoryNavigation, Route, TabId};
        use nsh_core::element::AnyElement;
        use std::rc::Rc;

        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum PageNavigation {
            Open(Route),
            Repository {
                route: Route,
                intent: RepositoryNavigation,
            },
            External(String),
            CanonicalProfile { tab: TabId, login: String },
        }

        pub type NavigationSink = Rc<dyn Fn(PageNavigation)>;

        pub trait PageView {
            fn element(&self) -> AnyElement;
            fn poll(&mut self, runtime: &RuntimeHandle);
            fn refresh(&mut self, runtime: &RuntimeHandle);
            fn set_active(&mut self, active: bool);
            fn retire(&mut self);
        }
    }
}

pub mod workspace {
    pub use crate::app::runtime::sessions::{
        PageIdentity, RepositoryNavigation, Route, RouteKey, Session, Sessions, TabId, Workspace,
    };
}

pub mod ui {
    pub mod foundation {
        #[path = "../../src/ui/foundation/mod.rs"]
        mod foundation_impl;
        pub use foundation_impl::*;
    }
    pub mod reader {
        #[path = "../../src/ui/reader/mod.rs"]
        mod reader_impl;
        pub use reader_impl::*;
    }
    pub mod repository {
        #[path = "../../src/ui/repository/mod.rs"]
        mod repository_impl;
        pub use repository_impl::*;
    }
    pub mod repository_overview {
        #[path = "../../src/ui/repository_overview/mod.rs"]
        mod repository_overview_impl;
        pub use repository_overview_impl::*;
    }
}

#[path = "../src/bin_support/repository_fixture.rs"]
pub mod fixture;

/// The product test assembly's fixture alias (`src/main.rs` declares the same module).
#[cfg(test)]
pub mod repository_modules {
    pub use crate::fixture;
}
