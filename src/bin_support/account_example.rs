//! Developer-only account preview window.
//!
//! This file is reachable only from `examples/account.rs` and `tests/account.rs` through the thin
//! `bin_support` module assembly; the product binary never declares it. It exists so a developer can
//! drive the real account controller and the real repository read in a standalone window without
//! adding a preview flag, a scenario selector or an exit hook to `app::run`.
//!
//! The window is thin on purpose: it owns one scroll region (a real `ScrollHandle`, the controller's
//! shared `Motion` and `Bars`) and renders the product's own account section. Every account rule —
//! generation, cancellation, reconciliation, repository invalidation — lives in the product
//! controller that the Shell also uses.

// This developer-only window exposes the product controller's state for the thin test assembly. It
// is a separate Cargo target, never part of the product module graph, so its extra accessors are
// intentionally narrow here rather than widening the product's API surface.
#![allow(dead_code)]

use std::time::Duration;

use ely_core::theme::ActiveTheme;
use gpui::{
    App, Context, FocusHandle, Focusable, IntoElement, ParentElement, Render, ScrollHandle, Styled,
    Task, Window, div, prelude::*, px, size,
};

use crate::app::accounts::{Accounts, callbacks};
use crate::ui::accounts::{AccountsSection, TokenForm, accounts_section};
use crate::ui::scroll::{Bar, BarId, Bars, Motion, Placement, Style, cancel_region, wheel_overlay};

/// A standalone window around one product account controller.
pub struct AccountPreview {
    accounts: gpui::Entity<Accounts>,
    scroll: ScrollHandle,
    motion: Motion<ScrollHandle>,
    bars: Bars,
    focus: FocusHandle,
    clock: std::time::Instant,
    #[cfg(test)]
    clock_pinned: Option<u64>,
    _receiver: Task<()>,
}

impl AccountPreview {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_runtime(None, window, cx)
    }

    /// Builds the preview around an injected runtime, which is how the integration tests drive the
    /// real service path with a temporary database and an injected transport.
    pub fn with_runtime(
        runtime: Option<std::sync::Arc<crate::app::accounts::runtime::AccountRuntime>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let accounts = cx.new(|cx| Accounts::new(runtime, window, cx));
        let scroll = ScrollHandle::new();
        let focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx);
        let bars = Bars::new();
        Self {
            accounts,
            motion: Motion::new(scroll.clone()),
            scroll,
            bars,
            focus,
            clock: std::time::Instant::now(),
            #[cfg(test)]
            clock_pinned: None,
            _receiver: cx.spawn(async |this: gpui::WeakEntity<Self>, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }),
        }
    }

    /// The product controller, for tests that inspect authoritative state.
    #[allow(dead_code)]
    pub(crate) fn accounts(&self) -> &gpui::Entity<Accounts> {
        &self.accounts
    }

    /// The preview window's scroll region, so its long-list behaviour can be exercised.
    pub(crate) fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub(crate) fn bars_mut(&mut self) -> &mut Bars {
        &mut self.bars
    }

    fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(pinned) = self.clock_pinned {
            return pinned;
        }
        self.clock.elapsed().as_millis() as u64
    }

    #[cfg(test)]
    pub fn pin_clock(&mut self, ms: u64) {
        self.clock_pinned = Some(ms);
    }

    pub(crate) fn settle_scroll(&mut self, cx: &App) {
        let target = self.scroll.clone();
        self.bars.settle(&target, self.now_ms(), cx.reduce_motion());
    }
}

impl Focusable for AccountPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AccountPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now_ms();
        if self.bars.dragging().is_some() && !cx.has_active_drag() {
            let target = self.scroll.clone();
            if self.bars.settle(&target, now, cx.reduce_motion()) {
                cx.notify();
            }
        }
        if self.bars.is_animating() {
            window.request_animation_frame();
        }
        let opacity = self.bars.opacity(now);
        let snapshot = self.accounts.read(cx).snapshot().clone();
        let busy = self.accounts.read(cx).busy();
        let replacing = self.accounts.read(cx).replacing().is_some();
        // Resolve each saved account's avatar through the same controller the product uses.
        let mut avatars = std::collections::HashMap::new();
        for summary in &snapshot.accounts {
            if let Some(key) = crate::ui::avatar::account_key(&summary.identity) {
                let state = self.accounts.update(cx, |accounts, _| accounts.avatar(key));
                if let crate::app::avatar::AvatarState::Ready(image) = state {
                    avatars.insert(
                        summary.identity.key.clone(),
                        gpui::ImageSource::Image(image),
                    );
                }
            }
        }
        let callbacks = callbacks(self.accounts.clone(), snapshot.status.clone());
        let section = accounts_section(
            AccountsSection {
                token: TokenForm {
                    token_input: self.accounts.read(cx).token_input().clone(),
                    replacing,
                    busy,
                    callbacks: &callbacks,
                },
                accounts: snapshot.accounts,
                active: snapshot.active,
                avatars,
                status: snapshot.status,
            },
            cx,
        );

        let owner = cx.entity_id();
        let drag_owner = cx.entity();
        let on_drag: crate::ui::scroll::OnDragging =
            std::rc::Rc::new(move |axis, dragging, _, cx| {
                drag_owner.update(cx, |preview, cx| {
                    let target = preview.scroll.clone();
                    let reduced = cx.reduce_motion();
                    if preview
                        .bars
                        .drag(&target, axis, dragging, preview.now_ms(), reduced)
                    {
                        cx.notify();
                    }
                });
            });
        let bar = Bar {
            target: &self.motion,
            id: BarId {
                owner,
                name: "account-preview-scrollbar-y",
                axis: gpui::Axis::Vertical,
            },
            placement: Placement::overlay(4.0, 8.0),
            style: Style::DEFAULT,
            opacity,
            on_drag,
        }
        .render(cx);

        div()
            .id("account-preview-root")
            .debug_selector(|| "account-preview-scroll-region".to_string())
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(cx.theme().colors.bg)
            .on_hover(cx.listener(|preview, hovered: &bool, _, cx| {
                let reduced = cx.reduce_motion();
                if preview.bars.hover(*hovered, preview.now_ms(), reduced) {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("account-preview-scroll")
                    .debug_selector(|| "account-preview-scroll".to_string())
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(section),
            )
            .child(wheel_overlay(
                "account-preview",
                gpui::Axis::Vertical,
                self.motion.clone(),
            ))
            .child(bar)
    }
}

impl Drop for AccountPreview {
    fn drop(&mut self) {
        let target = self.scroll.clone();
        cancel_region(&mut self.bars, &self.motion);
        let _ = target;
    }
}

/// Opens the standalone preview window with the platform title bar.
pub fn open_window(cx: &mut App) {
    let bounds = gpui::Bounds::centered(None, size(px(1120.0), px(850.0)), cx);
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(760.0), px(540.0))),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(gpui::SharedString::from("Githux · Account wiring preview")),
                ..Default::default()
            }),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| AccountPreview::new(window, cx)),
    )
    .expect("failed to open account preview");
}
