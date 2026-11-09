//! NSH rendering for the repository page: summary, in-page tabs, tree and reader.
//!
//! The structure follows the confirmed product layout: repository summary, then horizontal in-page
//! content tabs, then the Code area with a bounded file tree on the left and the reader on the
//! right, then the Languages strip at the page bottom. The page root clips inside the island like
//! every other page. Nothing here performs I/O: the view only reads the page's already-checked
//! presentation snapshot and asks the page for the next step.
//!
//! The reader is mounted once per content generation (a `dynamic` that only reads the reader
//! signal), so an unrelated state change never unmounts it or retires its image authorization.

use std::rc::Rc;

use nsh_core::prelude::*;

use super::{
    PageError, PageHandle, PageRuntime, ReaderStatus, RepositoryTab, RowState, Selection,
    TREE_ROW_HEIGHT, TREE_WIDTH, TreeRowKind, ViewModel, navigation,
};
use crate::domain::repo::RepoVisibility;
use crate::domain::repository::Completeness;
use crate::ui::foundation::theme as t;
use crate::ui::reader::reader;
use crate::ui::repository_overview::{
    OverviewAction,
    glyphs::{Glyph, colored_glyph, glyph},
    language_bar, overview,
    widgets::{badge, badge_outline, failed, loading, muted},
};

const INDENT: f32 = 14.0;
/// Room for the disclosure chevron, so file rows line up with directory names.
const CHEVRON_SLOT: f32 = 16.0;

/// The component's properties. A plain field (not a reactive prop) keeps the mounted page
/// instance stable across shell re-renders: the handle compares by identity.
#[derive(Clone, PartialEq, Props)]
pub struct RepositoryViewProps {
    pub page: PageHandle,
}

#[component]
pub fn RepositoryView(props: RepositoryViewProps) -> impl IntoElement {
    let Some(page) = props.page.0.clone() else {
        return view()
            .test_id("repository-unavailable")
            .p(px(t::PAGE_GUTTER))
            .child(t::label("This repository page is no longer available."));
    };
    let state = page.state;

    // Tree keyboard navigation is bound only while the tree itself has the focus, so the page
    // never steals keys from the platform, an input or the reader's own selection.
    let tree_focused = signal(false);
    let tree_ref = element_ref();
    {
        let page = page.clone();
        effect(move || {
            if !tree_focused.get() {
                return;
            }
            let keys = [
                ("down", TreeKey::Down),
                ("up", TreeKey::Up),
                ("left", TreeKey::Left),
                ("right", TreeKey::Right),
                ("home", TreeKey::Home),
                ("end", TreeKey::End),
                ("enter", TreeKey::Activate),
            ];
            for (key, action) in keys {
                let page = page.clone();
                on_key(key, move |_| page.tree_key(action));
            }
        });
    }

    let code_tab = { move || state.get().tab == RepositoryTab::Code };
    let failure = { move || state.get().page_error.is_some() && state.get().branch.is_none() };

    view()
        .test_id("repository-page")
        .flex_col()
        .size_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_hidden()
        .bg(t::theme().background)
        .text_color(t::theme().foreground)
        .child(dynamic({
            let page = page.clone();
            move || header(page.clone(), state.get())
        }))
        .child(dynamic({
            let page = page.clone();
            move || match state.get().overview {
                Some(overview_state) => view()
                    .flex_none()
                    .w_full()
                    .min_w(px(0.0))
                    .px(px(20.0))
                    .child(overview(&overview_state, &overview_sink(page.clone())))
                    .into_element(),
                None => view().into_element(),
            }
        }))
        .child(dynamic({
            let page = page.clone();
            move || tabs(page.clone(), state.get())
        }))
        .child(
            view()
                .flex_row()
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .w_full()
                .overflow_hidden()
                .child(
                    when(failure, {
                        let failure_page = page.clone();
                        move || page_failure(failure_page.clone(), state.get())
                    })
                    .otherwise({
                        let page = page.clone();
                        move || {
                            let tree_page = page.clone();
                            view()
                                .flex_row()
                                .flex_1()
                                .min_h(px(0.0))
                                .min_w(px(0.0))
                                .w_full()
                                .overflow_hidden()
                                .child(when(code_tab, {
                                    move || {
                                        tree_column(
                                            tree_page.clone(),
                                            state,
                                            tree_ref,
                                            tree_focused,
                                        )
                                    }
                                }))
                                .child(reader_column(page.clone(), state, code_tab))
                                .child(when(
                                    {
                                        let state = state;
                                        move || state.get().tab != RepositoryTab::Code
                                    },
                                    || {
                                        view()
                                            .flex_1()
                                            .min_h(px(0.0))
                                            .test_id("repository-tab-empty")
                                    },
                                ))
                                .into_element()
                        }
                    }),
                ),
        )
        .child(dynamic(move || {
            let model = state.get();
            match model.overview {
                Some(overview_state) => language_bar(&overview_state),
                None => view().into_element(),
            }
        }))
}

