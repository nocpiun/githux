//! The command palette opened from the title bar's fixed search entry.
//!
//! This round the palette is a navigation surface only: the shell's pages and the fixture
//! repositories. It makes no GitHub request and does not decide the future global search scope.
//!
//! Keys are handled by a keystroke interceptor the shell registers while the palette is open
//! (`Shell::open_palette`). That runs before key bindings are matched, which matters for two
//! reasons: the query field's own `Enter` binding emits a submit action and stops the event, so a
//! bubble-phase card listener never sees it; and an input-method composition must be able to claim
//! `Enter` for its candidate list. The interceptor therefore refuses `Enter` while the query field
//! reports a marked (composing) range through GPUI's public `EntityInputHandler` interface.

use std::rc::Rc;

use ely_core::{
    forms::TextInput,
    primitives::{Backdrop, FocusScope, Icon, IconName},
    theme::{ActiveTheme, ControlSize, Elevation, IconSize, Radius, TextSize},
};
use gpui::{
    AnyElement, App, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Window,
    div, prelude::*, px,
};

use crate::ui::fixtures::RepoFixture;

pub(super) type Run = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
pub(super) type Move = Rc<dyn Fn(isize, &mut Window, &mut App) + 'static>;
pub(super) type Pick = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

/// One palette result.
#[derive(Clone)]
pub(super) struct Command {
    pub value: String,
    pub label: String,
    pub group: &'static str,
    pub icon: IconName,
}

/// Every command this round offers: the shell's pages, then the fixture repositories.
pub(super) fn commands(repositories: &[RepoFixture]) -> Vec<Command> {
    let pages = [
        ("nav:home", "Go to Home", IconName::House),
        ("nav:settings", "Go to Settings", IconName::Settings),
        ("nav:profile", "Go to Profile", IconName::User),
        ("nav:notifications", "Go to Notifications", IconName::Bell),
        ("nav:issues", "Go to Issues", IconName::CircleDot),
        (
            "nav:pull-requests",
            "Go to Pull requests",
            IconName::GitPullRequest,
        ),
    ];
    let mut all: Vec<Command> = pages
        .into_iter()
        .map(|(value, label, icon)| Command {
            value: value.to_string(),
            label: label.to_string(),
            group: "Pages",
            icon,
        })
        .collect();
    all.extend(repositories.iter().map(|repo| Command {
        value: format!("repo:{}", repo.full_name()),
        label: repo.full_name(),
        group: "Repositories",
        icon: IconName::Folder,
    }));
    all
}

/// Case-insensitive subsequence score: earlier, tighter matches rank higher; `None` when the query
/// does not fit at all. An empty query fits everything with score 0.
fn score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0;
    let mut from = 0;
    let mut previous: Option<usize> = None;
    for needle in query.chars().flat_map(char::to_lowercase) {
        if needle.is_whitespace() {
            continue;
        }
        let found = hay[from..].iter().position(|ch| *ch == needle)?;
        let at = from + found;
        // A match at a word start, and a match that continues the previous one, score higher;
        // distance between consecutive matches costs.
        let word_start = at == 0 || hay[at - 1] == ' ' || hay[at - 1] == '/' || hay[at - 1] == '-';
        score += if word_start { 10 } else { 4 };
        if let Some(previous) = previous {
            let gap = at.saturating_sub(previous + 1) as i32;
            score -= gap.min(6);
        }
        previous = Some(at);
        from = at + 1;
    }
    Some(score)
}

