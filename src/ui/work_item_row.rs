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
pub(crate) const WORK_ITEM_ROW_HEIGHT: f32 = 72.0;

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
    /// Whether the cell takes the leftover metadata width (the repository name).
    flex: bool,
}

fn action_link(cell: LinkCell, action: RowAction, cx: &App) -> AnyElement {
    let LinkCell {
        id,
        debug,
        label,
        aria,
        tooltip,
        flex,
    } = cell;
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
    if flex {
        element = element.flex_1();
    }
    if let Some(debug) = debug {
        element = element.debug_selector(move || debug);
    }
    if let Some(tooltip) = tooltip {
        element = element.tooltip(crate::ui::tooltip::ui_tooltip(tooltip));
    }
    element
        // Native truncation, not Ely `Ellipsis`: same reason as the title (L017).
        .truncate()
        .child(label)
        .into_any_element()
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
    // action that does nothing. Geometry (flex_1/min_w_0/weight/truncate) is shared either way.
    let title = if let Some(action) = row.on_title_click {
        div()
            .id(SharedString::from(selectors.title.clone()))
            .role(gpui::Role::Link)
            .aria_label(row.title.clone())
            .tab_index(0)
            .flex_1()
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
            .tooltip(crate::ui::tooltip::ui_tooltip(row.title_tooltip.clone()))
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
            .flex_1()
            .min_w_0()
            .font_weight(gpui::FontWeight::MEDIUM)
            .truncate()
            .tooltip(crate::ui::tooltip::ui_tooltip(row.title_tooltip.clone()))
            .debug_selector({
                let debug = selectors.title.clone();
                move || debug.clone()
            })
            .child(row.title.clone())
    };

    // Both lines share one explicit line box: GPUI's default line height is a *font* metric
    // (phi × size, resolved per run), so the mono `#number` and the UI-font repository cells
    // measured different line boxes and `items_center` only centred those unequal boxes against
    // each other — the painted glyphs were still offset. A fixed line height in rems gives every
    // cell the same box under the window's own font scale, and centring equal boxes also aligns
    // the painted text. 1.5 is a deliberate, slightly tighter leading than GPUI's phi default so
    // the two lines fit the 72px row with balanced slack.
    let title_line_height = gpui::rems(body.0 * 1.5);
    let metadata_line_height = gpui::rems(tiny.0 * 1.5);
    let mut metadata = div()
        .flex()
        .items_center()
        .line_height(metadata_line_height)
        .gap_2()
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
    // The repository part stays a direct child of the metadata line (never a nested flex row: a
    // nested row inside a baseline-aligned parent collapses to zero height and would make the
    // links unclickable).
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
            if !owner.trim().is_empty() {
                metadata = metadata
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
                    .child(div().flex_none().child("/"));
            }
            metadata = metadata.child(action_link(
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
        .tooltip(crate::ui::tooltip::ui_tooltip(row.updated_tooltip.clone()))
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
        .child(title);
    if let Some(debug) = selectors.title_line.clone() {
        title_line = title_line.debug_selector(move || debug);
    }

    // Centre the two-line block inside the fixed row: `flex_col` + `gap_1` alone packed both
    // lines at the top, leaving all the slack (72 − 2×8 padding − the two line boxes − the gap)
    // beneath the metadata, which is exactly the lopsided selected fill the owner rejected.
    // `justify_center` puts half the slack above the title and half below the metadata, so the
    // rounded active background is balanced around the content at every width.
    let mut container = div()
        .id(SharedString::from(selectors.row.clone()))
        .aria_label(row.aria)
        .w_full()
        .h(px(WORK_ITEM_ROW_HEIGHT))
        .px_2()
        .py_2()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
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
