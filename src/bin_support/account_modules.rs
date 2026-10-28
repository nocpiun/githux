// The developer-only account example's module assembly.
//
// `examples/account.rs` and `tests/account.rs` each declare the product modules they need directly
// at their own crate root (`app::accounts`, `ui::accounts`, `ui::scroll`), because those modules
// resolve `crate::app::accounts::…` and `crate::ui::accounts::…` paths internally. Declaring them
// here inside a wrapper module would produce a second, incompatible copy of the product types —
// exactly what a thin assembly must avoid.
//
// This file therefore only records that contract for the two entry points; it deliberately
// re-exports nothing. Keep the two entry points' `#[path]` declarations in sync instead of adding
// code here.
#![allow(dead_code, unused_imports)]
