//! The application-level session tab strip drawn at the top of the island.
//!
//! Every tab is one workspace session. Tabs keep one readable width and never compress: once they
//! no longer fit, the scrolling viewport scrolls horizontally instead of shrinking labels, wrapping
//! or widening the island. The scrolling is GPUI's own horizontal overflow on a [`ScrollHandle`],
//! so the wheel path is the framework's: a vertical wheel maps onto the only scrollable axis,
//! Windows already turns Shift+wheel into a horizontal delta, and a device's native horizontal
//! delta arrives as one, so nothing is converted twice.
//!
//! Layout: one inset on every edge of the strip's own box, then a row that holds the scrolling
//! viewport and a pinned toolbar. The toolbar sits outside the viewport, so it neither scrolls nor
//! hides behind the last tab, and the viewport (and therefore the scrollbar and every drag
//! coordinate) is measured without it.
//!
//! A tab is a tab stop with its own focus handle, so keyboard navigation reaches it and Enter/Space
//! activates it. Middle-clicking anywhere in a tab closes it exactly once and does not activate it;
//! only the tab registers that handler, so the nested close button cannot run a second close. The
//! close affordance itself is shown while the tab is hovered or active, with no fade, and its slot
//! is always reserved so nothing shifts.
//!
//! Dragging a tab horizontally asks the shell to reorder the session collection by [`TabId`];
//! nothing is closed, recreated or re-identified.

use std::rc::Rc;

use ely_gpui_component::{
    buttons::{ButtonVariant, IconButton},
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, ElementId, FocusHandle, FontWeight, IntoElement, MouseButton, ParentElement,
    Role, ScrollHandle, StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
    transparent_black,
};

use crate::workspace::{Route, TabId};

use super::geometry::{
    TAB_CLOSE_SIZE, TAB_GAP, TAB_HEIGHT, TAB_ICON_GAP, TAB_SCROLLBAR_BAND, TAB_STRIP_HEIGHT,
    TAB_STRIP_INSET, TAB_TOOLBAR_WIDTH, TAB_WIDTH,
};

pub(super) type Run = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
/// A strip callback that reports one boolean, such as the pointer entering or leaving.
pub(super) type OnFlag = Rc<dyn Fn(bool, &mut Window, &mut App) + 'static>;
/// A strip callback that reports which session something happened to.
pub(super) type OnTab = Rc<dyn Fn(TabId, &mut Window, &mut App) + 'static>;
/// A strip callback that also carries the pointer position, used while a drag moves.
pub(super) type OnTabAt =
    Rc<dyn Fn(TabId, gpui::Point<gpui::Pixels>, &mut Window, &mut App) + 'static>;

/// One session as the shell has resolved it for this frame.
pub(super) struct Entry {
    pub id: TabId,
    pub index: usize,
    pub title: String,
    pub icon: IconName,
    pub active: bool,
    pub focus: FocusHandle,
    pub select: Run,
    pub close: Run,
}

/// A tab on its way, as GPUI's drag payload carries it.
pub(super) struct TabDrag {
    pub id: TabId,
    /// Which strip produced the drag, so another strip's drag is ignored.
    pub owner: gpui::EntityId,
}

/// Everything the strip needs for one frame.
pub(super) struct View {
    pub entries: Vec<Entry>,
    pub scroll: ScrollHandle,
    /// The tab being dragged, if any: it stays in place but is drawn as the lifted item.
    pub dragging: Option<TabId>,
    /// Where the dragged tab would land, as an insertion slot between the tabs currently shown.
    /// This is the same coordinate the marker is drawn in, so the line matches the drop.
    pub drop_at: Option<usize>,
    pub toolbar: AnyElement,
    pub on_hover: OnFlag,
    pub on_drag_start: OnTab,
    pub on_drag_move: OnTabAt,
    pub on_drop: OnTab,
}

/// The icon a session tab shows for its page kind.
pub(super) fn route_icon(route: &Route) -> IconName {
    match route {
        Route::Home => IconName::House,
        Route::Settings => IconName::Settings,
        Route::Profile => IconName::User,
        Route::Notifications => IconName::Bell,
        Route::Issues => IconName::CircleDot,
        Route::PullRequests => IconName::GitPullRequest,
        Route::Repository { .. } => IconName::Folder,
    }
}

