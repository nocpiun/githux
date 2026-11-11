//! One work-item row presented identically by Home's Involved column and the global Issues /
//! Pull Requests lists.
//!
//! Both callers used to render their own approximation of the same row (68px vs 72px height,
//! different metadata order and icon colour, one `Ellipsis` and one native truncation). This
//! module owns the presentation so the two surfaces cannot drift again. It is deliberately
//! presentation-only: the callers keep their own data models, account/authorization contexts,
//! typed navigation and keyboard handling, and hand in display strings plus callbacks.
//!
//! Metadata order is `#number → repository → state → relative time`; the number stays first
//! (owner-requested) instead of moving to the right when reused elsewhere.

#[cfg(test)]
#[path = "work_item_row_tests.rs"]
mod tests;

use ely_core::{
    primitives::{FocusRing, Icon, IconName},
    theme::{ActiveTheme, IconSize, Radius, TextSize},
};
use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use std::rc::Rc;

/// The single row height both lists reserve in their virtual list state.
pub(crate) const WORK_ITEM_ROW_HEIGHT: f32 = 56.0;

/// A plain action: the row itself, a link or the row-select callback.
pub(crate) type RowAction = Rc<dyn Fn(&mut Window, &mut App)>;
/// A title activation, told whether the click came from the pointer so the caller can keep
/// "pointer selects then activates, keyboard only activates".
pub(crate) type TitleAction = Rc<dyn Fn(bool, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph {
    Issue,
    PullRequest,
}

impl Glyph {
    /// The short noun used in fallback titles and accessible names.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Issue => "Issue",
            Self::PullRequest => "PR",
        }
    }
}

/// Semantic state. The colour comes from the theme's severity tokens, never from a fixed colour,
/// so Open/Closed/Merged/Unknown stay distinguishable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Open,
    Closed,
    Merged,
    Unknown,
}

impl Tone {
    /// Map a GitHub state string; anything unrecognised is Unknown rather than silently "open".
    pub(crate) fn from_state(state: &str) -> Self {
        match state.trim().to_ascii_lowercase().as_str() {
            "open" => Self::Open,
            "closed" => Self::Closed,
            "merged" => Self::Merged,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Closed => "Closed",
            Self::Merged => "Merged",
            Self::Unknown => "Unknown",
        }
    }
}

/// Debug selectors. Both lists keep their existing selector names so their own tests and any
/// operator tooling keep addressing the same elements.
#[derive(Clone, Debug)]
pub(crate) struct Selectors {
    pub row: String,
    pub title_line: Option<String>,
    pub icon: Option<String>,
    pub title: String,
    pub metadata: Option<String>,
    pub number: Option<String>,
    pub repository: Option<String>,
    pub owner: Option<String>,
    pub updated: Option<String>,
}

/// How the repository part of the metadata line is presented.
pub(crate) enum RepositoryLink {
    /// One link carrying `owner/name` (global lists).
    Whole { label: String, action: RowAction },
    /// An owner profile link, a separator, then the repository link (Home).
    Split {
        owner: String,
        owner_action: RowAction,
        name: String,
        name_action: RowAction,
    },
}

pub(crate) struct Row {
    pub selectors: Selectors,
    pub glyph: Glyph,
    pub tone: Tone,
    pub number: u64,
    pub title: String,
    /// Full title for the tooltip; the visible title truncates.
    pub title_tooltip: String,
    pub repository: RepositoryLink,
    pub updated_label: String,
    /// Exact timestamp for the tooltip and accessible name only.
    pub updated_tooltip: String,
    pub aria: String,
    pub selected: bool,
    pub on_row_click: Option<RowAction>,
    pub on_title_click: Option<TitleAction>,
}

/// One linking text cell.
struct LinkCell {
    /// Stable element id.
    id: String,
    /// GUI test-platform selector, when the calling list names its elements.
    debug: Option<String>,
    label: String,
    aria: String,
    tooltip: Option<String>,
    /// Whether a noninteractive slot takes the leftover metadata width.
    flex: bool,
}

