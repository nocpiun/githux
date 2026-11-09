//! Native Shell Notifications inbox.
//!
//! The account-scoped controller in `data::notifications` owns request identities, pagination
//! cursors, thread-action state, cooldowns and tombstones. This module owns the page: user events
//! queue intents, `poll` dispatches them on the captured services, replies are published through a
//! `PageSlot` gated on the runtime's account/incarnation/authorization ticket, and the published
//! snapshot is bound to NSH signals. Nothing here performs I/O or takes a service lock while an
//! element is built.
//!
//! Rows follow the owner-approved GitHub inbox reference: a narrow gutter for the unread dot and
//! the subject icon, two content lines (weakened owner/name above the title) and a fixed right
//! column with the relative time above three compact actions. Read state is background + dot +
//! title weight; it is never a "Read"/"Unread" label. Relative copy reuses the shared Home
//! formatter.
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nsh_core::element::{ImageSrc, create_root};
use nsh_core::prelude::*;

use crate::app::runtime::{PageContext, PageSlot, RuntimeHandle};
use crate::data::notifications::{InboxSnapshot, Notifications, RequestKind};
use crate::domain::notification::{
    NotificationError, NotificationPartial, NotificationThread, SubjectKind, ThreadAction,
    ThreadActionState,
};
use crate::domain::repository::RepositoryIdentity;
use crate::ui::foundation::{controls, theme as t};
use crate::ui::relative_time::relative_days as relative;
use crate::workspace::Sessions;

/// The host checks its workspace/tab incarnation when consuming navigation.
pub type OnRepository = Rc<dyn Fn(RepositoryIdentity)>;

/// The unread row wash and dot. NSH's default theme is neutral, so the application's existing
/// informational blue is carried as an explicit semantic color (for the same reason foundation
/// carries `WARNING`); it is not a second theme.
const UNREAD: AdaptiveColor = AdaptiveColor {
    light: rgb(0xeff6ff).to_hsla(),
    dark: rgb(0x16202f).to_hsla(),
};
const UNREAD_DOT: AdaptiveColor = AdaptiveColor {
    light: rgb(0x0969da).to_hsla(),
    dark: rgb(0x58a6ff).to_hsla(),
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Glyph {
    Issue,
    PullRequest,
    Commit,
    Release,
    Discussion,
    #[default]
    Bell,
    Check,
    EyeOff,
    Bookmark,
}

impl Glyph {
    fn path(self) -> &'static str {
        // Lucide paths, under the repository's existing ISC notice.
        match self {
            Self::Issue => "<circle cx='12' cy='12' r='9'/><circle cx='12' cy='12' r='1'/>",
            Self::PullRequest => {
                "<circle cx='6' cy='5' r='3'/><circle cx='6' cy='19' r='3'/><circle cx='18' cy='19' r='3'/><path d='M6 8v8M18 16V9a4 4 0 0 0-4-4h-1m3-3-3 3 3 3'/>"
            }
            Self::Commit => "<circle cx='12' cy='12' r='3.5'/><path d='M3 12h5.5M15.5 12H21'/>",
            Self::Release => "<path d='M3 12V4h8l9 9-8 8-9-9z'/><circle cx='8' cy='8' r='1.4'/>",
            Self::Discussion => {
                "<path d='M21 12a8 8 0 0 1-8 8H7l-4 3V12a8 8 0 0 1 8-8h2a8 8 0 0 1 8 8z'/>"
            }
            Self::Bell => "<path d='M18 9a6 6 0 1 0-12 0c0 5-2 6-2 6h16s-2-1-2-6M10 21h4'/>",
            Self::Check => "<path d='m4 12 5 5L20 6'/>",
            Self::EyeOff => {
                "<path d='M3 3l18 18M10.6 6.2A9.7 9.7 0 0 1 12 6c5 0 9 6 9 6a17 17 0 0 1-3.2 3.7M6.6 6.7A16 16 0 0 0 3 12s4 6 9 6a9.6 9.6 0 0 0 4.4-1.1M9.9 9.9a3 3 0 0 0 4.2 4.2'/>"
            }
            Self::Bookmark => "<path d='M6 3h12v18l-6-4-6 4z'/>",
        }
    }
}

fn glyph(glyph: Glyph, color: AdaptiveColor) -> impl IntoElement {
    svg(ImageSrc::svg_markup(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'>{}</svg>",
        glyph.path()
    )))
    .size(px(16.0))
    .text_color(color)
}

/// The same ring the rest of the migrated UI uses for an in-flight read.
fn spinner(size: f32) -> View {
    view()
        .size(px(size))
        .rounded(px(size / 2.0))
        .border_2()
        .border_color(t::theme().border)
        .border_t_color(t::theme().muted_foreground)
        .animation(KeyframeAnimation::new(Keyframes::Spin).duration(800.0))
}

