use super::tests::*;

#[gpui::test]
fn contextual_form_masks_clears_and_returns_focus(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::Normal);
    let (_, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    assert!(cx.debug_bounds("settings-pat-form").is_none());
    open(&binding, FormMode::Add, cx);
    cx.simulate_keystrokes("enter");
    assert!(binding.read_with(cx, |this, _| this.field_error.is_some()));
    open(&binding, FormMode::Replace(key(2)), cx);
    assert_eq!(
        binding.read_with(cx, |this, _| this.mode.clone()),
        Some(FormMode::Replace(key(2)))
    );
    assert!(binding.read_with(cx, |this, cx| this.input.read(cx).is_masked()));
    assert!(cx.update(|window, cx| {
        binding
            .read(cx)
            .input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }));
    put(&binding, "preview-sam", cx);
    let input = binding.read_with(cx, |this, _| this.input.clone());
    input.update(cx, |input, cx| input.set_masked(false, cx));
    assert!(!input.read_with(cx, |input, _| input.is_masked()));
    input.update(cx, |input, cx| input.set_masked(true, cx));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("settings-pat-form").is_none());
    assert!(binding.read_with(cx, |this, cx| this.input.read(cx).is_empty()));
    assert!(cx.update(|window, cx| binding.read(cx).replace_focus[&key(2)].is_focused(window)));
    open(&binding, FormMode::Add, cx);
    assert!(binding.read_with(cx, |this, cx| this.input.read(cx).is_empty()));
}

