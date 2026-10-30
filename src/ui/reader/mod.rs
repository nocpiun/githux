//! A reusable, read-only reader for repository content.
//!
//! The reader turns `content::PreparedContent` (or a raw UTF-8 body) plus a
//! fixed repository/commit/path context into one of five presentations:
//!
//! * `Empty` — a successful zero-byte file.
//! * `Unsupported` — non-UTF-8 bytes or binary control characters.
//! * `TooLarge` — content past the 2 MiB engineering budget.
//! * Markdown — Ely's virtualized preview (`MarkdownDocument` in `Preview`
//!   mode), which parses long documents off the UI thread and only draws the
//!   visible blocks.
//! * Source — the read-only [`SourceView`], which virtualizes lines and has no
//!   input handler at all.
//!
//! Link presses never reach the system opener here. Each one is resolved
//! against the context (same repository, same commit, same directory) and
//! reported as a [`ReaderAction`]; dangerous schemes are refused. The
//! integration layer owns actually opening an external URL or switching files.
//!
//! The module is declared by `ui::mod` so shared reader services can use the
//! same scrollbar implementation. `examples/reader.rs` includes a small
//! `#[path]` module tree for its independent preview; the real `Shell` wiring
//! remains a separate task. `dead_code` is scoped to this not-yet-wired slice.

#![allow(dead_code)]

pub mod fixtures;
mod images;
#[cfg(test)]
mod layout_tests;
mod links;
mod markdown;
#[cfg(test)]
mod motion_tests;
mod source;
#[cfg(test)]
mod tests;

// The picture contract a host implements or drives. Re-exported so an
// integration layer depends on the reader module alone.
#[allow(unused_imports)]
pub use crate::content::images::{
    DecodedImage, ImageCompletion, ImageFailure, ImageFormat, ImageKey, ImageLimits, ImageProvider,
    ImageRefusal, ImageRequest, ImageScope, ImageSink, ImageTarget,
};
pub use links::LinkTarget;
pub use source::SourceView;

use std::{cell::Cell, rc::Rc, time::Instant};

use crate::ui::scroll;
/// One owner clock for fade and wheel presentation. Tests pin the same time used by real events.
#[derive(Clone)]
pub(super) struct ScrollClock {
    origin: Instant,
    pinned: Rc<Cell<Option<Instant>>>,
}

impl ScrollClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            pinned: Rc::new(Cell::new(None)),
        }
    }

    fn now(&self) -> Instant {
        self.pinned.get().unwrap_or_else(Instant::now)
    }

    fn ms(&self) -> u64 {
        self.now()
            .saturating_duration_since(self.origin)
            .as_millis() as u64
    }

    #[cfg(test)]
    fn pin(&self, ms: u64) {
        self.pinned
            .set(Some(self.origin + std::time::Duration::from_millis(ms)));
    }
}

/// GPUI retains keyed state only while the element participates in consecutive frames (including
/// cached frames). Its release also catches a retained reader being hidden by its host.
fn scroll_lifetime<T: 'static>(cx: &Context<T>, cancel: fn(&mut T, &mut Context<T>)) -> AnyElement {
    let owner = cx.entity().downgrade();
    let key =
        gpui::ElementId::Name(format!("reader-scroll-lifetime-{}", cx.entity_id().as_u64()).into());
    gpui::canvas(
        move |_, window, cx| {
            let owner = owner.clone();
            window.use_keyed_state(key.clone(), cx, move |window, lease| {
                let keys_owner = owner.clone();
                let owning_window = window.window_handle();
                let keys = lease.intercept_keystrokes(move |event, window, cx| {
                    if window.window_handle() == owning_window && event.keystroke.key == "escape" {
                        let _ = keys_owner.update(cx, cancel);
                    }
                });
                lease
                    .on_release(move |_, cx| {
                        let _ = owner.update(cx, cancel);
                    })
                    .detach();
                keys
            });
        },
        |_, _, _, _| {},
    )
    .into_any_element()
}

use ely_core::{
    primitives::{Icon, IconName},
    theme::{ActiveTheme, IconSize, TextSize},
};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window, div,
};

use crate::content::{PreparedBody, PreparedContent, UnsupportedContent};
use crate::domain::blob::BlobIdentity;

/// How a UTF-8 body is presented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    /// GitHub-flavored Markdown.
    Markdown,
    /// Source text, with an optional language label.
    Source { language: Option<String> },
}

