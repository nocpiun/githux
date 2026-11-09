//! Integration target for the Native Shell repository overview component.
//!
//! The component's own suite lives beside the code (`src/ui/repository_overview/tests.rs`).
//! `cargo test` compiles integration targets but not example targets, so this file assembles the
//! real domain types, the shared NSH foundation tokens and the real component sources, then
//! activates its `#[cfg(test)]` suite. It is a test assembly, not another application.

#[path = "../src/domain/mod.rs"]
mod domain;

pub mod ui {
    pub mod foundation {
        #[path = "../../src/ui/foundation/mod.rs"]
        mod foundation_impl;
        pub use foundation_impl::*;
    }
    pub mod repository_overview {
        #[path = "../../src/ui/repository_overview/mod.rs"]
        mod repository_overview_impl;
        pub use repository_overview_impl::*;
    }
}