#[derive(Clone, Copy)]
enum TreeKey {
    Down,
    Up,
    Left,
    Right,
    Home,
    End,
    Activate,
}

impl PageRuntime {
    fn tree_key(self: &Rc<Self>, key: TreeKey) {
        self.user(|controller| match key {
            TreeKey::Down => controller.move_cursor(1),
            TreeKey::Up => controller.move_cursor(-1),
            TreeKey::Left => controller.cursor_left(),
            TreeKey::Right => controller.cursor_right(),
            TreeKey::Home => controller.focus_first_row(),
            TreeKey::End => controller.focus_last_row(),
            TreeKey::Activate => controller.cursor_activate(),
        });
    }
}

fn overview_sink(page: Rc<PageRuntime>) -> Rc<dyn Fn(OverviewAction)> {
    Rc::new(move |action| page.user(|controller| controller.overview_action(action)))
}

fn press(
    page: &Rc<PageRuntime>,
    action: impl Fn(&mut super::RepositoryController) + 'static,
) -> impl Fn() + 'static {
    let page = page.clone();
    move || page.user(|controller| action(controller))
}

/// Title, visibility, description, counters and the Star / Fork entries.
fn header(page: Rc<PageRuntime>, model: ViewModel) -> AnyElement {
    let name = model
        .details
        .as_ref()
        .map(|details| format!("{}/{}", details.owner.login, model.name))
        .unwrap_or_else(|| format!("{}/{}", model.owner, model.name));
    let mut facts: Vec<AnyElement> = Vec::new();
    if let Some(details) = &model.details {
        facts.push(
            badge(match details.visibility {
                RepoVisibility::Public => "Public",
                RepoVisibility::Internal => "Internal",
                RepoVisibility::Private => "Private",
            })
            .into_element(),
        );
        if details.archived {
            facts.push(badge_outline("Archived", t::WARNING).into_element());
        }
        if details.fork {
            facts.push(badge("Fork").into_element());
        }
    }
    let description = model
        .details
        .as_ref()
        .and_then(|details| details.description.clone())
        .map(|description| {
            view()
                .test_id("repository-description")
                .child(muted(description))
        });
    let summary = view()
        .flex_col()
        .gap_1()
        .min_w(px(0.0))
        .flex_1()
        .child(
            view()
                .test_id("repository-title")
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.0))
                .child(
                    t::label(name)
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .children(facts),
        )
        .children(description);
    let actions = view()
        .flex_row()
        .flex_wrap()
        .flex_none()
        .gap_2()
        .items_center()
        .child(star_entry(page.clone(), &model))
        .child(fork_entry(&model));
    let actions = match model.star.failure.clone() {
        Some(failure) => actions.child(failed(
            "Star action failed",
            failure,
            Some(Rc::new(press(&page, |controller| controller.retry_star()))),
        )),
        None => actions,
    };
    view()
        .test_id("repository-header")
        .flex_none()
        .w_full()
        .flex_row()
        .items_start()
        .gap_4()
        .px(px(20.0))
        .pt_4()
        .pb_2()
        .child(summary)
        .child(cached_marker(page, &model))
        .child(actions)
        .into_element()
}

/// Explicit "this page is showing a cached snapshot" marker with the last verified time and the one
/// explicit online refresh. Only historical data draws it.
fn cached_marker(page: Rc<PageRuntime>, model: &ViewModel) -> AnyElement {
    let Some(synced) = model.cached_label.clone() else {
        return view().into_element();
    };
    let tip = format!("Showing a cached snapshot from {synced}. Not a live GitHub read.");
    let refresh = press(&page, |controller| controller.refresh());
    view()
        .test_id("repository-cached-snapshot")
        .flex_row()
        .flex_none()
        .items_center()
        .gap_2()
        .child(
            view()
                .test_id("repository-cached-badge")
                .tooltip(t::label(tip).text_size(px(12.0)))
                .child(badge_outline("Cached", t::WARNING)),
        )
        .child(action_button("Refresh", refresh))
        .into_element()
}

