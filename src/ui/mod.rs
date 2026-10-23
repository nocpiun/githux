//! Ely component composition and view-local state. No business I/O during rendering.
//!
//! The window is a custom title bar over a fixed sidebar and one island. Only the sidebar's
//! repository group scrolls: the fixed rows and the island keep their places when the window is
//! resized. The island's top holds the application session tab strip, then the active page.
//! Sessions, their routes and their page-local state live in [`crate::workspace`].

use std::rc::Rc;

use ely_gpui_component::{
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
    platform::fonts,
    workspace::{MAX_OPEN_TABS, OpenResult, Route, TabId, Workspace},
};

mod chrome;
pub mod fixtures;
pub(crate) mod geometry;
mod page;
mod palette;
mod sidebar;
mod tabs;

#[cfg(test)]
mod tests;

actions!(
    githux,
    [OpenCommandPalette, CloseActiveTab, NextTab, PreviousTab]
);

/// The short notice shown when a new page cannot open at [`MAX_OPEN_TABS`]. It is cleared by the
/// next successful tab action, so it never becomes a permanent banner.
fn tab_limit_hint() -> String {
    format!("Tab limit reached ({MAX_OPEN_TABS}); close a tab to open another")
}

pub struct Shell {
    workspace: Workspace,
    repositories: Vec<fixtures::RepoFixture>,
    root_focus: FocusHandle,
    repo_scroll: ScrollHandle,
    /// One tab stop per repository row.
    repo_focus: Vec<FocusHandle>,
    /// The repository row last scrolled into view, so a still focus does not scroll every frame.
    revealed_row: Option<usize>,
    palette: Option<palette::Session>,
    restore_focus: bool,
    search_focus: FocusHandle,
    /// The tab-limit notice, cleared by the next successful tab action.
    hint: Option<String>,
}

