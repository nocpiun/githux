//! Runs the reusable Home/Feed module tests on GPUI's test platform.

#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/bin_support/home_modules.rs"]
mod home_modules;
#[allow(unused_imports)]
use home_modules as ui;
#[allow(unused_imports)]
use home_modules::home;
