//! Ely component composition and view-local state. No business I/O during rendering.
//!
//! The window is a custom title bar over a fixed sidebar and one island. Only the sidebar's
//! repository group scrolls: the fixed rows and the island keep their places when the window is
//! resized. The island's top holds the application session tab strip, then the active page.
//! Sessions, their routes and their page-local state live in [`crate::workspace`].

use std::rc::Rc;

use ely_core::{
    forms::InputEvent,
    primitives::{FocusScope, IconName},
    theme::ActiveTheme,
};
#[cfg(test)]
use gpui::Point;
use gpui::{
    App, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, Styled, Window, actions, div, prelude::*, px,
};

use crate::{
    app::{
        AccountsHandle,
        accounts::{Accounts, Snapshot},
    },
    domain::{AccountKey, AccountSummary, RepoSummary},
    platform::fonts,
    ui::accounts::RepositoryPreview,
    workspace::{OpenResult, Route, Sessions, TabId},
};

pub mod accounts;
mod chrome;
pub mod fixtures;
pub(crate) mod geometry;
mod page;
mod palette;
pub mod reader;
pub(crate) mod scroll;
mod sidebar;
mod sidebar_state;
mod tabs;

#[cfg(test)]
mod sidebar_tests;

#[cfg(test)]
mod tests;

actions!(
    githux,
    [
        OpenCommandPalette,
        CloseActiveTab,
        NextTab,
        PreviousTab,
        CancelTabDrag
    ]
);

pub struct Shell {
    /// One workspace per account plus the logged-out one. Deref reaches the visible workspace, so
    /// routing, tab handling and focus keep one code path.
    workspace: Sessions,
    /// The account controller, when the application installed one. The app-global owns the
    /// controller; the Shell keeps only a weak handle so it never creates a strong cycle that would
    /// stop the controller's `Drop` (and the account-runtime teardown) from running.
    accounts: Option<gpui::WeakEntity<Accounts>>,
    /// The snapshot of the last frame, so the account/sidebar/tab publication can be compared
    /// across frames instead of being re-read mid-render.
    account_state: Snapshot,
    /// Real `/user/repos` rows of the current account. Never fixture data.
    repositories: Vec<RepoSummary>,
    /// One tab stop per repository row, rebuilt when the row set changes.
    repo_focus: Vec<FocusHandle>,
    repo_ident: (Option<AccountKey>, usize),
    root_focus: FocusHandle,
    repo_scroll: ScrollHandle,
    repo_smooth: scroll::Motion<gpui::ScrollHandle>,
    /// Visibility and thumb drag belong only to the repository viewport.
    repo_bars: scroll::Bars,
    /// The repository row last scrolled into view, so a still focus does not scroll every frame.
    revealed_row: Option<usize>,
    palette: Option<palette::Session>,
    restore_focus: bool,
    search_focus: FocusHandle,
    /// The strip's horizontal offset. One handle for the whole row, so the drag, the wheel and the
    /// scrollbar all move the same scroll position.
    tab_scroll: ScrollHandle,
    /// Horizontal wheel motion belongs to the strip and shares the same target as its Bar.
    tab_motion: scroll::Motion<ScrollHandle>,
    /// A tab the strip still has to bring into view, resolved in render once its bounds exist.
    revealed_tab: Option<TabId>,
    /// The tab whose strip entry last took keyboard focus, so Tab onto an offscreen tab reveals it
    /// once instead of on every frame.
    focused_tab: Option<TabId>,
    /// The strip's scrollable reach as of the last frame, so a newly overflowing (or no longer
    /// overflowing) strip repaints once to show or hide the scrollbar.
    tab_reach: gpui::Pixels,
    /// The strip scrollbar's state: the hover/drag/hold fade, owned only by this region.
    tab_bars: scroll::Bars,
    /// A tab drag in progress: the session being carried and the slot it would land on.
    tab_drag: Option<TabDragState>,
    /// Monotonic start for the strip fade's clock. Read once per frame; the fade itself takes an
    /// explicit millisecond value so it can be driven deterministically in tests.
    tab_clock: std::time::Instant,
    /// A fixed clock for tests, so a fade can be stepped without sleeping.
    #[cfg(test)]
    tab_clock_pinned: Option<u64>,
    /// Identity stamped into tab drag payloads, so another strip's drag is ignored.
    tab_strip_owner: gpui::EntityId,
    /// Set while a test drives a tab drag directly, so the render loop's "the platform drag ended"
    /// check does not cancel it before the test can drop it.
    #[cfg(test)]
    tab_drag_pinned: bool,
    sidebar_state: sidebar_state::SidebarState,
    sidebar_activation: gpui::Subscription,
    sidebar_keys: gpui::Subscription,
    sidebar_bounds: gpui::Subscription,
    sidebar_more_focus: FocusHandle,
    /// The resize heat band in window coordinates, captured from the real layout each prepaint.
    /// Pointer events report window positions, while a band built from the shell body's local
    /// origin would be off by the title bar inset; this keeps both in one space.
    sidebar_resize_band: Option<gpui::Bounds<gpui::Pixels>>,
    /// The shell body's left edge in window coordinates, so pointer positions can be converted to
    /// the container space the sidebar geometry uses (title-bar/DPI safe).
    sidebar_body_left: Option<gpui::Pixels>,
    /// The collapse/expand toggle's own focus. Keyboard activation must keep focus here so a
    /// second Enter/Space reverses the mode instead of dropping into the shell root.
    sidebar_toggle_focus: FocusHandle,
    /// Focus while the collapsed More menu is open. A dedicated handle (not the trigger's) so
    /// Escape, arrows and Enter reach the panel, not the row behind it.
    sidebar_menu_focus: FocusHandle,
    /// Whether the collapsed More menu is open. Owned here so every dismissal path agrees.
    sidebar_menu: bool,
    /// Keyboard highlight inside the collapsed More menu.
    sidebar_menu_cursor: usize,
    /// Ends tab drags and scrollbar visibility when the window loses activation.
    _window_activation: Option<gpui::Subscription>,
}

/// One in-flight tab drag.
struct TabDragState {
    id: TabId,
    /// The insertion slot the marker is drawn in: a gap between the tabs as they are currently
    /// shown, in the still-unmodified collection.
    slot: usize,
    /// The index to hand to `Workspace::reorder`, which removes the dragged session first. It only
    /// differs from `slot` for rightward drags.
    target: usize,
    /// The pointer's position over the window, retained so edge auto-scroll can keep advancing on
    /// later frames while the pointer itself is stationary.
    position: gpui::Point<gpui::Pixels>,
}

