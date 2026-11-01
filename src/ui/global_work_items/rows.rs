use super::{RepositoryNavigation, WorkItemsView};
use crate::{data::RepositoryReadContext, domain::work_item::*};
use ely_core::{
    primitives::{FocusRing, Icon, IconName, Tooltip},
    theme::{ActiveTheme, IconSize, TextSize},
};
use gpui::{AnyElement, Context, div, prelude::*, px};

pub(super) const ROW_HEIGHT: f32 = 72.0;

pub(super) fn row(
    item: &WorkItem,
    selected: bool,
    context: Option<RepositoryReadContext>,
    cx: &mut Context<WorkItemsView>,
) -> AnyElement {
    let colors = cx.theme().colors.clone();
    let id = item.id;
    let weak = cx.entity().downgrade();
    let repository = item.repository.clone();
    let label = repository.full_name();
    let link = div()
        .id(format!("work-repo-{id}"))
        .debug_selector(move || format!("work-repo-{id}"))
        .role(gpui::Role::Link)
        .aria_label(label.clone())
        .tab_index(0)
        .focus_ring(cx)
        .min_w_0()
        .flex_1()
        .overflow_hidden()
        .cursor_pointer()
        .hover(|style| style.underline())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
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
        })
        .truncate()
        .child(label);
    let icon = match item.kind {
        WorkItemKind::Issue => IconName::CircleDot,
        WorkItemKind::PullRequest => IconName::GitPullRequest,
    };
    let color = match item.state {
        ItemState::Open => colors.success,
        ItemState::Closed => colors.fg_subtle,
        ItemState::Merged => colors.accent,
        ItemState::Unknown => colors.warning,
    };
    div()
        .id(format!("work-item-{id}"))
        .on_click(cx.listener(move |view, _, window, cx| {
            view.selected = Some(id);
            view.focus.focus(window, cx);
            cx.notify();
        }))
        .debug_selector(move || format!("work-item-{id}"))
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_2()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .min_w_0()
        .overflow_hidden()
        .when(selected, |row| row.bg(colors.active))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .child(Icon::new(icon).size(IconSize::Sm).color(color)),
                )
                .child(
                    div()
                        .id(format!("work-title-{id}"))
                        .debug_selector(move || format!("work-title-{id}"))
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .tooltip(Tooltip::text(item.title.clone()))
                        .truncate()
                        .child(item.title.clone()),
                ),
        )
        .child(
            div()
                .debug_selector(move || format!("work-metadata-{id}"))
                .flex()
                .items_center()
                .gap_2()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .text_size(cx.theme().text_size(TextSize::Xs))
                .text_color(colors.fg_subtle)
                .child(link)
                .child(
                    div()
                        .debug_selector(move || format!("work-number-{id}"))
                        .flex_none()
                        .font_family("Google Sans Code")
                        .child(format!("#{}", item.number)),
                )
                .child(div().flex_none().child(item.state.label()))
                .child(
                    div()
                        .id(format!("work-updated-{id}"))
                        .debug_selector(move || format!("work-updated-{id}"))
                        .min_w_0()
                        .overflow_hidden()
                        .tooltip(Tooltip::text(item.updated_at.clone()))
                        .truncate()
                        .child(format!(
                            "Updated {}",
                            item.updated_at.replace('T', " ").trim_end_matches('Z')
                        )),
                ),
        )
        .into_any_element()
}