/// A tooltip whose text can follow the row it belongs to.
fn tip(message: impl IntoReactive<SharedString>) -> AnyElement {
    nsh_base::tooltip()
        .bg(t::theme().popover)
        .rounded(t::theme().radius)
        .px_2()
        .py_1()
        .child(
            t::label(message)
                .text_size(px(12.0))
                .text_color(t::theme().popover_foreground),
        )
        .into_element()
}

/// Everything the view shows, derived from the published snapshot plus the wall clock.
/// Equality keeps a poll that changed nothing from touching the signal (and the frame).
#[derive(Clone, Default, PartialEq)]
struct Model {
    rows: Vec<NotificationThread>,
    states: BTreeMap<String, ThreadActionState>,
    busy: Option<RequestKind>,
    has_more: bool,
    failed: Option<(NotificationError, RequestKind)>,
    partial: Vec<NotificationPartial>,
    empty: bool,
    loaded: bool,
    count: usize,
    stale: bool,
    refresh_deadline: u64,
    action_deadline: u64,
}

impl Model {
    fn cooldown(&self, now: u64) -> Option<u64> {
        (self.refresh_deadline > now).then(|| self.refresh_deadline.saturating_sub(now))
    }

    fn action_cooling(&self, now: u64) -> bool {
        self.action_deadline > now
    }

    /// The action cell of one row, derived so unrelated rows' cells are not rebuilt.
    fn action_view(&self, index: usize, now: u64) -> ActionView {
        let Some(row) = self.rows.get(index) else {
            return ActionView::default();
        };
        let state = self.states.get(&row.id).cloned();
        let pending_done = matches!(
            &state,
            Some(ThreadActionState::UnsubscribedPendingDone { .. })
        );
        let warning = match &state {
            Some(ThreadActionState::Failed { action, error }) => {
                Some((*action, format!("{error} {}", error.recovery())))
            }
            Some(ThreadActionState::UnsubscribedPendingDone { error }) => Some((
                ThreadAction::Done,
                format!(
                    "Unsubscribed; finish removing from the inbox. {error} {}",
                    error.recovery()
                ),
            )),
            _ => None,
        };
        let busy = match &state {
            Some(ThreadActionState::Busy(action)) => Some(*action),
            _ => None,
        };
        ActionView {
            blocked: self.action_cooling(now) || busy.is_some(),
            busy,
            pending_done,
            warning,
        }
    }

    fn header(&self, now: u64) -> HeaderView {
        HeaderView {
            busy: self.busy.is_some(),
            cooldown: self.cooldown(now),
            failed: self.failed.as_ref().map(|(error, kind)| {
                (
                    format!("{error} {}", error.recovery()),
                    *kind == RequestKind::More,
                )
            }),
            partial: !self.partial.is_empty(),
            partial_detail: partial_message(&self.partial),
        }
    }

    fn exact_time(&self, index: usize) -> String {
        self.rows
            .get(index)
            .map(|row| row.updated_at.clone())
            .unwrap_or_default()
    }

    fn repository(&self, index: usize) -> Option<RepositoryIdentity> {
        self.rows.get(index).map(|row| row.repository.clone())
    }

    fn thread(&self, index: usize) -> Option<&NotificationThread> {
        self.rows.get(index)
    }
}

#[derive(Clone, Default, PartialEq)]
struct ActionView {
    blocked: bool,
    busy: Option<ThreadAction>,
    /// The ignore step is already confirmed: only the remaining Done may be retried.
    pending_done: bool,
    warning: Option<(ThreadAction, String)>,
}

