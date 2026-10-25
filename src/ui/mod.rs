//! Ely component composition and view-local state. No business I/O during rendering.
//!
//! The window is a custom title bar over a fixed sidebar and one island. Only the sidebar's
//! repository group scrolls: the fixed rows and the island keep their places when the window is
//! resized. Routes live in [`crate::workspace`]; this round has one active page, with no placeholder
//! session strip above its content. Multi-session tabs belong to the next R1 task.

use std::rc::Rc;

use ely_gpui_component::{
    forms::InputEvent,
    primitives::{FocusScope, IconName},
    theme::ActiveTheme,
};
use gpui::{
    App, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Point, Render,
    ScrollHandle, Styled, Window, actions, div, prelude::*, px,
};

use crate::{
    platform::fonts,
    workspace::{Route, Workspace},
};

mod chrome;
pub mod fixtures;
pub(crate) mod geometry;
mod page;
mod palette;
mod sidebar;
mod sidebar_state;

#[cfg(test)]
mod sidebar_tests;

#[cfg(test)]
mod tests;

actions!(githux, [OpenCommandPalette]);

pub struct Shell {
    workspace: Workspace,
    repositories: Vec<fixtures::RepoFixture>,
    root_focus: FocusHandle,
    repo_scroll: ScrollHandle,
    /// One tab stop per repository row.
    repo_focus: Vec<FocusHandle>,
    /// The repository row last scrolled into view, so a still focus does not scroll every frame.
    revealed_row: Option<usize>,
    page_scroll: ScrollHandle,
    palette: Option<palette::Session>,
    restore_focus: bool,
    search_focus: FocusHandle,
    sidebar_state: sidebar_state::SidebarState,
    sidebar_activation: gpui::Subscription,
    sidebar_keys: gpui::Subscription,
    sidebar_bounds: gpui::Subscription,
    sidebar_more_focus: FocusHandle,
}

