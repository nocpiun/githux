//! Rendering for the repository page: summary, in-page tabs, tree and reader.
//!
//! The structure follows the confirmed product layout: repository summary, then
//! horizontal in-page content tabs, then the Code area with a bounded file tree
//! on the left and the reader on the right.
//!
//! The tree is a bounded, virtualized list inside its own shared-scroll region;
//! the reader is the existing `ReaderView` entity and keeps its own scrolling.
//! Nothing here performs I/O: the view only reads the page's already-checked
//! state and asks it for the next step.

use std::rc::Rc;

use ely_core::{
    buttons::{Button, ButtonVariant},
    data_display::Tone,
    forms::{Choice, Select},
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px, uniform_list,
};

use super::navigation::{
    PageError, RowState, TreeRow, TreeRowKind, incompleteness_note, tree_incomplete,
};
use super::{ReaderStatus, RepositoryPage, RepositoryTab, Selection, TREE_ROW_HEIGHT, TREE_WIDTH};
use crate::domain::repo::RepoVisibility;
use crate::domain::repository::Completeness;
use crate::ui::{badge::Badge, scroll};

const INDENT: f32 = 14.0;
/// Room for the disclosure chevron, so file rows line up with directory names.
const CHEVRON_SLOT: f32 = 16.0;

pub(super) fn render(
    page: &mut RepositoryPage,
    rows: Rc<Vec<TreeRow>>,
    _window: &mut Window,
    cx: &mut Context<RepositoryPage>,
) -> AnyElement {
    let theme = cx.theme();
    let background = theme.colors.surface;
    let foreground = theme.colors.fg;
    let text_size = theme.text_size(TextSize::Sm);
    let header = header(page, cx);
    let tabs = tab_strip(page, cx);
    let browsing = page.selected_branch().is_some();
    let body: AnyElement =
        if !RepositoryTab::Code.has_content() || page.tab() != RepositoryTab::Code {
            // Undesigned repository tabs stay empty by product convention: no
            // "not implemented" card, no copied GitHub feature menu.
            div()
                .debug_selector(|| "repository-tab-empty".to_string())
                .flex_1()
                .min_h_0()
                .w_full()
                .into_any_element()
        } else {
            match page.error().cloned() {
                // A page-level failure replaces the browsing area: an empty tree
                // would read as "no files", which a failed read never proves.
                Some(error) if !browsing => {
                    failure_panel("repository-error", &error, page.is_active(), false, cx)
                }
                _ => {
                    let tree = tree_column(page, rows, cx);
                    let reader = reader_column(page, cx);
                    div()
                        .debug_selector(|| "repository-body".to_string())
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .w_full()
                        .overflow_hidden()
                        .child(tree)
                        .child(reader)
                        .into_any_element()
                }
            }
        };
    div()
        .id("repository-page-root")
        .debug_selector(|| "repository-page-root".to_string())
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .bg(background)
        .text_color(foreground)
        .text_size(text_size)
        .child(header)
        .children(
            page.overview()
                .cloned()
                .map(|overview| div().flex_none().min_w_0().w_full().px_5().child(overview)),
        )
        .child(tabs)
        .child(body)
        .children(
            page.overview()
                .map(|overview| overview.read(cx).language_bar(cx)),
        )
        .into_any_element()
}

/// Title, visibility, description, counters and the branch picker.
fn header(page: &RepositoryPage, cx: &mut Context<RepositoryPage>) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let title_size = theme.text_size(TextSize::Xl);
    let name = page
        .details()
        .map(|details| {
            format!(
                "{}/{}",
                details.owner.login,
                details.identity.locator().name()
            )
        })
        .unwrap_or_else(|| format!("{}/{}", page.locator().owner(), page.locator().name()));
    let mut facts: Vec<AnyElement> = Vec::new();
    if let Some(details) = page.details() {
        facts.push(
            Badge::new(match details.visibility {
                RepoVisibility::Public => "Public",
                RepoVisibility::Internal => "Internal",
                RepoVisibility::Private => "Private",
            })
            .into_any_element(),
        );
        if details.archived {
            facts.push(
                Badge::new("Archived")
                    .tone(Tone::Warning)
                    .into_any_element(),
            );
        }
        if details.fork {
            facts.push(Badge::new("Fork").into_any_element());
        }
    }
    let description = page
        .details()
        .and_then(|details| details.description.clone())
        .map(|description| {
            div()
                .debug_selector(|| "repository-description".to_string())
                .text_color(colors.fg_muted)
                .child(description)
        });
    let summary = div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w_0()
        .flex_1()
        .child(
            div()
                .debug_selector(|| "repository-title".to_string())
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .text_size(title_size)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(Ellipsis::new(name)),
                )
                .children(facts),
        )
        .children(description);
    let actions = repository_actions(page, cx);
    div()
        .debug_selector(|| "repository-header".to_string())
        .flex()
        .items_start()
        .gap_4()
        .flex_none()
        .w_full()
        .px_5()
        .pt_4()
        .pb_2()
        .child(summary)
        .child(actions)
        .into_any_element()
}