#[derive(Clone, Default, PartialEq)]
struct HeaderView {
    busy: bool,
    cooldown: Option<u64>,
    failed: Option<(String, bool)>,
    partial: bool,
    partial_detail: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct SubjectView {
    glyph: Glyph,
    type_label: &'static str,
}

#[derive(Clone, PartialEq, Eq)]
enum Intent {
    First,
    Refresh,
    More,
    Action { id: String, action: ThreadAction },
}

#[derive(Default)]
struct Shared {
    queue: VecDeque<Intent>,
    completion: Option<InboxSnapshot>,
}

#[derive(Clone, Copy)]
struct Signals {
    model: Signal<Model>,
    clock: Signal<u64>,
    selection: Signal<Option<usize>>,
    focused: Signal<bool>,
    list: ElementRef,
}

#[derive(Clone)]
struct Ctx {
    signals: Signals,
    shared: Rc<RefCell<Shared>>,
    controller: Option<Notifications>,
    on_repository: OnRepository,
}

pub struct NotificationsView {
    context: Option<PageContext>,
    controller: Option<Notifications>,
    slot: PageSlot<InboxSnapshot>,
    shared: Rc<RefCell<Shared>>,
    owner: Owner,
    ctx: Ctx,
    ticker: Option<Spawned<()>>,
    model: Model,
    active: bool,
    retired: bool,
    /// Set whenever the published snapshot or its validity changed: only then is the model
    /// recomputed, so a foreground tick costs no lock and no row clone.
    dirty: bool,
    stale: bool,
    refresh_deadline: u64,
    action_deadline: u64,
    /// A fixed wall clock for deterministic tests/preview; production keeps the live clock.
    fixed_now: Option<u64>,
}

impl NotificationsView {
    pub fn new(context: PageContext, on_repository: OnRepository) -> Self {
        let owner = create_root(|| ()).0;
        let signals = owner.run(Signals::new);
        let controller = context
            .services()
            .repositories()
            .notifications()
            .ok()
            .filter(|controller| controller.verify() && controller.context() == context.context());
        let shared = Rc::new(RefCell::new(Shared::default()));
        let ctx = Ctx {
            signals,
            shared: shared.clone(),
            controller: controller.clone(),
            on_repository,
        };
        let mut view = Self {
            context: Some(context),
            controller,
            slot: PageSlot::default(),
            shared,
            owner,
            ctx,
            ticker: None,
            model: Model::default(),
            active: true,
            retired: false,
            dirty: true,
            stale: false,
            refresh_deadline: 0,
            action_deadline: 0,
            fixed_now: None,
        };
        view.register_keys();
        view.start_ticker();
        view.ctx.queue(Intent::First);
        view.bind(None);
        view
    }

    /// The page tree. Safe to build again: it only binds the view's signals.
    pub fn element(&self) -> AnyElement {
        if self.controller.is_none() {
            return unavailable(
                "notifications-page-unavailable",
                "Sign in to read notifications. There is no offline page cache.",
            );
        }
        let ctx = self.ctx.clone();
        view()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex_col()
            .overflow_hidden()
            .px(px(t::PAGE_GUTTER))
            .pt(px(t::PAGE_TOP))
            .pb(px(t::PAGE_BOTTOM))
            .bg(t::theme().background)
            .text_color(t::theme().foreground)
            .test_id("notifications-view")
            .child(ctx.header())
            .child(
                view()
                    .flex_1()
                    .min_h_0()
                    .flex_col()
                    .child(ctx.list())
                    .child(dynamic({
                        let ctx = ctx.clone();
                        move || ctx.more()
                    })),
            )
            .into_element()
    }

    /// Dispatches queued user intents and publishes the controller's reply through the page slot.
    /// Runs on the UI thread from the Shell's foreground tick; it never blocks on I/O.
    /// The number of rows the page currently holds (tests assert pagination against the model,
    /// not against a virtualized viewport).
    pub fn loaded_rows(&self) -> usize {
        self.model.rows.len()
    }

    /// Fixed wall clock for deterministic tests; `None` resumes the live clock.
    pub fn set_now(&mut self, now: Option<u64>) {
        self.fixed_now = now;
        match now {
            Some(now) => {
                self.stop_ticker();
                self.ctx.signals.clock.set(now);
            }
            None => self.start_ticker(),
        }
    }

    pub fn poll(&mut self, runtime: &RuntimeHandle) {
        if self.retired {
            return;
        }
        let sessions = runtime.sessions();
        // Replies are gated on the runtime's live sessions, never on a captured copy: an accepted
        // completion must be current for the account, incarnation, tab, resource and cache
        // authorization in force now.
        let completion = self.shared.borrow_mut().completion.take();
        if let Some(snapshot) = completion {
            let cooldowns = (snapshot.cooldown_seconds, snapshot.action_cooldown_seconds);
            let published = match &self.context {
                Some(context) => {
                    let ticket = self.slot.begin(context.clone());
                    self.slot.publish(ticket, snapshot, sessions)
                }
                None => false,
            };
            if published {
                self.stale = false;
                self.dirty = true;
                let now = self.ctx.signals.clock.get();
                self.refresh_deadline = now.saturating_add(cooldowns.0);
                self.action_deadline = now.saturating_add(cooldowns.1);
            }
        }
        if self.slot.clear_invalid(sessions) {
            // A switch, replacement, removal or revocation invalidates the displayed ticket, not
            // only new requests: drop what is on screen and stop the in-flight read.
            if let Some(controller) = &self.controller {
                controller.cancel();
            }
            self.shared.borrow_mut().completion = None;
            self.stale = true;
            self.dirty = true;
        }
        let intents: Vec<Intent> = self.shared.borrow_mut().queue.drain(..).collect();
        for intent in intents {
            // A hidden page keeps its rows and selection but starts no new work; returning to the
            // foreground resumes a cold read through `set_active`.
            if self.active {
                self.dispatch(intent);
            }
        }
        self.bind(Some(sessions));
    }