fn star_entry(page: Rc<PageRuntime>, model: &ViewModel) -> AnyElement {
    let enabled = model.star.enabled;
    let label = model.star.label;
    let starred = model.star.observed.is_some_and(|state| state.is_starred());
    let color = if !enabled {
        t::theme().muted_foreground
    } else if starred {
        t::WARNING
    } else {
        t::theme().foreground
    };
    let press_star = press(&page, |controller| controller.star_entry());
    let mut entry = view()
        .test_id("repository-star")
        .flex_row()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(30.0))
        .px_2()
        .rounded(t::theme().radius)
        .border_1()
        .border_color(t::theme().border)
        .role(AccessibilityRole::Button)
        .label(label)
        .focusable(enabled)
        .on_press(press_star)
        .child(if model.star.busy {
            loading("Updating Star status…").into_element()
        } else {
            colored_glyph(Glyph::Star, color, 14.0).into_element()
        })
        .child(t::label(label).text_size(px(13.0)).text_color(color));
    if let Some(count) = model.star.count {
        entry = entry.child(
            view()
                .test_id("repository-star-count")
                .child(badge(count.to_string())),
        );
    }
    entry.into_element()
}

fn fork_entry(model: &ViewModel) -> AnyElement {
    let count = model.details.as_ref().map(|details| details.forks);
    // Fork has its own, always-false capability: there is no public API to fork into this account.
    view()
        .test_id("repository-fork")
        .flex_row()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(30.0))
        .px_2()
        .rounded(t::theme().radius)
        .border_1()
        .border_color(t::theme().border)
        .role(AccessibilityRole::Button)
        .label("Fork")
        .child(colored_glyph(
            Glyph::Fork,
            t::theme().muted_foreground,
            14.0,
        ))
        .child(
            t::label("Fork")
                .text_size(px(13.0))
                .text_color(t::theme().muted_foreground),
        )
        .children(count.map(|count| {
            view()
                .test_id("repository-fork-count")
                .child(badge(count.to_string()))
        }))
        .into_element()
}

fn tab_glyph(tab: RepositoryTab) -> Glyph {
    match tab {
        RepositoryTab::Code => Glyph::Code,
        RepositoryTab::Issues => Glyph::Issue,
        RepositoryTab::PullRequests => Glyph::PullRequest,
        RepositoryTab::Releases => Glyph::Tag,
        RepositoryTab::Settings => Glyph::Settings,
    }
}

/// The branch picker, plus an honest line about incomplete branch listings.
fn branch_picker(page: Rc<PageRuntime>, model: &ViewModel) -> AnyElement {
    let Some(branches) = &model.branches else {
        let loading_now = model.branches_loading;
        let mut row = view()
            .test_id("repository-branch-unavailable")
            .flex_row()
            .items_center()
            .gap_1()
            .flex_none()
            .w(px(220.0))
            .children(model.branch.clone().map(|branch| {
                view()
                    .test_id("repository-branch-fallback")
                    .child(muted(format!("branch {branch}")))
            }));
        if let Some(error) = &model.branches_error {
            row = row.child(failed(
                "Branch list unavailable",
                error.title.clone(),
                (!loading_now)
                    .then(|| Rc::new(press(&page, |c| c.retry_branches())) as Rc<dyn Fn()>),
            ));
        } else if model.branch.is_none() {
            row = row.child(loading("Loading branches…"));
        }
        return row.into_element();
    };
    let current = model.branch.clone().unwrap_or_default();
    let mut picker = view()
        .test_id("repository-branch")
        .tooltip(
            t::label(if current.is_empty() {
                "Branch".to_string()
            } else {
                current.clone()
            })
            .text_size(px(12.0)),
        )
        .flex_col()
        .items_end()
        .gap_1()
        .flex_none()
        .w(px(220.0));
    if branches.branches.is_empty() {
        picker = picker.child(view().test_id("repository-branch-empty").child(muted(
            if branches.completeness == Completeness::Complete {
                "No branches"
            } else {
                "No branches in the partial result"
            },
        )));
    } else {
        picker = picker.child(
            view()
                .test_id("repository-branch-select")
                .flex_col()
                .items_end()
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(glyph(Glyph::Fork))
                        .child(t::label(current.clone()).text_size(px(12.0))),
                )
                .children(branches.branches.iter().map(|branch| {
                    let name = branch.name.as_str().to_string();
                    let selected = name == current;
                    let picked = name.clone();
                    let pick = press(&page, move |controller| {
                        controller.select_branch(&picked.clone())
                    });
                    view()
                        .test_id(&format!("repository-branch-choice-{name}"))
                        .px_2()
                        .py_1()
                        .rounded(t::theme().radius)
                        .role(AccessibilityRole::Button)
                        .label(name.clone())
                        .focusable(true)
                        .cursor_pointer()
                        .bg(if selected {
                            t::theme().accent
                        } else {
                            AdaptiveColor::TRANSPARENT
                        })
                        .on_press(pick)
                        .child(t::label(name).text_size(px(12.0)))
                })),
        );
    }
    if let Some(note) = navigation::incompleteness_note(&branches.completeness) {
        picker = picker.child(
            view().test_id("repository-branches-incomplete").child(
                t::label(format!(
                    "Incomplete branch list: {note}. This is not the full set."
                ))
                .text_size(px(11.0))
                .text_color(t::WARNING),
            ),
        );
    }
    picker.into_element()
}

