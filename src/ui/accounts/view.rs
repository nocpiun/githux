//! Native Accounts controls. Events queue work; only UI-thread `poll` touches the shared runtime.
//! No storage/network work or raw avatar URL fetch occurs in this surface.
use super::controller::{
    AccountsController, Effect, Mode, Presentation, Recovery, Tone, authorization_state,
    display_name, failure_text, startup_text,
};
use crate::app::pages::{NavigationSink, PageNavigation};
use crate::app::runtime::{
    Command, Operation, RepositoryList, RepositoryListOrigin, RuntimeHandle, Startup,
};
use crate::domain::{AccountKey, AccountSummary};
use crate::ui::foundation::{controls, theme as t};
use crate::workspace::Route;
use nsh_core::{element::ImageSrc, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

/// Integrator-owned cache/auth adapter. Called on poll, never in render. It returns only prepared
/// image sources and must use the runtime's current account binding, including revocation checks.
/// Returning None immediately removes a formerly displayed picture. No raw remote URL is fetched
/// by AccountsView; the adapter must return a local asset/data source.
#[derive(Clone, PartialEq)]
pub struct AccountAvatar(ImageSrc);
impl AccountAvatar {
    /// Call on the avatar worker after its existing size/decode/auth checks, then cache this value.
    /// This constructor cannot produce a remote URL or attach an account credential.
    pub fn from_bytes(bytes: &[u8], mime: &str) -> Option<Self> {
        use base64::Engine;
        if !matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            return None;
        }
        Some(Self(ImageSrc::from(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))))
    }
}
pub trait AccountAvatarProvider {
    fn avatar(&self, account: &AccountKey) -> Option<AccountAvatar>;
}
struct Inner {
    controller: RefCell<AccountsController>,
    text: Signal<String>,
    tick: Signal<u64>,
    input_epoch: Signal<u64>,
    retired: Signal<bool>,
    revealed: Signal<bool>,
    show_repositories: bool,
    navigation: NavigationSink,
    avatars: RefCell<Option<Rc<dyn AccountAvatarProvider>>>,
    pictures: RefCell<HashMap<AccountKey, AccountAvatar>>,
    add_focus: ElementRef,
    replace_focus: RefCell<HashMap<AccountKey, ElementRef>>,
    origin: RefCell<Option<ElementRef>>,
    composing: Cell<bool>,
}
#[derive(Clone)]
pub struct AccountsView(Rc<Inner>);
impl AccountsView {
    /// Construct inside the NSH reactive scope; the shell owns and polls the RuntimeHandle.
    pub fn new(navigation: NavigationSink) -> Self {
        Self(Rc::new(Inner {
            controller: RefCell::new(AccountsController::new()),
            text: signal(String::new()),
            tick: signal(0),
            input_epoch: signal(0),
            retired: signal(false),
            revealed: signal(false),
            show_repositories: false,
            navigation,
            avatars: RefCell::new(None),
            pictures: RefCell::new(HashMap::new()),
            add_focus: element_ref(),
            replace_focus: RefCell::new(HashMap::new()),
            origin: RefCell::new(None),
            composing: Cell::new(false),
        }))
    }
    /// Developer example only; product Settings uses the shell's repository sidebar.
    pub fn with_repository_preview(mut self) -> Self {
        if let Some(inner) = Rc::get_mut(&mut self.0) {
            inner.show_repositories = true;
        }
        self
    }
    pub fn install_avatar_provider(&mut self, provider: Rc<dyn AccountAvatarProvider>) {
        *self.0.avatars.borrow_mut() = Some(provider);
    }
    pub fn element(&self) -> AnyElement {
        let state = self.0.clone();
        dynamic(move || {
            if state.retired.get() {
                return view().into_element();
            }
            let _ = state.tick.get();
            let p = state.controller.borrow().presentation();
            view()
                .w_full()
                .min_w_0()
                .flex_col()
                .gap_4()
                .child(heading(&state, &p))
                .child(status(&state, &p))
                .child(account_list(&state, &p))
                .child(form(&state, &p))
                .child(if state.show_repositories {
                    repositories(&state, &p)
                } else {
                    view().into_element()
                })
                .into_element()
        })
    }
    /// Called on the UI thread even for an inactive Settings page. Snapshot is authoritative,
    /// including an empty directory and invalidation of already displayed repository content.
    pub fn poll(&mut self, runtime: &mut RuntimeHandle) {
        if self.0.retired.get_untracked() {
            return;
        }
        let effects = self.0.controller.borrow_mut().take_effects();
        for effect in effects {
            match effect {
                Effect::Submit(command) => {
                    let op = operation(&command);
                    match runtime.submit(command) {
                        Ok(()) => self.0.controller.borrow_mut().submitted(op),
                        Err(error) => self.0.controller.borrow_mut().note_submit_error(error),
                    }
                }
                Effect::Cancel => runtime.cancel(),
                Effect::OpenExternal(url) => (self.0.navigation)(PageNavigation::External(url)),
            }
        }
        let snapshot = runtime.snapshot();
        let was_open = self.0.controller.borrow().presentation().form_open;
        self.0.controller.borrow_mut().end_poll();
        let changed = self.0.controller.borrow_mut().ingest(&snapshot);
        let mut pictures = HashMap::new();
        if let Some(provider) = self.0.avatars.borrow().as_ref() {
            for account in &snapshot.accounts {
                if let Some(image) = provider.avatar(&account.identity.key) {
                    pictures.insert(account.identity.key.clone(), image);
                }
            }
        }
        let pictures_changed = *self.0.pictures.borrow() != pictures;
        *self.0.pictures.borrow_mut() = pictures;
        if changed || pictures_changed {
            self.0.bump();
        }
        if was_open && !self.0.controller.borrow().presentation().form_open {
            self.0.restore_focus();
        }
    }
    /// Immediately removes displayed data, queued work and the transient field. Runtime remains
    /// shell-owned: retiring one surface must not cancel a mutation started in another.
    pub fn retire(&mut self) {
        self.0.controller.borrow_mut().retire();
        self.0.pictures.borrow_mut().clear();
        self.0.text.set(String::new());
        self.0.retired.set(true);
    }
}
impl Inner {
    fn bump(&self) {
        if self.retired.get_untracked() {
            return;
        }
        let text = self.controller.borrow().input_text().to_owned();
        if self.text.get_untracked() != text {
            self.text.set(text);
        }
        self.tick.update(|n| *n += 1);
    }
    fn reset_input(&self) {
        self.text.set(String::new());
        self.composing.set(false);
        self.revealed.set(false);
        // A fresh native field drops the previous edit/undo history, even if its controlled value
        // was already empty. Changing only Value would not provide this guarantee.
        self.input_epoch.update(|n| *n += 1);
    }
    fn restore_focus(&self) {
        if let Some(origin) = *self.origin.borrow() {
            origin.focus();
        }
    }
}
pub(super) fn operation(command: &Command) -> Operation {
    match command {
        Command::Add(_) => Operation::Add,
        Command::Replace { .. } => Operation::Replace,
        Command::Select(_) => Operation::Select,
        Command::Revalidate(_) => Operation::Revalidate,
        Command::Remove(_) => Operation::Remove,
        Command::Recover => Operation::Recover,
        Command::RetryRepositories => Operation::Repositories,
    }
}
fn heading(state: &Rc<Inner>, p: &Presentation) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            t::label("Accounts")
                .text_size(px(18.0))
                .font_weight(FontWeight::SEMIBOLD)
                .test_id("accounts-title"),
        )
        .child(button(
            "accounts-add",
            "Add account",
            !p.busy && p.startup == Startup::Ready,
            Some(state.add_focus),
            {
                let state = state.clone();
                move || {
                    state.controller.borrow_mut().start_add();
                    *state.origin.borrow_mut() = Some(state.add_focus);
                    state.reset_input();
                    state.bump();
                }
            },
        ))
        .into_element()
}
fn status(state: &Rc<Inner>, p: &Presentation) -> AnyElement {
    let mut body = view().flex_col().gap_1();
    if let Some((tone, text)) = startup_text(&p.startup) {
        body = body.child(status_line(tone, text, "accounts-startup-status"));
    }
    if p.busy {
        body = body.child(
            t::label(if p.settling {
                "Cancelling… waiting for the account change to settle"
            } else {
                "Working…"
            })
            .text_size(px(12.0))
            .test_id("accounts-busy"),
        );
    }
    if let Some(status) = &p.status {
        body = body.child(status_line(
            status.tone,
            status.text.clone(),
            "accounts-operation-status",
        ));
    }
    if !state.show_repositories
        && let RepositoryList::Failed(error) = &p.repositories
    {
        body = body.child(
            view()
                .test_id("accounts-repositories-failed")
                .child(controls::failed(
                    failure_text(error),
                    Some(Rc::new({
                        let state = state.clone();
                        move || {
                            state.controller.borrow_mut().retry_repositories();
                            state.bump();
                        }
                    })),
                )),
        );
    }
    if let Some(error) = &p.cache_error {
        body = body.child(controls::failed(
            format!("Cache unavailable ({error}); online reads remain available."),
            None,
        ));
    }
    body.into_element()
}
fn status_line(tone: Tone, text: String, id: &str) -> AnyElement {
    if matches!(tone, Tone::Error | Tone::Warning) {
        view()
            .test_id(id)
            .child(controls::failed(text, None))
            .into_element()
    } else {
        view()
            .test_id(id)
            .child(
                t::label(text)
                    .text_size(px(12.0))
                    .text_color(t::theme().muted_foreground),
            )
            .into_element()
    }
}
fn account_list(state: &Rc<Inner>, p: &Presentation) -> AnyElement {
    let mut body = view().w_full().min_w_0().flex_col().gap_2();
    if p.accounts.is_empty() {
        body = body.child(
            t::label("No saved accounts. Add an account to get started.").test_id("accounts-empty"),
        );
    }
    for account in &p.accounts {
        body = body.child(account_row(state, p, account));
    }
    if p.recovery != Recovery::None {
        let message = match &p.recovery {
            Recovery::Pending => "Retired credential cleanup is pending.".into(),
            Recovery::Unreadable(error) => format!("Cleanup status could not be read: {error}"),
            Recovery::None => unreachable!(),
        };
        body = body.child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .test_id("accounts-recovery")
                .child(controls::failed(message, None))
                .child(button(
                    "accounts-recover",
                    "Retry cleanup",
                    !p.busy,
                    None,
                    {
                        let state = state.clone();
                        move || {
                            state.controller.borrow_mut().recover();
                            state.bump();
                        }
                    },
                )),
        );
    }
    body.into_element()
}
fn account_row(state: &Rc<Inner>, p: &Presentation, account: &AccountSummary) -> AnyElement {
    let key = account.identity.key.clone();
    let active = p.active.as_ref() == Some(&key);
    let enabled = !p.busy && p.startup == Startup::Ready;
    let name = display_name(account);
    let picture = state
        .pictures
        .borrow()
        .get(&key)
        .map(|picture| picture.0.clone());
    let (auth, warning) = authorization_state(&account.authorization);
    let mut actions = view().flex_row().gap_1().flex_wrap();
    if !active {
        actions = actions.child(button(
            &format!("account-switch-{}", key.id),
            "Switch",
            enabled,
            None,
            {
                let (state, key) = (state.clone(), key.clone());
                move || {
                    state.controller.borrow_mut().select(key.clone());
                    state.reset_input();
                    state.bump();
                }
            },
        ));
    }
    actions = actions.child(button(
        &format!("account-revalidate-{}", key.id),
        "Revalidate",
        enabled,
        None,
        {
            let (state, key) = (state.clone(), key.clone());
            move || {
                state.controller.borrow_mut().revalidate(key.clone());
                state.bump();
            }
        },
    ));
    let focus = *state
        .replace_focus
        .borrow_mut()
        .entry(key.clone())
        .or_insert_with(element_ref);
    actions = actions.child(button(
        &format!("account-replace-{}", key.id),
        "Replace PAT",
        enabled,
        Some(focus),
        {
            let (state, key) = (state.clone(), key.clone());
            move || {
                state.controller.borrow_mut().start_replace(key.clone());
                *state.origin.borrow_mut() = Some(focus);
                state.reset_input();
                state.bump();
            }
        },
    ));
    let confirming = p.confirm_remove.as_ref() == Some(&key);
    actions = actions.child(button(
        &format!("account-remove-{}", key.id),
        "Remove",
        enabled && !confirming,
        None,
        {
            let (state, key) = (state.clone(), key.clone());
            move || {
                state.controller.borrow_mut().ask_remove(key.clone());
                state.reset_input();
                state.bump();
            }
        },
    ));
    if confirming {
        actions = actions
            .child(button(
                &format!("account-remove-confirm-{}", key.id),
                "Remove account",
                enabled,
                None,
                {
                    let (state, key) = (state.clone(), key.clone());
                    move || {
                        state.controller.borrow_mut().confirm_remove(key.clone());
                        state.bump();
                    }
                },
            ))
            .child(button("accounts-keep", "Keep account", enabled, None, {
                let state = state.clone();
                move || {
                    state.controller.borrow_mut().dismiss_remove();
                    state.bump();
                }
            }));
    }
    view()
        .w_full()
        .min_w_0()
        .flex_col()
        .gap_1()
        .py_2()
        .border_b_1()
        .border_color(t::theme().border)
        .test_id(&format!("saved-account-{}", key.id))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(
                            nsh_base::avatar()
                                .size(px(28.0))
                                .rounded(px(14.0))
                                .image_style(|s| s.rounded(px(14.0)))
                                .name(name.clone())
                                .src(move || picture.clone()),
                        )
                        .child(
                            view()
                                .flex_col()
                                .min_w_0()
                                .child(t::label(name).font_weight(FontWeight::MEDIUM))
                                .child(
                                    t::label(format!("@{}", account.identity.login))
                                        .text_size(px(12.0))
                                        .text_color(t::theme().muted_foreground),
                                ),
                        )
                        .child(if active {
                            controls::badge("Current").into_element()
                        } else {
                            view().into_element()
                        }),
                )
                .child(actions),
        )
        .child(if warning {
            controls::failed(auth, None).into_element()
        } else {
            t::label(auth)
                .text_size(px(12.0))
                .text_color(t::theme().muted_foreground)
                .into_element()
        })
        .into_element()
}
fn form(state: &Rc<Inner>, p: &Presentation) -> AnyElement {
    if !p.form_open {
        return view().into_element();
    }
    let title = match &p.mode {
        Mode::Add => "Add account".to_owned(),
        Mode::Replace(key) => format!(
            "Replace PAT for @{}",
            p.accounts
                .iter()
                .find(|a| &a.identity.key == key)
                .map(|a| a.identity.login.as_str())
                .unwrap_or("")
        ),
    };
    let input = {
        let state = state.clone();
        dynamic(move || {
            let generation = state.input_epoch.get();
            nsh_core::element::input()
                .key(format!("pat-{generation}"))
                .masked({
                    let state = state.clone();
                    move || !state.revealed.get()
                })
                .auto_correct(false)
                .auto_capitalize(AutoCapitalize::None)
                .value(state.text)
                .placeholder("Paste a Classic PAT")
                .auto_focus(true)
                .h(px(32.0))
                .w_full()
                .test_id("accounts-token-input")
                .readonly({
                    let state = state.clone();
                    move || {
                        let _ = state.tick.get();
                        state.controller.borrow().busy()
                    }
                })
                .on_change({
                    let state = state.clone();
                    move |text: &str| {
                        if !state.controller.borrow().busy() {
                            state.controller.borrow_mut().set_input(text);
                            state.text.set(text.to_owned());
                        }
                    }
                })
                // Native SubmitEditing is emitted after IME confirmation, not by a global Enter
                // handler; no key interceptor steals the candidate-confirmation key.
                .on_composition_change({
                    let state = state.clone();
                    move |composition: &Composition| {
                        state.composing.set(composition.composing);
                    }
                })
                .on_key_press({
                    let state = state.clone();
                    move |key: &str| {
                        if key == "Escape" && !state.composing.get() {
                            state.controller.borrow_mut().cancel();
                            state.reset_input();
                            state.bump();
                            state.restore_focus();
                        }
                    }
                })
                .on_submit({
                    let state = state.clone();
                    move |text: &str| {
                        if !state.composing.get() {
                            submit(&state, text);
                        }
                    }
                })
                .into_element()
        })
    };
    let actions = {
        let state = state.clone();
        dynamic(move || {
            let _ = state.tick.get();
            let text = state.text.get();
            let p = state.controller.borrow().presentation();
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(button(
                    "accounts-reveal",
                    if state.revealed.get() {
                        "Hide token"
                    } else {
                        "Show token"
                    },
                    !p.busy,
                    None,
                    {
                        let state = state.clone();
                        move || {
                            state.revealed.update(|revealed| *revealed = !*revealed);
                        }
                    },
                ))
                .child(button("accounts-help", "About Classic PAT", true, None, {
                    let state = state.clone();
                    move || {
                        state.controller.borrow_mut().toggle_help();
                        state.bump();
                    }
                }))
                .child(button(
                    "accounts-cancel",
                    if p.busy { "Cancel operation" } else { "Cancel" },
                    !p.settling,
                    None,
                    {
                        let state = state.clone();
                        move || {
                            state.controller.borrow_mut().cancel();
                            state.reset_input();
                            state.bump();
                            state.restore_focus();
                        }
                    },
                ))
                .child(button(
                    "accounts-submit",
                    if matches!(p.mode, Mode::Replace(_)) {
                        "Verify and replace"
                    } else {
                        "Verify and add"
                    },
                    !p.busy && !text.trim().is_empty() && p.startup == Startup::Ready,
                    None,
                    {
                        let state = state.clone();
                        move || submit(&state, &state.text.get_untracked())
                    },
                ))
                .into_element()
        })
    };
    view().w_full().min_w_0().flex_col().gap_2().pt_3().test_id("accounts-token-form")
        .child(t::label(title).font_weight(FontWeight::SEMIBOLD).test_id("accounts-form-title"))
        .child(t::label(if matches!(p.mode,Mode::Replace(_)) {
            "Enter a Classic PAT for this same account. A different identity is rejected."
        } else { "A Classic PAT is verified before it is saved." }).text_size(px(12.0)))
        .child(input).child(actions)
        .child(if p.help_open {
            view().flex_col().gap_1()
                .child(t::label("Public repositories need no extra scope. Private repositories need repo, which also grants write access. Notifications and organization SSO may need additional authorization.").text_size(px(12.0)))
                .child(button("accounts-create-pat", "Create a Classic PAT on GitHub", true, None, {
                    let state=state.clone(); move || { state.controller.borrow_mut().open_pat_help(); state.bump(); }
                })).into_element()
        } else { view().into_element() }).into_element()
}
fn submit(state: &Rc<Inner>, text: &str) {
    let submitted = state.controller.borrow_mut().submit(text);
    if submitted {
        state.reset_input();
    }
    state.bump();
}
fn repositories(state: &Rc<Inner>, p: &Presentation) -> AnyElement {
    let mut section = view().flex_col().gap_1();
    let retry = {
        let state = state.clone();
        Rc::new(move || {
            state.controller.borrow_mut().retry_repositories();
            state.bump();
        }) as Rc<dyn Fn()>
    };
    match &p.repositories {
        RepositoryList::Idle => {
            section = section.child(
                t::label("Choose an account to read repositories.")
                    .test_id("accounts-repositories-idle"),
            )
        }
        RepositoryList::Loading => {
            section = section.child(t::label("Loading…").test_id("accounts-repositories-loading"))
        }
        RepositoryList::Failed(error) => {
            section = section.child(
                view()
                    .test_id("accounts-repositories-failed")
                    .child(controls::failed(failure_text(error), Some(retry.clone()))),
            )
        }
        RepositoryList::Ready { outcome, origin } => {
            section = section.child(
                t::label(format!("{} repositories", outcome.repositories.len()))
                    .test_id("accounts-repositories-ready"),
            );
            if let RepositoryListOrigin::Historical { verified_at_ms } = origin {
                section = section.child(
                    t::label(format!(
                        "Offline snapshot · {}",
                        relative_time(*verified_at_ms)
                    ))
                    .text_size(px(12.0)),
                );
            }
            if matches!(
                outcome.readiness,
                crate::domain::repo::ReposReadiness::Partial { .. }
            ) {
                section = section.child(controls::failed(
                    "Partial repository list · Retry to read the remaining pages",
                    Some(retry),
                ));
            }
            for repo in &outcome.repositories {
                let route = Route::repository(repo);
                let state = state.clone();
                section = section.child(
                    nsh_base::button()
                        .on_press(move |_: &()| {
                            (state.navigation)(PageNavigation::Open(route.clone()));
                        })
                        .child(t::label(repo.full_name.clone())),
                );
            }
        }
    }
    section.into_element()
}
// Same accepted wording as ui::relative_time (which accepts RFC3339 rather than epoch milliseconds).
fn relative_time(ms: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i128)
        .unwrap_or(0);
    if i128::from(ms) > now {
        return "In the future".into();
    }
    let seconds = (now - i128::from(ms)) / 1000;
    let (value, unit) = match seconds {
        0..60 => return "Just now".into(),
        60..3600 => (seconds / 60, "minute"),
        3600..86400 => (seconds / 3600, "hour"),
        86400..172800 => return "Yesterday".into(),
        _ => (seconds / 86400, "day"),
    };
    format!("{value} {unit}{} ago", if value == 1 { "" } else { "s" })
}
fn button(
    id: &str,
    title: &'static str,
    enabled: bool,
    focus: Option<ElementRef>,
    action: impl Fn() + 'static,
) -> AnyElement {
    // This NSH revision does not expose test_id on ButtonElement. A child identifier resolves to
    // the native ancestor button, keeping tests on its actual disabled/press semantics.
    let mut button = nsh_base::button()
        .h(px(28.0))
        .px_2()
        .rounded(t::theme().radius)
        .accessibility_label(title)
        .disabled(!enabled)
        .hover(|s| s.bg(t::theme().accent))
        .focus(|s| s.bg(t::theme().accent))
        .on_press(move |_: &()| action())
        .child(
            view()
                .test_id(id)
                .child(t::label(title).text_size(px(13.0))),
        );
    if let Some(focus) = focus {
        button = button.element_ref(&focus);
    }
    button.into_element()
}
