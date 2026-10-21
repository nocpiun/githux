use gpui::{KeyUpEvent, Keystroke, TestAppContext, VisualTestContext};

use super::Hello;

fn press(key: &str, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(key);
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("valid test keystroke"),
    });
    cx.run_until_parked();
}

#[gpui::test]
fn tab_focuses_press_and_keyboard_activation_survives_repaint(cx: &mut TestAppContext) {
    cx.update(ely_gpui_component::init_for_tests);
    let (view, cx) = cx.add_window_view(Hello::new);
    cx.run_until_parked();

    // Activation belongs to the focused button, not a window-wide shortcut.
    press("enter", cx);
    press("space", cx);
    assert_eq!(view.read_with(cx, |hello, _| hello.presses), 0);

    press("tab", cx);
    let button_focus = cx.update(|window, cx| {
        let focused = window.focused(cx).expect("Tab reached a focus target");
        assert_ne!(focused, view.read(cx).focus, "Tab must leave the root");
        focused
    });
    press("enter", cx);
    assert_eq!(view.read_with(cx, |hello, _| hello.presses), 1);
    press("space", cx);
    assert_eq!(view.read_with(cx, |hello, _| hello.presses), 2);
    cx.update(|window, _| assert!(button_focus.is_focused(window)));

    // Start from the root again to check reverse navigation independently.
    cx.update(|window, cx| {
        let root = view.read(cx).focus.clone();
        window.focus(&root, cx);
    });
    cx.run_until_parked();
    press("shift-tab", cx);
    cx.update(|window, _| assert!(button_focus.is_focused(window)));
    press("space", cx);
    assert_eq!(view.read_with(cx, |hello, _| hello.presses), 3);

    // With one tab stop, either direction wraps to the same button.
    press("tab", cx);
    press("shift-tab", cx);
    cx.update(|window, _| assert!(button_focus.is_focused(window)));
    press("enter", cx);
    assert_eq!(view.read_with(cx, |hello, _| hello.presses), 4);
}