fn repository_actions(page: &RepositoryPage, cx: &mut Context<RepositoryPage>) -> AnyElement {
    let binding = page.repository_binding();
    let enabled = page.on_action.is_some() && binding.is_some();
    let buttons = [
        (true, "Star", IconName::Star),
        (false, "Fork", IconName::GitBranch),
    ]
    .into_iter()
    .map(|(star, label, icon)| {
        let owner = cx.weak_entity();
        let key_owner = owner.clone();
        let binding = binding.clone();
        let key_binding = binding.clone();
        let focus = if star {
            &page.star_focus
        } else {
            &page.fork_focus
        };
        div()
            .id(if star {
                "repository-star-entry"
            } else {
                "repository-fork-entry"
            })
            .debug_selector(move || {
                if star {
                    "repository-star".into()
                } else {
                    "repository-fork".into()
                }
            })
            .track_focus(focus)
            .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                if enabled
                    && !event.is_held
                    && !event.keystroke.modifiers.modified()
                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                {
                    cx.stop_propagation();
                    if let Some(binding) = key_binding.clone() {
                        let _ = key_owner.update(cx, |page, cx| {
                            page.repository_action(star, binding, window, cx)
                        });
                    }
                }
            })
            .child(
                Button::new(
                    if star {
                        "repository-star"
                    } else {
                        "repository-fork"
                    },
                    label,
                )
                .icon(icon)
                .focus_handle(focus)
                .size(ControlSize::Sm)
                .disabled(!enabled)
                .on_click(move |_, window, cx| {
                    if let Some(binding) = binding.clone() {
                        let _ = owner.update(cx, |page, cx| {
                            page.repository_action(star, binding, window, cx)
                        });
                    }
                }),
            )
            .into_any_element()
    });
    div()
        .flex()
        .flex_none()
        .gap_2()
        .items_center()
        .children(buttons)
        .into_any_element()
}

/// The branch picker, plus an honest line about incomplete branch listings.
fn branch_picker(page: &RepositoryPage, cx: &mut Context<RepositoryPage>) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let small = theme.text_size(TextSize::Xs);
    let Some(branches) = page.branches() else {
        let retry = cx.weak_entity();
        let error = page.branches_error().cloned();
        return div()
            .debug_selector(|| "repository-branch-unavailable".to_string())
            .flex()
            .flex_col()
            .gap_1()
            .flex_none()
            .w(px(220.0))
            .text_size(small)
            .text_color(colors.fg_subtle)
            .child(match page.selected_branch() {
                Some(branch) => format!("branch {branch}"),
                None => "Branch list unavailable".to_string(),
            })
            .children(error.map(|error| {
                div().child(error.title).child(
                    Button::new("repository-retry-branches", "Retry branches")
                        .disabled(page.branches_loading)
                        .size(ControlSize::Sm)
                        .on_click(move |_, _, cx| {
                            let _ = retry.update(cx, |page, cx| page.retry_branches(cx));
                        }),
                )
            }))
            .into_any_element();
    };
    let choices: Vec<Choice> = branches
        .branches
        .iter()
        .map(|branch| {
            Choice::new(
                branch.name.as_str().to_string(),
                branch.name.as_str().to_string(),
            )
        })
        .collect();
    let note = incompleteness_note(&branches.completeness).map(|reason| {
        div()
            .debug_selector(|| "repository-branches-incomplete".to_string())
            .text_size(small)
            .text_color(colors.warning)
            .child(format!(
                "Incomplete branch list ({} shown): {reason}. This is not the full set.",
                branches.branches.len()
            ))
    });
    let select: AnyElement = if choices.is_empty() {
        div()
            .debug_selector(|| "repository-branch-empty".to_string())
            .text_size(small)
            .text_color(colors.fg_subtle)
            .child(if branches.completeness == Completeness::Complete {
                "No branches"
            } else {
                "No branches in the partial result"
            })
            .into_any_element()
    } else {
        let entity = cx.entity().downgrade();
        let mut select = Select::new("repository-branch", choices)
            .label("Branch")
            .size(ControlSize::Sm);
        if let Some(current) = page.selected_branch() {
            select = select.selected(current.to_string());
        }
        select
            .on_change(move |value: &SharedString, _, cx| {
                let value = value.to_string();
                let _ = entity.update(cx, |page, cx| page.select_branch(&value, cx));
            })
            .into_any_element()
    };
    div()
        .id("repository-branch-container")
        .tooltip(Tooltip::text(
            page.selected_branch().unwrap_or("Branch").to_string(),
        ))
        .debug_selector(|| "repository-branch".to_string())
        .flex()
        .flex_col()
        .items_end()
        .gap_1()
        .flex_none()
        .w(px(220.0))
        .child(select)
        .children(note)
        .into_any_element()
}