impl Shell {
    /// Binds the shell's own shortcuts once per application. `secondary` is Ctrl on Windows and
    /// Linux and Cmd on macOS, the same chord the title bar's search entry prints.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([gpui::KeyBinding::new(
            "secondary-k",
            OpenCommandPalette,
            None,
        )]);
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root_focus = cx.focus_handle();
        window.focus(&root_focus, cx);
        let search_focus = cx.focus_handle().tab_stop(true);
        let repositories = fixtures::repositories();
        let repo_focus = (0..repositories.len())
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect::<Vec<_>>();
        let sidebar_activation = cx.observe_window_activation(window, |shell, window, cx| {
            if !window.is_window_active() && shell.sidebar_state.dragging {
                shell.sidebar_state.dragging = false;
                cx.stop_active_drag(window);
                cx.notify();
            }
        });
        // A changed window is not a new drag. The remembered expanded width is kept and
        // clamped only when used, so widening the window again restores the user's choice.
        let sidebar_bounds = cx.observe_window_bounds(window, |shell, window, cx| {
            if shell.sidebar_state.dragging {
                shell.sidebar_state.dragging = false;
                cx.stop_active_drag(window);
            }
            // Ely closes its menu when focus leaves. Keep it anchored safely when the
            // viewport changes, including maximize/restore, without retaining a stale panel.
            if shell.sidebar_state.menu_open.get() && shell.palette.is_none() {
                window.focus(&shell.sidebar_more_focus, cx);
            }
            shell
                .sidebar_state
                .settle(window.viewport_size().width.into());
            cx.notify();
        });
        let this = cx.entity();
        let sidebar_keys = cx.intercept_keystrokes(move |event, window, cx| {
            this.update(cx, |shell, cx| {
                if event.keystroke.key == "escape" && shell.sidebar_state.dragging {
                    shell.sidebar_state.dragging = false;
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    cx.notify();
                }
            });
        });
        Self {
            sidebar_keys,
            sidebar_bounds,
            sidebar_more_focus: cx.focus_handle(),
            sidebar_state: sidebar_state::SidebarState::default(),
            sidebar_activation,
            workspace: Workspace::new(),
            repositories,
            root_focus,
            repo_scroll: ScrollHandle::new(),
            repo_focus,
            revealed_row: None,
            page_scroll: ScrollHandle::new(),
            palette: None,
            restore_focus: false,
            search_focus,
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

    /// The island page's scroll offset, for the bounded-scroll test.
    #[cfg(test)]
    pub fn page_offset(&self) -> Point<gpui::Pixels> {
        self.page_scroll.offset()
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

    /// Moves to `route`, closing an open palette; true when the page changed.
    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) -> bool {
        self.close_palette(cx);
        let changed = self.workspace.navigate(route);
        if changed {
            self.page_scroll.set_offset(Point::default());
        }
        cx.notify();
        changed
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
                self.pick_palette(cursor, cx);
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
    fn pick_palette(&mut self, position: usize, cx: &mut Context<Self>) {
        let value = self.palette.as_ref().and_then(|session| {
            session
                .shown(cx)
                .get(position)
                .and_then(|index| session.commands.get(*index))
                .map(|command| command.value.clone())
        });
        match value {
            Some(value) => self.run_command(&value, cx),
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

    /// Runs one palette destination. Only local navigation is reachable: no network call is made
    /// and the future search scope is not decided here.
    pub fn run_command(&mut self, value: &str, cx: &mut Context<Self>) {
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
                self.navigate(route, cx);
            }
            None => { /* Not reachable: the card only offers known values. */ }
        }
    }

    fn sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let this = cx.entity();
        let route = self.workspace.route().clone();
        let collapsed = self.sidebar_state.collapsed;
        let frame = self.sidebar_state.frame;

        let home = {
            let this = this.clone();
            sidebar::nav_row(
                "nav-home",
                IconName::House,
                "Home",
                route == Route::Home,
                frame,
                move |_, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(Route::Home, cx);
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
                frame,
                move |_, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(Route::Settings, cx);
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
                    frame,
                    move |_, cx| {
                        let target = target.clone();
                        this.update(cx, |shell, cx| {
                            shell.navigate(target, cx);
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
                frame,
                move |_, cx| {
                    this.update(cx, |shell, cx| {
                        shell.navigate(Route::Profile, cx);
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
                move |_, cx| {
                    let Some(target) = target.clone() else {
                        return;
                    };
                    this.update(cx, |shell, cx| {
                        shell.navigate(target, cx);
                    });
                },
                cx,
            )
        };
        let quarters = if collapsed {
            sidebar::overflow(
                this.clone(),
                &route,
                &self.sidebar_more_focus,
                self.sidebar_state.menu_open.clone(),
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

        sidebar::sidebar(nav, heading, repositories, account, quarters, frame)
    }

    fn island(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let (surface, border, fg) = (colors.surface, colors.border, colors.fg);
        let body = page::page(self.workspace.route(), &self.repositories, cx);

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
            .child(
                div()
                    .id("island-page")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    // Content never paints past the island's right edge; wide text is cut inside.
                    .overflow_x_hidden()
                    .track_scroll(&self.page_scroll)
                    .child(body),
            )
            .into_any_element()
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
            Rc::new(move |position: usize, _: &mut Window, cx: &mut App| {
                this.update(cx, |shell, cx| shell.pick_palette(position, cx));
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
        let reduced = cx.theme().reduced_motion || cx.reduce_motion();
        if self.sidebar_state.sample(
            cx.background_executor().now(),
            window.viewport_size().width.into(),
            reduced,
        ) {
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
        let sidebar = self.sidebar(window, cx);
        let divider = self.sidebar_divider(window, cx);
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
                                    let width = drag.start_width
                                        + f32::from(event.event.position.x - drag.start_x.get());
                                    shell
                                        .sidebar_state
                                        .resize(width, window.viewport_size().width.into());
                                    cx.notify();
                                },
                            ))
                            .on_mouse_up(
                                gpui::MouseButton::Left,
                                cx.listener(|shell, _, _, cx| {
                                    shell.sidebar_state.dragging = false;
                                    cx.notify();
                                }),
                            )
                            .on_mouse_up_out(
                                gpui::MouseButton::Left,
                                cx.listener(|shell, _, _, cx| {
                                    shell.sidebar_state.dragging = false;
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