impl Shell {
    /// Binds the shell's own shortcuts once per application. `secondary` is Ctrl on Windows and
    /// Linux and Cmd on macOS, the same chord the title bar's search entry prints. The tab chords
    /// avoid Enter/Escape/arrows, so they never race input-method candidate confirmation or text
    /// entry; while the palette is open the shell leaves the keyboard to it.
    pub fn bind_keys(cx: &mut App) {
        // Static KeyBinding::new no longer resolves `secondary`; load it with the app's OS.
        let open_palette = gpui::KeyBinding::load(
            "secondary-k",
            Box::new(OpenCommandPalette),
            None,
            false,
            None,
            cx,
        )
        .expect("valid command palette binding");
        let close_tab = gpui::KeyBinding::load(
            "secondary-w",
            Box::new(CloseActiveTab),
            None,
            false,
            None,
            cx,
        )
        .expect("valid close tab binding");
        cx.bind_keys([
            open_palette,
            close_tab,
            gpui::KeyBinding::new("ctrl-tab", NextTab, None),
            gpui::KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
            // Escape only does something while a tab is being carried; the palette intercepts
            // its own Escape before bindings are matched, so this cannot close an overlay.
            gpui::KeyBinding::new("escape", CancelTabDrag, None),
        ]);
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root_focus = cx.focus_handle().tab_index(-2);
        window.focus(&root_focus, cx);
        // The search entry and the session tabs sit at the front of the tab order (index -1),
        // ahead of the long repository list, so keyboard navigation reaches them directly.
        let search_focus = cx.focus_handle().tab_stop(true).tab_index(-1);
        let repo_scroll = ScrollHandle::new();
        // The account controller is installed by `app::run` before this window is created. Its
        // first publication decides the logged-out or signed-in shell; no fixture account or
        // fixture repository list is ever used as a starting value.
        let accounts = AccountsHandle::get(cx);
        let account_state = accounts
            .as_ref()
            .map(|accounts| accounts.read(cx).snapshot().clone())
            .unwrap_or_else(Snapshot::logged_out_unavailable);
        let accounts = accounts.map(|accounts| accounts.downgrade());
        let repositories = account_state.repositories.rows().to_vec();
        let repo_focus = (0..repositories.len())
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect::<Vec<_>>();
        let repo_ident = (account_state.active.clone(), repositories.len());
        let sidebar_activation = cx.observe_window_activation(window, |shell, window, cx| {
            if !window.is_window_active() {
                scroll::cancel_region(&mut shell.repo_bars, &shell.repo_smooth);
                shell.close_sidebar_menu(window, cx);
                let resizing = shell.sidebar_state.dragging;
                shell.sidebar_state.end_resize(
                    cx.background_executor().now(),
                    false,
                    cx.theme().reduced_motion || cx.reduce_motion(),
                );
                if resizing {
                    cx.stop_active_drag(window);
                }
                cx.notify();
            }
        });
        // A changed window is not a new drag. The remembered expanded width is kept and
        // clamped only when used, so widening the window again restores the user's choice.
        let sidebar_bounds = cx.observe_window_bounds(window, |shell, window, cx| {
            if shell.sidebar_state.dragging || shell.repo_bars.dragging().is_some() {
                cx.stop_active_drag(window);
            }
            scroll::cancel_region(&mut shell.repo_bars, &shell.repo_smooth);
            scroll::cancel_region(&mut shell.tab_bars, &shell.tab_motion);
            for session in shell.workspace.sessions() {
                session.state().smooth.cancel();
            }
            if let Some(palette) = &shell.palette {
                palette.smooth.cancel();
            }
            shell.sidebar_state.end_resize(
                cx.background_executor().now(),
                false,
                cx.theme().reduced_motion || cx.reduce_motion(),
            );
            // Ely closes its menu when focus leaves. Keep it anchored safely when the
            // viewport changes, including maximize/restore, without retaining a stale panel.
            shell.close_sidebar_menu(window, cx);
            shell
                .sidebar_state
                .settle(window.viewport_size().width.into());
            cx.notify();
        });
        let tab_scroll = ScrollHandle::new();
        let this = cx.entity();
        let sidebar_keys = cx.intercept_keystrokes(move |event, window, cx| {
            this.update(cx, |shell, cx| {
                if matches!(event.keystroke.key.as_str(), "tab") && shell.sidebar_menu {
                    // The menu is modal while open: Tab must not park focus behind it.
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" && shell.sidebar_menu {
                    cx.stop_propagation();
                    shell.close_sidebar_menu(window, cx);
                } else if event.keystroke.key == "escape" && shell.repo_bars.dragging().is_some() {
                    scroll::cancel_region(&mut shell.repo_bars, &shell.repo_smooth);
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    cx.notify();
                } else if event.keystroke.key == "escape" && shell.sidebar_state.dragging {
                    shell.sidebar_state.end_resize(
                        cx.background_executor().now(),
                        false,
                        cx.theme().reduced_motion || cx.reduce_motion(),
                    );
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    cx.notify();
                }
            });
        });
        let mut this = Self {
            sidebar_keys,
            sidebar_bounds,
            sidebar_more_focus: cx.focus_handle(),
            sidebar_resize_band: None,
            sidebar_body_left: None,
            sidebar_toggle_focus: cx.focus_handle(),
            sidebar_menu_focus: cx.focus_handle(),
            sidebar_menu: false,
            sidebar_menu_cursor: 0,
            sidebar_state: sidebar_state::SidebarState::default(),
            sidebar_activation,
            workspace: Sessions::new(cx),
            accounts: accounts.clone(),
            account_state,
            repositories,
            root_focus,
            repo_smooth: scroll::Motion::new(repo_scroll.clone()),
            repo_bars: scroll::Bars::new(),
            repo_scroll,
            repo_focus,
            repo_ident,
            revealed_row: None,
            palette: None,
            restore_focus: false,
            search_focus,
            tab_scroll: tab_scroll.clone(),
            revealed_tab: None,
            focused_tab: None,
            tab_reach: gpui::Pixels::ZERO,
            tab_motion: scroll::Motion::new(tab_scroll.clone()),
            tab_bars: scroll::Bars::new(),
            tab_drag: None,
            tab_clock: std::time::Instant::now(),
            #[cfg(test)]
            tab_clock_pinned: None,
            tab_strip_owner: cx.entity_id(),
            #[cfg(test)]
            tab_drag_pinned: false,
            _window_activation: None,
        };
        // A window that loses activation cannot keep delivering a drag, and the pointer's
        // hover is stale too; drop both instead of leaving a pinned bar behind.
        this._window_activation =
            Some(cx.observe_window_activation(window, |shell, window, cx| {
                if !window.is_window_active() {
                    shell.end_drags_on_blur(window, cx);
                }
            }));
        // Publish the controller's state as soon as it changes: an account switch, a repository
        // list arriving, or a restore finishing. The projection runs once per change, not per
        // frame, and never on a background thread.
        if let Some(accounts) = accounts.as_ref().and_then(|weak| weak.upgrade()) {
            let accounts_weak = accounts.downgrade();
            cx.observe(&accounts, move |shell, _, cx| {
                // The observer carries only a weak handle to the controller; the closure must not
                // keep it alive forever, or its Drop (and the account-runtime teardown) never runs.
                if let Some(accounts) = accounts_weak.upgrade() {
                    shell.sync_accounts(Some(&accounts), cx);
                    cx.notify();
                }
            })
            .detach();
        }
        this.sync_accounts(
            accounts.as_ref().and_then(|weak| weak.upgrade()).as_ref(),
            cx,
        );
        this
    }

    /// Projects the account controller's snapshot into the shell: the visible workspace, the
    /// repository rows, their tab stops and the stored account identity.
    ///
    /// The account is taken from authoritative service state every time. Nothing here trusts a
    /// reply, an optimistic selection or an injected request; the controller only publishes a
    /// settled account, and this projection only reads it.
    fn sync_accounts(&mut self, accounts: Option<&gpui::Entity<Accounts>>, cx: &mut Context<Self>) {
        let Some(accounts) = accounts.cloned() else {
            return;
        };
        let snapshot = accounts.read(cx).snapshot().clone();
        let visible = snapshot.active.clone();
        let ident = (visible.clone(), snapshot.repositories.len());
        if self.account_state.active != snapshot.active
            || self.repo_ident != ident
            || self.account_state.repositories != snapshot.repositories
        {
            if self.workspace.show(visible, cx) {
                // A new visible workspace: nothing from the previous account may stay active.
                self.cancel_account_scoped_view_state(cx);
            }
            self.repositories = snapshot.repositories.rows().to_vec();
            self.repo_focus = (0..self.repositories.len())
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect();
            self.repo_ident = ident;
            self.revealed_row = None;
            // The row set changed under the pointer: an in-flight reveal or wheel target is stale.
            scroll::cancel_region(&mut self.repo_bars, &self.repo_smooth);
            self.repo_scroll
                .set_offset(gpui::Point::new(gpui::Pixels::ZERO, gpui::Pixels::ZERO));
        }
        self.account_state = snapshot;
    }

    /// Drops every piece of view state that belonged to the workspace that just stopped being
    /// visible. The remembered tabs themselves keep their scroll, focus and repository selection;
    /// what is dropped is the shell's own transient state and any pending background reveal.
    fn cancel_account_scoped_view_state(&mut self, cx: &mut Context<Self>) {
        self.close_palette(cx);
        // The tab drag carries a TabId from the workspace that is no longer visible.
        self.tab_drag = None;
        self.tab_motion.cancel();
        self.tab_bars
            .hold(false, self.tab_now_ms(), cx.reduce_motion());
        self.repo_smooth.cancel();
        self.revealed_tab = None;
        self.focused_tab = None;
        self.tab_reach = gpui::Pixels::ZERO;
        self.sidebar_state.menu_open.set(false);
        self.sidebar_menu = false;
    }

    /// The active account's identity, for the sidebar row and the Profile page.
    fn active_account(&self) -> Option<&AccountSummary> {
        let active = self.account_state.active.as_ref()?;
        self.account_state
            .accounts
            .iter()
            .find(|summary| &summary.identity.key == active)
    }

