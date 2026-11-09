//! One work-item row presented identically by Home's Involved column and the global Issues /
//! Pull Requests lists.
//!
//! Metadata order is `#number → repository → state → relative time`; the number stays first.

use crate::ui::foundation::{
    controls::{self, Icon},
    theme as t,
};
use nsh_core::prelude::*;
use std::rc::Rc;

pub const WORK_ITEM_ROW_HEIGHT: f32 = 72.0;

pub type RowAction = Rc<dyn Fn()>;
pub type TitleAction = Rc<dyn Fn(bool)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Issue,
    PullRequest,
}

impl Glyph {
    pub fn label(self) -> &'static str {
        match self {
            Self::Issue => "Issue",
            Self::PullRequest => "PR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Open,
    Closed,
    Merged,
    Unknown,
}

impl Tone {
    pub fn from_state(state: &str) -> Self {
        match state.trim().to_ascii_lowercase().as_str() {
            "open" => Self::Open,
            "closed" => Self::Closed,
            "merged" => Self::Merged,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Closed => "Closed",
            Self::Merged => "Merged",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Selectors {
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

pub enum RepositoryLink {
    Whole {
        label: String,
        action: RowAction,
    },
    Split {
        owner: String,
        owner_action: RowAction,
        name: String,
        name_action: RowAction,
    },
}

pub struct Row {
    pub selectors: Selectors,
    pub glyph: Glyph,
    pub tone: Tone,
    pub number: u64,
    pub title: String,
    pub title_tooltip: String,
    pub repository: RepositoryLink,
    pub updated_label: String,
    pub updated_tooltip: String,
    pub aria: String,
    pub selected: bool,
    pub on_row_click: Option<RowAction>,
    pub on_title_click: Option<TitleAction>,
}

fn action_link(
    label: String,
    aria: String,
    tooltip: Option<String>,
    action: RowAction,
    flex: bool,
) -> View {
    let mut v = view()
        .role(AccessibilityRole::Link)
        .label(aria)
        .cursor_pointer()
        .focusable(true)
        .on_press(move |_: &()| action());
    if flex {
        v = v.flex_1();
    }
    if let Some(tip) = tooltip {
        v = v.tooltip(controls::tooltip(tip));
    }
    v.child(
        t::label(label)
            .text_size(px(12.0))
            .text_color(t::theme().muted_foreground)
            .hover(|s| s.underline()),
    )
}

pub fn render(row: Row) -> AnyElement {
    let theme = t::theme();
    let glyph_icon = match row.glyph {
        Glyph::Issue => Icon::Issue,
        Glyph::PullRequest => Icon::PullRequest,
    };
    let tone_color = match row.tone {
        Tone::Open => theme.foreground,
        Tone::Closed => theme.muted_foreground,
        Tone::Merged => theme.primary,
        Tone::Unknown => theme.destructive,
    };

    let title_node = if let Some(action) = row.on_title_click {
        let title_clone = row.title.clone();
        view()
            .flex_1()
            .cursor_pointer()
            .focusable(true)
            .tooltip(controls::tooltip(row.title_tooltip))
            .on_press(move |_: &()| {
                action(true);
            })
            .child(
                t::label(title_clone)
                    .text_size(px(14.0))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| s.underline()),
            )
    } else {
        view()
            .flex_1()
            .tooltip(controls::tooltip(row.title_tooltip))
            .child(
                t::label(row.title)
                    .text_size(px(14.0))
                    .font_weight(FontWeight::MEDIUM),
            )
    };

    let title_line = view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            view()
                .size(px(16.0))
                .items_center()
                .justify_center()
                .text_color(tone_color)
                .child(controls::icon(glyph_icon)),
        )
        .child(title_node);

    let mut metadata = view().flex_row().items_center().gap_2().child(
        t::code(format!("#{}", row.number))
            .text_size(px(12.0))
            .text_color(theme.muted_foreground),
    );

    match row.repository {
        RepositoryLink::Whole { label, action } => {
            metadata = metadata.child(action_link(
                label.clone(),
                format!("Open repository {label}"),
                Some(label),
                action,
                true,
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
                        owner.clone(),
                        owner.clone(),
                        None,
                        owner_action,
                        false,
                    ))
                    .child(
                        t::label("/")
                            .text_size(px(12.0))
                            .text_color(theme.muted_foreground),
                    );
            }
            metadata = metadata.child(action_link(
                name.clone(),
                format!("Open repository {owner}/{name}"),
                Some(format!("{owner}/{name}")),
                name_action,
                true,
            ));
        }
    }

    metadata = metadata
        .child(
            t::label(row.tone.label())
                .text_size(px(12.0))
                .text_color(theme.muted_foreground),
        )
        .child(
            view()
                .tooltip(controls::tooltip(row.updated_tooltip))
                .child(
                    t::label(row.updated_label)
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground),
                ),
        );

    let mut container = view()
        .h(px(WORK_ITEM_ROW_HEIGHT))
        .px_2()
        .py_2()
        .gap_1()
        .justify_center()
        .rounded(theme.radius)
        .overflow_hidden()
        .bg(if row.selected {
            theme.secondary
        } else {
            AdaptiveColor::TRANSPARENT
        })
        .test_id(&row.selectors.row)
        .label(row.aria);

    if let Some(on_click) = row.on_row_click {
        container = container
            .cursor_pointer()
            .on_press(move |_: &()| on_click());
    }

    container.child(title_line).child(metadata).into_element()
}