impl DocumentKind {
    /// Markdown for README files and `.md`/`.markdown`/`.mdx`, source otherwise.
    ///
    /// The integration layer may override this; the reader only needs the
    /// decision, not the path.
    pub fn for_path(path: &str) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        let markdown = name.starts_with("readme")
            || name.ends_with(".md")
            || name.ends_with(".markdown")
            || name.ends_with(".mdx");
        match markdown {
            true => Self::Markdown,
            false => Self::Source { language: None },
        }
    }
}

/// The fixed repository version a document's relative links resolve against.
///
/// Built from a `BlobIdentity`, so the commit is the one the bytes were read
/// at. Nothing here is a branch or a "latest" pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceContext {
    owner: String,
    name: String,
    commit: String,
    path: String,
}

impl ResourceContext {
    pub fn new(
        owner: impl Into<String>,
        name: impl Into<String>,
        commit: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
            commit: commit.into(),
            path: path.into(),
        }
    }

    pub fn from_identity(identity: &BlobIdentity) -> Self {
        Self {
            owner: identity.repository().locator().owner().to_string(),
            name: identity.repository().locator().name().to_string(),
            commit: identity.commit().as_str().to_string(),
            path: identity.path().as_str().to_string(),
        }
    }

    pub fn from_prepared(content: &PreparedContent) -> Self {
        Self::from_identity(&content.identity)
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn commit(&self) -> &str {
        &self.commit
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

/// What a user action asks the integration layer to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderAction {
    /// A link was pressed and resolved; the reader did not open it.
    Link(LinkTarget),
}

/// The reader's action sink.
pub type OnAction = Rc<dyn Fn(ReaderAction, &mut Window, &mut App)>;

/// Called after the document's natural height may have changed (a picture landed,
/// the text re-wrapped, the document was replaced). A Flow host re-lays out its
/// own scroll container from here; a bounded host ignores it.
pub type OnLayoutChanged = Rc<dyn Fn(&mut Window, &mut App)>;

enum ReaderState {
    Empty,
    Unsupported(UnsupportedContent),
    TooLarge { size: u64, limit: usize },
    Markdown(Entity<markdown::MarkdownView>),
    Source(Entity<SourceView>),
}

/// How a Markdown document is sized vertically.
///
/// The layout is a host decision and never changes the document model:
/// selection, copying, code, tables, picture state and the top/bottom content
/// inset are identical in every mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkdownLayout {
    /// Existing hosts keep the original fixed outer spacing inside a bounded,
    /// self-scrolling viewport.
    #[default]
    Default,
    /// Full-height repository viewport, scrolling top/bottom content inset,
    /// and four additional logical pixels of horizontal spacing per side.
    /// Still a bounded viewport with its own scrollbar.
    Repository,
    /// Natural height: the document lays out every block in one column and
    /// reports its full height to the parent layout, so the host's own scroll
    /// container scrolls it. The reader draws no inner scrollbar, captures no
    /// wheel input and does not clip the document. Use this when the reader is
    /// one section of a larger scrolling page (a profile README, an article).
    Flow,
}

impl MarkdownLayout {
    /// Whether the reader reports the document's natural height instead of
    /// scrolling inside a bounded viewport.
    pub fn is_flow(self) -> bool {
        matches!(self, Self::Flow)
    }
}

/// The reader view.
pub struct ReaderView {
    context: ResourceContext,
    kind: DocumentKind,
    markdown_layout: MarkdownLayout,
    state: ReaderState,
    last_action: Option<ReaderAction>,
    on_action: Option<OnAction>,
    on_layout_changed: Option<OnLayoutChanged>,
    /// The last reported natural height, so a repeat report is not forwarded.
    reported_height: Option<gpui::Pixels>,
    _document_observer: Option<Subscription>,
    pending_markdown: Option<Task<()>>,
    /// The Markdown bar's fade and drag, on the same shared [`crate::ui::scroll::Bars`]
    /// as the tab strip and the sidebar.
    bars: crate::ui::scroll::Bars,
    scroll_clock: ScrollClock,
    _activation: Subscription,
    _visibility: Subscription,
    /// Pictures of the current document generation. Reading a document without
    /// a provider keeps one honest, clearly-labelled placeholder per picture.
    images: std::rc::Rc<std::cell::RefCell<images::ImageBoard>>,
    image_provider: Option<std::sync::Arc<dyn ImageProvider>>,
    image_pump: Option<Task<()>>,
    /// Whether the pump loop is alive. The loop clears this when it settles
    /// instead of dropping its own `Task`, which would cancel it mid-poll.
    pump_running: bool,
    /// Incremented for every document generation; a completion for any other
    /// generation is discarded. Paired with a per-reader id so two readers
    /// sharing one provider never cancel or accept each other's work.
    image_generation: u64,
    image_limits: ImageLimits,
}