    pub fn refresh(&mut self, runtime: &RuntimeHandle) {
        if self.retired {
            return;
        }
        self.ctx.queue(Intent::Refresh);
        self.poll(runtime);
    }

    /// Leaving the foreground keeps the loaded inbox and its selection but stops the local clock
    /// and cancels the read whose reply could no longer publish.
    pub fn set_active(&mut self, active: bool) {
        if self.retired || self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.start_ticker();
            if self.resume_needed() {
                self.ctx.queue(Intent::First);
            }
        } else {
            self.ctx.signals.focused.set(false);
            self.stop_ticker();
            if let Some(controller) = &self.controller {
                controller.cancel();
            }
        }
    }

    pub fn retire(&mut self) {
        if self.retired {
            return;
        }
        self.retired = true;
        self.active = false;
        self.stop_ticker();
        if let Some(controller) = &self.controller {
            controller.cancel();
        }
        self.slot.clear();
        self.shared.borrow_mut().completion = None;
        self.ctx.signals.model.set(Model::default());
        self.owner.dispose();
    }

    /// An interrupted first read (canceled by leaving the tab) must resume; a confirmed empty
    /// inbox, a request already running and an existing error are real states and are not retried.
    fn resume_needed(&self) -> bool {
        let Some(controller) = &self.controller else {
            return false;
        };
        let snapshot = controller.snapshot();
        !snapshot.loaded && snapshot.busy.is_none() && snapshot.error.is_none() && !self.stale
    }

    fn dispatch(&mut self, intent: Intent) {
        let (Some(controller), Some(context)) = (self.controller.clone(), self.context.clone())
        else {
            return;
        };
        match intent {
            Intent::Action { id, action } => {
                let request = match controller.begin_thread_action(&id, action) {
                    Ok(Some(request)) => request,
                    // Busy, cooling, stale, or the ignore step is already confirmed.
                    Ok(None) | Err(_) => return,
                };
                let services = context.services().clone();
                let worker = controller.clone();
                let snapshotter = controller.clone();
                let shared = self.shared.clone();
                let owner = self.owner;
                let (send, receive) = tokio::sync::oneshot::channel();
                if services
                    .spawn(async move {
                        worker.execute_thread_action(request).await;
                        let _ = send.send(());
                    })
                    .is_err()
                {
                    return;
                }
                owner.run(|| {
                    spawn(async move {
                        if receive.await.is_ok() {
                            shared.borrow_mut().completion = Some(snapshotter.snapshot());
                        }
                    })
                });
            }
            Intent::First | Intent::Refresh | Intent::More => {
                let kind = match intent {
                    Intent::First => RequestKind::First,
                    Intent::Refresh => RequestKind::Refresh,
                    _ => RequestKind::More,
                };
                let request = match controller.begin(kind) {
                    Ok(Some(request)) => request,
                    Ok(None) => return,
                    Err(error) => {
                        // A rejected begin (cooldown/stale) is shown, never a silent empty state.
                        self.note_local_error(error, kind);
                        return;
                    }
                };
                let services = context.services().clone();
                let worker = controller.clone();
                let snapshotter = controller.clone();
                let shared = self.shared.clone();
                let owner = self.owner;
                let (send, receive) = tokio::sync::oneshot::channel();
                if services
                    .spawn(async move {
                        worker.execute(request).await;
                        let _ = send.send(());
                    })
                    .is_err()
                {
                    self.note_local_error(NotificationError::Busy, kind);
                    return;
                }
                owner.run(|| {
                    spawn(async move {
                        if receive.await.is_ok() {
                            shared.borrow_mut().completion = Some(snapshotter.snapshot());
                        }
                    })
                });
            }
        }
    }

    fn note_local_error(&mut self, error: NotificationError, kind: RequestKind) {
        if let NotificationError::Cooldown { seconds } = error {
            let now = self.ctx.signals.clock.get();
            self.refresh_deadline = now.saturating_add(seconds);
            self.action_deadline = now.saturating_add(seconds);
        }
        self.model.failed = Some((error, kind));
        self.model.stale = self.stale;
        self.model.refresh_deadline = self.refresh_deadline;
        self.model.action_deadline = self.action_deadline;
        self.ctx.signals.model.set(self.model.clone());
    }

