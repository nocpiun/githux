//! Rendering for the repository page: summary, tree column and reader column.
//!
//! The tree is a bounded, virtualized list inside its own shared-scroll region;
//! the reader is the existing `ReaderView` entity and keeps its own scrolling.
//! Nothing here performs I/O: the view only reads the page's already-checked
//! state and asks it for the next step.

use std::rc::Rc;

use ely_core::{
    buttons::{Button, ButtonVariant},
    data_display::{Badge, Tone},
    forms::{Choice, Select},
    primitives::{Icon, IconName},
    theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px, uniform_list,
};

use super::navigation::{PageError, TreeRow, TreeRowKind, incompleteness_note, tree_incomplete};
use super::{ReaderStatus, RepositoryPage, Selection, TREE_ROW_HEIGHT, TREE_WIDTH};
use crate::domain::repo::RepoVisibility;
use crate::domain::repository::Completeness;
use crate::ui::scroll;

const INDENT: f32 = 12.0;

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
    let browsing = page.selected_branch().is_some();
    let body: AnyElement = match page.error().cloned() {
        // A page-level failure replaces the browsing area: an empty tree would
        // read as "no files", which a failed read never proves.
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
                .w_full()
                .child(tree)
                .child(reader)
                .into_any_element()
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
        .child(body)
        .into_any_element()
}

/// Title, visibility, description, counters and the branch picker.
fn header(page: &RepositoryPage, cx: &mut Context<RepositoryPage>) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let title_size = theme.text_size(TextSize::Xl);
    let small = theme.text_size(TextSize::Xs);
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
        facts.push(
            div()
                .debug_selector(|| "repository-counters".to_string())
                .text_size(small)
                .text_color(colors.fg_subtle)
                .child(format!(
                    "stars {} · forks {} · open issues & PRs {}",
                    details.stars, details.forks, details.open_issues_and_prs
                ))
                .into_any_element(),
        );
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
    let picker = branch_picker(page, cx);
    let border = colors.border;
    div()
        .debug_selector(|| "repository-header".to_string())
        .flex()
        .items_start()
        .gap_4()
        .flex_none()
        .w_full()
        .px_5()
        .py_4()
        .border_b_1()
        .border_color(border)
        .child(summary)
        .child(picker)
        .into_any_element()
}