impl ReaderView {
    /// A view of `body` presented as `kind`, resolving links against `context`.
    pub fn new(
        body: PreparedBody,
        kind: DocumentKind,
        context: ResourceContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (state, observer, pending) =
            build_state(body, &kind, &context, cx.entity().downgrade(), window, cx);
        cx.on_release(|view, cx| {
            view.cancel_scroll_app(cx);
            // A service clone is held by each in-flight task, so dropping the
            // provider field would not stop its loads. Cancel this reader's own
            // scope, which never affects another reader sharing the provider.
            if let Some(provider) = view.image_provider.clone() {
                provider.cancel(view.image_scope());
            }
            view.stop_image_pump();
        })
        .detach();
        let image_limits = ImageLimits::default();
        let mut view = Self {
            context,
            kind,
            markdown_layout: MarkdownLayout::Default,
            state,
            last_action: None,
            on_action: None,
            on_layout_changed: None,
            reported_height: None,
            _document_observer: observer,
            pending_markdown: pending,
            bars: crate::ui::scroll::Bars::new(),
            scroll_clock: ScrollClock::new(),
            images: std::rc::Rc::new(std::cell::RefCell::new(images::ImageBoard::new(
                image_limits.clone(),
            ))),
            image_provider: None,
            image_pump: None,
            pump_running: false,
            image_generation: 0,
            image_limits,
            _activation: cx.observe_window_activation(window, |view, window, cx| {
                if !window.is_window_active() {
                    view.cancel_scroll(cx);
                }
            }),
            _visibility: cx.observe_window_visibility(window, |view, visibility, _, cx| {
                if !visibility.is_visible() {
                    view.cancel_scroll(cx);
                    view.suspend_images(cx);
                }
            }),
        };
        view.advance_images(cx);
        view
    }

    /// A view of already-prepared content; the context comes from its identity.
    pub fn from_prepared(
        content: PreparedContent,
        kind: DocumentKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let context = ResourceContext::from_prepared(&content);
        let PreparedContent { body, .. } = content;
        Self::new(body, kind, context, window, cx)
    }

    /// Replaces the shown content, keeping the context.
    pub fn show(
        &mut self,
        body: PreparedBody,
        kind: DocumentKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_scroll(cx);
        self.kind = kind;
        self.pending_markdown = None;
        let (state, observer, pending) = build_state(
            body,
            &self.kind,
            &self.context,
            cx.entity().downgrade(),
            window,
            cx,
        );
        self.state = state;
        if let Some(document) = self.markdown().cloned() {
            let layout = self.markdown_layout;
            document.update(cx, |document, cx| document.set_layout(layout, cx));
        }
        self._document_observer = observer;
        self.pending_markdown = pending;
        self.last_action = None;
        self.bars = crate::ui::scroll::Bars::new();
        // A new document is a new picture generation: every old picture and
        // every old failure is dropped, and in-flight loads are cancelled.
        self.advance_images(cx);
        cx.notify();
    }

    /// Attaches the host's picture service. Reading without one keeps an honest
    /// "no image service attached" placeholder per picture; nothing pretends to
    /// have loaded.
    ///
    /// The reader never holds a PAT: the provider owns authorization and
    /// decoding, and the reader only decides when, for which generation and for
    /// which resolved source a picture is wanted.
    pub fn set_image_provider(
        &mut self,
        provider: Option<std::sync::Arc<dyn ImageProvider>>,
        cx: &mut Context<Self>,
    ) {
        // Replacing one provider with another keeps the board attached, so an
        // attachment flip would leave the old `Loading` states and the old
        // scope in place: the new provider would never be asked and a queued
        // completion from the old one could still match. Advancing the document
        // generation makes the replacement a real new picture generation.
        let replacing_attached = self.image_provider.is_some() && provider.is_some();
        if let Some(previous) = self.image_provider.take() {
            previous.cancel(self.image_scope());
        }
        self.image_provider = provider;
        if replacing_attached {
            self.image_generation = self.image_generation.wrapping_add(1);
            let generation = self.image_generation;
            let mut board = self.images.borrow_mut();
            board.begin(generation);
            board.set_attached(true);
        } else {
            let attached = self.image_provider.is_some();
            self.images.borrow_mut().set_attached(attached);
        }
        self.attach_images_to_document(cx);
        self.ensure_image_pump(cx);
        cx.notify();
    }

