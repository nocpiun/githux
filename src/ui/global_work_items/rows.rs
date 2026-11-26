//! Global Issues / Pull Requests rows, rendered by the row presentation shared with Home.
use super::{RepositoryNavigation, WorkItemNavigation, WorkItemsView};
use crate::ui::work_item_row::{
    self, Glyph, RepositoryLink, Row, RowAction, Selectors, TitleAction, Tone,
};
use crate::{data::RepositoryReadContext, domain::work_item::*};
use gpui::{AnyElement, Context, Window};
use std::rc::Rc;

/// The height every global list reserves in its virtual list state; it is the shared row height.
pub(super) const ROW_HEIGHT: f32 = work_item_row::WORK_ITEM_ROW_HEIGHT;

/// Main-row text: relative age only, never a redundant "Updated" prefix.
pub(super) fn time_label(value: &str, now: std::time::SystemTime) -> String {
    super::time::relative(Some(value), now)
}

/// Exact RFC3339 value for the tooltip and accessible name only.
pub(super) fn exact_time(value: &str) -> String {
    value.to_string()
}

pub(super) fn row(
    item: &WorkItem,
    kind: WorkItemKind,
    selected: bool,
    context: Option<RepositoryReadContext>,
    has_open_handler: bool,
    now: std::time::SystemTime,
    cx: &mut Context<WorkItemsView>,
) -> AnyElement {
    let id = item.id;
    let weak = cx.entity().downgrade();
    let repository = item.repository.clone();
    let label = repository.full_name();
    // The title is the row's single detail target. It is only a link when the host installed a
    // callback for this list's kind and the row really is that kind, so an unconnected preview or
    // a payload that contradicts the query (an `is:issue` result carrying a pull request) keeps
    // plain text instead of opening the other resource's route.
    let open_detail: Option<TitleAction> = (has_open_handler && item.kind == kind).then(|| {
        let weak = cx.entity().downgrade();
        let repository = item.repository.clone();
        let number = item.number;
        let context = context.clone();
        Rc::new(
            move |pointer: bool, window: &mut Window, cx: &mut gpui::App| {
                let _ = weak.update(cx, |view, cx| {
                    if pointer {
                        view.selected = Some(id);
                        view.focus.focus(window, cx);
                        cx.notify();
                    }
                    if let Some(context) = &context {
                        view.navigate_open(
                            WorkItemNavigation {
                                context: context.clone(),
                                repository: repository.clone(),
                                number,
                            },
                            window,
                            cx,
                        );
                    }
                });
            },
        ) as TitleAction
    });
    let open_repository: RowAction = Rc::new(move |window, cx| {
        if let Some(context) = &context {
            let _ = weak.update(cx, |view, cx| {
                view.navigate(
                    RepositoryNavigation {
                        context: context.clone(),
                        repository: repository.clone(),
                    },
                    window,
                    cx,
                )
            });
        }
    });
    let select: RowAction = {
        let weak = cx.entity().downgrade();
        Rc::new(move |window, cx| {
            let _ = weak.update(cx, |view, cx| {
                view.selected = Some(id);
                view.focus.focus(window, cx);
                cx.notify();
            });
        })
    };
    let glyph = match item.kind {
        WorkItemKind::Issue => Glyph::Issue,
        WorkItemKind::PullRequest => Glyph::PullRequest,
    };
    let updated_label = super::time::relative(Some(&item.updated_at), now);
    work_item_row::render(
        Row {
            selectors: Selectors {
                row: format!("work-item-{id}"),
                title_line: None,
                icon: None,
                title: format!("work-title-{id}"),
                metadata: Some(format!("work-metadata-{id}")),
                number: Some(format!("work-number-{id}")),
                repository: Some(format!("work-repo-{id}")),
                owner: None,
                updated: Some(format!("work-updated-{id}")),
            },
            glyph,
            tone: Tone::from_state(item.state.label()),
            state_label: None,
            number: item.number,
            title: item.title.clone(),
            title_tooltip: item.title.clone(),
            repository: RepositoryLink::Whole {
                label,
                action: open_repository,
            },
            updated_label,
            updated_tooltip: item.updated_at.clone(),
            aria: format!(
                "{} {} #{}, {}, {}",
                glyph.label(),
                item.title.trim(),
                item.number,
                item.state.label(),
                exact_time(&item.updated_at)
            ),
            selected,
            show_selected_bar: false,
            on_row_click: Some(select),
            on_title_click: open_detail,
            // The icon and number stay decorative here: this list already exposes the title as
            // its single activation target.
            on_identity_click: None,
        },
        cx,
    )
}