/// The strip: the inset box, the scrolling viewport with its scrollbar, and the pinned toolbar.
///
/// `owner` identifies this strip's drag payloads, so a payload from another strip is ignored.
pub(super) fn strip(
    view: View,
    owner: gpui::EntityId,
    scrollbar: AnyElement,
    cx: &App,
) -> AnyElement {
    let entries = &view.entries;
    // Only the tabs are children of the scrolling row. The drop marker is an overlay, so showing
    // it never shifts a tab and the slot arithmetic (`index × step`) stays exact.
    // The drop is accepted by the whole scrolling viewport, not by individual tabs: the marked
    // insertion positions are the gaps between tabs and the empty space after the last one, and a
    // release there must apply the reorder the marker showed. The pinned toolbar is outside this
    // element, so it can never be a drop target.
    let on_drop = view.on_drop.clone();
    let rows = div()
        .id("tab-scroll")
        .debug_selector(|| "tab-scroll".to_string())
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(TAB_GAP))
        .h(px(TAB_HEIGHT))
        .min_w_0()
        // The only scrollable axis: a vertical wheel therefore moves this row sideways, and a
        // native horizontal delta (or Windows' Shift+wheel) moves it by itself.
        .overflow_x_scroll()
        .track_scroll(&view.scroll)
        .on_drop(move |drag: &TabDrag, window, cx| {
            if drag.owner != owner {
                return;
            }
            on_drop(drag.id, window, cx);
        })
        .children(entries.iter().map(|entry| tab(entry, &view, owner, cx)));

    let marker = view
        .dragging
        .zip(view.drop_at)
        .map(|(_, slot)| insertion_marker(slot, view.scroll.offset().x, cx));

    let scrollbar_band = div()
        .debug_selector(|| "tab-scrollbar-band".to_string())
        .relative()
        .flex_none()
        .w_full()
        .h(px(TAB_SCROLLBAR_BAND))
        .child(scrollbar);

    let viewport = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .relative()
        .child(rows)
        .children(marker)
        .child(scrollbar_band);

    // The hover zone is the whole strip, so the inset edges count as "over the tab bar" too.
    let on_hover = view.on_hover.clone();
    div()
        .id("tab-strip-box")
        .debug_selector(|| "tab-strip".to_string())
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .h(px(TAB_STRIP_HEIGHT))
        .w_full()
        .min_w_0()
        .pt(px(TAB_STRIP_INSET))
        .pl(px(TAB_STRIP_INSET))
        .pr(px(TAB_STRIP_INSET))
        .overflow_hidden()
        .on_hover(move |hovered, window, cx| on_hover(*hovered, window, cx))
        .child(viewport)
        .child(
            div()
                .debug_selector(|| "tab-toolbar".to_string())
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                // Height matches the scroll viewport so the toolbar's button lines up with the
                // tabs rather than the scrollbar band.
                .h(px(TAB_HEIGHT + TAB_SCROLLBAR_BAND))
                .w(px(TAB_TOOLBAR_WIDTH))
                .child(view.toolbar),
        )
        .into_any_element()
}

/// The toolbar pinned to the strip's right edge. It holds one placeholder action this round: no
/// refresh logic, request or navigation is attached, and the button is disabled so it does not
/// suggest the capability already exists.
pub(super) fn toolbar() -> AnyElement {
    div()
        .debug_selector(|| "tab-toolbar".to_string())
        .flex()
        .items_center()
        .justify_center()
        .child(
            div().debug_selector(|| "tab-refresh".to_string()).child(
                IconButton::new("tab-refresh", IconName::RotateCw)
                    .size(ControlSize::Sm)
                    .variant(ButtonVariant::Ghost)
                    // Disabled: this round adds the entry, not the refresh it runs later.
                    .disabled(true)
                    .tooltip("Refresh"),
            ),
        )
        .into_any_element()
}

/// How close to the viewport's edge a drag must be held before the row scrolls itself.
pub(super) const EDGE_ZONE: f32 = 24.0;
/// How far one auto-scroll frame moves the row.
pub(super) const EDGE_STEP: f32 = 12.0;

/// The landing slot for a pointer at `pointer_x` inside the viewport, given the row's current
/// horizontal `offset` (negative once scrolled). `count` is the number of tabs.
///
/// The result is an insertion index: `0` means "before the first tab", `count` means "after the
/// last one". Everything is measured against the scrolling viewport, so the pinned toolbar can
/// never be a target.
pub(super) fn landing_slot(pointer_x: gpui::Pixels, offset: gpui::Pixels, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let step = TAB_WIDTH + TAB_GAP;
    // Row position of the pointer, independent of how far the row is scrolled.
    let row_x: f32 = (pointer_x - offset).into();
    let mut slot = 0;
    let mut edge = 0.0;
    for index in 0..count {
        let middle = edge + TAB_WIDTH / 2.0;
        if row_x >= middle {
            slot = index + 1;
        }
        edge += step;
    }
    slot
}

