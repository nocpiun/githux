// Included at the root of `examples/reader.rs` and `tests/reader.rs`.
// Declares the `ui` module tree the reader expects, with paths resolved from
// this file's location, so formatting is identical on every platform.

#[allow(dead_code, unused_imports)]
mod ui {
    #[path = "../../src/ui/geometry.rs"]
    pub mod geometry;
    #[path = "../../src/ui/reader/mod.rs"]
    pub mod reader;
    #[path = "../../src/ui/scrollbar.rs"]
    pub mod scrollbar;
    #[path = "../../src/ui/tab_scrollbar.rs"]
    pub mod tab_scrollbar;
}

#[allow(unused_imports)]
use ui::reader;