/// One group's hits: its name, its best score and `(score, command index)` rows.
type RankedGroup = (&'static str, i32, Vec<(i32, usize)>);

/// Result indices for `query`, best first, capped so the list stays short.
///
/// Groups stay contiguous: each group is ranked on its own, groups are ordered by their best hit,
/// and rows keep their group order. Without that, a strong single match would split a group and
/// the headings would repeat.
pub(super) fn results(commands: &[Command], query: &str) -> Vec<usize> {
    const SHOWN: usize = 50;
    let query = query.trim();
    let mut groups: Vec<RankedGroup> = Vec::new();
    for (index, command) in commands.iter().enumerate() {
        let Some(hit) = score(query, &command.label) else {
            continue;
        };
        match groups
            .iter()
            .position(|(name, _, _)| *name == command.group)
        {
            Some(at) => {
                let (_, best, rows) = &mut groups[at];
                *best = (*best).max(hit);
                rows.push((hit, index));
            }
            None => groups.push((command.group, hit, vec![(hit, index)])),
        }
    }
    groups.sort_by(|(a_name, a_best, a_rows), (b_name, b_best, b_rows)| {
        b_best
            .cmp(a_best)
            .then_with(|| a_rows[0].1.cmp(&b_rows[0].1))
            .then_with(|| a_name.cmp(b_name))
    });
    let mut shown = Vec::with_capacity(SHOWN);
    for (_, _, mut rows) in groups {
        rows.sort_by(|(a_score, a), (b_score, b)| {
            b_score
                .cmp(a_score)
                .then_with(|| commands[*a].label.len().cmp(&commands[*b].label.len()))
                .then_with(|| a.cmp(b))
        });
        for (_, index) in rows {
            if shown.len() == SHOWN {
                return shown;
            }
            shown.push(index);
        }
    }
    shown
}

/// One child of the palette's scroll list: a group heading or a result row.
///
/// The list's scroll handle counts *children*, headings included, while the highlight counts
/// results. Keeping both in one layout prevents the two indexes from drifting apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Entry {
    Heading(&'static str),
    Command(usize),
}

/// Every child the card draws for `commands` under `query`, headings in place.
pub(super) fn layout(commands: &[Command], query: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut group: Option<&'static str> = None;
    for index in results(commands, query) {
        let name = commands[index].group;
        if group != Some(name) {
            group = Some(name);
            entries.push(Entry::Heading(name));
        }
        entries.push(Entry::Command(index));
    }
    entries
}

/// The child index to scroll to for the `cursor`th result in `entries`.
pub(super) fn child_index(entries: &[Entry], cursor: usize) -> usize {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, Entry::Command(_)))
        .nth(cursor)
        .map_or(0, |(child, _)| child)
}

/// The palette's live state while it is open.
pub(super) struct Session {
    pub input: Entity<TextInput>,
    pub focus: FocusHandle,
    pub scroll: ScrollHandle,
    pub smooth: super::scrolling::SmoothScroll,
    pub cursor: usize,
    pub commands: Vec<Command>,
    /// The query the highlight was last reset for.
    pub last_query: String,
    /// The query field's change subscription, which drives the shell's redraw.
    pub changed: Option<gpui::Subscription>,
    /// The keystroke interceptor that owns Up, Down, Enter and Escape while the palette is open.
    pub keys: Option<gpui::Subscription>,
}

impl Session {
    pub(super) fn new(window: &mut Window, cx: &mut App, commands: Vec<Command>) -> Self {
        let input = cx.new(|cx| {
            TextInput::new(window, cx)
                .placeholder(SharedString::from("Search pages and repositories"))
        });
        let focus = cx.focus_handle();
        let field = input.read(cx).focus_handle(cx);
        window.focus(&field, cx);
        let scroll = ScrollHandle::new();
        let smooth = super::scrolling::SmoothScroll::new(scroll.clone());
        Self {
            input,
            focus,
            scroll,
            smooth,
            cursor: 0,
            commands,
            last_query: String::new(),
            changed: None,
            keys: None,
        }
    }

    pub(super) fn query(&self, cx: &App) -> String {
        self.input.read(cx).text().to_string()
    }

    /// True while an input method composes text: `Enter` then belongs to its candidate list.
    pub(super) fn composing(&self, window: &mut Window, cx: &mut App) -> bool {
        self.input.update(cx, |input, cx| {
            gpui::EntityInputHandler::marked_text_range(input, window, cx).is_some()
        })
    }

    /// The shown results for the current query.
    pub(super) fn shown(&self, cx: &App) -> Vec<usize> {
        results(&self.commands, &self.query(cx))
    }

    /// Resets the highlight when the query changed, so a new query starts at its best match.
    pub(super) fn sync(&mut self, cx: &App) {
        let query = self.query(cx);
        if query != self.last_query {
            self.last_query = query;
            self.cursor = 0;
            self.scroll.set_offset(gpui::Point::default());
            self.smooth.cancel();
        }
    }