#[gpui::test]
fn replacing_inactive_account_uses_authoritative_target_and_switch_omits_current(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::open(Scenario::Normal);
    let (_, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    assert!(
        cx.debug_bounds("settings-switch-1").is_none(),
        "current account has no Switch"
    );
    assert!(cx.debug_bounds("settings-switch-2").is_some());
    let before = fixture.runtime.accounts.summaries();
    open(&binding, FormMode::Replace(key(2)), cx);
    put(&binding, "preview-alex", cx);
    cx.simulate_keystrokes("enter");
    wait_idle(&binding, cx);
    assert_eq!(
        fixture.runtime.accounts.summaries(),
        before,
        "identity mismatch must change neither account"
    );
    assert_eq!(
        binding.read_with(cx, |this, _| this.mode.clone()),
        Some(FormMode::Replace(key(2)))
    );
    put(&binding, "preview-sam", cx);
    cx.simulate_keystrokes("enter");
    wait_idle(&binding, cx);
    let after = fixture.runtime.accounts.summaries();
    assert_eq!(after[0].epoch, before[0].epoch);
    assert_ne!(after[1].epoch, before[1].epoch);
    assert_eq!(
        fixture.runtime.accounts.active_context().unwrap().account,
        key(1)
    );
    assert!(cx.debug_bounds("settings-pat-form").is_none());
    assert!(cx.update(|window, cx| binding.read(cx).replace_focus[&key(2)].is_focused(window)));
}

#[gpui::test]
fn add_switch_and_revalidate_publish_authoritative_service_state(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::Normal);
    let (_, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    open(&binding, FormMode::Add, cx);
    put(&binding, "preview-jordan", cx);
    cx.simulate_keystrokes("enter");
    wait_idle(&binding, cx);
    assert_eq!(fixture.runtime.accounts.summaries().len(), 3);
    assert_eq!(
        fixture.runtime.accounts.active_context().unwrap().account,
        key(3)
    );
    let controller = binding.read_with(cx, |this, _| this.controller.clone());
    controller.update(cx, |this, cx| this.select(key(2), cx));
    wait_idle(&binding, cx);
    assert_eq!(
        fixture.runtime.accounts.active_context().unwrap().account,
        key(2)
    );
    let before = fixture
        .runtime
        .accounts
        .summaries()
        .into_iter()
        .find(|summary| summary.identity.key == key(2))
        .unwrap()
        .epoch;
    controller.update(cx, |this, cx| this.revalidate(key(2), cx));
    wait_idle(&binding, cx);
    let after = fixture
        .runtime
        .accounts
        .summaries()
        .into_iter()
        .find(|summary| summary.identity.key == key(2))
        .unwrap();
    assert_ne!(after.epoch, before);
    assert_eq!(
        after.authorization,
        crate::domain::account::AuthorizationState::Verified
    );
}

#[gpui::test]
fn a_startup_repository_failure_is_visible_before_any_user_action(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::ReposFail);
    let (_, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    assert!(!binding.read_with(cx, |this, _| this.activity));
    assert!(
        cx.debug_bounds("settings-status").is_some(),
        "an automatic read failure is an error, not suppressible startup progress"
    );
    let status = binding.read_with(cx, |this, cx| {
        this.controller.read(cx).snapshot().status.clone()
    });
    assert!(status.is_some_and(|status| status.tone == crate::ui::accounts::StatusTone::Error));
}

#[gpui::test]
fn committed_form_closes_even_when_the_follow_up_repository_read_fails(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::ReposFail);
    let (_, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    open(&binding, FormMode::Replace(key(2)), cx);
    put(&binding, "preview-sam", cx);
    cx.simulate_keystrokes("enter");
    wait_idle(&binding, cx);
    assert!(
        cx.debug_bounds("settings-pat-form").is_none(),
        "a committed replacement closes its form even if the follow-up read fails"
    );
    let (status, repositories) = binding.read_with(cx, |this, cx| {
        let controller = this.controller.read(cx);
        (
            controller.snapshot().status.clone(),
            controller.snapshot().repositories.clone(),
        )
    });
    assert!(
        status.is_none_or(|status| !status.text.contains("was not saved")),
        "repository failure must not be reported as a credential save failure"
    );
    assert!(matches!(
        repositories.preview,
        crate::ui::accounts::RepositoryPreview::Failed(_)
    ));
}

#[gpui::test]
fn a_failed_submission_is_not_completed_by_a_later_unrelated_revalidation(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::Normal);
    let (_, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    open(&binding, FormMode::Replace(key(2)), cx);
    put(&binding, "preview-alex", cx);
    cx.simulate_keystrokes("enter");
    wait_idle(&binding, cx);
    assert_eq!(
        binding.read_with(cx, |this, _| this.mode.clone()),
        Some(FormMode::Replace(key(2)))
    );
    assert!(
        !binding.read_with(cx, |this, _| this.submitted),
        "a failed submission must stop waiting for its own commit"
    );
    // The secret is cleared immediately after submission and never restored. A replacement text
    // entered after the failure still must not make the unrelated epoch change count as a commit.
    assert!(binding.read_with(cx, |this, cx| this.input.read(cx).text().is_empty()));
    put(&binding, "replacement-in-progress", cx);
    let controller = binding.read_with(cx, |this, _| this.controller.clone());
    controller.update(cx, |this, cx| this.revalidate(key(2), cx));
    wait_idle(&binding, cx);
    assert_eq!(
        binding.read_with(cx, |this, _| this.mode.clone()),
        Some(FormMode::Replace(key(2)))
    );
    assert_eq!(
        binding.read_with(cx, |this, cx| this.input.read(cx).text().to_owned()),
        "replacement-in-progress"
    );
}

#[gpui::test]
fn double_submit_and_cancellation_use_existing_controller(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::Slow);
    let (_, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    open(&binding, FormMode::Add, cx);
    put(&binding, "preview-jordan", cx);
    let before = fixture
        .transport
        .requests
        .load(std::sync::atomic::Ordering::SeqCst);
    cx.update_window_entity(&binding, |this, window, cx| {
        this.submit(window, cx);
        this.submit(window, cx);
    });
    assert!(binding.read_with(cx, |this, cx| this.controller.read(cx).busy()));
    assert!(binding.read_with(cx, |this, cx| this.input.read(cx).is_empty()));
    cx.update_window_entity(&binding, |this, window, cx| this.close(window, cx));
    draw(cx);
    assert!(cx.debug_bounds("settings-pat-form").is_none());
    assert!(
        fixture
            .transport
            .requests
            .load(std::sync::atomic::Ordering::SeqCst)
            <= before + 1
    );
}

#[gpui::test]
fn ime_confirmation_does_not_submit_or_close_form(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::Normal);
    let (_, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    open(&binding, FormMode::Add, cx);
    let input = binding.read_with(cx, |this, _| this.input.clone());
    cx.update_window_entity(&input, |input, window, cx| {
        gpui::EntityInputHandler::replace_and_mark_text_in_range(
            input,
            None,
            "中文",
            Some(0..2),
            window,
            cx,
        );
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(!binding.read_with(cx, |this, cx| this.controller.read(cx).busy()));
    assert_eq!(
        binding.read_with(cx, |this, _| this.mode.clone()),
        Some(FormMode::Add)
    );
    assert_eq!(fixture.runtime.accounts.summaries().len(), 2);
}

#[gpui::test]
fn narrow_tabs_scroll_without_content_overflow_and_keyboard_reveals_last(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::Expired);
    let (view, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    view.update(cx, |view, cx| {
        view.fixtures = 5;
        cx.notify();
    });
    // 880px window minus 200px expanded sidebar + host margins.
    cx.simulate_resize(size(px(648.0), px(720.0)));
    draw(cx);
    draw(cx);
    open(&binding, FormMode::Replace(key(2)), cx);
    let root = cx.debug_bounds("settings-root").expect("root");
    let tabs = cx
        .debug_bounds("settings-tabs-region")
        .expect("horizontal tabs");
    assert!(tabs.size.width <= root.size.width);
    let content = cx
        .debug_bounds("settings-accounts-content")
        .expect("content");
    assert!(content.right() <= root.right());
    for selector in ["settings-account-1", "settings-account-2"] {
        let row = cx.debug_bounds(selector).expect("row");
        assert!(row.right() <= root.right());
    }
    cx.update_window_entity(&view, |view, window, cx| view.select(0, window, cx));
    cx.simulate_keystrokes("end");
    draw(cx);
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.tabs.offset().x < px(0.0)));
    let last = cx.debug_bounds("settings-category-5").expect("last tab");
    assert!(last.right() <= tabs.right() + px(1.0));
}