/// The in-page content tabs. Only Code has content this round; the rest keep
/// the confirmed names and draw nothing inside.
fn tab_strip(page: &RepositoryPage, cx: &mut Context<RepositoryPage>) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let border = colors.border;
    let small = theme.text_size(TextSize::Sm);
    let active = page.tab();
    let picker = branch_picker(page, cx);
    let tabs: Vec<AnyElement> = RepositoryTab::ALL
        .iter()
        .map(|&tab| {
            let entity = cx.entity().downgrade();
            let is_active = tab == active;
            let fg = if is_active {
                colors.fg
            } else {
                colors.fg_muted
            };
            div()
                .id(SharedString::from(format!("repository-tab-{}", tab.id())))
                .debug_selector(move || format!("repository-tab-{}", tab.id()))
                .relative()
                .flex()
                .items_center()
                .gap_1p5()
                .flex_none()
                .px_2()
                .py_2()
                .cursor_pointer()
                .text_size(small)
                .font_weight(if is_active {
                    gpui::FontWeight::MEDIUM
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(fg)
                .hover(|style| style.text_color(colors.fg))
                .child(Icon::new(tab.icon()).size(IconSize::Xs).color(fg))
                .child(tab.label())
                .children(
                    match tab {
                        RepositoryTab::Issues => page
                            .extras
                            .counts
                            .as_ref()
                            .and_then(|c| c.issues.as_ref().ok()),
                        RepositoryTab::PullRequests => page
                            .extras
                            .counts
                            .as_ref()
                            .and_then(|c| c.pull_requests.as_ref().ok()),
                        _ => None,
                    }
                    .map(|count| {
                        div()
                            .debug_selector(move || format!("repository-badge-{}", tab.id()))
                            .child(Badge::new(count.to_string()))
                    }),
                )
                .when(is_active, |entry| {
                    entry.child(
                        div()
                            .debug_selector(move || format!("repository-tab-active-{}", tab.id()))
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(2.0))
                            .bg(colors.accent),
                    )
                })
                .on_click(move |_, _, cx| {
                    // A weak entity may already be released; nothing is left to notify.
                    let _ = entity.update(cx, |page, cx| page.select_tab(tab, cx));
                })
                .into_any_element()
        })
        .collect();
    div()
        .debug_selector(|| "repository-tabs".to_string())
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .flex_none()
        .w_full()
        .px_5()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .flex()
                .items_center()
                .min_w_0()
                .flex_wrap()
                .gap_2()
                .children(tabs),
        )
        .child(div().flex_1())
        .child(picker)
        .into_any_element()
}

