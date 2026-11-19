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
    // Install the real product dark palette, not Ely's bare default: the settings example compiles
    // `src/app/mod.rs`, so this asserts the colours the owner actually sees.
    cx.update(|_, cx| crate::app::theme::install(cx));
    wait_idle(&binding, cx);
    // One pinned clock for the whole test, so the shared band can be stepped by hand. The base is
    // far past the view's own clock so the first pinned frame settles anything still in flight.
    view.update(cx, |view, _| view.pin_clock(1_000_000));
    view.update(cx, |view, cx| {
        view.fixtures = 12;
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
    let last = cx.debug_bounds("settings-category-12").expect("last tab");
    assert!(last.right() <= tabs.right() + px(1.0));
    // The shared band is painted, not an element on the selected tab: after the reveal it sits
    // exactly on the last tab's measured box, including its (clipped) scrolled position.
    view.update(cx, |view, _| view.pin_clock(1_000_500));
    draw(cx);
    let band = view
        .read_with(cx, |view, _| view.category_underline_rect())
        .expect("the narrow strip paints its band");
    assert_eq!(band.size.height, px(2.0));
    assert_eq!(band.left(), last.left());
    assert_eq!(band.size.width, last.size.width);
    assert_eq!(band.bottom(), last.bottom());
    // #91's shared band is one of the consumers of `colors.accent`. Read the quad it actually
    // submitted: the product paints Settings in dark mode, and the accent family is inherited from
    // Ely rather than replaced by the comparison's violet.
    let expect_accent = cx.update(|_, cx| {
        use ely_core::theme::ActiveTheme;
        cx.theme().colors.accent
    });
    cx.update(|window, _| {
        let scale = window.scale_factor();
        let painted = window
            .painted_quads()
            .into_iter()
            .find(|quad| quad.bounds == band.scale(scale) && !quad.background.is_transparent())
            .expect("the shared band submits a background quad");
        assert_eq!(painted.background.as_solid(), Some(expect_accent));
        // The installed product palette inherits this accent from Ely; pin the pre-theme value so
        // the band cannot drift to the comparison's violet.
        let rgba = gpui::Rgba::from(expect_accent);
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        assert_eq!(
            (channel(rgba.r) << 16) | (channel(rgba.g) << 8) | channel(rgba.b),
            0xf3f1f0
        );
    });
    assert!(
        cx.debug_bounds("settings-category-indicator").is_none(),
        "the narrow layout must not paint the wide rail indicator"
    );
}

#[gpui::test]
fn wide_categories_are_compact_with_one_indicator_and_follow_selection(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::Normal);
    let (view, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    view.update(cx, |view, cx| {
        view.fixtures = 5;
        cx.notify();
    });
    draw(cx);
    draw(cx);
    let row = cx.debug_bounds("settings-category-0").expect("wide row");
    assert!(
        row.size.height < px(38.0),
        "wide rows use the shell's Md height, not the old 38px tabs"
    );
    let list = cx
        .debug_bounds("settings-category-list")
        .expect("category list");
    let marker = cx
        .debug_bounds("settings-category-indicator")
        .expect("current marker");
    assert_eq!(marker.left(), list.left());
    assert!(marker.top() >= row.top() && marker.bottom() <= row.bottom());
    let first = cx
        .debug_bounds("settings-category-1")
        .expect("fixture category");
    cx.simulate_click(first.center(), Modifiers::none());
    draw(cx);
    draw(cx);
    assert!(view.read_with(cx, |view, _| matches!(view.category, Category::Fixture(1))));
    let moved = cx
        .debug_bounds("settings-category-indicator")
        .expect("marker moved");
    let second = cx
        .debug_bounds("settings-category-1")
        .expect("selected row");
    assert!(moved.top() >= second.top() && moved.bottom() <= second.bottom());
}

/// The narrow strip's band follows its own keyboard activation, not only pointer clicks: the
/// arrows/Home/End already select a tab, and the band must move and resize with the same timing.
#[gpui::test]
fn narrow_tab_band_moves_and_resizes_with_keyboard_selection(cx: &mut TestAppContext) {
    let fixture = Fixture::open(Scenario::Normal);
    let (view, binding, cx) = mount(cx, &fixture);
    wait_idle(&binding, cx);
    view.update(cx, |view, cx| {
        view.pin_clock(1_000_000);
        view.fixtures = 3;
        cx.notify();
    });
    // Narrow host: the categories are one horizontally scrolling tab row.
    cx.simulate_resize(size(px(648.0), px(720.0)));
    draw(cx);
    draw(cx);
    cx.update_window_entity(&view, |view, window, cx| view.select(0, window, cx));
    draw(cx);
    let accounts = cx
        .debug_bounds("settings-category-0")
        .expect("the accounts tab is measured");
    let band = view
        .read_with(cx, |view, _| view.category_underline_rect())
        .expect("the strip paints its band");
    assert_eq!(band.left(), accounts.left());
    assert_eq!(band.size.width, accounts.size.width);
    assert_eq!(band.bottom(), accounts.bottom());

    cx.simulate_keystrokes("right");
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.category.clone()),
        Category::Fixture(1)
    );
    let still = view
        .read_with(cx, |view, _| view.category_underline_rect())
        .expect("band");
    assert!(
        (still.left() - accounts.left()).abs() <= px(0.1),
        "the band has not jumped ahead of its own move yet"
    );

    let target = cx
        .debug_bounds("settings-category-1")
        .expect("fixture tab measured");
    view.update(cx, |view, _| {
        view.pin_clock(1_000_000 + crate::ui::tab_underline::TAB_UNDERLINE_DURATION_MS / 2)
    });
    draw(cx);
    let midway = view
        .read_with(cx, |view, _| view.category_underline_rect())
        .expect("band");
    assert!(midway.left() > accounts.left() && midway.left() < target.left());
    assert!(
        view.read_with(cx, |view, _| view.category_underline_is_animating(
            1_000_000 + crate::ui::tab_underline::TAB_UNDERLINE_DURATION_MS / 2
        )),
        "a moving band asks for the next frame"
    );

    view.update(cx, |view, _| {
        view.pin_clock(1_000_000 + crate::ui::tab_underline::TAB_UNDERLINE_DURATION_MS)
    });
    draw(cx);
    let landed = view
        .read_with(cx, |view, _| view.category_underline_rect())
        .expect("band");
    assert_eq!(landed.left(), target.left());
    assert_eq!(landed.size.width, target.size.width);
    assert_eq!(landed.bottom(), target.bottom());
    assert!(
        !view.read_with(cx, |view, _| view.category_underline_is_animating(
            1_000_000 + crate::ui::tab_underline::TAB_UNDERLINE_DURATION_MS
        )),
        "a settled band stops requesting frames"
    );
}

