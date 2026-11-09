//! Native Shell global Issues / Pull Requests.
//!
//! `controller` keeps the query, cursor and request generation; this module owns the page: user
//! actions queue intents, `poll` dispatches them on the captured services, the controller's
//! generation gates the reply, and the accepted rows are published through a `PageSlot` gated on
//! the runtime's account/incarnation/authorization ticket before they reach NSH signals.
//!
//! Rows use the shared Home work-item row: only display values and the repository callback are
//! supplied here, while the data model, account context, typed navigation and keyboard selection
//! stay with this list.
pub mod controller;
mod presentation;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use controller::{Controller, Request, RequestMode};
use nsh_core::element::{ImageSrc, create_root};
use nsh_core::prelude::*;

use crate::app::runtime::{PageContext, PageSlot, RuntimeHandle};
use crate::domain::work_item::{
    ItemState, PartialReason, Relation, StateFilter, WorkItem, WorkItemError, WorkItemKind,
    WorkItemPage, WorkRepository,
};
use crate::github::work_items::WorkItemLimits;
use crate::ui::foundation::{controls, theme as t};
use crate::ui::relative_time::relative;
use crate::workspace::Sessions;

/// The host checks its workspace/tab incarnation when consuming navigation.
pub type OnRepository = Rc<dyn Fn(WorkRepository)>;

/// GitHub's state colors, carried as the application's semantic colors because NSH's default
/// theme is neutral.
const OPEN: AdaptiveColor = AdaptiveColor {
    light: rgb(0x1a7f37).to_hsla(),
    dark: rgb(0x3fb950).to_hsla(),
};
const MERGED: AdaptiveColor = AdaptiveColor {
    light: rgb(0x8250df).to_hsla(),
    dark: rgb(0xa371f7).to_hsla(),
};

fn glyph(kind: WorkItemKind, color: impl IntoReactive<AdaptiveColor>) -> impl IntoElement {
    // Lucide paths, under the repository's existing ISC notice.
    let path = match kind {
        WorkItemKind::Issue => "<circle cx='12' cy='12' r='9'/><circle cx='12' cy='12' r='1'/>",
        WorkItemKind::PullRequest => {
            "<circle cx='6' cy='5' r='3'/><circle cx='6' cy='19' r='3'/><circle cx='18' cy='19' r='3'/><path d='M6 8v8M18 16V9a4 4 0 0 0-4-4h-1m3-3-3 3 3 3'/>"
        }
    };
    svg(ImageSrc::svg_markup(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'>{path}</svg>"
    )))
    .size(px(16.0))
    .text_color(color)
}

fn spinner(size: f32) -> View {
    view()
        .size(px(size))
        .rounded(px(size / 2.0))
        .border_2()
        .border_color(t::theme().border)
        .border_t_color(t::theme().muted_foreground)
        .animation(KeyframeAnimation::new(Keyframes::Spin).duration(800.0))
}

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

fn tone(state: ItemState) -> AdaptiveColor {
    match state {
        ItemState::Open => OPEN,
        ItemState::Closed => t::theme().muted_foreground,
        ItemState::Merged => MERGED,
        ItemState::Unknown => t::WARNING,
    }
}

/// The published list view of one page. Equality keeps an unchanged poll from touching signals.
#[derive(Clone, PartialEq)]
struct Displayed {
    rows: Vec<WorkItem>,
    partial: Vec<PartialReason>,
    error: Option<String>,
    failed_append: bool,
    loaded: bool,
    empty: bool,
    has_more: bool,
    busy: bool,
    total: Option<u64>,
    relation: Relation,
    state: StateFilter,
    kind: WorkItemKind,
    stale: bool,
}

impl Default for Displayed {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            partial: Vec::new(),
            error: None,
            failed_append: false,
            loaded: false,
            empty: false,
            has_more: false,
            busy: false,
            total: None,
            relation: Relation::Assigned,
            state: StateFilter::Open,
            kind: WorkItemKind::Issue,
            stale: false,
        }
    }
}