    /// Stops every in-flight picture load and turns unfinished ones back into
    /// demand. Called when a retained reader's host hides its window; finished
    /// pictures stay drawn.
    fn suspend_images(&mut self, cx: &mut Context<Self>) {
        // Cancelling the provider cannot remove a completion that already sits
        // in the sink. Advancing the generation invalidates it, so a hidden
        // window's late result can never consume a slot belonging to the work
        // the visible window starts next.
        if let Some(provider) = self.image_provider.clone() {
            provider.cancel(self.image_scope());
        }
        self.stop_image_pump();
        self.image_generation = self.image_generation.wrapping_add(1);
        let generation = self.image_generation;
        let attached = self.image_provider.is_some();
        {
            let mut board = self.images.borrow_mut();
            board.advance(generation);
            board.set_attached(attached);
        }
        cx.notify();
    }

    /// Replaces the picture budgets. Limits set after pictures were requested
    /// re-create the board, so the next frame resolves and requests again.
    pub fn set_image_limits(&mut self, limits: ImageLimits, cx: &mut Context<Self>) {
        if self.image_limits == limits {
            return;
        }
        // The old board's reader identity disappears with it, so its scope must
        // be cancelled here: document changes and release only see the new one.
        if let Some(provider) = self.image_provider.clone() {
            provider.cancel(self.image_scope());
        }
        self.stop_image_pump();
        self.image_limits = limits.clone();
        let attached = self.image_provider.is_some();
        self.images = std::rc::Rc::new(std::cell::RefCell::new(images::ImageBoard::new(limits)));
        self.images.borrow_mut().set_attached(attached);
        self.image_generation = self.image_generation.wrapping_add(1);
        let generation = self.image_generation;
        self.images.borrow_mut().begin(generation);
        self.attach_images_to_document(cx);
        self.ensure_image_pump(cx);
        cx.notify();
    }

    /// The picture budgets this reader applies.
    pub fn image_limits(&self) -> &ImageLimits {
        &self.image_limits
    }

    /// This reader's current document generation scope. A host that wants to
    /// cancel the reader's picture work explicitly cancels exactly this scope.
    pub fn image_scope(&self) -> ImageScope {
        self.images.borrow().scope()
    }

    /// The state of one picture source, for tests and previews. `None` when the
    /// source was never requested in the current generation.
    #[cfg(test)]
    pub fn image_state(&self, key: &ImageKey) -> Option<String> {
        self.images
            .borrow()
            .state(key)
            .map(|state| state.label().to_string())
    }

    /// How many picture sources the current generation knows about.
    #[cfg(test)]
    pub fn known_image_states(&self) -> usize {
        self.images.borrow().known_states()
    }

    /// `(key, label)` for every picture the current generation knows, for test
    /// failure messages. Keys are the resolved source identities, so a failure
    /// shows which picture is stuck and in what state.
    #[cfg(test)]
    pub fn image_states_debug(&self) -> Vec<(String, &'static str)> {
        self.images.borrow().debug_states()
    }

    /// Whether the picture pump still has queued or outstanding work.
    #[cfg(test)]
    pub fn has_pending_images(&self) -> bool {
        self.images.borrow().work_outstanding()
    }

    /// Runs one picture beat immediately. The reader already beats on its own
    /// light timer; hosts and tests call this to advance deterministically
    /// instead of waiting for the timer.
    #[cfg(test)]
    pub fn beat_images(&mut self, cx: &mut Context<Self>) {
        self.pump_images(cx);
    }

    /// Changes Markdown layout without replacing the document, selection or reader entity.
    /// Repeated application of the same value is a no-op. Other hosts default to
    /// their previous spacing; the repository host sets this once at construction,
    /// and a page that wants the README to expand into its own scroll container
    /// sets [`MarkdownLayout::Flow`].
    pub fn set_markdown_layout(&mut self, layout: MarkdownLayout, cx: &mut Context<Self>) {
        if self.markdown_layout == layout {
            return;
        }
        self.markdown_layout = layout;
        if let Some(document) = self.markdown().cloned() {
            document.update(cx, |document, cx| document.set_layout(layout, cx));
        }
        cx.notify();
    }

    /// Whether the document currently expands to its natural height.
    pub fn is_flow(&self) -> bool {
        self.markdown_layout.is_flow()
    }