/// Keep observation attached to the real text node's hitbox in renderer tests. The
/// observer changes no geometry or event handling and is absent from release builds.
pub(crate) fn text_target<E: StatefulInteractiveElement>(target: E, _id: String) -> E {
    #[cfg(test)]
    {
        target.on_hover(move |hovered, _, cx| {
            if cx.has_global::<TextHover>() {
                let state = cx.global_mut::<TextHover>();
                if *hovered {
                    state.0.insert(_id.clone());
                } else {
                    state.0.remove(&_id);
                }
            }
        })
    }
    #[cfg(not(test))]
    {
        target
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TextHover(pub std::collections::HashSet<String>);
#[cfg(test)]
impl gpui::Global for TextHover {}

fn action_link(cell: LinkCell, action: RowAction, cx: &App) -> AnyElement {
    let LinkCell {
        id,
        debug,
        label,
        aria,
        tooltip,
        flex,
    } = cell;
    let hover_id = id.clone();
    let tooltip_id = format!("{id}-tooltip");
    let mut element = div()
        .id(SharedString::from(id))
        .role(gpui::Role::Link)
        .aria_label(aria)
        .tab_index(0)
        .min_w_0()
        .overflow_hidden()
        .cursor_pointer()
        .focus_ring(cx)
        .hover(|style| style.underline())
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.prevent_default()
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            action(window, cx);
        });
    if let Some(debug) = debug {
        element = element.debug_selector(move || debug);
    }
    if let Some(tooltip) = tooltip {
        element = element.tooltip(text_tooltip(tooltip, tooltip_id));
    }
    element = element
        // Native truncation, not Ely `Ellipsis`: same reason as the title (L017).
        .truncate()
        .child(label);

    if !flex {
        // The owner must not consume the entire repository slot on a narrow list.
        element = element.max_w(gpui::relative(0.45));
    }
    let element = text_target(element, hover_id);
    if flex {
        div()
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(element)
            .into_any_element()
    } else {
        element.into_any_element()
    }
}

/// Named tooltip content lets renderer regressions observe the actual delayed tooltip,
/// using the same builder in production and tests.
pub(crate) fn text_tooltip(
    text: String,
    selector: String,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    ely_core::primitives::Tooltip::rich(move |_, cx| {
        crate::ui::tooltip::ui_tooltip_content(text.clone(), cx)
            .debug_selector({
                let selector = selector.clone();
                move || selector
            })
            .into_any_element()
    })
}

