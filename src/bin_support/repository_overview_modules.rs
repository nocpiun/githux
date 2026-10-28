// Root of the standalone repository-overview module tree, pulled in by `#[path]` from
// `examples/repository_overview.rs` and `tests/repository_overview.rs`. Each `#[path]` below is
// resolved against this file's directory, so it is identical on every platform and `rustfmt` can
// resolve it.
//
// The component continues to use the real `crate::ui::...` paths, so no logic is duplicated here.
// The shared scrollbar's modules are declared through the `scroll` file itself, which resolves its
// own children next to it. `geometry` is declared because the shared scrollbar's own tests read
// `crate::ui::geometry` when this tree is compiled as an integration test.
#![allow(dead_code, unused_imports)]

#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/repository_overview/mod.rs"]
pub mod repository_overview;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