/// The in-page content tabs. Only Code has content this round.
fn tabs(page: Rc<PageRuntime>, model: ViewModel) -> AnyElement {
    let active = model.tab;
    let picker = branch_picker(page.clone(), &model);
    let tabs = RepositoryTab::ALL.into_iter().map(|tab| {
        let is_active = tab == active;
        let color = if is_active {
            t::theme().foreground
        } else {
            t::theme().muted_foreground
        };
        let count = match tab {
            RepositoryTab::Issues => model
                .counts
                .as_ref()
                .and_then(|counts| counts.issues.as_ref().ok()),
            RepositoryTab::PullRequests => model
                .counts
                .as_ref()
                .and_then(|counts| counts.pull_requests.as_ref().ok()),
            _ => None,
        };
        let select = press(&page, move |controller| controller.select_tab(tab));
        view()
            .test_id(&format!("repository-tab-{}", tab.id()))
            .flex_none()
            .flex_col()
            .items_center()
            .px_2()
            .py_2()
            .cursor_pointer()
            .role(AccessibilityRole::Tab)
            .label(tab.label())
            .on_press(select)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(colored_glyph(tab_glyph(tab), color, 14.0))
                    .child(
                        t::label(tab.label())
                            .text_size(px(13.0))
                            .font_weight(if is_active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(color),
                    )
                    .children(count.map(|count| {
                        view()
                            .test_id(&format!("repository-badge-{}", tab.id()))
                            .child(badge(count.to_string()))
                    })),
            )
            .child(view().h(px(2.0)).w_full().bg(if is_active {
                t::theme().primary
            } else {
                AdaptiveColor::TRANSPARENT
            }))
    });
    view()
        .test_id("repository-tabs")
        .flex_none()
        .w_full()
        .border_b_1()
        .border_color(t::theme().border)
        .px(px(20.0))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap_1()
                        .children(tabs),
                )
                .child(view().flex_1())
                .child(picker),
        )
        .into_element()
}

/// The browsing area's page-level failure. It replaces the columns instead of drawing an empty
/// tree, which a failed read never proved.
fn page_failure(page: Rc<PageRuntime>, model: ViewModel) -> AnyElement {
    match model.page_error {
        Some(error) => {
            let retry = Rc::new(press(&page, |controller| controller.retry()));
            failure_panel("repository-error", &error, retry).into_element()
        }
        None => view().into_element(),
    }
}