pub(crate) fn render(row: Row, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let radius = theme.radius(Radius::Md);
    let tiny = theme.text_size(TextSize::Xs);
    let mono = theme.mono_family.clone();
    let body = theme.text_size(TextSize::Base);
    let icon = match row.glyph {
        Glyph::Issue => IconName::CircleDot,
        Glyph::PullRequest => IconName::GitPullRequest,
    };
    let color = tone_color(row.tone, &colors);
    let selectors = row.selectors.clone();

    // A title is a link only when the caller actually supplied an activation. Global Issues/PR
    // deliberately have no detail navigation, so their titles must stay plain text: giving them
    // a Link role, pointer cursor and a Tab stop would promise keyboard/screen-reader users an
    // action that does nothing. Both variants use the same bounded flex slot, weight and truncation.
    let title = if let Some(action) = row.on_title_click {
        div()
            .id(SharedString::from(selectors.title.clone()))
            .role(gpui::Role::Link)
            .aria_label(row.title.clone())
            .tab_index(0)
            .min_w_0()
            .cursor_pointer()
            .focus_ring(cx)
            .font_weight(gpui::FontWeight::MEDIUM)
            .hover(|style| style.underline())
            // GPUI's native truncation instead of Ely `Ellipsis`: the latter's hidden full-text
            // measurement widens a `ListState` item to the whole string (see LESSONS L017).
            .truncate()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.prevent_default()
            })
            .tooltip(text_tooltip(
                row.title_tooltip.clone(),
                format!("{}-tooltip", selectors.title),
            ))
            .debug_selector({
                let debug = selectors.title.clone();
                move || debug.clone()
            })
            .on_click(move |event: &ClickEvent, window, cx| {
                cx.stop_propagation();
                let pointer = !matches!(event, ClickEvent::Keyboard(_));
                action(pointer, window, cx);
            })
            .child(row.title.clone())
    } else {
        div()
            .id(SharedString::from(selectors.title.clone()))
            .min_w_0()
            .font_weight(gpui::FontWeight::MEDIUM)
            .truncate()
            .tooltip(text_tooltip(
                row.title_tooltip.clone(),
                format!("{}-tooltip", selectors.title),
            ))
            .debug_selector({
                let debug = selectors.title.clone();
                move || debug.clone()
            })
            .child(row.title.clone())
    };
    let title = text_target(title, selectors.title.clone());

    // Explicit line boxes prevent the mono number and UI labels from acquiring different
    // heights. At the default theme the two lines plus the 3px gap occupy 39px inside a
    // 56px item. Painted glyph extents are checked separately with the bundled font;
    // equal boxes alone do not establish optical balance.
    let title_line_height = gpui::rems(body.0 * 1.5);
    let metadata_line_height = gpui::rems(tiny.0 * 1.5);
    let mut metadata = div()
        .flex()
        .items_center()
        .line_height(metadata_line_height)
        .gap_2()
        .pl(gpui::rems(theme.icon_size(IconSize::Sm).0 + 0.5))
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .text_size(tiny)
        .text_color(colors.fg_subtle);
    if let Some(debug) = selectors.metadata.clone() {
        metadata = metadata.debug_selector(move || debug);
    }
    let mut number = div()
        .flex_none()
        .font_family(mono)
        .child(format!("#{}", row.number));
    if let Some(debug) = selectors.number.clone() {
        number = number.debug_selector(move || debug);
    }
    metadata = metadata.child(number);
    // Only the repository slot grows. Its nested horizontal flex rows keep each link at
    // its intrinsic text width (or its truncated width), leaving the trailing slack inert.
    match row.repository {
        RepositoryLink::Whole { label, action } => {
            metadata = metadata.child(action_link(
                LinkCell {
                    id: selectors
                        .repository
                        .clone()
                        .unwrap_or_else(|| format!("{}-repository", selectors.row)),
                    debug: selectors.repository.clone(),
                    label: label.clone(),
                    aria: format!("Open repository {label}"),
                    tooltip: Some(label),
                    flex: true,
                },
                action,
                cx,
            ));
        }
        RepositoryLink::Split {
            owner,
            owner_action,
            name,
            name_action,
        } => {
            // Resolve the owner's percentage cap against available metadata space,
            // not the group's intrinsic owner/repo width (which truncates normal
            // owner-longer-than-repo pairs even in a wide row).
            let mut repo_cell = div().flex().items_center().flex_1().min_w_0().overflow_hidden();
            if !owner.trim().is_empty() {
                repo_cell = repo_cell
                    .child(action_link(
                        LinkCell {
                            id: selectors
                                .owner
                                .clone()
                                .unwrap_or_else(|| format!("{}-owner", selectors.row)),
                            debug: selectors.owner.clone(),
                            label: owner.clone(),
                            aria: owner.clone(),
                            tooltip: None,
                            flex: false,
                        },
                        owner_action,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .debug_selector({
                                let debug = format!("{}-separator", selectors.row);
                                move || debug
                            })
                            .child("/"),
                    );
            }
            repo_cell = repo_cell.child(action_link(
                LinkCell {
                    id: selectors
                        .repository
                        .clone()
                        .unwrap_or_else(|| format!("{}-repository", selectors.row)),
                    debug: selectors.repository.clone(),
                    label: name.clone(),
                    aria: format!("Open repository {owner}/{name}"),
                    tooltip: Some(format!("{owner}/{name}")),
                    flex: true,
                },
                name_action,
                cx,
            ));
            metadata = metadata.child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(repo_cell),
            );
        }
    }
    metadata = metadata.child(
        div()
            .flex_none()
            .debug_selector({
                let debug = format!("{}-state", selectors.row);
                move || debug.clone()
            })
            .child(row.tone.label()),
    );
    let mut updated = div()
        .id(SharedString::from(format!("{}-updated", selectors.row)))
        .min_w_0()
        .overflow_hidden()
        .tooltip(text_tooltip(
            row.updated_tooltip.clone(),
            format!("{}-updated-tooltip", selectors.row),
        ))
        .aria_label(format!("Updated {}", row.updated_tooltip))
        .truncate()
        .child(row.updated_label);
    if let Some(debug) = selectors.updated.clone() {
        updated = updated.debug_selector(move || debug);
    }
    metadata = metadata.child(updated);

    let mut title_line = div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .line_height(title_line_height)
        .text_size(body)
        .child({
            let mut icon_slot = div()
                .flex_none()
                .flex()
                .items_center()
                .child(Icon::new(icon).size(IconSize::Sm).color(color));
            if let Some(debug) = selectors.icon.clone() {
                icon_slot = icon_slot.debug_selector(move || debug);
            }
            icon_slot
        })
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(title),
        );
    if let Some(debug) = selectors.title_line.clone() {
        title_line = title_line.debug_selector(move || debug);
    }

    // The same element owns the list item's measured height and active fill. Compact padding
    // replaces the old 72px item; no viewport/gutter compensation belongs in this renderer.
    let mut container = div()
        .id(SharedString::from(selectors.row.clone()))
        .aria_label(row.aria)
        .w_full()
        .h(px(WORK_ITEM_ROW_HEIGHT))
        .px_2()
        // Noto Sans SC's painted ink sits below the middle of its line box. One logical
        // pixel of optical lift balances the raster extents; both lines and targets move together.
        .pt(px(7.0))
        .pb(px(9.0))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(3.0))
        .min_w_0()
        .overflow_hidden()
        .rounded(radius)
        .bg(if row.selected {
            colors.active
        } else {
            gpui::transparent_black()
        })
        .debug_selector({
            let debug = selectors.row.clone();
            move || debug.clone()
        });
    if let Some(action) = row.on_row_click {
        container = container.on_click(move |_, window, cx| {
            cx.stop_propagation();
            action(window, cx);
        });
    }
    container
        .child(title_line)
        .child(metadata)
        .into_any_element()
}

fn tone_color(tone: Tone, colors: &ely_core::theme::Palette) -> gpui::Hsla {
    match tone {
        Tone::Open => colors.success,
        Tone::Closed => colors.fg_subtle,
        Tone::Merged => colors.accent,
        Tone::Unknown => colors.warning,
    }
}