    /// Wraps the highlight by `by` inside `count` and keeps it in view.
    ///
    /// `count` is the number of results; the scroll handle wants the child index in the list that
    /// also holds the group headings, so the caller passes the layout it is drawing.
    pub(super) fn move_cursor(&mut self, by: isize, count: usize, entries: &[Entry]) {
        if count == 0 {
            return;
        }
        let at = self.cursor.min(count - 1) as isize;
        self.cursor = (at + by).rem_euclid(count as isize) as usize;
        let child = child_index(entries, self.cursor);
        // A keyboard move wins over any in-flight wheel easing.
        self.smooth.cancel();
        self.scroll.scroll_to_item(child);
    }
}

/// One frame's card.
pub(super) struct View {
    pub input: Entity<TextInput>,
    pub focus: FocusHandle,
    pub scroll: ScrollHandle,
    pub smooth: super::scrolling::SmoothScroll,
    pub cursor: usize,
    pub commands: Vec<Command>,
    pub on_close: Run,
    pub on_move: Move,
    pub on_pick: Pick,
}

/// Draws the card over a scrim. Tab stays inside it; a click on the scrim closes it.
pub(super) fn render(view: View, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (surface, border, fg, subtle, hover, active, accent) = (
        colors.overlay,
        colors.border,
        colors.fg,
        colors.fg_subtle,
        colors.hover,
        colors.active,
        colors.accent,
    );
    let (radius, text, tiny, row_height) = (
        theme.radius(Radius::Xl),
        theme.text_size(TextSize::Sm),
        theme.text_size(TextSize::Xs),
        theme.control_height(ControlSize::Lg),
    );
    let query = view.input.read(cx).text().to_string();
    let entries = layout(&view.commands, &query);
    let count = entries
        .iter()
        .filter(|entry| matches!(entry, Entry::Command(_)))
        .count();
    let cursor = view.cursor.min(count.saturating_sub(1));
    // Scrolling belongs to the user: the highlight is brought into view when the cursor moves
    // (`Session::move_cursor`), not on every render, or a wheel position would snap back.

    let mut rows: Vec<AnyElement> = Vec::with_capacity(entries.len() + 1);
    let mut position = 0;
    for entry in &entries {
        match entry {
            Entry::Heading(group) => rows.push(
                div()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_size(tiny)
                    .text_color(subtle)
                    .child(*group)
                    .into_any_element(),
            ),
            Entry::Command(index) => {
                let command = &view.commands[*index];
                let selected = position == cursor;
                let on_pick = view.on_pick.clone();
                let on_move = view.on_move.clone();
                rows.push(
                    div()
                        .id(("palette-row", position))
                        .role(Role::ListBoxOption)
                        .aria_selected(selected)
                        .aria_label(command.label.clone())
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .h(row_height)
                        .px_2()
                        .rounded(theme.radius(Radius::Md))
                        .when(selected, |row| row.bg(active))
                        .when(!selected, move |row| {
                            row.hover(move |style| style.bg(hover))
                        })
                        .cursor_pointer()
                        .on_mouse_move(move |_, window, cx| {
                            if position != cursor {
                                on_move(position as isize - cursor as isize, window, cx);
                            }
                        })
                        .on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.prevent_default();
                        })
                        .on_click(move |_, window, cx| on_pick(position, window, cx))
                        .child(Icon::new(command.icon).size(IconSize::Sm).color(subtle))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_color(fg)
                                .child(command.label.clone()),
                        )
                        .into_any_element(),
                );
                position += 1;
            }
        }
    }
    if count == 0 {
        rows.push(
            div()
                .px_2()
                .py_3()
                .text_color(subtle)
                .child("No matching commands")
                .into_any_element(),
        );
    }

    let close_click = view.on_close.clone();
    let card = div()
        .id("palette-card")
        .debug_selector(|| "palette-card".to_string())
        .flex()
        .flex_col()
        .w(px(620.0))
        .h(px(420.0))
        .rounded(radius)
        .border_1()
        .border_color(border)
        .bg(surface)
        .shadow(theme.elevation(Elevation::Modal))
        .text_size(text)
        .text_color(fg)
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(border)
                .child(Icon::new(IconName::Search).size(IconSize::Sm).color(subtle))
                .child(div().flex_1().min_w_0().child(view.input.clone()))
                .child(
                    div()
                        .flex_none()
                        .rounded(theme.radius(Radius::Sm))
                        .border_1()
                        .border_color(border)
                        .px_1()
                        .text_size(tiny)
                        .text_color(subtle)
                        .child("Esc"),
                ),
        )
        .child(
            div()
                .debug_selector(|| "palette-list-region".to_string())
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("palette-list")
                        .debug_selector(|| "palette-list".to_string())
                        .flex()
                        .flex_col()
                        .size_full()
                        .p_2()
                        .gap_0p5()
                        .overflow_y_scroll()
                        .track_scroll(&view.scroll)
                        .children(rows),
                )
                .child(super::scrolling::wheel_overlay(
                    "palette",
                    view.smooth.clone(),
                )),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_4()
                .py_2()
                .border_t_1()
                .border_color(border)
                .text_size(tiny)
                .text_color(subtle)
                .child(div().w(px(2.0)).h(px(10.0)).rounded_full().bg(accent))
                .child("↑ ↓ move · Enter opens · Esc closes"),
        );

    let scope = view.focus;
    Backdrop::new("palette-scrim")
        .on_dismiss(move |window, cx| close_click(window, cx))
        .child(FocusScope::new(&scope).trap().child(card))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Command> {
        let repos = [
            ("alpha", "alpha-tools"),
            ("beta", "beta"),
            ("gamma", "gamma-shell"),
        ];
        commands(
            &repos
                .iter()
                .map(|(namespace, name)| RepoFixture {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                    description: "fixture",
                    language: "Rust",
                })
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn the_score_prefers_tighter_earlier_matches_and_folds_case() {
        assert_eq!(score("", "anything"), Some(0));
        assert!(score("zz", "Go to Home").is_none());
        // Case folds in both directions.
        assert_eq!(score("home", "Go to Home"), score("HOME", "gO tO hOmE"));
        // A contiguous run beats the same letters with gaps inside a word.
        assert!(score("home", "home").unwrap() > score("home", "hxomxe").unwrap());
        // CJK labels match by character, not by byte.
        assert!(score("输入", "中文输入法").is_some());
        assert!(score("輸入", "中文输入法").is_none());
    }

    #[test]
    fn groups_stay_contiguous_so_no_heading_repeats() {
        let all = fixture();

        for query in ["", "a", "home", "gamma", "e"] {
            let shown = results(&all, query);
            let groups: Vec<&'static str> = shown.iter().map(|index| all[*index].group).collect();
            let mut seen: Vec<&'static str> = Vec::new();
            for group in groups {
                if seen.last() != Some(&group) {
                    assert!(
                        !seen.contains(&group),
                        "query {query:?} split the {group} group: {seen:?}"
                    );
                    seen.push(group);
                }
            }
        }
    }

    #[test]
    fn the_scroll_child_index_counts_the_headings_in_between() {
        let all = fixture();
        // Headings are inserted above each group, so a result's child index is larger than its
        // position in the result list as soon as a second group starts.
        let entries = layout(&all, "");
        let commands: Vec<usize> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Command(index) => Some(*index),
                Entry::Heading(_) => None,
            })
            .collect();
        assert_eq!(commands, results(&all, ""));
        for cursor in 0..commands.len() {
            let child = child_index(&entries, cursor);
            assert!(child >= cursor, "child {child} before result {cursor}");
            assert!(
                matches!(entries[child], Entry::Command(_)),
                "child {child} must be a row"
            );
        }
        let headings = entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Heading(_)))
            .count();
        let last = child_index(&entries, commands.len() - 1);
        assert_eq!(last, entries.len() - 1);
        assert!(headings >= 2);
    }

    #[test]
    fn an_empty_query_lists_every_command_once() {
        let all = fixture();
        let shown = results(&all, "");
        assert_eq!(shown.len(), all.len());
        let mut unique = shown.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), all.len());
    }
}