/// The bounded file tree: a real virtual list of the visible rows.
fn tree_column(
    page: Rc<PageRuntime>,
    state: Signal<ViewModel>,
    tree_ref: ElementRef,
    tree_focused: Signal<bool>,
) -> AnyElement {
    let focused_for_focus = tree_focused;
    let focused_for_blur = tree_focused;
    let page_for_list = page.clone();
    let list = virtual_list(
        move || state.get().rows.len(),
        move |at: Position| tree_row(page_for_list.clone(), state, at, tree_ref),
    )
    .scrollbar(true)
    .flex_1()
    .min_h(px(0.0))
    .test_id("repository-tree");
    view()
        .test_id("repository-tree-column")
        .flex_col()
        .flex_none()
        .w(px(TREE_WIDTH))
        .h_full()
        .min_h(px(0.0))
        .border_r_1()
        .border_color(t::theme().border)
        .child(code_header(
            view()
                .test_id("repository-tree-header")
                .flex_row()
                .items_center()
                .justify_between()
                .px_3()
                .child(
                    t::label("Files")
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t::theme().muted_foreground),
                )
                .child(
                    view().test_id("repository-commit").child(
                        t::code(move || {
                            state
                                .get()
                                .commit
                                .map(|commit| commit.chars().take(7).collect::<String>())
                                .unwrap_or_default()
                        })
                        .text_size(px(11.0))
                        .text_color(t::theme().muted_foreground),
                    ),
                ),
        ))
        .child(tree_warnings(page, state))
        .child(dynamic({
            move || {
                let model = state.get();
                if model.empty_root && model.rows.is_empty() {
                    view()
                        .px_3()
                        .py_2()
                        .child(muted("This commit has no files."))
                        .into_element()
                } else {
                    view().into_element()
                }
            }
        }))
        .child(
            view()
                .test_id("repository-tree-viewport")
                .element_ref(&tree_ref)
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_hidden()
                .focusable(true)
                .on_focus(move || focused_for_focus.set(true))
                .on_blur(move || focused_for_blur.set(false))
                .child(view().size_full().p(px(7.0)).child(list)),
        )
        .into_element()
}

fn code_header(inner: impl IntoElement) -> View {
    view()
        .h(px(30.0))
        .flex_none()
        .border_b_1()
        .border_color(t::theme().border)
        .child(inner)
}

fn tree_warnings(page: Rc<PageRuntime>, state: Signal<ViewModel>) -> AnyElement {
    dynamic(move || {
        let model = state.get();
        let mut warnings = view().flex_col().gap_1();
        let mut any = false;
        if model.tree_incomplete {
            any = true;
            warnings = warnings.child(
                view()
                    .test_id("repository-tree-incomplete")
                    .px_2()
                    .py_1()
                    .tooltip(
                        t::label(
                            "This directory listing is truncated or incomplete; it is not a complete empty directory.",
                        )
                        .text_size(px(12.0)),
                    )
                    .child(colored_glyph(Glyph::Warning, t::WARNING, 12.0)),
            );
        }
        if let Some(error) = model.root_failure.clone() {
            any = true;
            let retry = error
                .retryable
                .then(|| Rc::new(press(&page, |c| c.retry_directory(""))) as Rc<dyn Fn()>);
            warnings = warnings.child(
                view()
                    .test_id("repository-tree-error")
                    .px_2()
                    .py_1()
                    .child(failed("Files could not be loaded", error.title, retry)),
            );
        }
        if model.rows_capped {
            any = true;
            warnings = warnings.child(
                view()
                    .test_id("repository-tree-capped")
                    .px_2()
                    .py_1()
                    .tooltip(
                        t::label(format!(
                            "More rows are loaded than the {}-row tree cap. Collapse directories to review the rest; nothing was fetched recursively.",
                            navigation::MAX_TREE_ROWS
                        ))
                        .text_size(px(12.0)),
                    )
                    .child(colored_glyph(Glyph::Warning, t::WARNING, 12.0)),
            );
        }
        if any {
            warnings.into_element()
        } else {
            view().into_element()
        }
    })
    .into_element()
}