impl Shell {
    /// Binds the shell's own shortcuts once per application. `secondary` is Ctrl on Windows and
    /// Linux and Cmd on macOS, the same chord the title bar's search entry prints. The tab chords
    /// avoid Enter/Escape/arrows, so they never race input-method candidate confirmation or text
    /// entry; while the palette is open the shell leaves the keyboard to it.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            gpui::KeyBinding::new("secondary-k", OpenCommandPalette, None),
            gpui::KeyBinding::new("secondary-w", CloseActiveTab, None),
            gpui::KeyBinding::new("ctrl-tab", NextTab, None),
            gpui::KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        ]);
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root_focus = cx.focus_handle().tab_index(-2);
        window.focus(&root_focus, cx);
        // The search entry and the session tabs sit at the front of the tab order (index -1),
        // ahead of the long repository list, so keyboard navigation reaches them directly.
        let search_focus = cx.focus_handle().tab_stop(true).tab_index(-1);
        let repositories = fixtures::repositories();
        let repo_focus = (0..repositories.len())
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect::<Vec<_>>();
        Self {
            workspace: Workspace::new(cx),
            repositories,
            root_focus,
            repo_scroll: ScrollHandle::new(),
            repo_focus,
            revealed_row: None,
            palette: None,
            restore_focus: false,
            search_focus,
            hint: None,
        }
    }

    /// The current page, inspected by the routing tests.
    #[cfg(test)]
    pub fn route(&self) -> Route {
        self.workspace.route().clone()
    }

    /// The local fixture list behind the sidebar, inspected by the layout tests.
    #[cfg(test)]
    pub fn repositories(&self) -> &[fixtures::RepoFixture] {
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

    /// The open routes, in strip order.
    #[cfg(test)]
    pub fn tab_routes(&self) -> Vec<Route> {
        self.workspace
            .sessions()
            .iter()
            .map(|session| session.route().clone())
            .collect()
    }

    #[cfg(test)]
    pub fn tab_count(&self) -> usize {
        self.workspace.sessions().len()
    }

    #[cfg(test)]
    pub fn active_tab_id(&self) -> TabId {
        self.workspace.active_id()
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

    #[cfg(test)]
    pub fn tab_limit_hint(&self) -> Option<String> {
        self.hint.clone()
    }

    /// Opens or activates the session for `route`, closing an open palette; true when the active
    /// page changed. Every entry point (sidebar, quick entries, command palette) goes through
    /// here, so reuse rules live in one place. At the session limit the current page stays put and
    /// a short notice is shown instead.
    pub fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.close_palette(cx);
        let previous = self.workspace.active_id();
        match self.workspace.open(route, cx) {
            OpenResult::LimitReached => {
                self.hint = Some(tab_limit_hint());
                cx.notify();
                false
            }
            OpenResult::Opened(id) | OpenResult::Activated(id) => {
                self.hint = None;
                self.follow_page_focus(previous, id, window, cx);
                cx.notify();
                previous != id
            }
        }
    }

    /// Closes `id`. Closing the active session moves to its neighbour; the current page is kept
    /// when a non-active session closes. Focus follows only when it lived in the closed session,
    /// whether on its page or on its own tab-strip entry.
    pub fn close_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let focused_in_closed = self.workspace.session(id).is_some_and(|session| {
            let focused = window.focused(cx);
            let state = session.state();
            focused.as_ref() == Some(&state.focus)
                || focused.as_ref() == Some(&state.strip_focus)
                || state.focus.contains_focused(window, cx)
                || state.strip_focus.contains_focused(window, cx)
        });
        let was_active = self.workspace.active_id() == id;
        let Some(result) = self.workspace.close(id, cx) else {
            return;
        };
        self.hint = None;
        if was_active && focused_in_closed {
            self.focus_page(result.active, window, cx);
        }
        cx.notify();
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
        let previous = self.workspace.active_id();
        if let Some(next) = self.workspace.cycle(by) {
            self.hint = None;
            self.follow_page_focus(previous, next, window, cx);
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
        let Some(previous) = self.workspace.session(previous) else {
            return;
        };
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
    pub fn palette_input(&self) -> gpui::Entity<ely_gpui_component::forms::TextInput> {
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
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return;
        }
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
        if self.palette.take().is_none() {
            return;
        }
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
            "nav:profile" => Some(Route::Profile),
            "nav:notifications" => Some(Route::Notifications),
            "nav:issues" => Some(Route::Issues),
            "nav:pull-requests" => Some(Route::PullRequests),
            other => match other.strip_prefix("repo:") {
                Some(full_name) => self
                    .repositories
                    .iter()
                    .find(|repo| repo.full_name() == full_name)
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

    fn sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let this = cx.entity();
        let route = self.workspace.route().clone();

        let home = {
            let this = this.clone();
            sidebar::nav_row(
                "nav-home",
                IconName::House,
                "Home",
                route == Route::Home,
                move |window, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(Route::Home, window, cx);
                    });
                },
            )
        };
        let settings = {
            let this = this.clone();
            sidebar::nav_row(
                "nav-settings",
                IconName::Settings,
                "Settings",
                route == Route::Settings,
                move |window, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(Route::Settings, window, cx);
                    });
                },
            )
        };
        let nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_0p5()
            .children([home, settings])
            .into_any_element();

        let heading =
            sidebar::group_heading(format!("Repositories · {}", self.repositories.len()), cx);
        // A row reached with Tab must be visible: otherwise its focus ring and Enter's target sit
        // outside the viewport. Done in render, before this frame's prepaint, so the scroll and the
        // focused row land in the same frame; the stored index keeps a still focus from re-scrolling.
        let focused_row = self
            .repo_focus
            .iter()
            .position(|handle| handle.is_focused(window));
        match focused_row {
            Some(index) if self.revealed_row != Some(index) => {
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
        let repositories = sidebar::repository_group(rows, &self.repo_scroll, cx);

        let account = {
            let this = this.clone();
            sidebar::account_row(
                fixtures::ACCOUNT,
                route == Route::Profile,
                move |window, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(Route::Profile, window, cx);
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
        let quarters = sidebar::quarter_row([
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
        ]);

        sidebar::sidebar(nav, heading, repositories, account, quarters)
    }

    /// The island: the session tab strip, then the active page's own scroll area.
    fn island(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let (surface, border, fg) = (colors.surface, colors.border, colors.fg);
        let strip = self.tab_strip(cx);
        let session = self.workspace.active();
        let body = page::page(session, &self.repositories, cx);
        let scroll = session.state().scroll.clone();
        let focus = session.state().focus.clone();

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
                // One element per session id, so element state never carries over between pages.
                div()
                    .id(("island-page", session.id().get()))
                    .debug_selector(|| "island-page".to_string())
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    // Content never paints past the island's right edge; wide text is cut inside.
                    .overflow_x_hidden()
                    .track_scroll(&scroll)
                    .track_focus(&focus)
                    .child(body),
            )
            .into_any_element()
    }

    /// The application session strip. Each tab activates its session on click or Enter/Space; the
    /// close affordance closes that session and stops the click from also activating it.
    fn tab_strip(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
        tabs::strip(entries, self.hint.clone(), cx)
    }

    /// Makes an already open session active, following focus that lived in the previous page.
    fn activate_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.workspace.active_id();
        if !self.workspace.activate(id) {
            return;
        }
        self.hint = None;
        self.follow_page_focus(previous, id, window, cx);
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

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let (bg, fg) = (colors.bg, colors.fg);
        let this = cx.entity();
        let search_focus = self.search_focus.clone();
        let open_search = move |window: &mut Window, cx: &mut App| {
            this.update(cx, |shell, cx| shell.open_palette(window, cx));
        };
        let titlebar = chrome::titlebar(self.search_focus.clone(), open_search, window, cx);
        let sidebar = self.sidebar(window, cx);
        let island = self.island(cx);
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
                            .debug_selector(|| "shell-body".to_string())
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
                            ),
                    )
                    .children(palette),
            )
    }
}
