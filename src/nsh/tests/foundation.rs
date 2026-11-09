//! Headless verification of the native shell foundation, independent of the real window.
//! Selector names are test ids (`.test_id`); roles/labels mirror the real window.
use crate::shell::Foundation;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[test]
fn foundation_lays_out_and_reacts() {
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<Foundation>())).expect("headless app starts");
    app.wait_for("Native Shell 基础")
        .expect("page header renders");
    app.wait_for("row 1").expect("virtual list row renders");
    // The recycled virtual list scrolled to the end keeps rendering rows.
    app.scroll("id:foundation-list", nsh_core::geometry::px(4000.0))
        .expect("virtual list scrolls");
    app.wait_for("row 51")
        .expect("later rows render after scrolling");
}

#[test]
fn font_families_resolve_without_installed_fonts() {
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<Foundation>())).expect("headless app starts");
    app.wait_for("Native Shell 基础")
        .expect("page header renders");
    // Both families render through the shared root registration path.
    let ui = app
        .screen()
        .span_style("text:Native Shell 基础")
        .expect("title span present");
    assert_eq!(
        ui.font_family.expect("family set").name().as_ref(),
        "Githux UI"
    );
}
