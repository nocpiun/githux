//! Focused Settings tests: the production category configuration, the wide side navigation with
//! its compact row and single indicator, and the narrow default NSH tab strip.

#[cfg(test)]
mod headless_cases {
    use crate::ui::settings::SettingsView;
    use nsh_core::element::Root;
    use nsh_core::prelude::*;
    use nsh_core::{Bounds, MountBatch, MountItem, NodeId, Points, Size};
    use nsh_engine::EngineConfig;
    use nsh_testing::Headless;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    #[component]
    fn SettingsHarness() -> impl IntoElement {
        let view = stored(SettingsView::new(Rc::new(|_| {})).with_fixture_categories(2));
        view.with(|view| view.element())
    }

    #[component]
    fn ProductionSettingsHarness() -> impl IntoElement {
        let settings = SettingsView::new(Rc::new(|_| {}));
        settings.element()
    }

    fn start_with_observer(
        width: f32,
        observer: impl FnMut(&str, &MountBatch) + Send + 'static,
    ) -> Headless {
        let config = EngineConfig {
            viewport: Size::new(px(width), px(800.0)),
            ..EngineConfig::new(Root::of::<SettingsHarness>())
        };
        let mut app = Headless::start(config).expect("headless engine");
        app.observe(observer);
        app.first_render().expect("first render");
        app
    }

    fn start(width: f32) -> Headless {
        start_with_observer(width, |_, _| {})
    }

    #[test]
    fn a_wide_page_uses_the_compact_side_rail_and_one_indicator() {
        let ids: Arc<Mutex<HashMap<String, NodeId>>> = Arc::new(Mutex::new(HashMap::new()));
        let frames: Arc<Mutex<HashMap<NodeId, Bounds<Points>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (i, f) = (ids.clone(), frames.clone());
        let mut app = start_with_observer(1000.0, move |_: &str, batch: &MountBatch| {
            for item in &batch.items {
                match item {
                    MountItem::ViewProps { id, props } => {
                        if let Some(t) = &props.test_id {
                            i.lock().unwrap().insert(t.to_string(), *id);
                        }
                    }
                    MountItem::Frame { id, frame } => {
                        f.lock().unwrap().insert(*id, *frame);
                    }
                    _ => {}
                }
            }
        });
        app.wait_for("settings-category-list").unwrap();
        assert_eq!(
            app.screen().count("settings-category-indicator").unwrap(),
            1
        );
        assert_eq!(app.screen().count("settings-tabs-region").unwrap(), 0);
        assert!(app.screen().matches("settings-category-1").unwrap());

        let row = app
            .screen()
            .press_target("settings-category-0")
            .unwrap()
            .expect("native category tab");
        let frame = *frames.lock().unwrap().get(&row).expect("category frame");
        assert!(
            (frame.size.height.0 - 32.0).abs() < 0.5,
            "compact category height: {frame:?}"
        );

        app.press("settings-category-1").unwrap();
        app.wait_for("Fixture section 1").unwrap();
        app.wait_for("settings-fixture-content").unwrap();
        assert!(
            app.screen()
                .selected("role:tab[Fixture section 1]")
                .unwrap()
        );
        assert_eq!(
            app.screen().count("settings-category-indicator").unwrap(),
            1
        );
    }

    #[test]
    fn a_narrow_page_uses_the_default_nsh_tab_strip_without_the_rail_indicator() {
        let mut app = start(600.0);
        app.wait_for("settings-tabs-region").unwrap();
        assert_eq!(
            app.screen().count("settings-category-indicator").unwrap(),
            0
        );
        assert_eq!(app.screen().count("settings-category-list").unwrap(), 0);
        app.press("settings-category-2").unwrap();
        app.wait_for("Fixture section 2").unwrap();
        assert!(app.screen().matches("settings-fixture-content").unwrap());
        assert!(
            app.screen()
                .selected("role:tab[Fixture section 2]")
                .unwrap()
        );
    }

    #[test]
    fn the_accounts_category_is_the_default_content() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<ProductionSettingsHarness>())).unwrap();
        app.first_render().unwrap();
        app.wait_for("settings-accounts-content").unwrap();
        app.wait_for("accounts-token-form").unwrap();
        assert!(!app.screen().matches("settings-fixture-content").unwrap());
        assert_eq!(app.screen().count("role:tab").unwrap(), 1);
    }
}