    /// Ends scrolling/selection gestures before a retained reader's host invalidates its account
    /// context or changes visibility. Content replacement and disposal also clean up automatically.
    pub fn cancel_interactions(&mut self, cx: &mut Context<Self>) {
        self.cancel_scroll(cx);
    }

    pub fn context(&self) -> &ResourceContext {
        &self.context
    }

    pub fn kind(&self) -> &DocumentKind {
        &self.kind
    }

    /// The last action the reader produced, for tests and previews.
    pub fn last_action(&self) -> Option<&ReaderAction> {
        self.last_action.as_ref()
    }

    /// The source view, when source text is shown.
    pub fn source(&self) -> Option<&Entity<SourceView>> {
        match &self.state {
            ReaderState::Source(view) => Some(view),
            _ => None,
        }
    }

    /// The markdown document, when markdown is shown.
    pub fn markdown(&self) -> Option<&Entity<markdown::MarkdownView>> {
        match &self.state {
            ReaderState::Markdown(document) => Some(document),
            _ => None,
        }
    }

    /// Whether a non-text status panel is shown.
    pub fn is_status(&self) -> bool {
        matches!(
            self.state,
            ReaderState::Empty | ReaderState::Unsupported(_) | ReaderState::TooLarge { .. }
        )
    }

    /// Takes actions instead of leaving them to the caller.
    pub fn on_action(&mut self, handler: OnAction) {
        self.on_action = Some(handler);
    }

    /// Observes natural-height changes. A Flow host uses this to re-lay out its
    /// own scroll container after a picture lands or the text re-wraps; a
    /// bounded host does not need it.
    pub fn on_layout_changed(&mut self, handler: OnLayoutChanged) {
        self.on_layout_changed = Some(handler);
    }

    /// Records the document height measured by the frame that just laid out.
    /// A changed height is forwarded once to the host through `defer`, so the
    /// host re-lays out its own scroll container outside this dispatch.
    fn note_document_height(
        &mut self,
        height: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if height <= gpui::Pixels::ZERO || self.reported_height == Some(height) {
            return;
        }
        self.reported_height = Some(height);
        if let Some(handler) = self.on_layout_changed.clone() {
            window.defer(cx, move |window, cx| handler(window, cx));
        }
    }

    /// Resolves and records one link press. The renderer calls this through the
    /// hook installed in [`build_state`]; tests call it directly.
    pub fn handle_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = links::resolve(url, &self.context);
        self.record_link(target, window, cx);
    }

    fn record_link(&mut self, target: LinkTarget, window: &mut Window, cx: &mut Context<Self>) {
        let action = ReaderAction::Link(target);
        self.last_action = Some(action.clone());
        if let Some(handler) = self.on_action.clone() {
            handler(action, window, cx);
        }
        cx.notify();
    }
}

/// Builds the state for one body. Markdown gets Ely's preview document; source
/// gets the local read-only view; the rest are status presentations. The
/// subscription follows document notifications; the document owns its own
/// real scrolling and scrollbar. The task sanitizes repository images off-thread
/// and checks the active document entity before publishing a result.
fn build_state(
    body: PreparedBody,
    kind: &DocumentKind,
    context: &ResourceContext,
    view: WeakEntity<ReaderView>,
    window: &mut Window,
    cx: &mut Context<ReaderView>,
) -> (ReaderState, Option<Subscription>, Option<Task<()>>) {
    match body {
        PreparedBody::Empty => (ReaderState::Empty, None, None),
        PreparedBody::Unsupported(reason) => (ReaderState::Unsupported(reason), None, None),
        PreparedBody::TooLarge { size, limit } => {
            (ReaderState::TooLarge { size, limit }, None, None)
        }
        PreparedBody::Utf8 { text, .. } => match kind {
            DocumentKind::Markdown => {
                let context = context.clone();
                let document = cx.new(|cx| markdown::MarkdownView::new(window, cx));
                document.update(cx, |document, cx| document.reset(text, cx));
                let pending: Option<Task<()>> = None;
                document.update(cx, |document, _| {
                    document.on_link(move |url, window, cx| {
                        let target = links::resolve(url, &context);
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |view, cx| view.record_link(target, window, cx));
                        }
                    });
                });
                let observer = cx.observe(&document, |_, _, cx| cx.notify());
                (ReaderState::Markdown(document), Some(observer), pending)
            }
            DocumentKind::Source { language } => {
                let view = cx.new(|cx| SourceView::new(text, language.clone(), window, cx));
                (ReaderState::Source(view), None, None)
            }
        },
    }
}

