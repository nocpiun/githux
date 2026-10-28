use ely_core::{
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Axis, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, Role, StatefulInteractiveElement, Styled,
    Window, div, prelude::*, px,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    domain::{ProfileBasic, ProfileRepo},
    ui::{
        profile::heatmap::{CellLevel, DayPresentation},
        scroll::{self, Bar, BarId, Placement, Style},
    },
};

use super::{
    PresentationError, ProfileActionKind, ProfileView, RepositoryTarget, SectionState,
    model::repository_presentation,
};

const CELL: f32 = 12.0;
const CELL_GAP: f32 = 3.0;

fn section_heading(title: &'static str, detail: Option<String>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap_2()
        .child(
            div()
                .text_size(theme.text_size(TextSize::Lg))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.colors.fg)
                .child(title),
        )
        .children(detail.map(|detail| {
            div()
                .text_size(theme.text_size(TextSize::Sm))
                .font_weight(FontWeight::NORMAL)
                .text_color(theme.colors.fg_subtle)
                .child(detail)
        }))
        .into_any_element()
}

fn panel(child: AnyElement, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .p_4()
        .rounded(theme.radius(Radius::Lg))
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .child(child)
        .into_any_element()
}

pub(super) fn state_panel(state: &PresentationError, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .debug_selector(|| "profile-section-error".to_string())
        .flex()
        .items_start()
        .gap_2()
        .p_3()
        .rounded(theme.radius(Radius::Md))
        .bg(theme.colors.bg)
        .text_size(theme.text_size(TextSize::Sm))
        .text_color(theme.colors.warning)
        .child(Icon::new(IconName::CircleAlert).size(IconSize::Sm))
        .child(state.message())
        .into_any_element()
}

pub(super) fn loading(label: &'static str, cx: &App) -> AnyElement {
    div()
        .debug_selector(|| "profile-section-loading".to_string())
        .p_3()
        .text_size(cx.theme().text_size(TextSize::Sm))
        .text_color(cx.theme().colors.fg_subtle)
        .child(label)
        .into_any_element()
}

