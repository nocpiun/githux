// Root of the standalone `reader-images` preview/test assembly.
//
// `examples/reader_images.rs` and `tests/reader_images.rs` declare the product
// service modules (`auth`, `content`, `data`, `domain`, `github`, `network`,
// `storage`) directly at their own crate root, because those modules resolve
// `crate::…` paths internally. Declaring them here would produce a second,
// incompatible copy of the product types.
//
// This file therefore only assembles the UI side plus the one development
// fixture. `fixture` refers to `crate::network`, which resolves to the entry
// point's own crate-root declaration, exactly like `repository_modules.rs`.
#![allow(dead_code, unused_imports)]

#[path = "reader_images_fixture.rs"]
pub mod fixture;
#[path = "../ui/geometry.rs"]
pub mod geometry;
#[path = "../ui/reader/mod.rs"]
pub mod reader;
#[path = "../ui/scroll/mod.rs"]
pub mod scroll;