/// The bounded file tree: the visible rows only, with a chevron slot, a kind
/// icon and a full-width hit area.
fn tree_column(
    page: &mut RepositoryPage,
    rows: Rc<Vec<TreeRow>>,
    cx: &mut Context<RepositoryPage>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let small = theme.text_size(TextSize::Xs);
    let border = colors.border;
    let selected = page.current_path().map(str::to_string);
    let cursor = page.cursor().map(str::to_string);
    let warnings = tree_warnings(page, &colors, small, cx);
    let count = rows.len();
    let empty = page
        .tree("")
        .is_some_and(|tree| tree.entries.is_empty() && !tree_incomplete(tree));
    let list = uniform_list(
        "repository-tree",
        count,
        cx.processor(
            move |page: &mut RepositoryPage, range: std::ops::Range<usize>, window, cx| {
                range
                    .map(|index| match rows.get(index).cloned() {
                        Some(row) => {
                            tree_row(page, row, selected.clone(), cursor.clone(), window, cx)
                        }
                        None => div().into_any_element(),
                    })
                    .collect::<Vec<_>>()
            },
        ),
    )
    .track_scroll(&page.tree_scroll)
    .size_full();

    let now = page.now_ms();
    let reduce = cx.reduce_motion();
    // The shared motion owns the tree's discrete wheel easing; ticking it here
    // is the only place that requests follow-up frames for this region.
    page.tree_motion.tick(std::time::Instant::now(), reduce);
    if page.tree_bars.dragging().is_some() && !cx.has_active_drag() {
        let motion = page.tree_motion.clone();
        page.tree_bars.settle(&motion, now, reduce);
    }
    let opacity = page.tree_bars.opacity(now);
    let this = cx.entity().downgrade();
    let on_drag: scroll::OnDragging =
        Rc::new(move |axis, dragging, _: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |page, cx| {
                let motion = page.tree_motion.clone();
                if page
                    .tree_bars
                    .drag(&motion, axis, dragging, page.now_ms(), cx.reduce_motion())
                {
                    cx.notify();
                }
            });
        });
    let motion = page.tree_motion.clone();
    let bar = scroll::Bar {
        target: &motion,
        id: scroll::BarId {
            owner: cx.entity_id(),
            name: "repository-tree-scrollbar",
            axis: gpui::Axis::Vertical,
        },
        placement: scroll::Placement::overlay(4.0, 8.0),
        style: scroll::Style::DEFAULT,
        opacity,
        on_drag,
    }
    .render(cx);
    let wheel = scroll::wheel_overlay("repository-tree", gpui::Axis::Vertical, motion);
    // The commit is secondary information: a quiet short id under the header.
    let commit = page.fixed_commit().map(|commit| {
        div()
            .debug_selector(|| "repository-commit".to_string())
            .text_size(small)
            .text_color(colors.fg_subtle)
            .child(commit.as_str().chars().take(7).collect::<String>())
    });
    let empty_note = (empty && count == 0).then(|| {
        div()
            .px_3()
            .py_2()
            .text_size(small)
            .text_color(colors.fg_subtle)
            .child("This commit has no files.")
    });

    div()
        .id("repository-tree-column")
        .debug_selector(|| "repository-tree-column".to_string())
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(TREE_WIDTH))
        .h_full()
        .min_h_0()
        .border_r_1()
        .border_color(border)
        .on_hover(cx.listener(|page, hovered, _, cx| {
            if page
                .tree_bars
                .hover(*hovered, page.now_ms(), cx.reduce_motion())
            {
                cx.notify();
            }
        }))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .flex_none()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(border)
                .child(
                    div()
                        .text_size(small)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(colors.fg_subtle)
                        .child("Files"),
                )
                .children(commit),
        )
        .children(warnings)
        .children(empty_note)
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .id("repository-tree-viewport")
                .debug_selector(|| "repository-tree-viewport".into())
                .track_focus(&page.tree_focus)
                .on_key_down(cx.listener(|page, event, window, cx| {
                    page.press_tree_key_event(event, window, cx);
                }))
                .p(px(7.0))
                .child(
                    div()
                        .relative()
                        .size_full()
                        .min_h_0()
                        .debug_selector(|| "repository-tree-scroll-viewport".into())
                        .child(div().size_full().overflow_hidden().child(list))
                        .child(wheel)
                        .child(bar),
                ),
        )
        .into_any_element()
}