    fn bind(&mut self, sessions: Option<&Sessions>) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut model = sessions
            .and_then(|sessions| self.slot.value(sessions))
            .map(model_of)
            .unwrap_or_default();
        model.stale = self.stale;
        model.refresh_deadline = self.refresh_deadline;
        model.action_deadline = self.action_deadline;
        if model != self.model {
            self.model = model.clone();
            self.ctx.signals.model.set(model);
        }
        if self
            .ctx
            .signals
            .selection
            .get()
            .is_some_and(|index| index >= self.model.rows.len())
        {
            self.ctx.signals.selection.set(None);
        }
    }

    fn start_ticker(&mut self) {
        if self.ticker.is_some() {
            return;
        }
        let clock = self.ctx.signals.clock;
        clock.set(unix_now());
        let owner = self.owner;
        self.ticker = Some(owner.run(|| {
            set_interval(Duration::from_secs(1), move || {
                clock.set(unix_now());
            })
        }));
    }

    fn stop_ticker(&mut self) {
        if let Some(ticker) = self.ticker.take() {
            ticker.cancel();
        }
    }

    /// Arrow/Home/End select a row and reveal it; Enter opens the selected repository. The
    /// bindings are owned by this page and only act while the list holds focus, so a retained
    /// inactive page never consumes another page's keys.
    fn register_keys(&self) {
        let signals = self.ctx.signals;
        let on_repository = self.ctx.on_repository.clone();
        let controller = self.controller.clone();
        self.owner.run(|| {
            for (key, movement) in [
                ("down", Movement::Next),
                ("up", Movement::Previous),
                ("home", Movement::First),
                ("end", Movement::Last),
            ] {
                on_key(key, move |_| {
                    if !signals.focused.get() {
                        return;
                    }
                    let len = signals.model.with(|model| model.rows.len());
                    if len == 0 {
                        return;
                    }
                    let current = signals.selection.get().unwrap_or(0);
                    let target = match movement {
                        Movement::Next => (current + 1).min(len - 1),
                        Movement::Previous => current.saturating_sub(1),
                        Movement::First => 0,
                        Movement::Last => len - 1,
                    };
                    signals.selection.set(Some(target));
                    signals
                        .list
                        .scroll_to_index(target as u32, percentage(0.0), px(0.0), false);
                });
            }
            let on_repository = on_repository.clone();
            on_key("enter", move |_| {
                if !signals.focused.get() {
                    return;
                }
                let Some(index) = signals.selection.get() else {
                    return;
                };
                let repository = signals.model.with(|model| model.repository(index));
                if let Some(repository) = repository
                    && controller.as_ref().is_some_and(Notifications::verify)
                {
                    on_repository(repository);
                }
            });
        });
    }
}

#[derive(Clone, Copy)]
enum Movement {
    Next,
    Previous,
    First,
    Last,
}

impl Signals {
    fn new() -> Self {
        Self {
            model: signal(Model::default()),
            clock: signal(unix_now()),
            selection: signal(None),
            focused: signal(false),
            list: element_ref(),
        }
    }
}

impl Ctx {
    fn queue(&self, intent: Intent) {
        self.shared.borrow_mut().queue.push_back(intent);
    }