/// Every narrow category tab's measured box, in strip order.
#[cfg(target_os = "windows")]
fn category_bounds(
    view: &Entity<SettingsView>,
    count: usize,
    cx: &mut VisualTestContext,
) -> Vec<gpui::Bounds<gpui::Pixels>> {
    (0..=count)
        .map(|index| {
            let selector: &'static str = match index {
                0 => "settings-category-0",
                1 => "settings-category-1",
                2 => "settings-category-2",
                3 => "settings-category-3",
                4 => "settings-category-4",
                _ => unreachable!("this test uses at most five categories"),
            };
            let _ = &view;
            cx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("category {index} must be measured"))
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn assert_categories_kept_their_boxes(
    before: &[gpui::Bounds<gpui::Pixels>],
    after: &[gpui::Bounds<gpui::Pixels>],
) {
    for (index, (before, after)) in before.iter().zip(after).enumerate() {
        assert!(
            (before.left() - after.left()).abs() <= px(0.01),
            "category {index}: origin moved with the selection: {before:?} -> {after:?}"
        );
        assert!(
            (before.size.width - after.size.width).abs() <= px(0.01),
            "category {index}: width changed with the selection: {before:?} -> {after:?}"
        );
    }
}

/// With the bundled Regular/Medium faces a selected category used to be wider than the same
/// category unselected, shifting the tabs to its right. The reserved active-weight width keeps every
/// tab's `origin.x` and `size.width` identical across inactive <-> active.
#[cfg(target_os = "windows")]
#[test]
fn narrow_tab_boxes_keep_their_geometry_when_the_category_changes_with_bundled_fonts() {
    let mut cx = native_font_context();
    let fixture = Fixture::open(Scenario::Normal);
    let (view, binding, cx) = mount_themed(&mut cx, &fixture);
    wait_idle(&binding, cx);
    view.update(cx, |view, cx| {
        view.pin_clock(1_000_000);
        view.fixtures = 3;
        cx.notify();
    });
    // Narrow host: the categories are one horizontally scrolling tab row.
    cx.simulate_resize(size(px(648.0), px(720.0)));
    draw(cx);
    draw(cx);
    cx.update_window_entity(&view, |view, window, cx| view.select(0, window, cx));
    draw(cx);
    let accounts_active = category_bounds(&view, 3, cx);

    cx.update_window_entity(&view, |view, window, cx| view.select(1, window, cx));
    view.update(cx, |view, _| view.pin_clock(2_000_000));
    draw(cx);
    let fixture_active = category_bounds(&view, 3, cx);
    assert_categories_kept_their_boxes(&accounts_active, &fixture_active);

    // And back: the previously active tab must not shrink either.
    cx.update_window_entity(&view, |view, window, cx| view.select(0, window, cx));
    view.update(cx, |view, _| view.pin_clock(3_000_000));
    draw(cx);
    let accounts_again = category_bounds(&view, 3, cx);
    assert_categories_kept_their_boxes(&accounts_active, &accounts_again);
}
