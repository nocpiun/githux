//! Tests are compiled in the existing Cargo example target (no duplicate whole-app test target).
pub(super) use super::accounts::authorization;
pub(super) use super::{SettingsBinding, SettingsView, binding::FormMode};
use crate::domain::{account::AuthorizationState, error::GitHubError};
pub(super) use crate::{
    app::accounts::Accounts,
    domain::{AccountKey, HostId},
    settings_fixture::{Fixture, Scenario},
};
pub(super) use gpui::{
    AppContext, Entity, Focusable, TestAppContext, VisualContext, VisualTestContext, px, size,
};

pub(super) fn key(id: u64) -> AccountKey {
    AccountKey::new(id, HostId::github())
}

pub(super) fn mount<'a>(
    cx: &'a mut TestAppContext,
    fixture: &Fixture,
) -> (
    Entity<SettingsView>,
    Entity<SettingsBinding>,
    &'a mut VisualTestContext,
) {
    cx.update(ely_core::init_for_tests);
    let runtime = fixture.runtime.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let controller = cx.new(|cx| Accounts::new(Some(runtime), window, cx));
        let binding = cx.new(|cx| SettingsBinding::new(controller, window, cx));
        SettingsView::new(binding, window, cx)
    });
    let binding = view.read_with(cx, |view, _| view.binding.clone());
    cx.simulate_resize(size(px(1000.0), px(820.0)));
    draw(cx);
    (view, binding, cx)
}
pub(super) fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
}
pub(super) fn wait_idle(binding: &Entity<SettingsBinding>, cx: &mut VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        draw(cx);
        if !binding.read_with(cx, |this, cx| this.controller.read(cx).busy()) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "account operation did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
pub(super) fn open(binding: &Entity<SettingsBinding>, mode: FormMode, cx: &mut VisualTestContext) {
    cx.update_window_entity(binding, |this, window, cx| this.open(mode, window, cx));
    draw(cx);
}
pub(super) fn put(binding: &Entity<SettingsBinding>, text: &str, cx: &mut VisualTestContext) {
    let input = binding.read_with(cx, |this, _| this.input.clone());
    input.update(cx, |input, cx| input.set_text(text, cx));
    draw(cx);
}

#[test]
fn authorization_distinguishes_missing_expired_and_connectivity() {
    let missing = authorization(&AuthorizationState::MissingCredential).0;
    let expired = authorization(&AuthorizationState::VerificationFailed(
        GitHubError::Unauthorized,
    ))
    .0;
    let network = authorization(&AuthorizationState::VerificationFailed(
        GitHubError::Network {
            message: "fixture".into(),
        },
    ))
    .0;
    assert!(missing.contains("missing"));
    assert!(expired.contains("expired"));
    assert!(network.contains("connect"));
    assert_ne!(missing, expired);
    assert_eq!(
        authorization(&AuthorizationState::PendingVerification).0,
        "Verification pending · Revalidate to check"
    );
}