/// A thin line showing where a dropped tab would land. It is positioned from the slot index and
/// the row's own offset, so it sits exactly in the gap the drop would use.
fn insertion_marker(slot: usize, offset: gpui::Pixels, cx: &App) -> AnyElement {
    let step = TAB_WIDTH + TAB_GAP;
    // Before the first tab, between two tabs, or after the last one.
    let along = if slot == 0 {
        -TAB_GAP / 2.0
    } else {
        slot as f32 * step - TAB_GAP - TAB_GAP / 2.0
    };
    let x: f32 = (gpui::px(along) + offset).into();
    div()
        .debug_selector(move || format!("tab-drop-{slot}"))
        .absolute()
        .left(px(x))
        .top_0()
        .w(px(2.0))
        .h(px(TAB_HEIGHT))
        .rounded_full()
        .bg(cx.theme().colors.accent)
        .into_any_element()
}

fn tab(entry: &Entry, view: &View, owner: gpui::EntityId, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (fg, muted, hover, active_bg) = (colors.fg, colors.fg_muted, colors.hover, colors.active);
    let (radius, text) = (theme.radius(Radius::Md), theme.text_size(TextSize::Sm));
    let active = entry.active;
    let dragging = view.dragging == Some(entry.id);
    let title = entry.title.clone();
    let tooltip = entry.title.clone();
    let id = entry.id;
    let index = entry.index;
    let close_label = format!("Close {title}");
    let icon_color = if active { fg } else { muted };
    let element_id: ElementId = ("workspace-tab", id.get()).into();
    let group: gpui::SharedString = format!("workspace-tab-{index}").into();

    let select = entry.select.clone();
    let close = entry.close.clone();
    let middle_close = entry.close.clone();
    let start = view.on_drag_start.clone();
    let moved = view.on_drag_move.clone();
    let payload = TabDrag { id, owner };

    div()
        .id(element_id)
        .debug_selector(move || format!("tab-{index}"))
        .role(Role::Tab)
        .aria_selected(active)
        .aria_label(title.clone())
        .group(group.clone())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(TAB_ICON_GAP))
        .w(px(TAB_WIDTH))
        .h(px(TAB_HEIGHT))
        .pl(px(8.0))
        .pr(px(4.0))
        .rounded(radius)
        // A transparent border keeps the box the same size in every state; it never takes the
        // theme's focus colour.
        .border_1()
        .border_color(transparent_black())
        .text_size(text)
        .text_color(if active { fg } else { muted })
        .when(active, |tab| {
            tab.bg(active_bg).font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |tab| tab.hover(move |style| style.bg(hover)))
        // Keyboard focus is a neutral foreground change on top of whatever background the tab
        // already has, so a focused inactive tab still does not look active.
        .focus(move |style| style.text_color(fg))
        .cursor_pointer()
        .track_focus(&entry.focus)
        .on_click(move |_, window, cx| select(window, cx))
        // Middle click closes; `on_aux_click` only receives non-primary buttons, so it never runs
        // the selection, and this is the only aux handler in the tab subtree.
        .on_aux_click(move |event, window, cx| {
            if event.is_middle_click() {
                middle_close(window, cx);
            }
        })
        .when(dragging, |tab| tab.opacity(0.45))
        // Drag handlers are always registered: this `on_drag` callback is what starts a reorder,
        // so gating it on a drag already being in progress would make it unreachable.
        .on_drag(payload, move |drag, _, window, cx| {
            start(drag.id, window, cx);
            cx.new(|_| gpui::EmptyView)
        })
        .on_drag_move(move |event: &gpui::DragMoveEvent<TabDrag>, window, cx| {
            if event.drag(cx).owner != owner {
                return;
            }
            // Hover is left to the strip's own hover callback, which reports the real pointer
            // position. The drag's own flag, not a forced hover, keeps the scrollbar visible.
            moved(event.drag(cx).id, event.event.position, window, cx);
        })
        .tooltip(Tooltip::text(tooltip))
        .child(Icon::new(entry.icon).size(IconSize::Xs).color(icon_color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(Ellipsis::new(title)),
        )
        .child(
            // The slot is always present and keeps its width, so showing or hiding the button
            // never moves the label or the neighbouring tabs. Only the button itself is hidden.
            div()
                .id(("workspace-tab-close", id.get()))
                .debug_selector(move || format!("tab-close-{index}"))
                .role(Role::Button)
                .aria_label(close_label)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(TAB_CLOSE_SIZE))
                .rounded(theme.radius(Radius::Sm))
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                // Shown while the tab is hovered or active, instantly and without a fade. A
                // keyboard focus is deliberately not a third condition.
                .when(!active, |button| {
                    button
                        .invisible()
                        .group_hover(group.clone(), |style| style.visible())
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    close(window, cx);
                })
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .tooltip(Tooltip::text("Close tab"))
                .child(Icon::new(IconName::X).size(IconSize::Xs).color(muted)),
        )
        .into_any_element()
}