/// A centered status panel for the non-text outcomes.
fn status_panel(icon: IconName, title: &str, detail: Option<String>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    div()
        .debug_selector(|| "reader-status".to_string())
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .px_8()
        .text_center()
        .child(Icon::new(icon).size(IconSize::Lg).color(colors.fg_subtle))
        .child(
            div()
                .text_size(theme.text_size(TextSize::Base))
                .text_color(colors.fg)
                .child(title.to_string()),
        )
        .children(detail.map(|detail| {
            div()
                .text_size(theme.text_size(TextSize::Sm))
                .text_color(colors.fg_muted)
                .child(detail)
        }))
        .into_any_element()
}

fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} bytes", bytes as u64)
    }
}

impl ReaderView {
    /// Starts a new picture generation: old pictures, old failures, queued
    /// requests and in-flight loads all belong to the previous document.
    fn advance_images(&mut self, cx: &mut Context<Self>) {
        if let Some(provider) = self.image_provider.clone() {
            provider.cancel(self.image_scope());
        }
        self.stop_image_pump();
        self.image_generation = self.image_generation.wrapping_add(1);
        let generation = self.image_generation;
        let attached = self.image_provider.is_some();
        {
            let mut board = self.images.borrow_mut();
            board.begin(generation);
            board.set_attached(attached);
        }
        self.attach_images_to_document(cx);
        self.ensure_image_pump(cx);
    }

    /// Hands the board and the fixed resource context to the Markdown document.
    fn attach_images_to_document(&self, cx: &mut Context<Self>) {
        if let Some(document) = self.markdown().cloned() {
            let board = self.images.clone();
            let context = self.context.clone();
            document.update(cx, |document, _| document.set_images(board, context));
        }
    }