    /// Ends the tab drag and the thumb drag without a drop, and lets the bar fade back to the
    /// real pointer state.
    fn end_drags_on_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_tab_drag(window, cx);
        scroll::cancel_region(&mut self.tab_bars, &self.tab_motion);
        // A lost activation also clears every page region's fade and thumb drag (the Settings
        // region is one of these), matching the repository and tab bars.
        for id in self
            .workspace
            .sessions()
            .iter()
            .map(|session| session.id())
            .collect::<Vec<_>>()
        {
            if let Some(state) = self.workspace.state_mut(id) {
                scroll::cancel_region(&mut state.bars, &state.smooth);
            }
        }
        if let Some(palette) = &self.palette {
            palette.smooth.cancel();
        }
        cx.notify();
    }

    /// Replaces the shell's repository rows in tests, exactly as a real `/user/repos` result would
    /// populate them. Production only ever takes them from the account controller.
    #[cfg(test)]
    pub fn set_repositories(&mut self, rows: Vec<RepoSummary>, cx: &mut Context<Self>) {
        self.repositories = rows;
        self.repo_focus = (0..self.repositories.len())
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect();
        self.revealed_row = None;
        cx.notify();
    }

    /// Sets the real account identity used by the account row and the profile page in tests that
    /// do not need the network path. Production only ever reads this from the service snapshot.
    #[cfg(test)]
    pub fn set_account(&mut self, summary: Option<AccountSummary>, cx: &mut Context<Self>) {
        self.account_state.accounts = summary.clone().into_iter().collect();
        self.account_state.active = summary.map(|summary| summary.identity.key);
        cx.notify();
    }

    /// The current page, inspected by the routing tests.
    #[cfg(test)]
    pub fn route(&self) -> Route {
        self.workspace.route().clone()
    }

    /// The local fixture list behind the sidebar, inspected by the layout tests.
    #[cfg(test)]
    pub fn repositories(&self) -> &[RepoSummary] {
        &self.repositories
    }

    /// The repository group's scroll offset, for the bounded-scroll test.
    /// The repository rows' tab stops, inspected by the keyboard-scroll test.
    #[cfg(test)]
    pub fn repo_focus(&self) -> &[FocusHandle] {
        &self.repo_focus
    }

    #[cfg(test)]
    pub fn repo_offset(&self) -> Point<gpui::Pixels> {
        self.repo_scroll.offset()
    }

    /// The active page's scroll offset, for the bounded-scroll test.
    #[cfg(test)]
    pub fn page_offset(&self) -> Point<gpui::Pixels> {
        self.workspace.active().state().scroll.offset()
    }

    /// The active page's maximum scroll offset, for the wheel-occlusion test.
    #[cfg(test)]
    pub fn page_max_offset(&self) -> Point<gpui::Pixels> {
        self.workspace.active().state().scroll.max_offset()
    }

    /// The Settings page region's `Bars`, for the fade/drag lifecycle regression tests.
    #[cfg(test)]
    pub fn page_bars(&self, id: TabId) -> Option<&scroll::Bars> {
        self.workspace.page_state(id).map(|state| &state.bars)
    }

    /// Drives the Settings region's fade clock and returns the painted opacity, mirroring the tab
    /// strip helper so a bidirectional fade can be stepped without sleeping.
    #[cfg(test)]
    pub fn page_bars_hover(&mut self, id: TabId, hovered: bool, now: u64) -> bool {
        self.workspace
            .state_mut(id)
            .is_some_and(|state| state.bars.hover(hovered, now, false))
    }

    #[cfg(test)]
    pub fn page_bars_opacity(&mut self, id: TabId, now: u64) -> f32 {
        self.workspace
            .state_mut(id)
            .map_or(0.0, |state| state.bars.opacity(now))
    }

    #[cfg(test)]
    pub fn page_bars_animating(&self, id: TabId) -> bool {
        self.workspace
            .page_state(id)
            .is_some_and(|state| state.bars.is_animating())
    }

    #[cfg(test)]
    pub fn page_bars_dragging(&self, id: TabId) -> Option<gpui::Axis> {
        self.workspace
            .page_state(id)
            .and_then(|state| state.bars.dragging())
    }

    /// Marks this session's page-region thumb drag as started, the way `Bars::drag(true)` would.
    #[cfg(test)]
    pub fn start_page_thumb_drag(&mut self, id: TabId, now: u64) {
        if let Some(state) = self.workspace.state_mut(id) {
            state
                .bars
                .drag(&state.smooth, gpui::Axis::Vertical, true, now, false);
        }
    }

    /// The scroll offset of one session's page.
    #[cfg(test)]
    pub fn tab_page_offset(&self, id: TabId) -> Option<Point<gpui::Pixels>> {
        self.workspace
            .session(id)
            .map(|session| session.state().scroll.offset())
    }

    /// The page focus handle of one session.
    #[cfg(test)]
    pub fn tab_page_focus(&self, id: TabId) -> Option<FocusHandle> {
        self.workspace
            .session(id)
            .map(|session| session.state().focus.clone())
    }

    /// The strip focus handle of one session.
    #[cfg(test)]
    pub fn tab_strip_focus(&self, id: TabId) -> Option<FocusHandle> {
        self.workspace
            .session(id)
            .map(|session| session.state().strip_focus.clone())
    }

    /// The open session ids, in strip order.
    #[cfg(test)]
    pub fn tab_ids(&self) -> Vec<TabId> {
        self.workspace.sessions().iter().map(|s| s.id()).collect()
    }

    #[cfg(test)]
    pub fn tab_count(&self) -> usize {
        self.workspace.sessions().len()
    }

    #[cfg(test)]
    pub fn active_tab_id(&self) -> TabId {
        self.workspace.active_id()
    }

    #[cfg(test)]
    pub fn scroll_repositories_by(&mut self, dy: gpui::Pixels) {
        let offset = self.repo_scroll.offset();
        self.repo_smooth.cancel();
        self.repo_scroll
            .set_offset(gpui::point(offset.x, offset.y + dy));
    }

    /// For the sidebar test helper only: directly scroll the real handle, never through a
    /// partially clipped hit target.
    #[cfg(test)]
    pub fn scroll_repositories_for_test(&mut self, dy: gpui::Pixels, cx: &mut Context<Self>) {
        self.scroll_repositories_by(dy);
        cx.notify();
    }

    /// One session's repository browsing state, when the session is a repository page.
    #[cfg(test)]
    pub fn repository_state(&self, id: TabId) -> Option<&crate::workspace::RepositorySession> {
        self.workspace
            .session(id)
            .and_then(|session| session.state().repository.as_ref())
    }

    /// Drives one repository session's browsing selection, the way the R3 reader will.
    #[cfg(test)]
    pub fn select_repository(&mut self, id: TabId, branch: &str, file: Option<&str>) {
        if let Some(session) = self.workspace.session_mut(id)
            && let Some(repository) = session.state_mut().repository.as_mut()
        {
            repository.branch = Some(branch.to_string());
            repository.file = file.map(str::to_string);
        }
    }

    /// The strip's horizontal scroll offset, for the strip-scrolling tests.
    #[cfg(test)]
    pub fn tab_scroll_offset(&self) -> Point<gpui::Pixels> {
        self.tab_scroll.offset()
    }

    /// The strip's scrollable reach, for the clamp test.
    #[cfg(test)]
    pub fn tab_scroll_max(&self) -> gpui::Point<gpui::Pixels> {
        self.tab_scroll.max_offset()
    }

    /// Scrolls the strip by hand, the way a wheel or a scrollbar drag would.
    #[cfg(test)]
    pub fn scroll_tabs_by(&self, dx: gpui::Pixels) {
        let offset = self.tab_scroll.offset();
        self.tab_motion
            .set_offset(gpui::point(offset.x + dx, offset.y));
    }

    /// Opens or activates the session for `route`, closing an open palette; true when the active
    /// page changed. Every entry point (sidebar, quick entries, command palette) goes through
    /// here, so reuse rules live in one place, and the resulting tab is brought into view.
    pub fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.close_palette(cx);
        self.tab_motion.cancel();
        let previous = self.workspace.active_id();
        let id = match self.workspace.open(route, cx) {
            OpenResult::Opened(id) | OpenResult::Activated(id) => id,
        };
        self.follow_page_focus(previous, id, window, cx);
        self.reveal_tab(id);
        cx.notify();
        previous != id
    }

    /// Closes `id`. Closing the active session moves to its neighbour; the current page is kept
    /// when a non-active session closes. Focus only moves when the closed session held it, and it
    /// lands on the matching live target: the active session's tab-strip entry when the focus was
    /// on the closed tab's entry, otherwise the active session's page. The active session stays
    /// whatever it was.
    pub fn close_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let focus_in_closed = self.workspace.session(id).map(|session| {
            let focused = window.focused(cx);
            let state = session.state();
            let strip = focused.as_ref() == Some(&state.strip_focus)
                || state.strip_focus.contains_focused(window, cx);
            let page =
                focused.as_ref() == Some(&state.focus) || state.focus.contains_focused(window, cx);
            (strip, page)
        });
        if let Some(session) = self.workspace.session(id) {
            session.state().smooth.cancel();
        }
        let Some(result) = self.workspace.close(id, cx) else {
            return;
        };
        self.tab_motion.cancel();
        // A drag of the tab that just closed is over: keeping it would let a stale slot move
        // whichever tab now sits at that index.
        if self.tab_drag.as_ref().is_some_and(|drag| drag.id == id) {
            self.tab_drag = None;
            self.tab_bars
                .hold(false, self.tab_now_ms(), cx.reduce_motion());
            #[cfg(test)]
            {
                self.tab_drag_pinned = false;
            }
        }
        // The strip got shorter: drop the remembered tab so the next frame reveals the survivor
        // and the offset clamps to the new content instead of leaving empty space at the end.
        self.revealed_tab = None;
        self.clamp_tab_scroll();
        match focus_in_closed {
            Some((true, _)) => self.focus_strip(result.active, window, cx),
            Some((false, true)) => self.focus_page(result.active, window, cx),
            _ => {}
        }
        cx.notify();
    }

    /// Keeps the strip's offset inside its content. Called after a close so the tail never leaves
    /// a blank stretch, and harmless when nothing was clamped.
    fn clamp_tab_scroll(&self) {
        self.tab_motion.cancel();
        let max = self.tab_scroll.max_offset();
        let offset = self.tab_scroll.offset();
        let x = offset
            .x
            .clamp(-max.x.max(gpui::Pixels::ZERO), gpui::Pixels::ZERO);
        if x != offset.x {
            self.tab_motion.set_offset(gpui::point(x, offset.y));
        }
    }

    /// Records that `id` should be brought into view. The actual scroll happens in render, using
    /// that frame's measured bounds, and only once per newly revealed tab so a manual scroll is
    /// never overridden on later frames.
    fn reveal_tab(&mut self, id: TabId) {
        self.tab_motion.cancel();
        self.revealed_tab = Some(id);
    }

    /// Resolves pending reveals: a strip entry that just took keyboard focus, then any tab an
    /// open/activate/cycle asked for. Runs before this frame's layout, so the scroll and the frame
    /// that shows the tab agree.
    fn reveal_pending_tabs(&mut self, window: &mut Window) {
        let focused = self
            .workspace
            .sessions()
            .iter()
            .find(|session| session.state().strip_focus.is_focused(window))
            .map(|session| session.id());
        if focused != self.focused_tab {
            self.focused_tab = focused;
            if focused.is_some() {
                self.revealed_tab = focused;
            }
        }
        let Some(id) = self.revealed_tab else {
            return;
        };
        if self.reveal_tab_in_strip(id) {
            self.revealed_tab = None;
        }
    }

    /// Brings a tab fully into the strip's viewport with the least movement; true when the scroll
    /// position could be decided.
    ///
    /// The tab's place in the row is arithmetic (index × (width + gap)), so this needs no measured
    /// child bounds and never waits a frame for a tab that was just created. Only the viewport
    /// width comes from layout, and every tab strip has one after its first frame.
    fn reveal_tab_in_strip(&mut self, id: TabId) -> bool {
        let Some(index) = self.workspace.position(id) else {
            return true;
        };
        let viewport: f32 = self.tab_scroll.bounds().size.width.into();
        if viewport <= 0.0 {
            return false;
        }
        let step = geometry::TAB_WIDTH + geometry::TAB_GAP;
        let left = index as f32 * step;
        let right = left + geometry::TAB_WIDTH;
        let offset: f32 = self.tab_scroll.offset().x.into();
        let visible_left = -offset;
        let visible_right = visible_left + viewport;
        let target = if left < visible_left {
            -left
        } else if right > visible_right {
            -(right - viewport)
        } else {
            return true;
        };
        // Never past the row's left edge; the lower end is left to GPUI's own clamp, which runs
        // against this frame's freshly laid out content and is therefore not a frame behind.
        let x = target.min(0.0);
        self.tab_motion
            .set_offset(gpui::point(gpui::px(x), gpui::Pixels::ZERO));
        true
    }

    fn focus_strip(&self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.workspace.session(id) {
            let focus = session.state().strip_focus.clone();
            window.focus(&focus, cx);
        }
    }

    /// `Ctrl+W`: closes the active session unless the palette owns the keyboard.
    fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return;
        }
        let active = self.workspace.active_id();
        self.close_tab(active, window, cx);
    }

    /// `Ctrl+Tab` / `Shift+Ctrl+Tab`: cycles the active session, wrapping around.
    fn cycle_tab(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return;
        }
        self.tab_motion.cancel();
        let previous = self.workspace.active_id();
        if let Some(next) = self.workspace.cycle(by) {
            self.follow_page_focus(previous, next, window, cx);
            self.reveal_tab(next);
            cx.notify();
        }
    }

    fn focus_page(&self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.workspace.session(id) {
            let focus = session.state().focus.clone();
            window.focus(&focus, cx);
        }
    }

    /// Moves focus into the incoming page only when it lived in the page that is being left, so
    /// clicking a sidebar row or a tab keeps its own focus. Each session's handle is distinct, so
    /// focus never leaks between pages.
    fn follow_page_focus(
        &self,
        previous: TabId,
        next: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if previous == next {
            return;
        }
        // Switching freezes both session positions at their current presentation, rather than
        // letting a hidden page's old wheel target resume when it is reopened.
        if let Some(session) = self.workspace.session(next) {
            session.state().smooth.cancel();
        }
        let Some(previous) = self.workspace.session(previous) else {
            return;
        };
        previous.state().smooth.cancel();
        let focused = window.focused(cx);
        let in_previous_page = focused.as_ref() == Some(&previous.state().focus)
            || previous.state().focus.contains_focused(window, cx);
        if in_previous_page {
            self.focus_page(next, window, cx);
        }
    }
    /// Whether the palette is open, inspected by the palette tests.
    #[cfg(test)]
    pub fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    #[cfg(test)]
    pub fn palette_cursor(&self) -> usize {
        self.palette.as_ref().map_or(0, |session| session.cursor)
    }

    #[cfg(test)]
    pub fn palette_result_count(&self, cx: &App) -> usize {
        self.palette
            .as_ref()
            .map_or(0, |session| session.shown(cx).len())
    }

    #[cfg(test)]
    pub fn palette_input(&self) -> gpui::Entity<ely_core::forms::TextInput> {
        self.palette
            .as_ref()
            .expect("the palette is open")
            .input
            .clone()
    }

    #[cfg(test)]
    pub fn palette_composing(&self, window: &mut Window, cx: &mut App) -> bool {
        self.palette
            .as_ref()
            .is_some_and(|session| session.composing(window, cx))
    }

    #[cfg(test)]
    pub fn palette_scroll_offset(&self) -> Point<gpui::Pixels> {
        self.palette
            .as_ref()
            .map_or(Point::default(), |session| session.scroll.offset())
    }

    /// Opens the palette over the shell. Closing gives focus back to the search entry.
    /// Whether the collapsed More menu is open, for tests and keyboard dismissal.
    #[cfg(test)]
    pub fn sidebar_menu_open(&self) -> bool {
        self.sidebar_menu
    }

    /// Opens the collapsed More menu at its first entry and focuses it.
    pub fn open_sidebar_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_menu = true;
        self.sidebar_menu_cursor = 0;
        self.sidebar_state.menu_open.set(true);
        window.focus(&self.sidebar_menu_focus, cx);
        cx.notify();
    }

    /// Moves the menu highlight, wrapping over the three enabled entries (New stays disabled).
    pub fn move_sidebar_menu_cursor(&mut self, by: isize, cx: &mut Context<Self>) {
        const ENABLED: [usize; 3] = [0, 1, 2];
        let at = ENABLED
            .iter()
            .position(|&i| i == self.sidebar_menu_cursor)
            .unwrap_or(0) as isize;
        let next = (at + by).rem_euclid(ENABLED.len() as isize) as usize;
        self.sidebar_menu_cursor = ENABLED[next];
        cx.notify();
    }

    /// Runs the highlighted menu entry, if it is one of the enabled destinations.
    pub fn pick_sidebar_menu_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.sidebar_menu_cursor {
            0 => Some(Route::Notifications),
            1 => Some(Route::Issues),
            2 => Some(Route::PullRequests),
            _ => None,
        };
        self.pick_sidebar_menu(target, window, cx);
    }

    /// Runs one menu destination, closing the menu first so focus and the panel both settle
    /// before navigation.
    pub fn pick_sidebar_menu(
        &mut self,
        target: Option<Route>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_sidebar_menu(window, cx);
        if let Some(target) = target {
            self.navigate(target, window, cx);
        }
    }

    /// Closes the collapsed More menu and returns focus to its trigger.
    pub fn close_sidebar_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_menu {
            return;
        }
        self.sidebar_menu = false;
        self.sidebar_state.menu_open.set(false);
        window.focus(&self.sidebar_more_focus, cx);
        cx.notify();
    }

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_motion.cancel();
        self.repo_smooth.cancel();
        self.workspace.active().state().smooth.cancel();
        if self.palette.is_some() {
            return;
        }
        // Only one overlay at a time: an open More menu closes before the palette takes focus.
        self.close_sidebar_menu(window, cx);
        // The palette returns focus to the entry, not to whatever transient node holds it.
        window.focus(&self.search_focus, cx);
        let mut session = palette::Session::new(window, cx, palette::commands(&self.repositories));
        // The query field notifies only itself; the shell owns the filtered rows, so it must redraw
        // whenever the query changes.
        let input = session.input.clone();
        session.changed = Some(cx.subscribe(&input, |shell, _, _: &InputEvent, cx| {
            if let Some(session) = shell.palette.as_mut() {
                session.sync(cx);
            }
            cx.notify();
        }));
        // Keys are intercepted before bindings so the query field's own Enter binding cannot run a
        // row, and so an input-method composition can keep Enter for its candidate list.
        let this = cx.entity();
        session.keys = Some(cx.intercept_keystrokes(move |event, window, cx| {
            this.update(cx, |shell, cx| shell.on_palette_key(event, window, cx));
        }));
        self.palette = Some(session);
        cx.notify();
    }

    /// Handles the palette's keys before bindings; returns without consuming anything it does not own.
    fn on_palette_key(
        &mut self,
        event: &gpui::KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.palette.as_ref() else {
            return;
        };
        let cursor = session.cursor;
        let composing = session.composing(window, cx);
        match event.keystroke.key.as_str() {
            // While an input method composes, Enter confirms its candidate, Up/Down walk the
            // candidate list and Escape cancels the composition. Those keys belong to the input
            // method then, so the palette lets them through instead of acting on its own list.
            _ if composing => {}
            "escape" => {
                cx.stop_propagation();
                self.close_palette(cx);
            }
            "up" | "down" => {
                cx.stop_propagation();
                let by = if event.keystroke.key == "up" { -1 } else { 1 };
                self.move_palette_cursor(by, cx);
            }
            "enter" => {
                cx.stop_propagation();
                self.pick_palette(cursor, window, cx);
            }
            _ => {}
        }
    }

    /// Wraps the highlight by `by` and keeps it in view.
    fn move_palette_cursor(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(session) = self.palette.as_mut() {
            let query = session.query(cx);
            let entries = palette::layout(&session.commands, &query);
            let count = entries
                .iter()
                .filter(|entry| matches!(entry, palette::Entry::Command(_)))
                .count();
            session.move_cursor(by, count, &entries);
        }
        cx.notify();
    }

    /// Runs the result at `position` in the current query's order, then closes the palette.
    fn pick_palette(&mut self, position: usize, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.palette.as_ref().and_then(|session| {
            session
                .shown(cx)
                .get(position)
                .and_then(|index| session.commands.get(*index))
                .map(|command| command.value.clone())
        });
        match value {
            Some(value) => self.run_command(&value, window, cx),
            None => self.close_palette(cx),
        }
    }

    /// Closes the palette. Focus returns to the search entry on the next frame, so it is never
    /// left on a card that just left the tree.
    pub fn close_palette(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.palette.take() else {
            return;
        };
        session.smooth.cancel();
        self.restore_focus = true;
        cx.notify();
    }

    /// Runs one palette destination through the same open-or-activate path as every other entry.
    /// Only local navigation is reachable: no network call is made and the future search scope is
    /// not decided here.
    pub fn run_command(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let route = match value {
            "nav:home" => Some(Route::Home),
            "nav:settings" => Some(Route::Settings),
            "nav:profile" if self.active_account().is_some() => Some(Route::Profile),
            "nav:notifications" => Some(Route::Notifications),
            "nav:issues" => Some(Route::Issues),
            "nav:pull-requests" => Some(Route::PullRequests),
            // Repository commands come from the same current-account list the sidebar draws, so
            // the palette cannot reach a resource the sidebar does not have.
            other => match other.strip_prefix("repo:") {
                Some(full_name) => self
                    .repositories
                    .iter()
                    .find(|repo| repo.full_name.eq_ignore_ascii_case(full_name))
                    .map(Route::repository),
                None => None,
            },
        };
        match route {
            Some(route) => {
                self.navigate(route, window, cx);
            }
            None => { /* Not reachable: the card only offers known values. */ }
        }
    }

    /// Maps the controller's repository state to the sidebar's status and its one explicit refresh
    /// action. The Refresh here is scoped to this list; the fixed tab-toolbar Refresh stays disabled
    /// and unrelated.
    fn repository_list_status(
        &self,
        cx: &mut Context<Self>,
    ) -> (Option<sidebar::RepositoryStatus>, Option<gpui::AnyElement>) {
        let state = &self.account_state.repositories;
        let loading = matches!(state.preview, RepositoryPreview::Loading);
        let can_refresh = self
            .accounts
            .as_ref()
            .is_some_and(|weak| weak.upgrade().is_some())
            && state.account.is_some()
            && !loading;
        let (incomplete, message) = match &state.preview {
            RepositoryPreview::Ready(outcome) => {
                sidebar::list_status(&outcome.readiness, outcome.repositories.len())
            }
            RepositoryPreview::Failed(message) => (true, message.clone()),
            RepositoryPreview::Loading => {
                (false, "Reading the visible repository set…".to_string())
            }
            RepositoryPreview::Idle(message) => (false, message.clone()),
        };
        let refresh = self.accounts.clone();
        let status = sidebar::RepositoryStatus {
            message: Some(message),
            incomplete,
            loading,
            can_refresh,
            on_refresh: std::rc::Rc::new(move |_, cx| {
                if let Some(accounts) = &refresh {
                    let _ = accounts.update(cx, |accounts, cx| accounts.refresh(cx));
                }
            }),
        };
        let line = sidebar::repository_status(&status, cx);
        (Some(status), line)
    }

    fn sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let this = cx.entity();
        let route = self.workspace.route().clone();
        // One presentation decision for the whole footer: the compact More entries during the
        // critical band and during mode transitions, the four entries once clearly expanded.
        let footer_compact = self.sidebar_state.footer_compact();
        // Leaving the compact presentation unmounts both the More trigger and its panel. Drop the
        // menu state and put focus on a control that stays mounted, or the shell's Tab interceptor
        // would keep treating an invisible menu as open.
        if !footer_compact && self.sidebar_menu {
            self.close_sidebar_menu(window, cx);
            window.focus(&self.root_focus, cx);
        }
        let frame = self.sidebar_state.frame;
        let nav_metrics = sidebar::nav_metrics(frame.expansion, window.rem_size(), cx);

        let home = {
            let this = this.clone();
            sidebar::nav_row(
                "nav-home",
                IconName::House,
                "Home",
                route == Route::Home,
                nav_metrics,
                move |window, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(Route::Home, window, cx);
                    });
                },
                cx,
            )
        };
        let settings = {
            let this = this.clone();
            sidebar::nav_row(
                "nav-settings",
                IconName::Settings,
                "Settings",
                route == Route::Settings,
                nav_metrics,
                move |window, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(Route::Settings, window, cx);
                    });
                },
                cx,
            )
        };
        let nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_0p5()
            .children([home, settings])
            .into_any_element();

        let (status, status_line) = self.repository_list_status(cx);
        let heading = sidebar::group_heading(
            format!("Repositories ({})", self.repositories.len()),
            status.as_ref(),
            cx,
        );
        let heading = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(heading)
            .children(status_line)
            .into_any_element();
        // A row reached with Tab must be visible: otherwise its focus ring and Enter's target sit
        // outside the viewport. Done in render, before this frame's prepaint, so the scroll and the
        // focused row land in the same frame; the stored index keeps a still focus from re-scrolling.
        let focused_row = self
            .repo_focus
            .iter()
            .position(|handle| handle.is_focused(window));
        match focused_row {
            Some(index) if self.revealed_row != Some(index) => {
                // A keyboard reveal wins over any in-flight wheel easing.
                self.repo_smooth.cancel();
                if let Some(row) = self.repo_scroll.bounds_for_item(index) {
                    sidebar::reveal_row(&self.repo_scroll, row);
                }
                self.revealed_row = Some(index);
            }
            None => self.revealed_row = None,
            Some(_) => {}
        }

        let rows = self
            .repositories
            .iter()
            .enumerate()
            .map(|(index, repo)| {
                let this = this.clone();
                let target = Route::repository(repo);
                let active = route.is_repository(repo);
                let focus = self.repo_focus[index].clone();
                sidebar::repo_row(
                    index,
                    repo,
                    &focus,
                    active,
                    frame,
                    move |window, cx| {
                        let target = target.clone();
                        this.update(cx, |shell, cx| {
                            shell.navigate(target, window, cx);
                        });
                    },
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let repo_bar = scroll::Bar {
            target: &self.repo_smooth,
            id: scroll::BarId {
                owner: cx.entity_id(),
                name: "repo-scrollbar",
                axis: gpui::Axis::Vertical,
            },
            placement: scroll::Placement::overlay(4.0, 8.0),
            style: scroll::Style::DEFAULT,
            opacity: self.repo_bars.opacity(self.tab_now_ms()),
            on_drag: {
                let this = cx.entity();
                Rc::new(move |axis, dragging, _, cx| {
                    this.update(cx, |shell, cx| {
                        let reduced = cx.theme().reduced_motion || cx.reduce_motion();
                        if shell.repo_bars.drag(
                            &shell.repo_smooth,
                            axis,
                            dragging,
                            shell.tab_now_ms(),
                            reduced,
                        ) {
                            cx.notify();
                        }
                    });
                })
            },
        }
        .render(cx);
        let repositories = sidebar::repository_group(
            rows,
            &self.repo_scroll,
            self.repo_smooth.clone(),
            repo_bar,
            {
                let this = cx.entity();
                move |hovered, cx| {
                    this.update(cx, |shell, cx| {
                        let reduced = cx.theme().reduced_motion || cx.reduce_motion();
                        if shell.repo_bars.hover(hovered, shell.tab_now_ms(), reduced) {
                            cx.notify();
                        }
                    });
                }
            },
            cx,
        );

        // The account row is the real `/user` identity of the active account. Logged out, it is an
        // honest sign-in entry: no name, login, status or profile route is invented.
        let account = {
            let this = this.clone();
            let target = if self.active_account().is_some() {
                Some(Route::Profile)
            } else {
                None
            };
            let display = self.active_account().map(|summary| {
                let name = summary
                    .identity
                    .display_name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(&summary.identity.login)
                    .to_string();
                sidebar::AccountDisplay::new(name, summary.identity.login.clone())
            });
            sidebar::account_row(
                display,
                route == Route::Profile,
                frame,
                move |window, cx| {
                    let target = target.clone();
                    this.update(cx, |shell, cx| match target {
                        Some(target) => {
                            shell.navigate(target, window, cx);
                        }
                        None => {
                            // Signed out: the same reachable sign-in path as the palette, so the
                            // entry does something honest instead of opening a fake profile.
                            shell.navigate(Route::Settings, window, cx);
                            shell.focus_accounts_form(window, cx);
                        }
                    });
                },
                cx,
            )
        };

        let quarter = |name: &'static str,
                       icon: IconName,
                       label: &'static str,
                       target: Option<Route>,
                       active: bool,
                       this: gpui::Entity<Self>| {
            sidebar::quarter_button(
                name,
                icon,
                label,
                active,
                target.is_none(),
                move |window, cx| {
                    let Some(target) = target.clone() else {
                        return;
                    };
                    this.update(cx, |shell, cx| {
                        shell.navigate(target, window, cx);
                    });
                },
                cx,
            )
        };
        let quarters = if footer_compact {
            sidebar::overflow(
                sidebar::OverflowMenuView {
                    shell: this.clone(),
                    route: &route,
                    focus: &self.sidebar_more_focus,
                    menu_focus: &self.sidebar_menu_focus,
                    open: self.sidebar_state.menu_open.clone(),
                    menu_open: self.sidebar_menu,
                    cursor: self.sidebar_menu_cursor,
                    viewport: window.viewport_size(),
                },
                cx,
            )
        } else {
            sidebar::quarter_row([
                quarter(
                    "notifications",
                    IconName::Bell,
                    "Notifications",
                    Some(Route::Notifications),
                    route == Route::Notifications,
                    this.clone(),
                ),
                quarter(
                    "issues",
                    IconName::CircleDot,
                    "Issues",
                    Some(Route::Issues),
                    route == Route::Issues,
                    this.clone(),
                ),
                quarter(
                    "pull-requests",
                    IconName::GitPullRequest,
                    "Pull requests",
                    Some(Route::PullRequests),
                    route == Route::PullRequests,
                    this.clone(),
                ),
                // New has no approved resource list this round: named, disabled, does nothing.
                quarter("new", IconName::Plus, "New", None, false, this.clone()),
            ])
        };

        sidebar::sidebar(nav, heading, repositories, account, quarters, frame, cx)
    }

    /// Settings › Accounts. The minimal account surface: only the operations the accepted account
    /// preview already defined (add, select, replace PAT, revalidate), plus the current directory
    /// state. No general settings design, no account deletion, no logout and no cache policy.
    fn settings_accounts(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(controller) = self.accounts.as_ref().and_then(|weak| weak.upgrade()) else {
            return div()
                .debug_selector(|| "settings-accounts-unavailable".to_string())
                .p_6()
                .child("Account services are not available in this build.")
                .into_any_element();
        };
        let snapshot = controller.read(cx).snapshot().clone();
        let replacing =
            self.account_state.active.is_some() && controller.read(cx).replacing().is_some();
        let busy = controller.read(cx).busy();
        let restored = snapshot.accounts.iter().any(|summary| {
            summary.authorization == crate::domain::account::AuthorizationState::PendingVerification
        });
        let callbacks =
            crate::app::accounts::callbacks(controller.clone(), snapshot.status.clone());
        let section = crate::ui::accounts::accounts_section(
            crate::ui::accounts::AccountsSection {
                token: crate::ui::accounts::TokenForm {
                    token_input: controller.read(cx).token_input().clone(),
                    replacing,
                    busy,
                    callbacks: &callbacks,
                },
                accounts: snapshot.accounts,
                active: snapshot.active,
                restored,
                status: snapshot.status,
            },
            cx,
        );
        div()
            .debug_selector(|| "page-settings".to_string())
            .w_full()
            .px_6()
            .py_5()
            .child(section)
            .into_any_element()
    }

    /// Moves focus to the masked PAT field, used by the logged-out sidebar entry so it lands on a
    /// real control instead of an empty page.
    pub fn focus_accounts_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(controller) = self.accounts.as_ref().and_then(|weak| weak.upgrade()) else {
            return;
        };
        let input = controller.read(cx).token_input().clone();
        let focus = gpui::Focusable::focus_handle(input.read(cx), cx);
        window.focus(&focus, cx);
    }

    /// The island: the session tab strip, then the active page's own scroll area.
    fn island(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let (surface, border, fg) = (colors.surface, colors.border, colors.fg);
        let strip = self.tab_strip(window, cx);
        let settings_route = self.workspace.route() == &Route::Settings;
        let session_id = self.workspace.active_id();
        let settings = settings_route.then(|| self.settings_accounts(window, cx));
        let account = self.active_account().cloned();
        let (body, scroll, smooth, focus) = {
            let session = self.workspace.active();
            let body = page::page(
                session,
                page::PageData {
                    account: account.as_ref(),
                    repositories: &self.repositories,
                    settings,
                },
                cx,
            );
            (
                body,
                session.state().scroll.clone(),
                session.state().smooth.clone(),
                session.state().focus.clone(),
            )
        };
        let session_id_value = session_id.get();
        // Settings › Accounts is not a second scrolling stack: it is the session's page region, so
        // its bar targets the very `Motion` the page's wheel overlay already drives. One handle,
        // one easing, one fade and one drag owner; nothing here re-implements `ui::scroll`.
        let now = self.tab_now_ms();
        let settings_bar = if settings_route {
            let owner = cx.entity_id();
            let bars = self
                .workspace
                .state_mut(session_id)
                .expect("the active session is present");
            let opacity = bars.bars.opacity(now);
            let on_drag = account_scroll_drag(cx.entity(), session_id);
            Some(
                scroll::Bar {
                    target: &bars.smooth,
                    id: scroll::BarId {
                        owner,
                        name: "settings-scrollbar-y",
                        axis: gpui::Axis::Vertical,
                    },
                    placement: scroll::Placement::overlay(4.0, 8.0),
                    style: scroll::Style::DEFAULT,
                    opacity,
                    on_drag,
                }
                .render(cx),
            )
        } else {
            None
        };

        div()
            .debug_selector(|| "island".to_string())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            // The island's own corner; the inset keeps it clear of the window's edge.
            .rounded(px(geometry::ISLAND_RADIUS))
            .border_1()
            .border_color(border)
            .bg(surface)
            .text_color(fg)
            .overflow_hidden()
            .child(strip)
            .child(
                // The wheel overlay covers only the page under the strip: the strip has its own
                // horizontal scrolling and must keep receiving its own wheel deltas.
                div()
                    .debug_selector(|| "island-scroll-region".to_string())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        // One element per session id, so element state never carries over
                        // between pages.
                        div()
                            .id(("island-page", session_id_value))
                            .debug_selector(|| "island-page".to_string())
                            .size_full()
                            .overflow_y_scroll()
                            // Content never paints past the island's right edge; wide text is
                            // cut inside.
                            .overflow_x_hidden()
                            .track_scroll(&scroll)
                            .track_focus(&focus)
                            .when(settings_route, |page| {
                                // The account page region owns one `Bars`: its bar only shows while
                                // the pointer is inside this region, exactly like the sidebar and
                                // the tab strip.
                                page.on_hover(cx.listener(|shell, hovered: &bool, _, cx| {
                                    let reduced = cx.theme().reduced_motion || cx.reduce_motion();
                                    let now = shell.tab_now_ms();
                                    let id = shell.workspace.active_id();
                                    if let Some(state) = shell.workspace.state_mut(id)
                                        && state.bars.hover(*hovered, now, reduced)
                                    {
                                        cx.notify();
                                    }
                                }))
                            })
                            .child(body),
                    )
                    .child(scroll::wheel_overlay("page", gpui::Axis::Vertical, smooth))
                    .children(settings_bar),
            )
            .into_any_element()
    }

    /// The application session strip. Each tab activates its session on click or Enter/Space, its
    /// close affordance closes that session, and a horizontal drag reorders it.
    fn tab_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let _ = window;
        let this = cx.entity();
        let entries = self
            .workspace
            .sessions()
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let id = session.id();
                let select_this = this.clone();
                let select = Rc::new(move |window: &mut Window, cx: &mut App| {
                    select_this.update(cx, |shell, cx| shell.activate_tab(id, window, cx));
                });
                let close_this = this.clone();
                let close = Rc::new(move |window: &mut Window, cx: &mut App| {
                    close_this.update(cx, |shell, cx| shell.close_tab(id, window, cx));
                });
                tabs::Entry {
                    id,
                    index,
                    title: session.route().title(),
                    icon: tabs::route_icon(session.route()),
                    active: id == self.workspace.active_id(),
                    focus: session.state().strip_focus.clone(),
                    select,
                    close,
                }
            })
            .collect::<Vec<_>>();

        let on_hover = {
            let this = this.clone();
            Rc::new(move |hovered: bool, _window: &mut Window, cx: &mut App| {
                this.update(cx, |shell, cx| shell.set_tab_hovered(hovered, cx));
            })
        };
        let on_drag_start = {
            let this = this.clone();
            Rc::new(move |id: TabId, window: &mut Window, cx: &mut App| {
                this.update(cx, |shell, cx| shell.start_tab_drag(id, window, cx));
            })
        };
        let on_drag_move = {
            let this = this.clone();
            Rc::new(
                move |id: TabId,
                      position: gpui::Point<gpui::Pixels>,
                      window: &mut Window,
                      cx: &mut App| {
                    this.update(cx, |shell, cx| {
                        shell.move_tab_drag(id, position, window, cx)
                    });
                },
            )
        };

        let on_drop = {
            let this = this.clone();
            Rc::new(move |id: TabId, _window: &mut Window, cx: &mut App| {
                this.update(cx, |shell, cx| shell.drop_tab_drag(id, cx));
            })
        };
        let owner = self.tab_strip_owner;
        let opacity = self.tab_bars.opacity(self.tab_now_ms());
        let on_thumb_drag: scroll::OnDragging = {
            let this = cx.entity();
            std::rc::Rc::new(
                move |axis: gpui::Axis, dragging: bool, _window: &mut Window, cx: &mut App| {
                    this.update(cx, |shell, cx| shell.set_thumb_dragging(axis, dragging, cx));
                },
            )
        };
        let scrollbar = scroll::Bar {
            target: &self.tab_motion,
            id: scroll::BarId {
                owner,
                name: "tab-scrollbar",
                axis: gpui::Axis::Horizontal,
            },
            placement: scroll::Placement::BAND,
            style: scroll::Style::DEFAULT,
            opacity,
            on_drag: on_thumb_drag,
        }
        .render(cx);
        let view = tabs::View {
            entries,
            scroll: self.tab_scroll.clone(),
            dragging: self.tab_drag.as_ref().map(|drag| drag.id),
            drop_at: self.tab_drag.as_ref().map(|drag| drag.slot),
            toolbar: tabs::toolbar(),
            on_hover,
            on_drag_start,
            on_drag_move,
            on_drop,
        };
        tabs::strip(
            view,
            owner,
            scrollbar,
            scroll::wheel_overlay("tabs", gpui::Axis::Horizontal, self.tab_motion.clone()),
            cx,
        )
    }

    /// Milliseconds since the shell started, the clock the strip's scrollbar fade runs on.
    fn tab_now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(pinned) = self.tab_clock_pinned {
            return pinned;
        }
        self.tab_clock.elapsed().as_millis() as u64
    }

    /// Pins the strip's fade clock, so tests can step a fade without sleeping.
    #[cfg(test)]
    pub fn pin_tab_clock(&mut self, ms: u64) {
        self.tab_clock_pinned = Some(ms);
    }

    /// The strip scrollbar's painted opacity, for the fade tests.
    #[cfg(test)]
    pub fn tab_scrollbar_opacity(&mut self) -> f32 {
        self.tab_bars.opacity(self.tab_now_ms())
    }

    /// Drives the strip scrollbar's hover state at an explicit clock value, so the fade can be
    /// stepped without sleeping.
    #[cfg(test)]
    pub fn tab_scrollbar_hover(&mut self, hovered: bool, now: u64) -> bool {
        self.tab_bars.hover(hovered, now, false)
    }

    /// Makes the strip scrollbar fully visible at once, for tests that only need it to exist.
    /// The fade itself is covered by its own test and by the module's unit tests.
    #[cfg(test)]
    pub fn show_tab_scrollbar(&mut self) {
        self.tab_bars.snap(true);
    }

    #[cfg(test)]
    pub fn tab_scrollbar_animating(&self) -> bool {
        self.tab_bars.is_animating()
    }

    /// The tab being dragged, if any.
    #[cfg(test)]
    pub fn tab_drag_id(&self) -> Option<TabId> {
        self.tab_drag.as_ref().map(|drag| drag.id)
    }

    /// The landing slot of the drag in progress, in the coordinate the marker draws in.
    #[cfg(test)]
    pub fn tab_drag_slot(&self) -> Option<usize> {
        self.tab_drag.as_ref().map(|drag| drag.slot)
    }

    /// The index the drag would hand to `Workspace::reorder`.
    #[cfg(test)]
    pub fn tab_drag_target(&self) -> Option<usize> {
        self.tab_drag.as_ref().map(|drag| drag.target)
    }

    /// Starts a tab drag the way the strip's handlers do. The pin keeps the render loop from
    /// treating the absent platform drag as an ended one, so a test can move and drop it itself.
    #[cfg(test)]
    pub fn drag_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        self.start_tab_drag(id, window, cx);
        self.tab_drag_pinned = true;
    }

    #[cfg(test)]
    pub fn drag_tab_to(
        &mut self,
        id: TabId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_tab_drag(id, position, window, cx);
    }

    #[cfg(test)]
    pub fn drop_dragged_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        self.drop_tab_drag(id, cx);
        self.tab_drag_pinned = false;
        self.tab_drag = None;
    }

    #[cfg(test)]
    pub fn cancel_tab_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_drag_pinned = false;
        self.finish_tab_drag(window, cx);
    }

    /// Records whether the pointer is over the strip, which is the only thing that shows the
    /// strip's scrollbar.
    pub(crate) fn set_tab_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self
            .tab_bars
            .hover(hovered, self.tab_now_ms(), cx.reduce_motion())
        {
            cx.notify();
        }
    }

    /// The scrollbar thumb being dragged pins the bar on. The render loop clears it once the
    /// platform drag is over, so a drag that ends anywhere (including outside the strip) hands
    /// visibility back to the hover state.
    fn set_thumb_dragging(&mut self, axis: gpui::Axis, dragging: bool, cx: &mut Context<Self>) {
        let target = self.tab_motion.clone();
        if self.tab_bars.drag(
            &target,
            axis,
            dragging,
            self.tab_now_ms(),
            cx.reduce_motion(),
        ) {
            cx.notify();
        }
    }

    fn start_tab_drag(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_motion.cancel();
        let Some(index) = self.workspace.position(id) else {
            return;
        };
        self.tab_drag = Some(TabDragState {
            id,
            slot: index,
            target: index,
            position: gpui::Point::default(),
        });
        // A drag keeps the scrollbar visible and stops a pending reveal from pulling the row back
        // under the pointer.
        self.revealed_tab = None;
        self.focused_tab = None;
        // The fade keeps the bar visible for the whole tab drag through its hold flag, which is
        // independent of the pointer's real hover state and is not a thumb drag.
        if self
            .tab_bars
            .hold(true, self.tab_now_ms(), cx.reduce_motion())
        {
            cx.notify();
        }
        let _ = window;
        cx.notify();
    }

    /// Tracks a dragging tab: recomputes the landing slot from the real viewport and offset, and
    /// scrolls the row when the pointer is held near an edge so tabs beyond the viewport can be
    /// reached. The slot is orientation-only — the tab can never be dropped into the toolbar,
    /// because the viewport is what the arithmetic is measured against.
    fn move_tab_drag(
        &mut self,
        id: TabId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.tab_drag.as_ref() else {
            return;
        };
        // The payload's id is authoritative: an id that no longer exists (its tab was closed) ends
        // the drag instead of moving whatever now sits at its old index.
        if drag.id != id || self.workspace.position(id).is_none() {
            self.finish_tab_drag(window, cx);
            return;
        }
        if let Some(drag) = self.tab_drag.as_mut() {
            drag.position = position;
        }
        self.advance_tab_drag();
    }

    /// Advances the drag's edge auto-scroll and landing slot from the retained pointer position.
    /// Called both from `move_tab_drag` (a mouse move) and from the render loop, so holding the
    /// pointer still in an edge zone keeps scrolling toward tabs outside the viewport.
    fn advance_tab_drag(&mut self) {
        let Some(drag) = self.tab_drag.as_ref() else {
            return;
        };
        let Some(from) = self.workspace.position(drag.id) else {
            return;
        };
        let position = drag.position;
        let view = self.tab_scroll.bounds();
        let width: f32 = view.size.width.into();
        if width <= 0.0 {
            return;
        }
        let local: f32 = (position.x - view.left()).into();
        let zone = tabs::EDGE_ZONE;
        let step = if local < zone {
            tabs::EDGE_STEP
        } else if local > width - zone {
            -tabs::EDGE_STEP
        } else {
            0.0
        };
        if step != 0.0 {
            let offset: f32 = self.tab_scroll.offset().x.into();
            let max: f32 = self.tab_scroll.max_offset().x.into();
            let next = (offset + step).clamp(-max, 0.0);
            if next != offset {
                self.tab_motion
                    .set_offset(gpui::point(gpui::px(next), gpui::Pixels::ZERO));
            }
        }
        // `landing_slot` is an insertion slot in the collection as it is currently shown, which is
        // exactly the coordinate the marker draws in. `reorder` removes the dragged session before
        // inserting, so the index it needs shifts left by one for a rightward drag.
        let slot = tabs::landing_slot(
            position.x - view.left(),
            self.tab_scroll.offset().x,
            self.workspace.sessions().len(),
        );
        let target = if slot > from { slot - 1 } else { slot };
        if let Some(drag) = self.tab_drag.as_mut() {
            drag.slot = slot;
            drag.target = target;
        }
    }

    /// Commits a drop: the dragged session moves to its landing slot by id, once.
    fn drop_tab_drag(&mut self, id: TabId, cx: &mut Context<Self>) {
        let Some(drag) = self.tab_drag.as_ref() else {
            return;
        };
        if drag.id != id {
            return;
        }
        self.tab_motion.cancel();
        let target = drag.target;
        // Re-checked here rather than trusting the slot: a closed tab is ignored, and the session
        // keeps its own id, state and route either way.
        if self.workspace.reorder(id, target) {
            self.revealed_tab = None;
            self.reveal_tab(id);
            cx.notify();
        }
    }

    /// Ends a drag without moving anything: the original order and the active session stay as they
    /// were. Also the path for Escape, a lost window focus and a drop outside a valid target.
    fn finish_tab_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        let dragging = self.tab_drag.take().is_some();
        // Released even when no tab drag was recorded any more, so a stale hold can never keep the
        // bar on after a close, a lost activation or a drop outside.
        let held = self
            .tab_bars
            .hold(false, self.tab_now_ms(), cx.reduce_motion());
        if dragging || held {
            cx.notify();
        }
    }

    /// Makes an already open session active, following focus that lived in the previous page and
    /// bringing its tab into the strip's view.
    fn activate_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.workspace.active_id();
        if !self.workspace.activate(id) {
            return;
        }
        self.tab_motion.cancel();
        self.follow_page_focus(previous, id, window, cx);
        self.reveal_tab(id);
        cx.notify();
    }

    fn palette(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let mut session = self.palette.take()?;
        session.sync(cx);
        let this = cx.entity();
        let close = {
            let this = this.clone();
            Rc::new(move |_: &mut Window, cx: &mut App| {
                this.update(cx, |shell, cx| shell.close_palette(cx));
            })
        };
        let moved = {
            let this = this.clone();
            Rc::new(move |by: isize, _: &mut Window, cx: &mut App| {
                this.update(cx, |shell, cx| shell.move_palette_cursor(by, cx));
            })
        };
        let picked = {
            let this = this.clone();
            Rc::new(move |position: usize, window: &mut Window, cx: &mut App| {
                this.update(cx, |shell, cx| shell.pick_palette(position, window, cx));
            })
        };
        let view = palette::View {
            input: session.input.clone(),
            focus: session.focus.clone(),
            scroll: session.scroll.clone(),
            smooth: session.smooth.clone(),
            cursor: session.cursor,
            commands: session.commands.clone(),
            on_close: close,
            on_move: moved,
            on_pick: picked,
        };
        let card = palette::render(view, cx);
        self.palette = Some(session);
        Some(card)
    }
}

