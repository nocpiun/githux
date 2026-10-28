//! Integration assembly for the reusable Profile Overview tests.

#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/bin_support/profile_modules.rs"]
mod profile_modules;
#[allow(unused_imports)]
use profile_modules as ui;
#[allow(unused_imports)]
use profile_modules::profile;