/// Every honest note about the tree itself: incompleteness, a scoped failure
/// and the flattening cap.
fn tree_warnings(
    page: &RepositoryPage,
    colors: &ely_core::theme::Palette,
    small: gpui::Rems,
    cx: &mut Context<RepositoryPage>,
) -> Vec<AnyElement> {
    let mut warnings = Vec::new();
    if page.tree("").is_some_and(tree_incomplete) {
        warnings.push(
            div()
                .debug_selector(|| "repository-tree-incomplete".to_string())
                .px_2()
                .py_1()
                .text_size(small)
                .text_color(colors.warning)
                .child(
                    "This directory listing is truncated or incomplete; it is not a complete empty directory.",
                )
                .into_any_element(),
        );
    }
    if let Some(error) = page.tree_failure("") {
        warnings.push(
            div()
                .debug_selector(|| "repository-tree-error".to_string())
                .px_2()
                .py_1()
                .text_size(small)
                .text_color(colors.danger)
                .child(error.title.clone())
                .when(error.retryable, |row| {
                    let page = cx.weak_entity();
                    row.child(
                        Button::new("repository-retry-root", "Retry files")
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = page.update(cx, |page, cx| page.retry_directory("", cx));
                            }),
                    )
                })
                .into_any_element(),
        );
    }
    if page.tree_capped() {
        warnings.push(
            div()
                .debug_selector(|| "repository-tree-capped".to_string())
                .px_2()
                .py_1()
                .text_size(small)
                .text_color(colors.warning)
                .child(format!(
                    "More rows are loaded than the {}-row tree cap. Collapse directories to review the rest; nothing was fetched recursively.",
                    super::navigation::MAX_TREE_ROWS
                ))
                .into_any_element(),
        );
    }
    warnings
}

/// One tree row. The whole row is the hit area: chevron slot, icon, name, and
/// (for directories) an inline state or a scoped retry.
fn tree_row(
    page: &mut RepositoryPage,
    row: TreeRow,
    selected: Option<String>,
    cursor: Option<String>,
    window: &mut Window,
    cx: &mut Context<RepositoryPage>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let radius = theme.radius(Radius::Sm);
    let small = theme.text_size(TextSize::Xs);
    let (icon, icon_color) = match row.kind {
        TreeRowKind::Directory { .. } => (IconName::Folder, colors.fg_muted),
        TreeRowKind::File => (IconName::FileText, colors.fg_subtle),
        TreeRowKind::Symlink => (IconName::Link, colors.fg_subtle),
        TreeRowKind::Submodule => (IconName::Package, colors.fg_subtle),
        TreeRowKind::Unknown => (IconName::File, colors.fg_subtle),
    };
    let path = row.path.clone();
    let name = row.name.clone();
    let tooltip_path = row.path.clone();
    let debug_path = row.path.clone();
    let depth = row.depth;
    let kind = row.kind;
    let state = row.state.clone();
    let is_selected = selected.as_deref() == Some(row.path.as_str());
    let is_cursor =
        page.tree_focus.is_focused(window) && cursor.as_deref() == Some(row.path.as_str());
    let hover = colors.hover;
    let focus_ring = colors.fg_subtle;
    let entity = cx.entity();

    let chevron: AnyElement = match kind {
        TreeRowKind::Directory { expanded } => {
            let entity = entity.clone();
            let path = path.clone();
            // The chevron is its own hit area, and it stops propagation so one
            // press toggles exactly once.
            div()
                .id(SharedString::from(format!("repository-chevron-{path}")))
                .debug_selector({
                    let path = path.clone();
                    move || format!("repository-chevron-{path}")
                })
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(CHEVRON_SLOT))
                .rounded(radius)
                .cursor_pointer()
                .hover(|style| style.bg(colors.border))
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(IconSize::Xs)
                    .color(colors.fg_muted),
                )
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let focus = entity.read(cx).tree_focus.clone();
                    window.focus(&focus, cx);
                    entity.update(cx, |page, cx| {
                        page.focus_row(&path, cx);
                        page.toggle_directory(&path, cx);
                    });
                })
                .into_any_element()
        }
        // Files leave the chevron slot empty so names line up under folders.
        _ => div().flex_none().w(px(CHEVRON_SLOT)).into_any_element(),
    };

    // A directory's own state: loading, proven empty, or a scoped failure.
    let state_element: Option<AnyElement> = match &state {
        RowState::Normal => None,
        RowState::Loading => Some(
            div()
                .debug_selector(|| "repository-row-loading".to_string())
                .flex_none()
                .text_size(small)
                .text_color(colors.fg_subtle)
                .child("loading…")
                .into_any_element(),
        ),
        RowState::Empty => Some(
            div()
                .debug_selector(|| "repository-row-empty".to_string())
                .flex_none()
                .text_size(small)
                .text_color(colors.fg_subtle)
                .child("empty")
                .into_any_element(),
        ),
        RowState::Incomplete => Some(
            div()
                .debug_selector(|| "repository-row-incomplete".into())
                .text_color(colors.warning)
                .flex_none()
                .child("Incomplete")
                .into_any_element(),
        ),
        RowState::Failed(error) => {
            let entity = entity.clone();
            let path = path.clone();
            let retryable = error.retryable;
            Some(
                div()
                    .id(SharedString::from(format!("repository-row-error-{path}")))
                    .tooltip(Tooltip::text(error.title.clone()))
                    .debug_selector(|| "repository-row-failed".to_string())
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_size(small)
                            .text_color(colors.danger)
                            .child("Failed"),
                    )
                    .when(retryable, |row| {
                        row.child(
                            Button::new(
                                SharedString::from(format!("repository-row-retry-{path}")),
                                "Retry",
                            )
                            .variant(ButtonVariant::Ghost)
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                entity.update(cx, |page, cx| page.retry_directory(&path, cx));
                            }),
                        )
                    })
                    .into_any_element(),
            )
        }
    };

    div()
        .id(SharedString::from(format!("repository-row-{path}")))
        .debug_selector(move || format!("repository-row-{debug_path}"))
        .tooltip(Tooltip::text(tooltip_path))
        .flex()
        .items_center()
        .gap_1p5()
        .flex_none()
        .w_full()
        .h(px(TREE_ROW_HEIGHT))
        .pl(px(6.0 + INDENT * depth as f32))
        .pr_2()
        .rounded(radius)
        .text_size(small)
        .cursor_pointer()
        .border_1()
        .border_color(gpui::transparent_black())
        .when(is_selected, |row| row.bg(colors.border))
        .when(!is_selected, |row| row.hover(move |style| style.bg(hover)))
        // Keyboard focus is a ring, distinct from hover and selection.
        .when(is_cursor, |row| row.border_color(focus_ring))
        .child(chevron)
        .child(Icon::new(icon).size(IconSize::Xs).color(icon_color))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .child(Ellipsis::new(name)),
        )
        .children(state_element)
        .on_click(move |_, window, cx| {
            // One press acts once: a directory toggles, a file opens. The reader
            // keeps whatever it is showing when a directory is toggled.
            let focus = entity.read(cx).tree_focus.clone();
            window.focus(&focus, cx);
            entity.update(cx, |page, cx| {
                page.focus_row(&path, cx);
                match kind {
                    TreeRowKind::Directory { .. } => page.toggle_directory(&path, cx),
                    _ => page.select_file(&path, cx),
                }
            });
        })
        .into_any_element()
}