/// The branch picker, plus an honest line about incomplete branch listings.
fn branch_picker(page: &RepositoryPage, cx: &mut Context<RepositoryPage>) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let small = theme.text_size(TextSize::Xs);
    let Some(branches) = page.branches() else {
        return div()
            .debug_selector(|| "repository-branch-unavailable".to_string())
            .flex_none()
            .text_size(small)
            .text_color(colors.fg_subtle)
            .child(match page.selected_branch() {
                Some(branch) => format!("branch {branch}"),
                None => "branch list unavailable".to_string(),
            })
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

/// The bounded file tree: a README row, then the visible rows only.
fn tree_column(
    page: &mut RepositoryPage,
    rows: Rc<Vec<TreeRow>>,
    cx: &mut Context<RepositoryPage>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let small = theme.text_size(TextSize::Xs);
    let radius = theme.radius(Radius::Sm);
    let selected = match page.selection() {
        Selection::Readme => None,
        Selection::File(path) => Some(path.clone()),
    };
    let readme_selected = matches!(page.selection(), Selection::Readme);
    let warnings: Vec<AnyElement> = tree_warnings(page, &colors, small);
    let count = rows.len();
    let empty = page
        .tree("")
        .is_some_and(|tree| tree.entries.is_empty() && !tree_incomplete(tree));
    let entity = cx.entity().downgrade();
    let readme_row = div()
        .id("repository-readme-row")
        .debug_selector(|| "repository-readme-row".to_string())
        .flex()
        .items_center()
        .gap_2()
        .flex_none()
        .h(px(TREE_ROW_HEIGHT))
        .mx_1()
        .px_2()
        .rounded(radius)
        .cursor_pointer()
        .hover(|style| style.bg(colors.hover))
        .when(readme_selected, |row| row.bg(colors.hover))
        .child(
            Icon::new(IconName::BookOpen)
                .size(IconSize::Xs)
                .color(colors.fg_muted),
        )
        .child(div().min_w_0().child(Ellipsis::new("README")))
        .on_click({
            let entity = entity.clone();
            move |_, _, cx| {
                // A weak entity may already be gone; the page is released then.
                let _ = entity.update(cx, |page, cx| page.show_readme(cx));
            }
        });

    let list = uniform_list(
        "repository-tree",
        count,
        cx.processor(
            move |page: &mut RepositoryPage, range: std::ops::Range<usize>, window, cx| {
                range
                    .map(|index| match rows.get(index).cloned() {
                        Some(row) => tree_row(page, row, selected.clone(), window, cx),
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
    let border = colors.border;
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
        .child(readme_row)
        .children(empty_note)
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(div().size_full().overflow_hidden().child(list))
                .child(wheel)
                .child(bar),
        )
        .into_any_element()
}

/// Every honest note about the tree itself: incompleteness, a scoped failure
/// and the flattening cap.
fn tree_warnings(
    page: &RepositoryPage,
    colors: &ely_core::theme::Palette,
    small: gpui::Rems,
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

fn tree_row(
    page: &mut RepositoryPage,
    row: TreeRow,
    selected: Option<String>,
    _window: &mut Window,
    cx: &mut Context<RepositoryPage>,
) -> AnyElement {
    let _ = page;
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
    let is_selected =
        row.kind == TreeRowKind::File && selected.as_deref() == Some(row.path.as_str());
    let is_directory = row.kind.is_directory();
    let index = row.index;
    let path = row.path.clone();
    let name = row.name.clone();
    let note = row.note;
    let depth = row.depth;
    let entity = cx.entity();
    let hover = colors.hover;
    div()
        .id(SharedString::from(format!("repository-row-{path}")))
        .debug_selector(move || format!("repository-tree-row-{index}"))
        .flex()
        .items_center()
        .gap_2()
        .flex_none()
        .h(px(TREE_ROW_HEIGHT))
        .pl(px(8.0 + INDENT * depth as f32))
        .pr_2()
        .mx_1()
        .rounded(radius)
        .text_size(small)
        .when(is_selected, |row| row.bg(hover))
        .when(is_directory, |row| row.cursor_pointer())
        .hover(move |style| style.bg(hover))
        .child(Icon::new(icon).size(IconSize::Xs).color(icon_color))
        .child(div().min_w_0().flex_1().child(Ellipsis::new(name)))
        .children(note.map(|note| {
            div()
                .flex_none()
                .text_color(colors.fg_subtle)
                .child(note.to_string())
        }))
        .on_click(move |_, _, cx| {
            entity.update(cx, |page, cx| {
                if is_directory {
                    page.toggle_directory(&path, cx);
                } else {
                    page.select_file(&path, cx);
                }
            });
        })
        .into_any_element()
}

/// The right column: what is shown, plus the reader or an honest status.
fn reader_column(page: &RepositoryPage, cx: &mut Context<RepositoryPage>) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let small = theme.text_size(TextSize::Xs);
    let path = match page.selection() {
        Selection::Readme => page.readme_path().unwrap_or("README").to_string(),
        Selection::File(path) => path.clone(),
    };
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
                .child(Ellipsis::new(path)),
        )
        .children((!matches!(page.selection(), Selection::Readme)).then(|| {
            Button::new("repository-show-readme", "README")
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
            div()
                .debug_selector(|| "repository-reader-body".to_string())
                .flex_1()
                .min_h_0()
                .w_full()
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

fn readme_uncertain_panel(cx: &App) -> AnyElement {
    plain_panel(
        "repository-readme-uncertain",
        "README not confirmed",
        "GitHub answered 404 for the README and this is not a confirmed empty tree. The repository may have no README, or this token may not see it. This is not evidence of an empty repository.",
        cx,
    )
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

/// A failure with its reason and, when it could help, one explicit retry.
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
        .child(div().text_size(base).text_color(colors.fg).child(title))
        .children(detail.map(|detail| div().text_size(small).text_color(muted).child(detail)))
        .when(retryable, |panel| {
            panel.child(
                Button::new("repository-retry", label)
                    .variant(ButtonVariant::Secondary)
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |page, cx| {
                            if scoped_retry {
                                page.retry(cx)
                            } else {
                                page.activate(cx)
                            }
                        });
                    }),
            )
        })
        .into_any_element()
}
