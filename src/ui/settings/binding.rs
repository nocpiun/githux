//! A host binding for the existing account controller; no service implementation lives here.
use std::{collections::HashMap, rc::Rc};

use ely_core::forms::{InputEvent, TextInput};
use gpui::{App, AppContext, Context, Entity, FocusHandle, Focusable, Subscription, Window};

use crate::{
    app::accounts::Accounts,
    domain::{AccountKey, AuthEpoch},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FormMode {
    Add,
    Replace(AccountKey),
}

pub(crate) type SettingsAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// Stable host seam: construct from the host's existing controller, then mount SettingsView.
/// Form state is view-local. Authentication, cancellation and reconciliation stay in Accounts.
pub(crate) struct SettingsBinding {
    pub controller: Entity<Accounts>,
    pub mode: Option<FormMode>,
    pub input: Entity<TextInput>,
    pub help: bool,
    /// Controller outcome applied for the current submitted token operation. Cleared on failure so
    /// unrelated later operations cannot be mistaken for this submission's commit.
    pub submitted: bool,
    pub field_error: Option<&'static str>,
    /// Whether this mounted page has seen an account action. It suppresses the controller's
    /// startup "restoring…" message without rewriting controller state.
    pub activity: bool,
    pub add_focus: FocusHandle,
    pub replace_focus: HashMap<AccountKey, FocusHandle>,
    origin: Option<FocusHandle>,
    before_submit: Vec<(AccountKey, AuthEpoch)>,
    changed: Subscription,
    observed: Subscription,
    keys: Option<Subscription>,
}

impl SettingsBinding {
    pub fn new(controller: Entity<Accounts>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (input, changed) = Self::input(window, cx);
        let observed = cx.observe(&controller, |_, _, cx| cx.notify());
        let weak = cx.weak_entity();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.mode.is_none() || !this.input.read(cx).focus_handle(cx).is_focused(window) {
                    return;
                }
                let composing = this.input.update(cx, |input, cx| {
                    gpui::EntityInputHandler::marked_text_range(input, window, cx).is_some()
                });
                if composing {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "enter" => {
                        cx.stop_propagation();
                        this.submit(window, cx);
                    }
                    "escape" => {
                        cx.stop_propagation();
                        this.close(window, cx);
                    }
                    _ => {}
                }
            });
        });
        Self {
            controller,
            mode: None,
            input,
            help: false,
            submitted: false,
            field_error: None,
            activity: false,
            add_focus: cx.focus_handle().tab_stop(true),
            replace_focus: HashMap::new(),
            origin: None,
            before_submit: Vec::new(),
            changed,
            observed,
            keys: Some(keys),
        }
    }

    fn input(window: &mut Window, cx: &mut Context<Self>) -> (Entity<TextInput>, Subscription) {
        let input = cx.new(|cx| {
            TextInput::new(window, cx)
                .masked()
                .label("Classic personal access token")
                .placeholder("Paste a Classic PAT")
        });
        let changed = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if *event == InputEvent::Changed {
                this.field_error = None;
            }
            cx.notify();
        });
        (input, changed)
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Replacing the entity drops the complete secret edit/undo history.
        (self.input, self.changed) = Self::input(window, cx);
    }

    pub fn open(&mut self, mode: FormMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.controller.read(cx).busy() {
            return;
        }
        if self.mode.as_ref() == Some(&mode) {
            return;
        }
        self.reset(window, cx);
        self.origin = Some(match &mode {
            FormMode::Add => self.add_focus.clone(),
            FormMode::Replace(key) => self
                .replace_focus
                .entry(key.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone(),
        });
        let replacement = match &mode {
            FormMode::Replace(key) => Some(key.clone()),
            _ => None,
        };
        self.controller
            .update(cx, |controller, cx| match replacement {
                Some(key) => controller.replace(key, cx),
                None => controller.cancel_replace(cx),
            });
        let input_focus = self.input.read(cx).focus_handle(cx);
        window.defer(cx, move |window, cx| window.focus(&input_focus, cx));
        self.mode = Some(mode);
        self.field_error = None;
        self.submitted = false;
        self.help = false;
        cx.notify();
    }

    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.is_none() || self.controller.read(cx).busy() {
            return;
        }
        // The secret travels directly from the input to the controller's existing submission seam.
        if self.input.read(cx).text().trim().is_empty() {
            self.field_error = Some("Paste a Classic PAT before verifying.");
            window.focus(&self.input.read(cx).focus_handle(cx), cx);
            cx.notify();
            return;
        }
        self.field_error = None;
        self.activity = true;
        self.before_submit = self
            .controller
            .read(cx)
            .snapshot()
            .accounts
            .iter()
            .map(|summary| (summary.identity.key.clone(), summary.epoch))
            .collect();
        let value = self.input.read(cx).text().to_owned();
        let mode = self.mode.clone();
        self.controller.update(cx, |controller, cx| {
            match mode {
                Some(FormMode::Replace(key)) => controller.replace(key, cx),
                _ => controller.cancel_replace(cx),
            }
            controller
                .token_input()
                .update(cx, |input, cx| input.set_text(value, cx));
            controller.submit(window, cx);
        });
        self.reset(window, cx);
        window.focus(&self.input.read(cx).focus_handle(cx), cx);
        self.submitted = true;
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.controller.update(cx, |controller, cx| {
            if controller.busy() {
                controller.cancel(cx);
            }
            // cancel_replace's "unchanged" message is only valid before submission.
            if !self.submitted {
                controller.cancel_replace(cx);
            }
        });
        self.mode = None;
        self.submitted = false;
        self.activity = true;
        self.reset(window, cx);
        if let Some(origin) = &self.origin {
            window.focus(origin, cx);
        }
        cx.notify();
    }

    /// Called with a Window by the view, so completion restores focus to the initiating row.
    pub fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.submitted {
            return;
        }
        let controller = self.controller.read(cx);
        // A confirmed commit is proven by the authoritative directory advancing the submitted
        // identity's epoch. Do not gate this on the latest status: a successful account mutation
        // starts an automatic repository read whose later failure must stay a separate message,
        // not keep the committed form open or imply the PAT was not saved.
        let committed = match &self.mode {
            Some(FormMode::Replace(target)) => {
                controller.snapshot().accounts.iter().any(|summary| {
                    &summary.identity.key == target
                        && !self
                            .before_submit
                            .contains(&(target.clone(), summary.epoch))
                })
            }
            Some(FormMode::Add) => controller.snapshot().accounts.iter().any(|summary| {
                !self
                    .before_submit
                    .contains(&(summary.identity.key.clone(), summary.epoch))
            }),
            None => false,
        };
        if committed {
            self.mode = None;
            self.submitted = false;
            if let Some(origin) = &self.origin {
                window.focus(origin, cx);
            }
            cx.notify();
            return;
        }
        // The submission failed without an authoritative epoch change. Forget its pending
        // completion so a later unrelated operation (a row Revalidate advancing an epoch, for
        // example) is never mistaken for a successful save of this PAT. The form stays open with
        // its input for correction.
        if !controller.busy()
            && controller
                .snapshot()
                .status
                .as_ref()
                .is_some_and(|status| status.tone == crate::ui::accounts::StatusTone::Error)
        {
            self.submitted = false;
            cx.notify();
        }
    }

    pub fn select(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        self.activity = true;
        self.controller
            .update(cx, |controller, cx| controller.select(key, cx));
    }

    pub fn revalidate(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        self.activity = true;
        self.controller
            .update(cx, |controller, cx| controller.revalidate(key, cx));
    }

    pub fn action(
        this: Entity<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> SettingsAction {
        Rc::new(move |window, cx| this.update(cx, |this, cx| f(this, window, cx)))
    }
}