pub(super) fn clean(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

pub(super) fn initials(basic: &ProfileBasic) -> String {
    let display = clean(&basic.name).unwrap_or_else(|| basic.login.trim());
    let mut words = display.split_whitespace().filter(|word| !word.is_empty());
    let first = words.next().unwrap_or("?");
    let second = words.next();
    let grapheme = |word: &str| {
        word.trim_start_matches('@')
            .graphemes(true)
            .next()
            .unwrap_or("?")
            .to_uppercase()
    };
    match second {
        Some(second) => format!("{}{}", grapheme(first), grapheme(second)),
        None => grapheme(first),
    }
}

impl Render for ProfileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = cx.theme().colors.bg;
        let measure_owner = cx.entity().downgrade();
        let measure = gpui::canvas(
            move |bounds, _, cx| {
                let _ = measure_owner.update(cx, |view, cx| {
                    let width = f32::from(bounds.size.width);
                    if (view.content_width - width).abs() > 0.5 {
                        view.content_width = width;
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        self.tick_scroll(window, cx.reduce_motion());
        self.repository_keyboard_focus = self.repository_focus.is_focused(window);
        self.heatmap_keyboard_focus = self.heatmap_focus.is_focused(window);
        let header = self.identity_header(cx);
        let readme = self.render_readme(cx);
        let repositories = self.render_repositories(cx);
        let contributions = self.render_contributions(window, cx);
        let now = self.now_ms();
        if self.page_bars.dragging().is_some() && !cx.has_active_drag() {
            let target = self.page_motion.clone();
            self.page_bars.settle(&target, now, cx.reduce_motion());
        }
        let page_opacity = self.page_bars.opacity(now);
        if self.page_bars.is_animating() {
            window.request_animation_frame();
        }
        let page_target = self.page_motion.clone();
        let page_drag = cx.entity().downgrade();
        let page_bar = Bar {
            target: &self.page_motion,
            id: BarId {
                owner: cx.entity_id(),
                name: "profile-page-scrollbar",
                axis: Axis::Vertical,
            },
            placement: Placement::overlay(4.0, 8.0),
            style: Style::DEFAULT,
            opacity: page_opacity,
            on_drag: std::rc::Rc::new(move |axis, active, _, cx| {
                let _ = page_drag.update(cx, |view, cx| {
                    view.page_bars.drag(
                        &page_target,
                        axis,
                        active,
                        view.now_ms(),
                        cx.reduce_motion(),
                    );
                    cx.notify();
                });
            }),
        }
        .render(cx);
        div()
            .id("profile-overview")
            .debug_selector(|| "profile-overview".to_string())
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(background)
            .text_color(cx.theme().colors.fg)
            .on_hover(cx.listener(|view, hovered, _, cx| {
                if view
                    .page_bars
                    .hover(*hovered, view.now_ms(), cx.reduce_motion())
                {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("profile-page-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(
                        div()
                            .w_full()
                            .max_w_full()
                            .overflow_hidden()
                            .min_w_0()
                            .px_6()
                            .py_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(header)
                            .child(self.overview_marker(cx))
                            .child(readme)
                            .child(repositories)
                            .child(contributions),
                    ),
            )
            .child(scroll::wheel_overlay(
                "profile-page",
                Axis::Vertical,
                self.page_motion.clone(),
            ))
            .child(page_bar)
            .child(measure)
    }
}

impl ProfileView {
    fn overview_marker(&self, cx: &App) -> AnyElement {
        div()
            .w_full()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .py_2()
                    .px_2()
                    .border_b_2()
                    .border_color(cx.theme().colors.accent)
                    .w(px(112.0))
                    .text_size(cx.theme().text_size(TextSize::Sm))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(IconName::BookOpen).size(IconSize::Sm))
                    .child("Overview"),
            )
            .into_any_element()
    }

    fn render_readme(&self, cx: &mut Context<Self>) -> AnyElement {
        let body = match &self.readme {
            SectionState::Loading => loading("Loading profile README…", cx),
            SectionState::Error(error) => state_panel(error, cx),
            SectionState::Ready(()) => div()
                .debug_selector(|| "profile-readme-reader".to_string())
                .w_full()
                .max_w_full()
                .min_w_0()
                .flex_none()
                // Legacy viewport pending the coordinated Reader Flow API.
                // This does not satisfy the rework's intrinsic README acceptance.
                .h(px(280.0))
                .overflow_hidden()
                .child(self.reader.clone().expect("ready README owns a ReaderView"))
                .into_any_element(),
        };
        let identity = self.reader.as_ref().map(|reader| {
            let context = reader.read(cx).context();
            format!("{} / {}", context.owner(), context.path())
        });
        div()
            .debug_selector(|| "profile-readme-section".to_string())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded(cx.theme().radius(Radius::Md))
            .border_1()
            .border_color(cx.theme().colors.border)
            .children(identity.map(|identity| {
                div()
                    .font_family(cx.theme().mono_family.clone())
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .text_color(cx.theme().colors.fg_muted)
                    .child(identity)
            }))
            .child(body)
            .into_any_element()
    }

    fn render_repositories(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let SectionState::Ready(pinned) = self.repositories.clone() else {
            let body = match &self.repositories {
                SectionState::Loading => loading("Loading repository pins…", cx),
                SectionState::Error(error) => state_panel(error, cx),
                SectionState::Ready(_) => unreachable!(),
            };
            return div()
                .debug_selector(|| "profile-repositories-section".to_string())
                .child(section_heading("Pinned repositories", None, cx))
                .child(panel(body, cx))
                .into_any_element();
        };
        let presentation = repository_presentation(&pinned);
        let count = presentation.repositories.len();
        self.selected_repository = self.selected_repository.min(count.saturating_sub(1));
        let cards = presentation
            .repositories
            .iter()
            .enumerate()
            .map(|(index, repo)| self.repository_card(index, repo, cx))
            .collect::<Vec<_>>();
        let body = if cards.is_empty() {
            div()
                .debug_selector(|| "profile-repositories-empty".to_string())
                .p_3()
                .text_size(cx.theme().text_size(TextSize::Sm))
                .text_color(cx.theme().colors.fg_subtle)
                .child(
                    presentation
                        .empty
                        .unwrap_or("No repository pins were returned."),
                )
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "profile-repository-list".to_string())
                .id("profile-repository-list")
                .track_focus(&self.repository_focus)
                .focus(|style| style.border_1().border_color(cx.theme().colors.focus))
                .rounded(cx.theme().radius(Radius::Md))
                .grid()
                .grid_cols(self.repository_columns() as u16)
                .gap_2()
                .children(cards)
                .on_key_down(
                    cx.listener(|view, event, window, cx| view.repository_key(event, window, cx)),
                )
                .into_any_element()
        };
        div()
            .debug_selector(|| "profile-repositories-section".to_string())
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .child(section_heading(
                presentation.title,
                Some(format!("{count}")),
                cx,
            ))
            .children(presentation.note.map(|note| {
                div()
                    .debug_selector(|| "profile-repository-note".to_string())
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .text_color(cx.theme().colors.fg_subtle)
                    .child(note)
            }))
            .child(body)
            .into_any_element()
    }

    fn repository_card(
        &self,
        index: usize,
        repo: &ProfileRepo,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let request = self.request();
        let target = RepositoryTarget::from_repo(repo);
        let name = target.name_with_owner.clone();
        let focused = self.repository_keyboard_focus;
        let description = clean(&repo.description).unwrap_or("").to_string();
        let visibility = repo
            .is_private
            .map(|private| if private { "Private" } else { "Public" });
        let mut stats = Vec::new();
        if let Some(language) = clean(&repo.primary_language) {
            stats.push(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .size(px(8.0))
                            .rounded_full()
                            .bg(theme.colors.fg_subtle),
                    )
                    .child(language.to_string())
                    .into_any_element(),
            );
        }
        if let Some(stars) = repo.stargazer_count {
            stats.push(stat(IconName::Star, compact_count(stars), cx));
        }
        if let Some(forks) = repo.fork_count.filter(|count| *count > 0) {
            stats.push(stat(IconName::GitBranch, compact_count(forks), cx));
        }
        div()
            .id(("profile-repository", index))
            .debug_selector(move || format!("profile-repository-{index}"))
            .role(Role::Button)
            .aria_label(format!("Open repository {name}"))
            .cursor_pointer()
            .min_w_0()
            .h(px(94.0))
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .rounded(theme.radius(Radius::Md))
            .border_1()
            .border_color(if focused && self.selected_repository == index {
                theme.colors.focus
            } else {
                theme.colors.border
            })
            .bg(theme.colors.surface.opacity(0.35))
            .hover(|s| s.bg(theme.colors.hover.opacity(0.5)))
            .tooltip(Tooltip::text(name.clone()))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(move |view, _, window, cx| {
                if view.accepts(&request) {
                    view.selected_repository = index;
                    view.emit(ProfileActionKind::Repository(target.clone()), window, cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Icon::new(IconName::BookOpen)
                            .size(IconSize::Sm)
                            .color(theme.colors.fg_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(theme.text_size(TextSize::Sm))
                            .text_color(theme.colors.link)
                            .child(Ellipsis::new(name)),
                    )
                    .children(visibility.map(|label| {
                        div()
                            .flex_none()
                            .px_1p5()
                            .rounded_full()
                            .border_1()
                            .border_color(theme.colors.border)
                            .text_size(theme.text_size(TextSize::Xs))
                            .text_color(theme.colors.fg_muted)
                            .child(label)
                    })),
            )
            .child(
                div()
                    .id(("profile-repository-description", index))
                    .debug_selector(move || format!("profile-repository-description-{index}"))
                    .tooltip(Tooltip::text(description.clone()))
                    .pl(px(24.0))
                    .min_w_0()
                    .text_size(theme.text_size(TextSize::Sm))
                    .text_color(theme.colors.fg_muted)
                    .child(Ellipsis::new(description)),
            )
            .child(
                div()
                    .mt_auto()
                    .pl(px(24.0))
                    .flex()
                    .items_center()
                    .gap_4()
                    .text_size(theme.text_size(TextSize::Xs))
                    .text_color(theme.colors.fg_muted)
                    .children(stats),
            )
            .into_any_element()
    }

    fn repository_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SectionState::Ready(pinned) = &self.repositories else {
            return;
        };
        let count = repository_presentation(pinned).repositories.len();
        if count == 0 {
            return;
        }
        let key = event.keystroke.key.as_str();
        let columns = self.repository_columns();
        match key {
            "right" => {
                if columns > 1
                    && self.selected_repository.is_multiple_of(columns)
                    && self.selected_repository + 1 < count
                {
                    self.selected_repository += 1;
                }
            }
            "left" => {
                if !self.selected_repository.is_multiple_of(columns) {
                    self.selected_repository -= 1;
                }
            }
            "down" => {
                if self.selected_repository + columns < count {
                    self.selected_repository += columns;
                }
            }
            "up" => {
                if self.selected_repository >= columns {
                    self.selected_repository -= columns;
                }
            }
            "home" => self.selected_repository = 0,
            "end" => self.selected_repository = count - 1,
            "enter" | "space" => {
                let pinned = self.repositories.clone();
                let SectionState::Ready(pinned) = pinned else {
                    return;
                };
                let repo =
                    repository_presentation(&pinned).repositories[self.selected_repository].clone();
                self.emit(
                    ProfileActionKind::Repository(RepositoryTarget::from_repo(&repo)),
                    window,
                    cx,
                );
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn render_contributions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let range = format!("{} → {}", self.range.from(), self.range.to());
        let SectionState::Ready(calendar) = self.contributions.clone() else {
            let body = match &self.contributions {
                SectionState::Loading => loading("Loading contributions…", cx),
                SectionState::Error(error) => state_panel(error, cx),
                SectionState::Ready(_) => unreachable!(),
            };
            return div()
                .debug_selector(|| "profile-contributions-section".to_string())
                .flex()
                .flex_col()
                .gap_2()
                .child(section_heading("Contributions", Some(range), cx))
                .child(panel(body, cx))
                .into_any_element();
        };
        let model = super::heatmap::heatmap_presentation(&self.range, &calendar);
        let flat_count = model
            .weeks
            .iter()
            .map(|week| week.days.len())
            .sum::<usize>();
        self.selected_day = self
            .selected_day
            .map(|day| day.min(flat_count.saturating_sub(1)));
        let mut notes = Vec::new();
        if model.missing_days > 0 {
            notes.push(format!(
                "{} days have no returned data (not zero).",
                model.missing_days
            ));
        }
        if model.invalid_days > 0 {
            notes.push(format!(
                "{} invalid day records were not plotted.",
                model.invalid_days
            ));
        }
        if model.ignored_out_of_range_days > 0 {
            notes.push(format!(
                "{} out-of-range records were ignored.",
                model.ignored_out_of_range_days
            ));
        }
        if model.has_field_errors {
            notes.push("Partial data: field errors accompanied the usable calendar.".into());
        }
        let days = model
            .weeks
            .iter()
            .flat_map(|week| &week.days)
            .collect::<Vec<_>>();
        let leading = super::heatmap::leading_days(&self.range);
        let week_count = (leading + days.len()).div_ceil(7);
        let markers = super::heatmap::month_markers(&self.range);
        let weeks = (0..week_count)
            .map(|week| {
                div()
                    .debug_selector(move || format!("profile-contribution-week-{week}"))
                    .flex()
                    .flex_none()
                    .w(px(CELL))
                    .flex_col()
                    .gap(px(CELL_GAP))
                    .children((0..7).map(|row| {
                        let slot = week * 7 + row;
                        match slot
                            .checked_sub(leading)
                            .and_then(|index| days.get(index).map(|day| (index, day)))
                        {
                            Some((index, day)) => self.contribution_cell(index, day, cx),
                            None => div().size(px(CELL)).flex_none().into_any_element(),
                        }
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let month_labels = div()
            .relative()
            .w(px(week_count as f32 * (CELL + CELL_GAP)))
            .h(px(18.0))
            .children(markers.into_iter().map(|(column, name)| {
                div()
                    .absolute()
                    .left(px(column as f32 * (CELL + CELL_GAP)))
                    .top_0()
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .text_color(cx.theme().colors.fg_muted)
                    .child(name)
            }));
        let now = self.now_ms();
        let opacity = self.heatmap_bars.opacity(now);
        if self.heatmap_bars.is_animating() {
            window.request_animation_frame();
        }
        if self.heatmap_bars.dragging().is_some() && !cx.has_active_drag() {
            let target = self.heatmap_motion.clone();
            self.heatmap_bars.settle(&target, now, cx.reduce_motion());
        }
        let target = self.heatmap_motion.clone();
        let this = cx.entity().downgrade();
        let bar = Bar {
            target: &self.heatmap_motion,
            id: BarId {
                owner: cx.entity_id(),
                name: "profile-heatmap-scrollbar",
                axis: Axis::Horizontal,
            },
            placement: Placement::overlay(3.0, 8.0),
            style: Style::DEFAULT,
            opacity,
            on_drag: std::rc::Rc::new(move |axis, active, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    view.heatmap_bars.drag(
                        &target,
                        axis,
                        active,
                        view.now_ms(),
                        cx.reduce_motion(),
                    );
                    cx.notify();
                });
            }),
        }
        .render(cx);
        let grid = div()
            .debug_selector(|| "profile-heatmap-grid".to_string())
            .id("profile-heatmap-grid")
            .track_focus(&self.heatmap_focus)
            .focus(|style| style.border_1().border_color(cx.theme().colors.focus))
            .relative()
            .flex_none()
            .w_full()
            .min_w_0()
            .h(px(18.0 + 7.0 * CELL + 6.0 * CELL_GAP + 10.0))
            .overflow_hidden()
            .on_hover(cx.listener(|view, hovered, _, cx| {
                if view
                    .heatmap_bars
                    .hover(*hovered, view.now_ms(), cx.reduce_motion())
                {
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|view, event, _, cx| view.heatmap_key(event, cx)))
            .child(
                div()
                    .id("profile-heatmap-scroll")
                    .debug_selector(|| "profile-heatmap-scroll".into())
                    .w_full()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.heatmap_scroll)
                    .child(
                        div()
                            .w(px(week_count as f32 * (CELL + CELL_GAP) - CELL_GAP))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .child(month_labels)
                            .child(div().flex().flex_none().gap(px(CELL_GAP)).children(weeks)),
                    ),
            )
            .child(scroll::wheel_overlay(
                "profile-heatmap",
                Axis::Horizontal,
                self.heatmap_motion.clone(),
            ))
            .child(bar)
            .into_any_element();
        let heading = format!("{} contributions", model.total_contributions);
        let weekday = div()
            .flex_none()
            .w(px(30.0))
            .pt(px(18.0))
            .flex()
            .flex_col()
            .gap(px(CELL_GAP))
            .children(
                ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                    .into_iter()
                    .map(|label| {
                        div()
                            .h(px(CELL))
                            .text_size(cx.theme().text_size(TextSize::Xs))
                            .text_color(cx.theme().colors.fg_muted)
                            .child(label)
                    }),
            );
        let detail = self
            .selected_day_detail()
            .filter(|_| self.heatmap_focus.is_focused(window));
        let legend = div()
            .flex()
            .items_center()
            .gap_1()
            .text_size(cx.theme().text_size(TextSize::Xs))
            .text_color(cx.theme().colors.fg_muted)
            .child("Less")
            .children((0..=4).map(|level| {
                div()
                    .size(px(CELL))
                    .rounded(px(2.0))
                    .bg(cell_color(Some(CellLevel::Known(level)), cx))
            }))
            .child("More");
        div()
            .debug_selector(|| "profile-contributions-section".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div()
                            .text_size(cx.theme().text_size(TextSize::Lg))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(heading),
                    )
                    .child(
                        div()
                            .text_size(cx.theme().text_size(TextSize::Xs))
                            .text_color(cx.theme().colors.fg_muted)
                            .child(range),
                    ),
            )
            .children((!notes.is_empty()).then(|| {
                div()
                    .debug_selector(|| "profile-heatmap-partial".into())
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .text_color(cx.theme().colors.fg_muted)
                    .child(notes.join(" "))
            }))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .p_3()
                    .rounded(cx.theme().radius(Radius::Md))
                    .border_1()
                    .border_color(cx.theme().colors.border)
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .gap_2()
                            .child(weekday)
                            .child(div().flex_1().min_w_0().child(grid)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .mt_1()
                            .h(px(18.0))
                            .child(
                                div()
                                    .debug_selector(|| "profile-day-detail".into())
                                    .text_size(cx.theme().text_size(TextSize::Xs))
                                    .text_color(cx.theme().colors.fg_muted)
                                    .child(detail.unwrap_or_default()),
                            )
                            .child(legend),
                    ),
            )
            .into_any_element()
    }

    fn contribution_cell(
        &self,
        index: usize,
        day: &DayPresentation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_day == Some(index) && self.heatmap_keyboard_focus;
        let color = cell_color(day.level(), cx);
        let detail = day.detail();
        div()
            .debug_selector(move || format!("profile-contribution-day-{index}"))
            .id(("profile-contribution-day", index))
            .role(Role::Button)
            .aria_label(detail.clone())
            .size(px(CELL))
            .flex_none()
            .rounded(px(2.0))
            .bg(color)
            .border_1()
            .border_color(if selected {
                cx.theme().colors.fg
            } else {
                cx.theme().colors.border.opacity(0.35)
            })
            .tooltip(Tooltip::text(detail.clone()))
            .on_mouse_move(cx.listener(move |view, _, _, cx| {
                if view.selected_day != Some(index) {
                    view.selected_day = Some(index);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.selected_day = Some(index);
                view.heatmap_focus.focus(window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn heatmap_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(model) = self.heatmap_model() else {
            return;
        };
        let count = model
            .weeks
            .iter()
            .map(|week| week.days.len())
            .sum::<usize>();
        if count == 0 {
            return;
        }
        let current = self.selected_day.unwrap_or(0);
        self.selected_day = Some(match event.keystroke.key.as_str() {
            "right" => (current + 7).min(count - 1),
            "left" => current.saturating_sub(7),
            "down" => (current + 1).min(count - 1),
            "up" => current.saturating_sub(1),
            "home" => 0,
            "end" => count - 1,
            _ => return,
        });
        self.reveal_selected_week();
        cx.stop_propagation();
        cx.notify();
    }

    fn reveal_selected_week(&self) {
        let Some(selected) = self.selected_day else {
            return;
        };
        let week = (selected + super::heatmap::leading_days(&self.range)) / 7;
        let left = px(week as f32 * (CELL + CELL_GAP));
        let right = left + px(CELL);
        let viewport = self.heatmap_scroll.bounds();
        let visible_left = px(0.0);
        let visible_right = viewport.size.width;
        let current = self.heatmap_scroll.offset().x;
        let offset = if left + current < visible_left {
            visible_left - left
        } else if right + current > visible_right {
            visible_right - right
        } else {
            current
        };
        self.heatmap_motion
            .set_axis_offset(Axis::Horizontal, offset);
    }
}

fn cell_color(level: Option<CellLevel>, cx: &App) -> Hsla {
    let colors = &cx.theme().colors;
    match level {
        Some(CellLevel::Known(0)) => colors.border,
        Some(CellLevel::Known(level @ 1..=4)) => {
            colors.success.opacity(0.2 + f32::from(level) * 0.18)
        }
        Some(CellLevel::Known(_)) | Some(CellLevel::Unknown) => colors.warning.opacity(0.65),
        None => colors.sunken,
    }
}

fn stat(icon: IconName, label: String, cx: &App) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            Icon::new(icon)
                .size(IconSize::Xs)
                .color(cx.theme().colors.fg_muted),
        )
        .child(label)
        .into_any_element()
}
fn compact_count(count: u64) -> String {
    if count >= 1000 {
        format!("{:.1}k", count as f64 / 1000.0)
    } else {
        count.to_string()
    }
}