/// The right column: the current path, a "back to README" entry when it is
/// useful, and the reader or an honest status.
fn reader_column(page: &RepositoryPage, cx: &mut Context<RepositoryPage>) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let small = theme.text_size(TextSize::Xs);
    let path = page.current_path().map(str::to_string);
    let entity = cx.entity();
    let border = colors.border;
    let header = div()
        .debug_selector(|| "repository-reader-header".to_string())
        .flex()
        .items_center()
        .gap_2()
        .flex_none()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(border)
        .child(
            Icon::new(match page.selection() {
                Selection::Readme => IconName::BookOpen,
                Selection::File(_) => IconName::FileText,
            })
            .size(IconSize::Xs)
            .color(colors.fg_subtle),
        )
        .child(
            div()
                .debug_selector(|| "repository-selected-path".to_string())
                .flex_1()
                .min_w_0()
                .text_size(small)
                .text_color(colors.fg_muted)
                .child(match path {
                    Some(path) => Ellipsis::new(path).into_any_element(),
                    None => div().child("No file selected").into_any_element(),
                }),
        )
        // Only when a usable README exists and a different file is shown.
        .children(page.can_return_to_readme().then(|| {
            Button::new("repository-back-to-readme", "Back to README")
                .variant(ButtonVariant::Ghost)
                .size(ControlSize::Sm)
                .on_click(move |_, _, cx| {
                    entity.update(cx, |page, cx| page.show_readme(cx));
                })
        }));
    let body: AnyElement = match (page.reader_status().clone(), page.reader()) {
        (ReaderStatus::Showing, Some(reader)) => reader.clone().into_any_element(),
        (status, _) => match status {
            ReaderStatus::Loading => loading_panel(cx),
            ReaderStatus::NoReadme => no_readme_panel(cx),
            ReaderStatus::ReadmeUncertain => readme_uncertain_panel(cx),
            ReaderStatus::ReadmeUnavailable(error) => {
                failure_panel("repository-readme-error", &error, true, true, cx)
            }
            ReaderStatus::ContentFailed(error) => {
                failure_panel("repository-content-error", &error, true, true, cx)
            }
            ReaderStatus::SelectionUnavailable { reason } => plain_panel(
                "repository-selection-unavailable",
                "Nothing to show",
                &reason,
                cx,
            ),
            ReaderStatus::Showing => loading_panel(cx),
        },
    };
    div()
        .debug_selector(|| "repository-reader-column".to_string())
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .h_full()
        .child(header)
        .child(
            // `min_w_0` + hidden overflow is what lets the reader's own content
            // wrap inside this column instead of widening the page; the reader
            // keeps its own scrolling and never gets a second scroll surface.
            div()
                .debug_selector(|| "repository-reader-body".to_string())
                .flex_1()
                .min_h_0()
                .min_w_0()
                .w_full()
                .overflow_hidden()
                .child(body),
        )
        .into_any_element()
}