/// The thumb-drag hook for the account page region. It is a free function so it can be handed to
/// `scroll::Bar` without borrowing the shell for the whole frame.
fn account_scroll_drag(owner: gpui::Entity<Shell>, id: TabId) -> scroll::OnDragging {
    Rc::new(move |axis, dragging, _, cx| {
        owner.update(cx, |shell, cx| {
            let reduced = cx.theme().reduced_motion || cx.reduce_motion();
            let now = shell.tab_now_ms();
            let Some(state) = shell.workspace.state_mut(id) else {
                return;
            };
            if state.bars.drag(&state.smooth, axis, dragging, now, reduced) {
                cx.notify();
            }
        });
    })
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reduced = cx.theme().reduced_motion || cx.reduce_motion();
        if self.sidebar_state.sample(
            cx.background_executor().now(),
            window.viewport_size().width.into(),
            reduced,
        ) {
            window.request_animation_frame();
        }
        let now = std::time::Instant::now();
        if self.sidebar_state.sample_sash(now, reduced) {
            window.request_animation_frame();
        }
        if self.repo_bars.dragging().is_some() && !cx.has_active_drag() {
            self.repo_bars
                .settle(&self.repo_smooth, self.tab_now_ms(), reduced);
        }
        // Sample before testing is_animating, so the last fade frame retires immediately.
        self.repo_bars.opacity(self.tab_now_ms());
        if self.repo_bars.is_animating() {
            window.request_animation_frame();
        }
        if self.repo_smooth.tick(now, reduced) {
            window.request_animation_frame();
        }
        if self.tab_motion.tick(now, reduced) {
            window.request_animation_frame();
        }
        // The Settings › Accounts region keeps its own `Bars`/`Motion` (it is the session's page
        // region), so its fade and thumb drag need the same per-frame lifecycle as the repository
        // and tab bars: settle a released thumb, sample the opacity before testing `is_animating`,
        // and only request frames while the fade or the content easing is actually running.
        {
            let session_id = self.workspace.active_id();
            let settings_route = self.workspace.route() == &Route::Settings;
            let now_ms = self.tab_now_ms();
            if let Some(state) = self.workspace.state_mut(session_id) {
                if settings_route {
                    if state.bars.dragging().is_some() && !cx.has_active_drag() {
                        let target = state.smooth.clone();
                        if state.bars.settle(&target, now_ms, reduced) {
                            cx.notify();
                        }
                    }
                    // Sample before testing is_animating, so the last fade frame retires at once.
                    state.bars.opacity(now_ms);
                    if state.bars.is_animating() {
                        window.request_animation_frame();
                    }
                }
                if state.smooth.tick(now, reduced) {
                    window.request_animation_frame();
                }
            }
        }
        if self
            .palette
            .as_ref()
            .is_some_and(|session| session.smooth.tick(now, reduced))
        {
            window.request_animation_frame();
        }
        let theme = cx.theme();
        let colors = &theme.colors;
        let (bg, fg) = (colors.bg, colors.fg);
        let this = cx.entity();
        let search_focus = self.search_focus.clone();
        let open_search = move |window: &mut Window, cx: &mut App| {
            this.update(cx, |shell, cx| shell.open_palette(window, cx));
        };
        let titlebar = chrome::titlebar(self.search_focus.clone(), open_search, window, cx);
        // Keep the subscription alive for the lifetime of the shell.
        let _ = (
            &self.sidebar_activation,
            &self.sidebar_keys,
            &self.sidebar_bounds,
        );
        // One clock read per frame: the strip's fade and a scrollbar that just became necessary
        // (or unnecessary) both advance from it, so no timer of their own is needed.
        if self.tab_bars.is_animating() {
            window.request_animation_frame();
        }
        // A drag that ended without a drop (released outside, window focus lost, or the tab was
        // closed underneath it) leaves no half-finished state behind.
        #[cfg(test)]
        let drag_ended = self.tab_drag.is_some() && !self.tab_drag_pinned && !cx.has_active_drag();
        #[cfg(not(test))]
        let drag_ended = self.tab_drag.is_some() && !cx.has_active_drag();
        if drag_ended {
            self.finish_tab_drag(window, cx);
        }
        // A thumb drag ends wherever the pointer is released; visibility then follows hover again.
        if self.tab_bars.dragging().is_some() && !cx.has_active_drag() {
            let target = self.tab_motion.clone();
            if self
                .tab_bars
                .settle(&target, self.tab_now_ms(), cx.reduce_motion())
            {
                cx.notify();
            }
        }
        // Keep edge auto-scroll advancing while the pointer is held still in an edge zone, and
        // repaint so the moved row and its landing marker are drawn.
        if self.tab_drag.is_some() {
            self.advance_tab_drag();
            window.request_animation_frame();
        }
        // The strip's measurement lands at the end of a frame, so a reveal and a scrollbar that
        // just became necessary (or unnecessary) both need exactly one more frame to apply.
        let reach = self.tab_scroll.max_offset().x;
        if reach != self.tab_reach {
            self.tab_reach = reach;
            window.request_animation_frame();
        }
        self.reveal_pending_tabs(window);
        if self.revealed_tab.is_some() {
            window.request_animation_frame();
        }
        let sidebar = self.sidebar(window, cx);
        let divider = self.sidebar_divider(window, cx);
        let island = self.island(window, cx);
        let palette = self.palette(cx);

        // Closing the palette puts focus back on the search entry, never on a card that left the
        // tree; this also keeps Tab walking stable after an overlay.
        let restore = self.restore_focus;
        if restore {
            self.restore_focus = false;
        }

        // The action sits on an ancestor of the focus scope so it is on the dispatch path of the
        // focused node; the shell's shortcut stays a key binding, not a window-wide raw key hook.
        if restore {
            window.focus(&search_focus, cx);
        }
        #[cfg(target_os = "windows")]
        let content_top = crate::platform::caption::content_top_inset(window);
        #[cfg(not(target_os = "windows"))]
        let content_top = 0.0;
        div()
            .size_full()
            .pt(px(content_top))
            .on_action(cx.listener(|shell, _: &OpenCommandPalette, window, cx| {
                shell.open_palette(window, cx);
            }))
            .on_action(cx.listener(|shell, _: &CloseActiveTab, window, cx| {
                shell.close_active_tab(window, cx);
            }))
            .on_action(cx.listener(|shell, _: &NextTab, window, cx| {
                shell.cycle_tab(1, window, cx);
            }))
            .on_action(cx.listener(|shell, _: &PreviousTab, window, cx| {
                shell.cycle_tab(-1, window, cx);
            }))
            .on_action(cx.listener(|shell, _: &CancelTabDrag, window, cx| {
                // Escape while a tab is being carried: keep the original order and drop the drag.
                if shell.tab_drag.is_some() {
                    shell.finish_tab_drag(window, cx);
                }
            }))
            .child(
                FocusScope::new(&self.root_focus)
                    .root()
                    .size_full()
                    .flex()
                    .flex_col()
                    .text_color(fg)
                    .font_family(fonts::UI_FAMILY)
                    .child(titlebar)
                    .child(
                        div()
                            .id("shell-body")
                            .debug_selector(|| "shell-body".to_string())
                            .relative()
                            .on_drag_move(cx.listener(
                                |shell,
                                 event: &gpui::DragMoveEvent<sidebar_state::SidebarDrag>,
                                 window,
                                 cx| {
                                    let drag = event.drag(cx);
                                    if drag.owner != cx.entity_id() || !shell.sidebar_state.dragging
                                    {
                                        return;
                                    }
                                    // One coordinate space for the whole gesture: the pointer's
                                    // window x minus the body's own left edge. The sash anchor is
                                    // the presented sidebar width in that same space.
                                    let body_left =
                                        shell.sidebar_body_left.map(f32::from).unwrap_or(0.0);
                                    shell.sidebar_state.resize_pointer_at(
                                        f32::from(event.event.position.x) - body_left,
                                        cx.background_executor().now(),
                                        window.viewport_size().width.into(),
                                        cx.theme().reduced_motion || cx.reduce_motion(),
                                    );
                                    // The gesture itself keeps the line visible. Pointer hover is
                                    // still recorded for the moment the drag ends, so a release
                                    // outside fades out and a release inside stays lit.
                                    let hovered = shell
                                        .sidebar_resize_band
                                        .is_some_and(|band| band.contains(&event.event.position));
                                    shell.sidebar_state.note_pointer(
                                        hovered,
                                        cx.background_executor().now(),
                                        cx.theme().reduced_motion || cx.reduce_motion(),
                                    );
                                    cx.notify();
                                },
                            ))
                            .on_mouse_up(
                                gpui::MouseButton::Left,
                                cx.listener(|shell, event: &gpui::MouseUpEvent, _window, cx| {
                                    let hovered = shell
                                        .sidebar_resize_band
                                        .is_some_and(|band| band.contains(&event.position));
                                    let reduced = cx.theme().reduced_motion || cx.reduce_motion();
                                    shell.sidebar_state.end_resize(
                                        cx.background_executor().now(),
                                        hovered,
                                        reduced,
                                    );
                                    cx.notify();
                                }),
                            )
                            .on_mouse_up_out(
                                gpui::MouseButton::Left,
                                cx.listener(|shell, _, _, cx| {
                                    let reduced = cx.theme().reduced_motion || cx.reduce_motion();
                                    shell.sidebar_state.end_resize(
                                        cx.background_executor().now(),
                                        false,
                                        reduced,
                                    );
                                    cx.notify();
                                }),
                            )
                            .bg(bg)
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(sidebar)
                            .child(
                                // The island keeps a restrained gap from the sidebar and the
                                // window edges.
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    // The title bar already supplies the island's top gap.
                                    .pr(px(geometry::ISLAND_INSET))
                                    .pb(px(geometry::ISLAND_INSET))
                                    .pl(px(geometry::ISLAND_GAP))
                                    .child(island),
                            )
                            .child(divider),
                    )
                    .children(palette),
            )
    }
}
