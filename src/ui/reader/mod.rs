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
mod links;
mod scrollbar;
mod source;
#[cfg(test)]
mod tests;

pub use links::LinkTarget;
pub use source::SourceView;

use std::rc::Rc;

use ely_gpui_component::{
    documents::{MarkdownDocument, MarkdownMode},
    primitives::{Icon, IconName},
    theme::{ActiveTheme, IconSize, TextSize},
};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div,
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

enum ReaderState {
    Empty,
    Unsupported(UnsupportedContent),
    TooLarge { size: u64, limit: usize },
    Markdown(Entity<MarkdownDocument>),
    Source(Entity<SourceView>),
}

/// The reader view.
pub struct ReaderView {
    context: ResourceContext,
    kind: DocumentKind,
    state: ReaderState,
    last_action: Option<ReaderAction>,
    on_action: Option<OnAction>,
    /// The Markdown reading area's hover/drag fade, shared with the source and
    /// tab scrollbars.
    scroll_fade: crate::ui::scrollbar::Fade,
    scroll_clock: std::time::Instant,
    thumb_dragging: bool,
    /// Re-renders this view when the Markdown list scrolls, so its overlay
    /// thumb follows the real offset instead of a stale frame.
    _document_observer: Option<Subscription>,
    #[cfg(test)]
    pinned_clock: Option<u64>,
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
        let (state, observer) =
            build_state(body, &kind, &context, cx.entity().downgrade(), window, cx);
        Self {
            context,
            kind,
            state,
            last_action: None,
            on_action: None,
            scroll_fade: Default::default(),
            scroll_clock: std::time::Instant::now(),
            thumb_dragging: false,
            _document_observer: observer,
            #[cfg(test)]
            pinned_clock: None,
        }
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
        self.kind = kind;
        let (state, observer) = build_state(
            body,
            &self.kind,
            &self.context,
            cx.entity().downgrade(),
            window,
            cx,
        );
        self.state = state;
        self._document_observer = observer;
        self.last_action = None;
        self.thumb_dragging = false;
        self.scroll_fade = Default::default();
        cx.notify();
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
    pub fn markdown(&self) -> Option<&Entity<MarkdownDocument>> {
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
/// returned subscription keeps this view redrawing while the Markdown list
/// scrolls, so its overlay thumb tracks the real offset.
fn build_state(
    body: PreparedBody,
    kind: &DocumentKind,
    context: &ResourceContext,
    view: WeakEntity<ReaderView>,
    window: &mut Window,
    cx: &mut Context<ReaderView>,
) -> (ReaderState, Option<Subscription>) {
    match body {
        PreparedBody::Empty => (ReaderState::Empty, None),
        PreparedBody::Unsupported(reason) => (ReaderState::Unsupported(reason), None),
        PreparedBody::TooLarge { size, limit } => (ReaderState::TooLarge { size, limit }, None),
        PreparedBody::Utf8 { text, .. } => match kind {
            DocumentKind::Markdown => {
                let context = context.clone();
                // The text is handed to `reset` rather than to `new`: with the
                // text at construction, Ely's document splices an async read
                // into a list that already measured zero items and then draws
                // no blocks at all (reproduced on the GPUI test platform at
                // ~130 KiB and above). `reset` asks for the top line once the
                // read lands, which keeps the list anchored at block 0, and
                // still parses off the UI thread. See `docs/content-reader.md`.
                let document = cx.new(|cx| {
                    let mut document =
                        MarkdownDocument::new(String::new(), MarkdownMode::Preview, window, cx);
                    document.reset(text, cx);
                    document
                });
                document.update(cx, |document, _| {
                    document.on_link(move |url, window, cx| {
                        let target = links::resolve(url, &context);
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |view, cx| view.record_link(target, window, cx));
                        }
                    });
                    document.set_reader_inline(true);
                });
                let observer = cx.observe(&document, |_, _, cx| cx.notify());
                (ReaderState::Markdown(document), Some(observer))
            }
            DocumentKind::Source { language } => {
                let view = cx.new(|cx| SourceView::new(text, language.clone(), window, cx));
                (ReaderState::Source(view), None)
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
    fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(ms) = self.pinned_clock {
            return ms;
        }
        self.scroll_clock.elapsed().as_millis() as u64
    }

    /// Freezes the Markdown scrollbar's fade clock, for tests that need the
    /// bar present without stepping a real fade.
    #[cfg(test)]
    pub fn pin_scrollbar_clock(&mut self, ms: u64) {
        self.pinned_clock = Some(ms);
    }

    /// Shows the Markdown scrollbar without waiting for a hover fade.
    #[cfg(test)]
    pub fn show_scrollbar(&mut self) {
        self.scroll_fade.snap(true);
    }
}

impl Render for ReaderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            ReaderState::Markdown(document) => {
                let document = document.clone();
                let state = document.read(cx).list_state().clone();
                // A drag ends when GPUI clears the active drag; release it here
                // so the list's frozen drag height is dropped and the fade can
                // fall back to the hover state.
                if self.thumb_dragging && !cx.has_active_drag() {
                    self.thumb_dragging = false;
                    self.scroll_fade.set_dragging(false, self.now_ms());
                    state.scrollbar_drag_ended();
                }
                let opacity = self.scroll_fade.opacity(self.now_ms());
                if self.scroll_fade.is_animating() {
                    window.request_animation_frame();
                }
                let owner = cx.entity_id();
                let this = cx.entity().downgrade();
                let on_dragging =
                    std::rc::Rc::new(move |dragging: bool, _: &mut Window, cx: &mut App| {
                        let _ = this.update(cx, |view, cx| {
                            view.thumb_dragging = dragging;
                            view.scroll_fade.set_dragging(dragging, view.now_ms());
                            cx.notify();
                        });
                    });
                let bar = scrollbar::render_list(
                    &state,
                    opacity,
                    owner,
                    "reader-markdown-scrollbar",
                    on_dragging,
                    cx,
                );
                div()
                    .id("reader-markdown-root")
                    .debug_selector(|| "reader-markdown".to_string())
                    .relative()
                    .size_full()
                    .on_hover(cx.listener(|view, hovered, _, cx| {
                        if view.scroll_fade.set_hovered(*hovered, view.now_ms()) {
                            cx.notify();
                        }
                    }))
                    .child(document)
                    .child(bar)
                    .into_any_element()
            }
            // Keep the source root's overlay siblings direct: an extra
            // full-size wrapper can reorder hit testing against the rows.
            ReaderState::Source(view) => view.clone().into_any_element(),
        };
        div()
            .debug_selector(|| "reader".to_string())
            .size_full()
            .min_h_0()
            .bg(theme.colors.surface)
            .text_color(theme.colors.fg)
            .child(body)
    }
}