    fn header(&self) -> View {
        let signals = self.signals;
        let ctx = self.clone();
        let head = computed(move || {
            signals
                .model
                .with(|model| model.header(signals.clock.get()))
        });
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(t::PAGE_HEADER_HEIGHT))
            .px(px(t::PAGE_HEADER_INSET))
            .test_id("notifications-header")
            .child(
                t::label("Notifications")
                    .text_size(px(18.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .test_id("notifications-title"),
            )
            .child(
                view().flex_1().child(
                    text(move || {
                        let model = signals.model.get();
                        if model.loaded {
                            model.count.to_string()
                        } else {
                            String::new()
                        }
                    })
                    .text_size(px(12.0))
                    .text_color(t::theme().muted_foreground)
                    .test_id("notifications-loaded")
                    .tooltip(dynamic({
                        // signals captured
                        move || {
                            tip(format!(
                                "{} loaded in this view; not a remote total",
                                signals.model.with(|model| model.count)
                            ))
                        }
                    })),
                ),
            )
            .child(dynamic(move || header_status(&ctx, head.get())))
    }

    fn list(&self) -> VirtualList {
        let signals = self.signals;
        let ctx = self.clone();
        virtual_list(
            move || signals.model.with(|model| model.rows.len()),
            move |at| ctx.row(at.index()),
        )
        .keys(move || {
            signals.model.with(|model| {
                model
                    .rows
                    .iter()
                    .map(|row| row.id.clone())
                    .collect::<Vec<_>>()
            })
        })
        .estimated_item_size(px(64.0))
        .content_padding_bottom(px(8.0))
        .scrollbar(true)
        .flex_1()
        .min_h_0()
        .element_ref(&signals.list)
        .focusable(true)
        .test_id("notifications-rows")
        .on_focus({
            // signals captured
            move |_: &()| signals.focused.set(true)
        })
        .on_blur({
            // signals captured
            move |_: &()| signals.focused.set(false)
        })
        .empty(dynamic({
            let ctx = self.clone();
            move || ctx.empty()
        }))
    }

    fn empty(&self) -> AnyElement {
        let model = self.signals.model.get();
        if model.stale {
            return muted("Account changed; open Notifications again for the current account.");
        }
        if model.failed.is_some() {
            // A genuine failure is not an empty inbox; the header carries the compact error.
            return view().into_element();
        }
        if model.empty {
            return muted("No unread notifications");
        }
        view().into_element()
    }

    fn more(&self) -> AnyElement {
        let model = self.signals.model.get();
        if !model.has_more {
            return view().into_element();
        }
        let busy = model.busy.is_some();
        view()
            .pt_2()
            .child(
                view().test_id("notifications-more").child(
                    nsh_base::button()
                        .h(px(28.0))
                        .px_2()
                        .rounded(t::theme().radius)
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .accessibility_label("Load more notifications")
                        .disabled(busy)
                        .cursor_pointer()
                        .hover(|s| s.bg(t::theme().accent))
                        .focus(|s| s.bg(t::theme().accent))
                        .on_press({
                            let ctx = self.clone();
                            move |_: &()| ctx.queue(Intent::More)
                        })
                        .child(t::label("Load more")),
                ),
            )
            .into_element()
    }

    fn row(&self, index: usize) -> AnyElement {
        let signals = self.signals;
        let model = signals.model;
        let clock = signals.clock;
        let selection = signals.selection;
        let unread =
            computed(move || model.with(|model| model.thread(index).is_some_and(|row| row.unread)));
        let subject = computed(move || {
            model.with(|model| model.thread(index).map(subject_view).unwrap_or_default())
        });
        let repository = move || model.with(|model| model.repository(index));
        let repository_label = move || {
            model.with(|model| {
                model
                    .thread(index)
                    .map(|row| {
                        format!(
                            "{}/{}",
                            row.repository.locator().owner(),
                            row.repository.locator().name()
                        )
                    })
                    .unwrap_or_default()
            })
        };
        let title = move || {
            model.with(|model| {
                model
                    .thread(index)
                    .map(|row| SharedString::from(row.subject.title.clone()))
                    .unwrap_or_default()
            })
        };
        let time_label = move || {
            let now = SystemTime::UNIX_EPOCH + Duration::from_secs(clock.get());
            model.with(|model| {
                model
                    .thread(index)
                    .map(|row| relative(Some(&row.updated_at), now))
                    .unwrap_or_default()
            })
        };
        let exact_time = move || model.with(|model| model.exact_time(index));
        let actions = computed(move || {
            let now = clock.get();
            model.with(|model| {
                (
                    model.action_view(index, now),
                    model.thread(index).map(|row| row.id.clone()),
                )
            })
        });
        let open = {
            let ctx = self.clone();
            move |_: &()| {
                let repository = repository();
                if let Some(repository) = repository
                    && ctx.controller.as_ref().is_some_and(Notifications::verify)
                {
                    (ctx.on_repository)(repository);
                }
            }
        };
        let select = {
            // signals captured
            move |_: &()| signals.focused.set(true)
        };
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .min_w_0()
            .px_3()
            .py_2()
            .border_b_1()
            .border_b_color(t::theme().border)
            .bg(move || {
                if selection.get() == Some(index) {
                    t::theme().accent
                } else if model.with(|model| model.thread(index).is_some_and(|row| row.unread)) {
                    UNREAD
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .hover(|s| s.bg(t::theme().accent))
            .accessibility_state(move || AccessibilityState {
                selected: selection.get() == Some(index),
                ..AccessibilityState::default()
            })
            .test_id("notification-row")
            .on_press(select)
            // The unread dot keeps its own fixed gutter so the subject icon never shifts.
            .child(
                view()
                    .w(px(8.0))
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .child(view().size(px(6.0)).rounded(px(3.0)).bg(move || {
                        if unread.get() {
                            UNREAD_DOT
                        } else {
                            AdaptiveColor::TRANSPARENT
                        }
                    })),
            )
            .child(
                view()
                    .h(px(24.0))
                    .w(px(18.0))
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .label(move || subject.get().type_label)
                    .tooltip(dynamic({
                        // signals captured
                        move || {
                            tip(signals
                                .model
                                .with(|model| subject_label(model.rows.get(index))))
                        }
                    }))
                    .test_id("notification-icon")
                    .child(dynamic(move || {
                        glyph(subject.get().glyph, t::theme().muted_foreground).into_element()
                    })),
            )
            .child(
                view()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        view().flex_row().items_center().min_w_0().child(
                            view()
                                .role(AccessibilityRole::Link)
                                .label(move || format!("Open repository {}", repository_label()))
                                .focusable(true)
                                .flex_1()
                                .min_w_0()
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .tooltip(dynamic({
                                    // repository_label
                                    move || tip(repository_label())
                                }))
                                .test_id("notification-repository")
                                .on_press(open)
                                .child(
                                    text(repository_label)
                                        .number_of_lines(1)
                                        .text_size(px(12.0))
                                        .text_color(t::theme().muted_foreground),
                                ),
                        ),
                    )
                    .child(
                        view().min_w_0().flex_row().child(
                            when(move || unread.get(), {
                                // title
                                move || title_text(title(), FontWeight::MEDIUM)
                            })
                            .otherwise({
                                // title
                                move || title_text(title(), FontWeight::NORMAL)
                            }),
                        ),
                    ),
            )
            .child(
                view()
                    .w(px(112.0))
                    .flex_none()
                    .flex_col()
                    .items_end()
                    .gap_1()
                    .child(
                        text(time_label)
                            .text_size(px(11.0))
                            .text_color(t::theme().muted_foreground)
                            .test_id("notification-time")
                            .tooltip(dynamic(move || tip(exact_time()))),
                    )
                    .child(dynamic({
                        let ctx = self.clone();
                        move || {
                            let (view, id) = actions.get();
                            action_cell(&ctx, view, id)
                        }
                    })),
            )
            .into_element()
    }

    fn queue_action(&self, id: Option<String>, action: ThreadAction) {
        if let Some(id) = id {
            self.queue(Intent::Action { id, action });
        }
    }
}

fn title_text(value: impl IntoReactive<SharedString>, weight: FontWeight) -> Text {
    t::label(value)
        .text_size(px(14.0))
        .font_weight(weight)
        .number_of_lines(1)
        .test_id("notification-title")
}

/// The page cannot be bound to an account; this is a stated state, never an empty inbox.
fn unavailable(test_id: &'static str, message: &'static str) -> AnyElement {
    view()
        .size_full()
        .items_center()
        .justify_center()
        .p_4()
        .test_id(test_id)
        .child(
            text(message)
                .text_size(px(13.0))
                .text_color(t::theme().muted_foreground),
        )
        .into_element()
}

fn muted(message: &'static str) -> AnyElement {
    view()
        .p_3()
        .child(
            text(message)
                .text_size(px(13.0))
                .text_color(t::theme().muted_foreground),
        )
        .into_element()
}

fn subject_view(row: &NotificationThread) -> SubjectView {
    let (glyph, type_label) = match row.subject.kind {
        SubjectKind::Issue => (Glyph::Issue, "Issue"),
        SubjectKind::PullRequest => (Glyph::PullRequest, "Pull request"),
        SubjectKind::Commit => (Glyph::Commit, "Commit"),
        SubjectKind::Release => (Glyph::Release, "Release"),
        SubjectKind::Discussion => (Glyph::Discussion, "Discussion"),
        SubjectKind::Unknown(_) => (Glyph::Bell, "Notification"),
    };
    SubjectView { glyph, type_label }
}

fn subject_label(row: Option<&NotificationThread>) -> String {
    let Some(row) = row else {
        return String::new();
    };
    format!(
        "{} · {}",
        subject_view(row).type_label,
        reason_label(&row.reason)
    )
}

fn reason_label(reason: &str) -> &'static str {
    match reason {
        "subscribed" => "Watching",
        "mention" => "Mentioned",
        "team_mention" => "Team mentioned",
        "assign" => "Assigned",
        "review_requested" => "Review requested",
        "author" => "Author",
        "comment" => "Commented",
        "ci_activity" => "CI activity",
        "manual" => "Subscribed",
        "state_change" => "State changed",
        _ => "Other notification",
    }
}

fn partial_message(partial: &[NotificationPartial]) -> String {
    let mut parts = Vec::new();
    for reason in partial {
        let text = match reason {
            NotificationPartial::Sso { organizations } => {
                if organizations.is_empty() {
                    "Some organizations need SSO authorization".to_string()
                } else {
                    format!(
                        "Some organizations need SSO authorization: {}",
                        organizations.join(", ")
                    )
                }
            }
            NotificationPartial::UnsafePagination => {
                "The response contained an unusable next page".to_string()
            }
            NotificationPartial::PageLimit => "The loaded page limit was reached".to_string(),
            NotificationPartial::ItemLimit => "The loaded item limit was reached".to_string(),
        };
        if !parts.contains(&text) {
            parts.push(text);
        }
    }
    parts.join("; ")
}

fn header_status(ctx: &Ctx, head: HeaderView) -> AnyElement {
    let mut status = view().flex_row().items_center().gap_2();
    if head.busy {
        status = status.child(
            view()
                .size(px(16.0))
                .items_center()
                .justify_center()
                .label("Loading notifications")
                .test_id("notifications-loading")
                .child(spinner(14.0)),
        );
    } else {
        let cooling = head.cooldown.is_some();
        let ctx = ctx.clone();
        status = status.child(
            view()
                .test_id("notifications-refresh")
                .tooltip(tip(if cooling {
                    "Refresh available after the server cooldown"
                } else {
                    "Refresh unread notifications"
                }))
                .child(
                    nsh_base::button()
                        .size(px(28.0))
                        .rounded(t::theme().radius)
                        .items_center()
                        .justify_center()
                        .accessibility_label("Refresh unread notifications")
                        .disabled(cooling)
                        .cursor_pointer()
                        .hover(|s| s.bg(t::theme().accent))
                        .focus(|s| s.bg(t::theme().accent))
                        .on_press(move |_: &()| ctx.queue(Intent::Refresh))
                        .child(controls::icon(controls::Icon::Refresh)),
                ),
        );
        if let Some(seconds) = head.cooldown {
            status = status.child(
                text(format!("{seconds}s"))
                    .text_size(px(11.0))
                    .text_color(t::theme().muted_foreground),
            );
        }
    }
    if let Some((message, more)) = head.failed {
        let ctx = ctx.clone();
        status = status.child(
            controls::failed(
                message,
                Some(Rc::new(move || {
                    ctx.queue(if more { Intent::More } else { Intent::Refresh })
                })),
            )
            .test_id("notifications-error"),
        );
    }
    if head.partial {
        status = status.child(
            view()
                .px_1()
                .rounded(t::theme().radius)
                .bg(t::theme().muted)
                .tooltip(tip(head.partial_detail))
                .test_id("notifications-partial")
                .child(
                    text("Partial")
                        .text_size(px(11.0))
                        .text_color(t::theme().muted_foreground),
                ),
        );
    }
    status.into_element()
}

fn action_cell(ctx: &Ctx, state: ActionView, id: Option<String>) -> AnyElement {
    let mut row = view()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .flex_none()
        .test_id("notification-actions");
    if let Some((action, message)) = state.warning.clone() {
        let ctx = ctx.clone();
        let retry = id.clone();
        row = row.child(
            controls::failed(
                message,
                Some(Rc::new(move || ctx.queue_action(retry.clone(), action))),
            )
            .test_id("notification-action-warning"),
        );
    }
    for action in [ThreadAction::Done, ThreadAction::Unsubscribe] {
        let (label, glyph_kind, test_id) = match action {
            ThreadAction::Done => ("Done", Glyph::Check, "notification-action-done"),
            ThreadAction::Unsubscribe => (
                "Unsubscribe",
                Glyph::EyeOff,
                "notification-action-unsubscribe",
            ),
        };
        if state.busy == Some(action) {
            row = row.child(
                view()
                    .size(px(28.0))
                    .items_center()
                    .justify_center()
                    .label(format!("{label} in progress"))
                    .role(AccessibilityRole::Progressbar)
                    .test_id(test_id)
                    .child(spinner(13.0)),
            );
            continue;
        }
        // The ignore is already confirmed; only Done may be retried.
        let disabled = state.blocked || (action == ThreadAction::Unsubscribe && state.pending_done);
        let tooltip = if action == ThreadAction::Unsubscribe && state.pending_done {
            "Already unsubscribed; finish removing it with Done"
        } else {
            label
        };
        let id = id.clone();
        let ctx = ctx.clone();
        row = row.child(
            view().test_id(test_id).tooltip(tip(tooltip)).child(
                nsh_base::button()
                    .size(px(28.0))
                    .rounded(t::theme().radius)
                    .items_center()
                    .justify_center()
                    .accessibility_label(label)
                    .disabled(disabled)
                    .cursor_pointer()
                    .hover(|s| s.bg(t::theme().accent))
                    .focus(|s| s.bg(t::theme().accent))
                    .on_press(move |_: &()| ctx.queue_action(id.clone(), action))
                    .child(glyph(glyph_kind, t::theme().muted_foreground)),
            ),
        );
    }
    // Save for later has no public REST/GraphQL inbox API: the entry stays visible but disabled,
    // never a local fake success.
    row.child(
        view()
            .tooltip(tip("Save for later (unavailable)"))
            .test_id("notification-action-save")
            .child(
                nsh_base::button()
                    .size(px(28.0))
                    .rounded(t::theme().radius)
                    .items_center()
                    .justify_center()
                    .accessibility_label("Save for later (unavailable)")
                    .disabled(true)
                    .child(glyph(Glyph::Bookmark, t::theme().muted_foreground)),
            ),
    )
    .into_element()
}

fn model_of(snapshot: &InboxSnapshot) -> Model {
    let count = snapshot.items.len();
    Model {
        rows: snapshot.items.clone(),
        states: snapshot.thread_states.clone().into_iter().collect(),
        busy: snapshot.busy,
        has_more: snapshot.has_more,
        failed: snapshot.error.clone().map(|error| {
            (
                error,
                snapshot.failed_request.unwrap_or(RequestKind::Refresh),
            )
        }),
        partial: snapshot.partial.clone(),
        empty: snapshot.is_empty(),
        loaded: snapshot.loaded,
        count,
        stale: false,
        refresh_deadline: 0,
        action_deadline: 0,
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
