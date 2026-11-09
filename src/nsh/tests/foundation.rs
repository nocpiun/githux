//! Headless verification of the native shell foundation, independent of the real window.
//! Selector names are test ids (`.test_id`); roles/labels mirror the real window.
use crate::shell::Foundation;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[test]
fn foundation_lays_out_and_reacts() {
    let mut app = Headless::start(EngineConfig::new(crate::foundation::fonts::install(
        Root::of::<Foundation>(),
    )))
    .expect("headless app starts");
    app.wait_for("Native Shell 基础")
        .expect("page header renders");
    app.wait_for("row 1").expect("virtual list row renders");
    app.wait_for("Actions: 0").expect("toolbar actually mounts");
    app.press("Act").expect("button works");
    app.wait_for("Actions: 1")
        .expect("callback updates displayed state");
    app.press("id:foundation-retry")
        .expect("warning retries directly");
    app.wait_for("Actions: 2").expect("warning invokes retry");
    assert!(!app.screen().shows("text:Retry"));
    app.type_text("id:foundation-input", "native input")
        .expect("input is mounted");
    // The recycled virtual list scrolled to the end keeps rendering rows.
    app.scroll("id:foundation-list", nsh_core::geometry::px(4000.0))
        .expect("virtual list scrolls");
    app.wait_for("row 51")
        .expect("later rows render after scrolling");
}

#[test]
fn registered_root_requests_the_embedded_ui_family() {
    let mut app = Headless::start(EngineConfig::new(crate::foundation::fonts::install(
        Root::of::<Foundation>(),
    )))
    .expect("headless app starts");
    app.wait_for("Native Shell 基础")
        .expect("page header renders");
    // This checks the requested family using the registered root, not per-glyph fallback.
    let ui = app
        .screen()
        .span_style("text:Native Shell 基础")
        .expect("title span present");
    assert_eq!(
        ui.font_family.expect("family set").name().as_ref(),
        "Githux UI"
    );
}

#[test]
fn combined_preview_switches_reader_and_foundation() {
    let mut app = Headless::start(EngineConfig::new(crate::root())).unwrap();
    app.wait_for("row 1").unwrap();
    app.press("Reader").unwrap();
    app.wait_for("reader-document").unwrap();
    assert!(!app.screen().shows("secret-hidden"));
    app.press("Source").unwrap();
    app.wait_for("reader-source").unwrap();
    app.press("Foundation").unwrap();
    app.wait_for("Actions: 0").unwrap();
    app.press("Act").unwrap();
    app.wait_for("Actions: 1").unwrap();
}