fn loading_panel(cx: &App) -> AnyElement {
    let colors = &cx.theme().colors;
    div()
        .debug_selector(|| "repository-loading".to_string())
        .flex()
        .items_center()
        .justify_center()
        .size_full()
        .text_color(colors.fg_subtle)
        .child("Loading…")
        .into_any_element()
}

/// A restrained true-empty reader: the tree beside it stays usable.
fn no_readme_panel(cx: &App) -> AnyElement {
    plain_panel(
        "repository-no-readme",
        "No README",
        "This commit has no README. Pick a file from the tree to read it; nothing is generated in its place.",
        cx,
    )
}

fn readme_uncertain_panel(cx: &mut Context<RepositoryPage>) -> AnyElement {
    let owner = cx.weak_entity();
    div().debug_selector(||"repository-readme-uncertain".into())
        .flex().flex_col().items_center().justify_center().gap_2().px_8().size_full()
        .child("README not confirmed")
        .child(div().text_color(cx.theme().colors.fg_muted).text_center()
            .child("The README may be absent or not visible to this account. You can browse files or retry discovery."))
        .child(div().debug_selector(||"repository-retry-readme-discovery".into())
            .child(Button::new("repository-retry-readme-discovery","Retry README")
                .size(ControlSize::Sm)
                .on_click(move |_,_,cx| {let _=owner.update(cx,|page,cx|page.retry_readme(cx));})))
        .into_any_element()
}

fn plain_panel(id: &'static str, title: &str, detail: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    div()
        .debug_selector(move || id.to_string())
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .size_full()
        .px_8()
        .text_center()
        .child(
            div()
                .min_w_0()
                .text_size(theme.text_size(TextSize::Base))
                .text_color(colors.fg)
                .child(title.to_string()),
        )
        .child(
            div()
                .min_w_0()
                .max_w(px(560.0))
                .text_size(theme.text_size(TextSize::Sm))
                .text_color(colors.fg_muted)
                .child(detail.to_string()),
        )
        .into_any_element()
}

/// A failure with its reason and, when it could help, one explicit retry that
/// re-runs only this resource.
fn failure_panel(
    id: &'static str,
    error: &PageError,
    active: bool,
    scoped_retry: bool,
    cx: &mut Context<RepositoryPage>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let base = theme.text_size(TextSize::Base);
    let small = theme.text_size(TextSize::Sm);
    let entity = cx.entity();
    let retryable = error.retryable;
    let label = if active { "Retry" } else { "Activate" };
    let detail = error.detail.clone();
    let title = error.title.clone();
    let muted = colors.fg_muted;
    div()
        .debug_selector(move || id.to_string())
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .size_full()
        .px_8()
        .text_center()
        .child(
            div()
                .min_w_0()
                .text_size(base)
                .text_color(colors.fg)
                .child(title),
        )
        .children(detail.map(|detail| {
            div()
                .min_w_0()
                .max_w(px(560.0))
                .text_size(small)
                .text_color(muted)
                .child(detail)
        }))
        .when(retryable, |panel| {
            panel.child(
                Button::new("repository-retry", label)
                    .variant(ButtonVariant::Secondary)
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |page, cx| {
                            if scoped_retry {
                                page.retry_reader(cx)
                            } else {
                                page.retry(cx)
                            }
                        });
                    }),
            )
        })
        .into_any_element()
}