/// One tree row. The whole row is the hit area.
fn tree_row(
    page: Rc<PageRuntime>,
    state: Signal<ViewModel>,
    at: Position,
    tree_ref: ElementRef,
) -> AnyElement {
    let index = at.index();
    let row = move || state.get().rows.get(index).cloned();
    let path = move || row().map(|row| row.path).unwrap_or_default();
    let name = move || row().map(|row| row.name).unwrap_or_default();
    let kind = move || row().map(|row| row.kind);
    let depth = move || row().map(|row| row.depth).unwrap_or(0);
    let selected = move || {
        let model = state.get();
        let current = match &model.selection {
            Selection::Readme => model.readme_path.clone(),
            Selection::File(path) => Some(path.clone()),
        };
        current.as_deref() == Some(path().as_str())
    };
    let cursor = move || {
        let model = state.get();
        let path = path();
        model.cursor.as_deref() == Some(path.as_str())
    };
    let press_row = {
        let page = page.clone();
        move || {
            let path = path();
            let directory = kind().is_some_and(|kind| kind.is_directory());
            page.user(|controller| {
                controller.focus_row(&path);
                if directory {
                    controller.toggle_directory(&path);
                } else {
                    controller.select_file(&path);
                }
            })
        }
    };
    let toggle_page = page.clone();
    let toggle_path = path;
    let toggle = move || {
        let path = toggle_path();
        toggle_page.user(|controller| controller.toggle_directory(&path));
    };
    view()
        .test_id(&format!("repository-row-{index}"))
        .tooltip(t::label(path()).text_size(px(12.0)))
        .flex_row()
        .items_center()
        .gap_1()
        .flex_none()
        .w_full()
        .h(px(TREE_ROW_HEIGHT))
        .pl(px(6.0))
        .pr_2()
        .rounded(t::theme().radius)
        .bg(move || {
            if selected() {
                t::theme().border
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .border_1()
        .border_color(move || {
            if cursor() {
                t::theme().muted_foreground
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .role(AccessibilityRole::Button)
        .label(name())
        .focusable(true)
        .on_focus({
            move || tree_ref.focus()
        })
        .on_press(move || {
            tree_ref.focus();
            press_row();
        })
        .child(view().flex_none().w(px(CHEVRON_SLOT)).h_full().items_center().justify_center().child(dynamic(move || {
            match kind() {
                Some(TreeRowKind::Directory { expanded }) => view()
                    .on_press({
                        let toggle = toggle.clone();
                        move || toggle()
                    })
                    .child(colored_glyph(
                        if expanded {
                            Glyph::ChevronDown
                        } else {
                            Glyph::ChevronRight
                        },
                        t::theme().muted_foreground,
                        12.0,
                    ))
                    .into_element(),
                _ => view().into_element(),
            }
        })))
        .child(
            t::label(move || format!("{}{}", " ".repeat(depth()), name())).text_size(px(12.0)),
        )
        .child(dynamic({
            move || match row().map(|row| row.state) {
                Some(RowState::Loading) => loading("Loading directory…").into_element(),
                Some(RowState::Empty) => view().child(muted("empty")).into_element(),
                Some(RowState::Incomplete) => view()
                    .tooltip(
                        t::label(
                            "This directory listing is truncated or incomplete; it is not a complete empty directory.",
                        )
                        .text_size(px(12.0)),
                    )
                    .child(colored_glyph(Glyph::Warning, t::WARNING, 12.0))
                    .into_element(),
                Some(RowState::Failed(error)) => {
                    failed("Failed to load", error.title, None).into_element()
                }
                _ => view().into_element(),
            }
        }))
        .into_element()
}

/// The right column: the current path, a "back to README" entry when useful, and the reader.
fn reader_column(
    page: Rc<PageRuntime>,
    state: Signal<ViewModel>,
    code: impl Fn() -> bool + 'static,
) -> AnyElement {
    let page_for_header = page.clone();
    let page_for_body = page.clone();
    view()
        .test_id("repository-reader-column")
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .h_full()
        .child(dynamic(move || {
            reader_header(page_for_header.clone(), state.get())
        }))
        .child(
            view()
                .test_id("repository-reader-body")
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .w_full()
                .overflow_hidden()
                .child(when(code, {
                    let body_page = page_for_body.clone();
                    move || {
                        let page = body_page.clone();
                        dynamic(move || reader_body(page.clone(), state.get()))
                    }
                })),
        )
        .into_element()
}

fn reader_header(page: Rc<PageRuntime>, model: ViewModel) -> AnyElement {
    let path = match &model.selection {
        Selection::Readme => model.readme_path.clone(),
        Selection::File(path) => Some(path.clone()),
    };
    let mut header = code_header(
        view()
            .test_id("repository-reader-header")
            .px_4()
            .flex_row()
            .items_center()
            .gap_2()
            .child(colored_glyph(
                match model.selection {
                    Selection::Readme => Glyph::BookOpen,
                    Selection::File(_) => Glyph::File,
                },
                t::theme().muted_foreground,
                14.0,
            ))
            .child(
                view()
                    .test_id("repository-selected-path")
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .tooltip(
                        t::label(path.clone().unwrap_or_else(|| "No file selected".into()))
                            .text_size(px(12.0)),
                    )
                    .child(muted(path.unwrap_or_else(|| "No file selected".into()))),
            ),
    );
    if model.can_return_to_readme {
        let back = press(&page, |controller| controller.show_readme());
        header = header.child(
            view()
                .test_id("repository-back-to-readme")
                .child(action_button("Back to README", back)),
        );
    }
    header.into_element()
}

fn reader_body(page: Rc<PageRuntime>, model: ViewModel) -> AnyElement {
    match &model.reader_status {
        ReaderStatus::Showing => dynamic({
            let page = page.clone();
            move || match page.reader.get() {
                Some(handle) => reader(handle).into_element(),
                None => view().into_element(),
            }
        })
        .into_element(),
        ReaderStatus::Loading => loading_panel().into_element(),
        ReaderStatus::NoReadme => plain_panel(
            "repository-no-readme",
            "No README",
            "This commit has no README. Pick a file from the tree to read it; nothing is generated in its place.",
        )
        .into_element(),
        ReaderStatus::ReadmeUncertain => retry_panel(
            "repository-readme-uncertain",
            "README not confirmed",
            "The README may be absent or not visible to this account. You can browse files or retry discovery.",
            "Retry README",
            Rc::new(press(&page, |controller| controller.retry_readme())),
        )
        .into_element(),
        ReaderStatus::ReadmeUnavailable(error) => failure_panel(
            "repository-readme-error",
            error,
            Rc::new(press(&page, |controller| controller.retry_reader())),
        )
        .into_element(),
        ReaderStatus::ContentFailed(error) => failure_panel(
            "repository-content-error",
            error,
            Rc::new(press(&page, |controller| controller.retry_reader())),
        )
        .into_element(),
        ReaderStatus::SelectionUnavailable { reason } => {
            plain_panel("repository-selection-unavailable", "Nothing to show", reason).into_element()
        }
    }
}

fn loading_panel() -> View {
    view()
        .test_id("repository-loading")
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .size_full()
        .child(loading("Loading file…"))
}

fn plain_panel(id: &'static str, title: &'static str, detail: &str) -> View {
    view()
        .test_id(id)
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .size_full()
        .px_8()
        .child(t::label(title).text_size(px(14.0)))
        .child(
            t::label(detail.to_string())
                .text_size(px(12.0))
                .text_color(t::theme().muted_foreground),
        )
}

fn retry_panel(
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    label: &'static str,
    retry: Rc<dyn Fn()>,
) -> View {
    view()
        .test_id(id)
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .size_full()
        .px_8()
        .child(t::label(title).text_size(px(14.0)))
        .child(
            t::label(detail.to_string())
                .text_size(px(12.0))
                .text_color(t::theme().muted_foreground),
        )
        .child(action_button(label, move || retry()))
}

fn failure_panel(id: &'static str, error: &PageError, retry: Rc<dyn Fn()>) -> View {
    let mut panel = view()
        .test_id(id)
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .size_full()
        .px_8()
        .child(t::label(error.title.clone()).text_size(px(14.0)));
    if let Some(detail) = &error.detail {
        panel = panel.child(
            t::label(detail.clone())
                .text_size(px(12.0))
                .text_color(t::theme().muted_foreground),
        );
    }
    if error.retryable {
        let press_retry = retry.clone();
        panel = panel.child(action_button("Retry", move || press_retry()));
    }
    panel
}

/// Whether the tree's flattened row cap cut the list. Kept for the devlog's evidence hooks.
#[allow(dead_code)]
fn tree_is_capped(model: &ViewModel) -> bool {
    model.rows_capped
}

/// A plain pressable control. It is the page's own hit target: no nested button, so one press is
/// one action and the debug id, label and focus ring describe the same box.
fn action_button(label: &'static str, action: impl Fn() + 'static) -> View {
    view()
        .h(px(26.0))
        .px_2()
        .rounded(t::theme().radius)
        .border_1()
        .border_color(t::theme().border)
        .flex_row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .focusable(true)
        .on_press(action)
        .child(t::label(label).text_size(px(12.0)))
}