impl Displayed {
    fn of(controller: &Controller) -> Self {
        Self {
            rows: controller.rows.clone(),
            partial: controller.partial.clone(),
            error: controller.error.as_ref().map(presentation::error_message),
            failed_append: controller.failed_mode == Some(RequestMode::Append),
            loaded: controller.loaded,
            empty: controller.is_empty(),
            has_more: controller.next.is_some(),
            busy: controller.busy.is_some(),
            total: controller.total_count,
            relation: controller.query.relation,
            state: controller.query.state,
            kind: controller.query.kind,
            stale: false,
        }
    }
}

#[derive(Clone, Default, PartialEq)]
struct HeaderView {
    title: &'static str,
    busy: bool,
    failed: Option<(String, bool)>,
    partial: bool,
    partial_detail: String,
}

impl Displayed {
    fn header(&self) -> HeaderView {
        HeaderView {
            title: title_of(self.kind),
            busy: self.busy,
            failed: self.error.clone().map(|error| (error, self.failed_append)),
            partial: !self.partial.is_empty(),
            partial_detail: presentation::partial_message(&self.partial),
        }
    }
}

fn title_of(kind: WorkItemKind) -> &'static str {
    match kind {
        WorkItemKind::Issue => "Issues",
        WorkItemKind::PullRequest => "Pull Requests",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Intent {
    Replace,
    Append,
    Filter {
        relation: Relation,
        state: StateFilter,
    },
}

type Reply = (Request, Result<WorkItemPage, WorkItemError>);

#[derive(Default)]
struct Shared {
    queue: VecDeque<Intent>,
    completion: Option<Reply>,
}

#[derive(Clone, Copy)]
struct Signals {
    model: Signal<Displayed>,
    clock: Signal<u64>,
    selection: Signal<Option<u64>>,
    focused: Signal<bool>,
    list: ElementRef,
}

#[derive(Clone)]
struct Ctx {
    signals: Signals,
    shared: Rc<RefCell<Shared>>,
    controller: Rc<RefCell<Option<Controller>>>,
    on_repository: OnRepository,
}

pub struct WorkItemsView {
    context: Option<PageContext>,
    controller: Rc<RefCell<Option<Controller>>>,
    slot: PageSlot<Displayed>,
    shared: Rc<RefCell<Shared>>,
    owner: Owner,
    ctx: Ctx,
    limits: WorkItemLimits,
    ticker: Option<Spawned<()>>,
    model: Displayed,
    active: bool,
    retired: bool,
    dirty: bool,
    stale: bool,
    /// A fixed wall clock for deterministic tests/preview; production keeps the live clock.
    fixed_now: Option<u64>,
}

impl WorkItemsView {
    pub fn new(context: PageContext, kind: WorkItemKind, on_repository: OnRepository) -> Self {
        let owner = create_root(|| ()).0;
        let signals = owner.run(Signals::new);
        let controller = context
            .services()
            .repositories()
            .work_item_session()
            .ok()
            .filter(|session| session.verify() && session.context() == context.context())
            .map(|session| Controller::new(session, kind));
        let controller = Rc::new(RefCell::new(controller));
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
            limits: WorkItemLimits::default(),
            ticker: None,
            model: Displayed {
                kind,
                ..Displayed::default()
            },
            active: true,
            retired: false,
            dirty: true,
            stale: false,
            fixed_now: None,
        };
        view.register_keys();
        view.start_ticker();
        view.ctx.queue(Intent::Replace);
        view.bind(None);
        view
    }

    pub fn element(&self) -> AnyElement {
        if self.controller.borrow().is_none() {
            return unavailable(
                "work-items-page-unavailable",
                "Sign in to read global issues and pull requests. There is no offline page cache.",
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
            .gap_2()
            .bg(t::theme().background)
            .text_color(t::theme().foreground)
            .test_id("global-work-items")
            .child(ctx.header())
            .child(ctx.filters())
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

    /// Dispatches queued intents and publishes an accepted reply for the live sessions.
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
        let completion = self.shared.borrow_mut().completion.take();
        if let Some((request, result)) = completion {
            let accepted = self
                .controller
                .borrow_mut()
                .as_mut()
                .is_some_and(|controller| controller.finish(&request, result));
            if accepted {
                self.publish(sessions);
            }
        }
        if self.slot.clear_invalid(sessions) {
            // A switch, replacement, removal or revocation clears what is already displayed.
            if let Some(controller) = self.controller.borrow_mut().as_mut() {
                controller.cancel();
                controller.rows.clear();
                controller.next = None;
                controller.loaded = false;
                controller.partial.clear();
                controller.error = None;
                controller.failed_mode = None;
                controller.total_count = None;
            }
            self.shared.borrow_mut().completion = None;
            self.stale = true;
            self.dirty = true;
            self.ctx.signals.selection.set(None);
        }
        let intents: Vec<Intent> = self.shared.borrow_mut().queue.drain(..).collect();
        for intent in intents {
            // A hidden page keeps its rows and selection but starts no new work; returning to the
            // foreground resumes a cold read through `set_active`.
            if self.active {
                self.dispatch(intent, sessions);
            }
        }
        self.bind(Some(sessions));
    }

    /// Publishes the controller's current list through the page slot, gated on the live sessions.
    fn publish(&mut self, sessions: &Sessions) {
        let displayed = {
            let controller = self.controller.borrow();
            let Some(controller) = controller.as_ref() else {
                return;
            };
            Displayed::of(controller)
        };
        let Some(context) = &self.context else {
            return;
        };
        let ticket = self.slot.begin(context.clone());
        if self.slot.publish(ticket, displayed, sessions) {
            self.stale = false;
            self.dirty = true;
        }
    }

    pub fn refresh(&mut self, runtime: &RuntimeHandle) {
        if self.retired {
            return;
        }
        self.ctx.queue(Intent::Replace);
        self.poll(runtime);
    }

    pub fn set_active(&mut self, active: bool) {
        if self.retired || self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.start_ticker();
            let loaded = self
                .controller
                .borrow()
                .as_ref()
                .is_some_and(|controller| controller.loaded);
            if !loaded && !self.stale {
                self.ctx.queue(Intent::Replace);
            }
        } else {
            self.ctx.signals.focused.set(false);
            self.stop_ticker();
            if let Some(controller) = self.controller.borrow_mut().as_mut() {
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
        if let Some(controller) = self.controller.borrow_mut().as_mut() {
            controller.cancel();
        }
        self.slot.clear();
        self.shared.borrow_mut().completion = None;
        self.ctx.signals.model.set(Displayed::default());
        self.owner.dispose();
    }

    fn dispatch(&mut self, intent: Intent, sessions: &Sessions) {
        let filter = matches!(intent, Intent::Filter { .. });
        let Some(context) = self.context.clone() else {
            return;
        };
        let request = {
            let mut controller = self.controller.borrow_mut();
            let Some(controller) = controller.as_mut() else {
                return;
            };
            if !controller.current() {
                return;
            }
            let kind = controller.query.kind;
            match intent {
                Intent::Replace => controller.begin(RequestMode::Replace),
                Intent::Append => controller.begin(RequestMode::Append),
                Intent::Filter { relation, state } => controller.filter(kind, relation, state),
            }
        };
        if filter {
            // A filter clears the old rows before its page arrives; publish that immediately
            // instead of leaving the previous filter's results on screen.
            self.publish(sessions);
        }
        let Some(request) = request else {
            return;
        };
        let services = context.services().clone();
        let limits = self.limits.clone();
        let shared = self.shared.clone();
        let owner = self.owner;
        let (send, receive) = tokio::sync::oneshot::channel();
        if services
            .spawn(async move {
                let result = request.execute(&limits).await;
                let _ = send.send((request, result));
            })
            .is_err()
        {
            return;
        }
        owner.run(|| {
            spawn(async move {
                if let Ok(reply) = receive.await {
                    shared.borrow_mut().completion = Some(reply);
                }
            })
        });
    }

    fn bind(&mut self, sessions: Option<&Sessions>) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut model = sessions
            .and_then(|sessions| self.slot.value(sessions))
            .cloned()
            .unwrap_or_default();
        model.stale = self.stale;
        if self
            .ctx
            .signals
            .selection
            .get()
            .is_some_and(|id| !model.rows.iter().any(|row| row.id == id))
        {
            self.ctx.signals.selection.set(None);
        }
        if model != self.model {
            self.model = model.clone();
            self.ctx.signals.model.set(model);
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
                    let current = signals
                        .selection
                        .get()
                        .and_then(|id| {
                            signals
                                .model
                                .with(|model| model.rows.iter().position(|row| row.id == id))
                        })
                        .unwrap_or(0);
                    let target = match movement {
                        Movement::Next => (current + 1).min(len - 1),
                        Movement::Previous => current.saturating_sub(1),
                        Movement::First => 0,
                        Movement::Last => len - 1,
                    };
                    let id = signals
                        .model
                        .with(|model| model.rows.get(target).map(|row| row.id));
                    signals.selection.set(id);
                    signals
                        .list
                        .scroll_to_index(target as u32, percentage(0.0), px(0.0), false);
                });
            }
            // Enter activates only the selected repository link, exactly like the pointer: the
            // title has no detail destination.
            on_key("enter", move |_| {
                if !signals.focused.get() {
                    return;
                }
                let Some(id) = signals.selection.get() else {
                    return;
                };
                let repository = signals.model.with(|model| {
                    model
                        .rows
                        .iter()
                        .find(|row| row.id == id)
                        .map(|row| row.repository.clone())
                });
                if let Some(repository) = repository
                    && controller
                        .borrow()
                        .as_ref()
                        .is_some_and(Controller::current)
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
            model: signal(Displayed::default()),
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
        let head = computed(move || signals.model.with(Displayed::header));
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(t::PAGE_HEADER_HEIGHT))
            .px(px(t::PAGE_HEADER_INSET))
            .test_id("work-header")
            .child(
                text(move || head.get().title)
                    .text_size(px(18.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .test_id("work-title"),
            )
            .child(view().flex_1())
            .child(dynamic(move || header_status(&ctx, head.get())))
    }

    fn filters(&self) -> AnyElement {
        let signals = self.signals;
        let model = signals.model;
        let ctx = self.clone();
        let state = computed(move || model.with(|model| (model.relation, model.state)));
        dynamic(move || {
            let (relation, is) = state.get();
            let option = |label: &'static str,
                          active: bool,
                          intent: Intent,
                          test_id: &'static str| {
                let ctx = ctx.clone();
                view()
                    .test_id(test_id)
                    .style_when(active, |s| s.bg(t::theme().accent))
                    .rounded(t::theme().radius)
                    .child(
                        nsh_base::button()
                            .h(px(26.0))
                            .px_2()
                            .rounded(t::theme().radius)
                            .flex_row()
                            .items_center()
                            .justify_center()
                            .accessibility_label(label)
                            .selected(active)
                            .cursor_pointer()
                            .focus(|s| s.bg(t::theme().accent))
                            .on_press(move |_: &()| ctx.queue(intent))
                            .child(t::label(label).text_size(px(12.0)).font_weight(if active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })),
                    )
            };
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_none()
                .test_id("work-filters")
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .test_id("work-relation")
                        .child(option(
                            "Assigned",
                            relation == Relation::Assigned,
                            Intent::Filter {
                                relation: Relation::Assigned,
                                state: is,
                            },
                            "work-relation-assigned",
                        ))
                        .child(option(
                            "Created",
                            relation == Relation::Created,
                            Intent::Filter {
                                relation: Relation::Created,
                                state: is,
                            },
                            "work-relation-created",
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .test_id("work-state")
                        .child(option(
                            "Open",
                            is == StateFilter::Open,
                            Intent::Filter {
                                relation,
                                state: StateFilter::Open,
                            },
                            "work-state-open",
                        ))
                        .child(option(
                            "Closed",
                            is == StateFilter::Closed,
                            Intent::Filter {
                                relation,
                                state: StateFilter::Closed,
                            },
                            "work-state-closed",
                        ))
                        .child(option(
                            "All",
                            is == StateFilter::All,
                            Intent::Filter {
                                relation,
                                state: StateFilter::All,
                            },
                            "work-state-all",
                        )),
                )
                .into_element()
        })
    }

    fn list(&self) -> VirtualList {
        let signals = self.signals;
        let ctx = self.clone();
        virtual_list(
            move || signals.model.with(|model| model.rows.len()),
            move |at| ctx.row(at.index()),
        )
        .keys(move || {
            signals
                .model
                .with(|model| model.rows.iter().map(|row| row.id).collect::<Vec<_>>())
        })
        .estimated_item_size(px(t::WORK_ITEM_ROW_HEIGHT))
        .content_padding_bottom(px(8.0))
        .scrollbar(true)
        .flex_1()
        .min_h_0()
        .element_ref(&signals.list)
        .focusable(true)
        .test_id("work-rows")
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
            return muted("Account changed; open this list again for the current account.");
        }
        if model.error.is_some() {
            // A genuine failure is not an empty result set; the header carries the compact error.
            return view().into_element();
        }
        if model.empty {
            return muted("No results");
        }
        view().into_element()
    }

    fn more(&self) -> AnyElement {
        let model = self.signals.model.get();
        if !model.has_more {
            return view().into_element();
        }
        view()
            .pt_2()
            .child(
                view().test_id("work-more").child(
                    nsh_base::button()
                        .h(px(28.0))
                        .px_2()
                        .rounded(t::theme().radius)
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .accessibility_label("Load more results")
                        .disabled(model.busy)
                        .cursor_pointer()
                        .hover(|s| s.bg(t::theme().accent))
                        .focus(|s| s.bg(t::theme().accent))
                        .on_press({
                            let ctx = self.clone();
                            move |_: &()| ctx.queue(Intent::Append)
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
        let selected = computed(move || {
            model.with(|model| {
                model
                    .rows
                    .get(index)
                    .is_some_and(|row| selection.get() == Some(row.id))
            })
        });
        let item = move || model.with(|model| model.rows.get(index).cloned());
        let number = move || {
            model.with(|model| {
                model
                    .rows
                    .get(index)
                    .map(|row| SharedString::from(format!("#{}", row.number)))
                    .unwrap_or_default()
            })
        };
        let repository_label = move || {
            model.with(|model| {
                model
                    .rows
                    .get(index)
                    .map(|row| SharedString::from(row.repository.full_name()))
                    .unwrap_or_default()
            })
        };
        let state_label = move || {
            model.with(|model| {
                model
                    .rows
                    .get(index)
                    .map(|row| row.state.label())
                    .unwrap_or_default()
            })
        };
        let state_tone = move || {
            model.with(|model| {
                model
                    .rows
                    .get(index)
                    .map(|row| tone(row.state))
                    .unwrap_or_else(|| t::theme().muted_foreground)
            })
        };
        let title_label = move || {
            model.with(|model| {
                model
                    .rows
                    .get(index)
                    .map(|row| SharedString::from(row.title.clone()))
                    .unwrap_or_default()
            })
        };
        let exact_time = move || {
            model.with(|model| {
                model
                    .rows
                    .get(index)
                    .map(|row| row.updated_at.clone())
                    .unwrap_or_default()
            })
        };
        let time_label = move || {
            let now = SystemTime::UNIX_EPOCH + Duration::from_secs(clock.get());
            model.with(|model| {
                model
                    .rows
                    .get(index)
                    .map(|row| relative(Some(&row.updated_at), now))
                    .unwrap_or_default()
            })
        };
        let open = {
            let ctx = self.clone();
            // item
            move |_: &()| {
                let Some(row) = item() else {
                    return;
                };
                if ctx
                    .controller
                    .borrow()
                    .as_ref()
                    .is_some_and(Controller::current)
                {
                    (ctx.on_repository)(row.repository);
                }
            }
        };
        dynamic(move || {
            let kind = item().map(|row| row.kind).unwrap_or(WorkItemKind::Issue);
            let title_line = view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w_0()
                .line_height(px(21.0))
                .child(
                    view()
                        .flex_none()
                        .items_center()
                        .child(glyph(kind, state_tone())),
                )
                .child(
                    text(title_label)
                        .flex_1()
                        .min_w_0()
                        .number_of_lines(1)
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .test_id("work-title-row")
                        .tooltip(dynamic({
                            // title_label
                            move || tip(title_label())
                        })),
                );
            let metadata = view()
                .flex_row()
                .items_center()
                .gap_2()
                .w_full()
                .min_w_0()
                .line_height(px(18.0))
                .text_size(px(12.0))
                .text_color(t::theme().muted_foreground)
                .child(
                    t::code(number)
                        .text_size(px(12.0))
                        .text_color(t::theme().muted_foreground)
                        .flex_none()
                        .test_id("work-number"),
                )
                .child(
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
                        .test_id("work-repository")
                        .on_press(open.clone())
                        .child(
                            text(repository_label)
                                .number_of_lines(1)
                                .text_size(px(12.0))
                                .text_color(t::theme().muted_foreground),
                        ),
                )
                .child(
                    text(state_label)
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(state_tone())
                        .test_id("work-state-label"),
                )
                .child(
                    text(time_label)
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(t::theme().muted_foreground)
                        .test_id("work-updated")
                        .tooltip(dynamic(move || tip(exact_time()))),
                );
            // The shared Home row owns the height, padding, rounded selection fill and two-line
            // block; the title has no detail destination, so only the repository link is pressable.
            controls::work_item_row(title_line, metadata, selected.get())
        })
    }
}

/// The page cannot be bound to an account; this is a stated state, never an empty list.
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

fn header_status(ctx: &Ctx, head: HeaderView) -> AnyElement {
    let mut status = view().flex_row().items_center().gap_2();
    if head.busy {
        status = status.child(
            view()
                .size(px(16.0))
                .items_center()
                .justify_center()
                .label("Loading results")
                .test_id("work-loading")
                .child(spinner(14.0)),
        );
    } else {
        let ctx = ctx.clone();
        status = status.child(
            view()
                .test_id("work-refresh")
                .tooltip(tip("Refresh"))
                .child(
                    nsh_base::button()
                        .size(px(28.0))
                        .rounded(t::theme().radius)
                        .items_center()
                        .justify_center()
                        .accessibility_label("Refresh")
                        .cursor_pointer()
                        .hover(|s| s.bg(t::theme().accent))
                        .focus(|s| s.bg(t::theme().accent))
                        .on_press(move |_: &()| ctx.queue(Intent::Replace))
                        .child(controls::icon(controls::Icon::Refresh)),
                ),
        );
    }
    if let Some((message, append)) = head.failed {
        let ctx = ctx.clone();
        status = status.child(
            controls::failed(
                message,
                Some(Rc::new(move || {
                    ctx.queue(if append {
                        Intent::Append
                    } else {
                        Intent::Replace
                    })
                })),
            )
            .test_id("work-error"),
        );
    }
    if head.partial {
        status = status.child(
            view()
                .px_1()
                .rounded(t::theme().radius)
                .bg(t::theme().muted)
                .tooltip(tip(head.partial_detail))
                .test_id("work-partial")
                .child(
                    text("Partial")
                        .text_size(px(11.0))
                        .text_color(t::theme().muted_foreground),
                ),
        );
    }
    status.into_element()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
