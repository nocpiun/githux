// The developer-only account example's module assembly.
//
// `examples/account.rs` declares the product modules it needs directly at its own crate root
// (`app::accounts`, `ui::accounts`, `ui::scroll`), because those modules resolve
// `crate::app::accounts::…` and `crate::ui::accounts::…` paths internally. Declaring them here
// inside a wrapper module would produce a second, incompatible copy of the product types — exactly
// what a thin assembly must avoid.
//
// This file therefore only records that contract for the entry point; it deliberately re-exports
// nothing.
#![allow(dead_code, unused_imports)]