    /// Drains completions of the current generation and starts a receiver while
    /// a provider is attached. Only this reader's own generation is applied.
    /// Starts the picture pump when work is outstanding.
    ///
    /// The pump is a light beat on the GPUI executor, not a cross-thread wake
    /// and not a per-frame loop: it self-terminates as soon as nothing is queued
    /// or owed, and the next frame that meets an unrequested picture starts it
    /// again. No I/O or decoding happens here.
    fn ensure_image_pump(&mut self, cx: &mut Context<Self>) {
        if self.image_provider.is_none() {
            self.images.borrow_mut().clear_pending();
            self.stop_image_pump();
            return;
        }
        self.dispatch_images(cx);
        if self.pump_running || !self.images.borrow().work_outstanding() {
            return;
        }
        self.pump_running = true;
        self.image_pump = Some(cx.spawn(async move |view: WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(20))
                    .await;
                let more = view
                    .update(cx, |view, cx| view.pump_images(cx))
                    .unwrap_or(false);
                if !more {
                    // The handle points at this very task; clearing it here
                    // would cancel the future mid-update, so the stop is
                    // recorded and the field is released after the loop ends.
                    break;
                }
            }
        }));
    }

    /// One pump beat: apply what finished, hand out what is queued, and report
    /// whether anything is still outstanding.
    fn pump_images(&mut self, cx: &mut Context<Self>) -> bool {
        if self.image_provider.is_none() {
            self.images.borrow_mut().clear_pending();
            self.pump_running = false;
            return false;
        }
        self.drain_images(cx);
        self.dispatch_images(cx);
        let more = self.images.borrow().work_outstanding();
        if !more {
            // Only the flag: the running loop reads it and exits on its own, so
            // dropping the `Task` can never cancel the future mid-poll.
            self.pump_running = false;
        }
        more
    }

    /// Cancels the pump task and marks it stopped. Called when the document,
    /// the provider or the window visibility changes.
    fn stop_image_pump(&mut self) {
        self.image_pump = None;
        self.pump_running = false;
    }

    /// Applies finished loads. A completion from another generation, another
    /// reader or a replaced document is dropped without touching state.
    fn drain_images(&mut self, cx: &mut Context<Self>) {
        let completions = self.images.borrow().sink().drain();
        if completions.is_empty() {
            return;
        }
        let mut changed = false;
        for completion in completions {
            let blocks = self.images.borrow_mut().apply(completion);
            if blocks.is_empty() {
                continue;
            }
            changed = true;
            if let Some(document) = self.markdown().cloned() {
                for block in blocks {
                    document.update(cx, |document, cx| document.remeasure_block(block, cx));
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Hands queued requests to the provider, bounded by the in-flight budget.
    fn dispatch_images(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.image_provider.clone() else {
            return;
        };
        let (requests, sink) = {
            let mut board = self.images.borrow_mut();
            let max = board.limits().max_in_flight.max(1);
            (board.take_pending(max), board.sink())
        };
        for request in requests {
            provider.start(request, sink.clone());
        }
        let released = self.images.borrow_mut().evict();
        if !released.is_empty() {
            if let Some(document) = self.markdown().cloned() {
                for block in released {
                    document.update(cx, |document, cx| document.remeasure_block(block, cx));
                }
            }
            cx.notify();
        }
    }

    fn cancel_scroll(&mut self, cx: &mut Context<Self>) {
        self.cancel_scroll_app(cx);
        cx.notify();
    }

    fn cancel_scroll_app(&mut self, cx: &mut App) {
        match &self.state {
            ReaderState::Markdown(document) => {
                let motion = document.read(cx).motion().clone();
                scroll::cancel_region(&mut self.bars, &motion);
                document.update(cx, |doc, cx| doc.cancel_gesture(cx));
            }
            ReaderState::Source(source) => source.update(cx, |view, cx| view.cancel_scroll(cx)),
            _ => {}
        }
    }

    fn settle_bars(&mut self, cx: &mut Context<Self>) {
        if let Some(document) = self.markdown() {
            let motion = document.read(cx).motion().clone();
            if !cx.has_active_drag() {
                self.bars.settle(&motion, self.now_ms(), cx.reduce_motion());
            }
            // Active drag is cleared by GPUI after mouse-up dispatch.
            cx.notify();
        }
    }

    fn now_ms(&self) -> u64 {
        self.scroll_clock.ms()
    }

    /// Freezes the Markdown bar's fade clock for tests.
    #[cfg(test)]
    pub fn pin_scrollbar_clock(&mut self, ms: u64) {
        self.scroll_clock.pin(ms);
    }

    /// Shows the Markdown bar without waiting for a hover fade.
    #[cfg(test)]
    pub fn show_scrollbar(&mut self) {
        self.bars.snap(true);
    }
}

impl Render for ReaderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lifetime = scroll_lifetime(cx, Self::cancel_scroll);
        // A frame that first meets a picture queues it on the board; this only
        // restarts the pump a hidden window or a replaced provider stopped. No
        // fetch or decode happens here.
        if self.image_provider.is_some() && self.image_pump.is_none() {
            self.ensure_image_pump(cx);
        }
        let theme = cx.theme();
        let body = match &self.state {
            ReaderState::Empty => status_panel(
                IconName::FileText,
                "This file is empty.",
                Some("GitHub returned zero bytes for this file.".to_string()),
                cx,
            ),
            ReaderState::Unsupported(reason) => {
                let why = match reason {
                    UnsupportedContent::NonUtf8 => "It is not valid UTF-8 text.",
                    UnsupportedContent::BinaryControlCharacters => {
                        "It looks like binary data rather than text."
                    }
                };
                status_panel(
                    IconName::FileText,
                    "This file cannot be shown as text.",
                    Some(why.to_string()),
                    cx,
                )
            }
            ReaderState::TooLarge { size, limit } => status_panel(
                IconName::TriangleAlert,
                "This file is too large to show.",
                Some(format!(
                    "{} exceeds the {} read limit.",
                    human_size(*size),
                    human_size(*limit as u64)
                )),
                cx,
            ),
            ReaderState::Markdown(document) if self.markdown_layout.is_flow() => {
                // Natural height: the document column measures every block and
                // the host's scroll container scrolls it. No inner bar, no
                // wheel capture and no clipping here.
                //
                // Both callbacks are deferred: prepaint runs after this render,
                // so the measured height and any demand the child blocks queued
                // while building this frame are handled by the next dispatch
                // instead of re-entering it.
                let measured = cx.entity().downgrade();
                div()
                    .debug_selector(|| "reader-flow".to_string())
                    .w_full()
                    .min_w_0()
                    .on_children_prepainted(move |painted, window, cx| {
                        let height = painted
                            .first()
                            .map(|bounds| bounds.size.height)
                            .unwrap_or(gpui::Pixels::ZERO);
                        let measured = measured.clone();
                        window.defer(cx, move |window, cx| {
                            let _ = measured.update(cx, |view, cx| {
                                view.note_document_height(height, window, cx);
                            });
                        });
                    })
                    .child(document.clone())
                    .into_any_element()
            }
            ReaderState::Markdown(document) => {
                let document = document.clone();
                let state = document.read(cx).motion().clone();
                let now = self.now_ms();
                let reduce = cx.reduce_motion() || cx.theme().reduced_motion;
                // A programmatic end anchor uses item_count as a sentinel until
                // the list measures backwards from its lazy tail. Clamping that
                // provisional offset through Motion before layout would replace
                // the sentinel with an earlier measured row and miss the end.
                let measuring_tail = self.markdown_layout == MarkdownLayout::Repository
                    && document.read(cx).block_count() > 0
                    && document.read(cx).list_state().logical_scroll_top().item_ix
                        >= document.read(cx).block_count();
                let moving = if measuring_tail {
                    state.cancel();
                    true
                } else {
                    state.tick(self.scroll_clock.now(), reduce)
                };
                // The platform may end a thumb drag outside the region; settle
                // it so the bar returns to its hover resting state.
                if self.bars.dragging().is_some() && !cx.has_active_drag() {
                    self.bars.settle(&state, now, reduce);
                }
                let opacity = self.bars.opacity(now);
                if moving || self.bars.is_animating() {
                    window.request_animation_frame();
                }
                let this = cx.entity().downgrade();
                let on_drag: crate::ui::scroll::OnDragging =
                    std::rc::Rc::new(move |axis, dragging, _: &mut Window, cx: &mut App| {
                        let _ = this.update(cx, |view, cx| {
                            let now = view.now_ms();
                            let reduce = cx.reduce_motion();
                            if let Some(state) = view
                                .markdown()
                                .map(|document| document.read(cx).motion().clone())
                            {
                                view.bars.drag(&state, axis, dragging, now, reduce);
                            }
                            cx.notify();
                        });
                    });
                let bar = crate::ui::scroll::Bar {
                    target: &state,
                    id: crate::ui::scroll::BarId {
                        owner: cx.entity_id(),
                        name: "reader-markdown-scrollbar",
                        axis: gpui::Axis::Vertical,
                    },
                    placement: crate::ui::scroll::Placement::overlay(1.5, 8.0),
                    style: crate::ui::scroll::Style::DEFAULT,
                    opacity,
                    on_drag,
                }
                .render(cx);
                let clock = self.scroll_clock.clone();
                div()
                    .id("reader-markdown-root")
                    .debug_selector(|| "reader-markdown".to_string())
                    .relative()
                    .size_full()
                    .on_hover(cx.listener(|view, hovered, _, cx| {
                        if view.bars.hover(*hovered, view.now_ms(), cx.reduce_motion()) {
                            cx.notify();
                        }
                    }))
                    .child(document)
                    .child(scroll::wheel_overlay_with(
                        "reader-markdown",
                        scroll::WheelAxes::Vertical,
                        state,
                        move || clock.now(),
                    ))
                    .child(bar)
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|view, _, _, cx| view.settle_bars(cx)),
                    )
                    .on_mouse_up_out(
                        gpui::MouseButton::Left,
                        cx.listener(|view, _, _, cx| view.settle_bars(cx)),
                    )
                    .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            view.cancel_scroll(cx);
                        }
                    }))
                    .into_any_element()
            }
            // Keep the source root's overlay siblings direct: an extra
            // full-size wrapper can reorder hit testing against the rows.
            ReaderState::Source(view) => view.clone().into_any_element(),
        };
        // Blocks queue their picture demand while a frame is built, which is
        // after the pump already settled for that render. This prepaint hook
        // restarts the pump for whatever was queued, so the requests reach the
        // provider without needing an unrelated redraw. It covers both the
        // bounded viewport and the natural-height presentation.
        let demand = cx.entity().downgrade();
        let element = div()
            .debug_selector(|| "reader".to_string())
            .size_full()
            .min_h_0()
            .bg(theme.colors.surface)
            .text_color(theme.colors.fg)
            .on_children_prepainted(move |_, window, cx| {
                // Blocks queue their picture demand while `body` is built, which
                // happens after the pump settled for the previous frame. Hand it
                // to the provider and make sure a receiver is running, so the
                // request reaches the provider without an unrelated redraw.
                let demand = demand.clone();
                window.defer(cx, move |_window, cx| {
                    let _ = demand.update(cx, |view, cx| {
                        if view.image_provider.is_some() {
                            view.ensure_image_pump(cx);
                        }
                    });
                });
            })
            .child(body)
            .child(lifetime)
            .into_any_element();
        // Dispatch synchronously too: picture demand is complete by the time
        // `body` was built above, so the request starts in this frame. Spawning
        // a task is left to the deferred hook, because a draw must not spawn.
        if self.image_provider.is_some() {
            self.dispatch_images(cx);
        }
        element
    }
}
