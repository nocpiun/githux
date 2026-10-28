use ely_core::{
    buttons::{Button, ButtonVariant},
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Axis, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, Role, StatefulInteractiveElement, Styled,
    Window, div, prelude::*, px, rgb,
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

const CELL: f32 = 11.0;
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

fn state_panel(state: &PresentationError, cx: &App) -> AnyElement {
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

fn loading(label: &'static str, cx: &App) -> AnyElement {
    div()
        .debug_selector(|| "profile-section-loading".to_string())
        .p_3()
        .text_size(cx.theme().text_size(TextSize::Sm))
        .text_color(cx.theme().colors.fg_subtle)
        .child(label)
        .into_any_element()
}

fn clean(value: &Option<String>) -> Option<&str> {
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

fn meta_text(text: String, cx: &App) -> AnyElement {
    div()
        .min_w_0()
        .text_size(cx.theme().text_size(TextSize::Xs))
        .child(Ellipsis::new(text))
        .into_any_element()
}

impl Render for ProfileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = cx.theme().colors.bg;
        self.tick_scroll(window, cx.reduce_motion());
        let header = self.render_header(cx);
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
                            .max_w(px(1120.0))
                            .overflow_hidden()
                            .min_w_0()
                            .mx_auto()
                            .px_5()
                            .py_5()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .child(header)
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
    }
}

impl ProfileView {
    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let SectionState::Ready(basic) = self.basic.clone() else {
            return match &self.basic {
                SectionState::Loading => panel(loading("Loading public profile…", cx), cx),
                SectionState::Error(error) => panel(state_panel(error, cx), cx),
                SectionState::Ready(_) => unreachable!(),
            };
        };
        let display = clean(&basic.name).unwrap_or(basic.login.trim()).to_string();
        let avatar = div()
            .debug_selector(|| "profile-avatar-initials".to_string())
            .flex_none()
            .size(px(80.0))
            .rounded_full()
            .border_1()
            .border_color(colors.border)
            .bg(colors.hover)
            .flex()
            .items_center()
            .justify_center()
            .text_size(theme.text_size(TextSize::Xl))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.fg)
            .child(initials(&basic));
        let name = div()
            .min_w_0()
            .text_size(theme.text_size(TextSize::Xl))
            .font_weight(FontWeight::SEMIBOLD)
            .child(Ellipsis::new(display));
        let login = div()
            .text_size(theme.text_size(TextSize::Base))
            .text_color(colors.fg_subtle)
            .child(format!("@{}", basic.login));
        let profile_button =
            Button::new("profile-open", "Open profile")
                .variant(ButtonVariant::Link)
                .on_click(cx.listener(|view, _, window, cx| {
                    view.emit(ProfileActionKind::Profile, window, cx)
                }));
        let mut metadata = Vec::new();
        for value in [clean(&basic.company), clean(&basic.location)]
            .into_iter()
            .flatten()
        {
            metadata.push(meta_text(value.to_string(), cx));
        }
        for (label, value) in [
            ("followers", basic.followers),
            ("following", basic.following),
            ("public repos", basic.public_repos),
            ("public gists", basic.public_gists),
        ] {
            if let Some(value) = value {
                metadata.push(meta_text(format!("{value} {label}"), cx));
            }
        }
        if let Some(created) = clean(&basic.created_at) {
            metadata.push(meta_text(format!("Joined {created}"), cx));
        }
        let website = clean(&basic.website_url).map(|url| {
            let url = url.to_string();
            Button::new("profile-website", "Website")
                .variant(ButtonVariant::Link)
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.emit(
                        ProfileActionKind::ExternalLink { url: url.clone() },
                        window,
                        cx,
                    )
                }))
        });
        panel(
            div()
                .debug_selector(|| "profile-basic".to_string())
                .flex()
                .items_start()
                .gap_4()
                .min_w_0()
                .child(avatar)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .flex_wrap()
                                .gap_x_3()
                                .gap_y_1()
                                .min_w_0()
                                .child(name)
                                .child(login),
                        )
                        .children(clean(&basic.bio).map(|bio| {
                            div()
                                .w_full()
                                .text_size(theme.text_size(TextSize::Sm))
                                .child(bio.to_string())
                        }))
                        .child(
                            div()
                                .debug_selector(|| "profile-public-metadata".to_string())
                                .w_full()
                                .min_w_0()
                                .grid()
                                .grid_cols(2)
                                .gap_x_4()
                                .gap_y_1()
                                .text_size(theme.text_size(TextSize::Xs))
                                .text_color(colors.fg_subtle)
                                .children(metadata),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_3()
                                .child(profile_button)
                                .children(website),
                        ),
                )
                .into_any_element(),
            cx,
        )
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
                .h(px(280.0))
                .overflow_hidden()
                .rounded(cx.theme().radius(Radius::Md))
                .border_1()
                .border_color(cx.theme().colors.border)
                .child(self.reader.clone().expect("ready README owns a ReaderView"))
                .into_any_element(),
        };
        div()
            .debug_selector(|| "profile-readme-section".to_string())
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .child(section_heading("README", None, cx))
            .child(panel(body, cx))
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
                .grid_cols(2)
                .gap_3()
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
            .child(panel(body, cx))
            .into_any_element()
    }

    fn repository_card(
        &self,
        index: usize,
        repo: &ProfileRepo,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let selected = self.selected_repository == index;
        let target = RepositoryTarget::from_repo(repo);
        let name = target.name_with_owner.clone();
        let description = clean(&repo.description)
            .unwrap_or("No public description supplied.")
            .to_string();
        let language = clean(&repo.primary_language).map(str::to_string);
        let stats = [
            repo.stargazer_count.map(|count| format!("{count} stars")),
            repo.fork_count.map(|count| format!("{count} forks")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        div()
            .debug_selector(move || format!("profile-repository-{index}"))
            .id(("profile-repository", index))
            .role(Role::Button)
            .aria_label(format!("Open repository {name}"))
            .cursor_pointer()
            .min_w_0()
            .h(px(112.0))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .rounded(theme.radius(Radius::Md))
            .border_1()
            .border_color(if selected {
                theme.colors.focus
            } else {
                theme.colors.border
            })
            .bg(theme.colors.bg)
            .hover(|style| style.bg(theme.colors.hover))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(move |view, _, window, cx| {
                view.selected_repository = index;
                view.emit(ProfileActionKind::Repository(target.clone()), window, cx);
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
                            .color(theme.colors.fg_subtle),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.colors.fg)
                            .child(Ellipsis::new(name)),
                    ),
            )
            .child(
                div()
                    .debug_selector(move || format!("profile-repository-description-{index}"))
                    .h(px(34.0))
                    .overflow_hidden()
                    .text_size(theme.text_size(TextSize::Sm))
                    .text_color(theme.colors.fg_subtle)
                    .child(description),
            )
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .text_size(theme.text_size(TextSize::Xs))
                    .text_color(theme.colors.fg_subtle)
                    .children(language.map(|language| div().child(Ellipsis::new(language))))
                    .child(Ellipsis::new(stats)),
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
        match key {
            "right" | "down" => {
                self.selected_repository = (self.selected_repository + 1).min(count - 1)
            }
            "left" | "up" => self.selected_repository = self.selected_repository.saturating_sub(1),
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
        let detail = self
            .selected_day_detail()
            .unwrap_or_else(|| "Focus the graph and use arrow keys to inspect a day.".into());
        let weeks = model
            .weeks
            .iter()
            .enumerate()
            .map(|(week_index, week)| {
                div()
                    .debug_selector(move || format!("profile-contribution-week-{week_index}"))
                    .flex()
                    .flex_none()
                    .w(px(CELL))
                    .flex_col()
                    .gap(px(CELL_GAP))
                    .children(week.days.iter().enumerate().map(|(day_index, day)| {
                        self.contribution_cell(week_index * 7 + day_index, day, cx)
                    }))
            })
            .collect::<Vec<_>>();
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
            .h(px(7.0 * CELL + 6.0 * CELL_GAP + 12.0))
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
                    .debug_selector(|| "profile-heatmap-scroll".to_string())
                    .w_full()
                    .h_full()
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .min_w_0()
                    .gap(px(CELL_GAP))
                    .overflow_x_scroll()
                    .track_scroll(&self.heatmap_scroll)
                    .children(weeks),
            )
            .child(scroll::wheel_overlay(
                "profile-heatmap",
                Axis::Horizontal,
                self.heatmap_motion.clone(),
            ))
            .child(bar)
            .into_any_element();
        div()
            .debug_selector(|| "profile-contributions-section".to_string())
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .child(section_heading(
                "Contributions",
                Some(format!("{} · {range}", model.total_contributions)),
                cx,
            ))
            .children((!notes.is_empty()).then(|| {
                div()
                    .debug_selector(|| "profile-heatmap-partial".to_string())
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .text_color(cx.theme().colors.fg_subtle)
                    .child(notes.join(" "))
            }))
            .child(panel(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(grid)
                    .child(
                        div()
                            .debug_selector(|| "profile-day-detail".to_string())
                            .min_h(px(20.0))
                            .text_size(cx.theme().text_size(TextSize::Xs))
                            .text_color(cx.theme().colors.fg_subtle)
                            .child(detail),
                    )
                    .into_any_element(),
                cx,
            ))
            .into_any_element()
    }

    fn contribution_cell(
        &self,
        index: usize,
        day: &DayPresentation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_day == Some(index);
        let color = cell_color(day.level());
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
        cx.stop_propagation();
        cx.notify();
    }
}

fn cell_color(level: Option<CellLevel>) -> Hsla {
    match level {
        Some(CellLevel::Known(0)) => rgb(0x30353b).into(),
        Some(CellLevel::Known(1)) => rgb(0x0e4429).into(),
        Some(CellLevel::Known(2)) => rgb(0x006d32).into(),
        Some(CellLevel::Known(3)) => rgb(0x26a641).into(),
        Some(CellLevel::Known(4)) => rgb(0x39d353).into(),
        Some(CellLevel::Known(_)) | None => rgb(0x30353b).into(),
        Some(CellLevel::Unknown) => rgb(0x6e5494).into(),
    }
}
