//! Read-only Files changed pane. Its host activates it only on demand.
#[cfg(test)]
#[path = "../../bin_support/pr_files_tests.rs"]
pub(crate) mod tests;
use crate::data::{Scoped, pr_files::FilesSession};
use crate::domain::{error::GitHubError, repository::RepositoryError};
use crate::github::pr_files::{FILE_LIMIT, File, Page, PageEnd, Patch, RowKind, Status};
use crate::network::{
    cancel::{CancellationToken, CancellationTokenSource},
    executor::BackgroundExecutor,
};
use crate::ui::scroll::{Bar, BarId, OnDragging, Placement, Style};
use ely_core::{
    buttons::{Button, ButtonVariant},
    motion::Spinner,
    theme::{ActiveTheme, ControlSize},
};
use gpui::{
    Context, IntoElement, ListHorizontalSizingBehavior, Render, Task, UniformListScrollHandle,
    Window, div, prelude::*, px, uniform_list,
};
use std::{ops::Range, rc::Rc, sync::Arc};

pub struct FilesView {
    session: FilesSession,
    executor: Arc<BackgroundExecutor>,
    active: bool,
    page: Option<Page>,
    selected: usize,
    widest_row: Option<usize>,
    request: Option<CancellationTokenSource>,
    receiver: Option<Task<()>>,
    failure: Option<(u32, RepositoryError)>,
    diff_scroll: UniformListScrollHandle,
    file_scroll: UniformListScrollHandle,
}

impl FilesView {
    pub fn new(session: FilesSession, executor: Arc<BackgroundExecutor>) -> Self {
        Self {
            session,
            executor,
            active: false,
            page: None,
            selected: 0,
            widest_row: None,
            request: None,
            receiver: None,
            failure: None,
            diff_scroll: UniformListScrollHandle::new(),
            file_scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn rebind(&mut self, session: FilesSession, cx: &mut Context<Self>) {
        self.cancel();
        self.session = session;
        self.page = None;
        self.failure = None;
        self.reset_selection();
        if self.active {
            self.load(1, cx);
        }
        cx.notify();
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if !active {
            self.cancel();
        } else if self.page.is_none() && self.failure.is_none() {
            self.load(1, cx);
        }
        cx.notify();
    }

    pub fn is_loading(&self) -> bool {
        self.request.is_some() && self.session.verify()
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        self.load(1, cx)
    }

    fn load(&mut self, number: u32, cx: &mut Context<Self>) -> bool {
        if !self.active || self.request.is_some() || !self.session.verify() {
            return false;
        }
        self.failure = None;
        let session = self.session.clone();
        let (token, source) = CancellationToken::new();
        let worker_token = token.clone();
        let worker_session = session.clone();
        let worker = self
            .executor
            .handle()
            .spawn(async move { worker_session.page(number, worker_token).await });
        match worker {
            Ok(worker) => {
                self.request = Some(source);
                self.receiver = Some(cx.spawn(async move |view, cx| {
                    let result = worker.await.unwrap_or_else(|_| Err(worker_closed()));
                    let _ = view.update(cx, |view, cx| {
                        view.finish(&session, &token, number, result, cx);
                    });
                }));
            }
            Err(_) => self.failure = Some((number, worker_closed())),
        }
        cx.notify();
        true
    }

    fn finish(
        &mut self,
        session: &FilesSession,
        token: &CancellationToken,
        number: u32,
        result: Result<Scoped<Page>, RepositoryError>,
        cx: &mut Context<Self>,
    ) {
        if !self.active
            || token.is_cancelled()
            || !self.session.verify()
            || session.target() != self.session.target()
            || session.context() != self.session.context()
        {
            return;
        }
        self.request = None;
        match result {
            Ok(scoped)
                if &scoped.context == self.session.context() && scoped.value.number == number =>
            {
                self.page = Some(scoped.value);
                self.reset_selection();
            }
            Ok(_) => {
                self.failure = Some((
                    number,
                    RepositoryError::InvalidResponse {
                        message: "unexpected pull request files page identity".into(),
                    },
                ))
            }
            Err(error) => self.failure = Some((number, error)),
        }
        cx.notify();
    }

    fn cancel(&mut self) {
        if let Some(source) = self.request.take() {
            source.cancel();
        }
        self.receiver = None;
    }

    fn reset_selection(&mut self) {
        self.select_file(0);
        self.file_scroll = UniformListScrollHandle::new();
    }

    fn select_file(&mut self, index: usize) {
        self.selected = index;
        self.diff_scroll = UniformListScrollHandle::new();
        self.widest_row = match self.file().map(|file| &file.patch) {
            Some(Patch::Text { rows, .. }) => rows
                .iter()
                .enumerate()
                .max_by_key(|(_, row)| visible_width(&row.text))
                .map(|(index, _)| index),
            _ => None,
        };
    }

    fn file(&self) -> Option<&File> {
        self.page.as_ref()?.files.get(self.selected)
    }

    fn render_file(&self, index: usize, cx: &mut Context<Self>) -> Button {
        let file = &self.page.as_ref().expect("file list is loaded").files[index];
        let label = format!("{}  {}", status(&file.status), visible_text(&file.filename));
        Button::new(("pr-file", index), label)
            .full_width()
            .variant(if index == self.selected {
                ButtonVariant::Secondary
            } else {
                ButtonVariant::Ghost
            })
            .size(ControlSize::Sm)
            .on_click(cx.listener(move |view, _, _, cx| {
                if view.selected != index {
                    view.select_file(index);
                    cx.notify();
                }
            }))
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut pane = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden();
        let Some(file) = self.file() else {
            return pane;
        };
        let mut path = div()
            .id("pr-diff-file-path")
            .flex_shrink_0()
            .overflow_x_scroll()
            .flex()
            .flex_col()
            .p_2()
            .font_family(theme.mono_family.clone())
            .whitespace_nowrap();
        if let Some(previous) = &file.previous_filename {
            path = path.child(div().child(format!("{} →", visible_text(previous))));
        }
        pane = pane.child(path.child(visible_text(&file.filename))).child(
            div()
                .px_2()
                .pb_2()
                .text_sm()
                .text_color(theme.colors.fg_muted)
                .child(format!(
                    "{}  +{} −{}",
                    status(&file.status),
                    file.additions,
                    file.deletions
                )),
        );
        let (rows, partial) = match &file.patch {
            Patch::Unavailable => return pane.child(div().p_2().child("Text diff unavailable.")),
            Patch::Text { rows, partial } => (rows, *partial),
        };
        if partial {
            pane = pane.child(div().p_2().text_sm().child(if rows.is_empty() {
                "The text patch could not be displayed."
            } else {
                "This text patch is incomplete."
            }));
        }
        if rows.is_empty() {
            return pane;
        }
        let list = uniform_list(
            "pr-diff-lines",
            rows.len(),
            cx.processor(|view, range: Range<usize>, _, cx| {
                let Some(File {
                    patch: Patch::Text { rows, .. },
                    ..
                }) = view.file()
                else {
                    return Vec::new();
                };
                let theme = cx.theme();
                range
                    .map(|index| {
                        let row = &rows[index];
                        let colors = &theme.colors;
                        let color = match row.kind {
                            RowKind::Addition => colors.success,
                            RowKind::Deletion => colors.danger,
                            RowKind::Context => colors.fg,
                            RowKind::Header | RowKind::Note => colors.fg_muted,
                        };
                        let number = |line: Option<u64>| {
                            div()
                                .w(px(64.))
                                .flex_shrink_0()
                                .text_right()
                                .pr_2()
                                .text_color(colors.fg_muted)
                                .child(line.map_or_else(String::new, |line| line.to_string()))
                        };
                        div()
                            .flex()
                            .items_center()
                            .h(px(24.))
                            .font_family(theme.mono_family.clone())
                            .text_sm()
                            .whitespace_nowrap()
                            .text_color(color)
                            .child(number(row.old))
                            .child(number(row.new))
                            .child(div().flex_shrink_0().px_2().child(format!(
                                "{}{}",
                                row.kind.prefix(),
                                visible_text(&row.text)
                            )))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.diff_scroll)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(self.widest_row)
        .size_full();
        pane.child(scroll_region(list, &self.diff_scroll, "pr-diff", true, cx))
    }
}

impl Drop for FilesView {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Render for FilesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.colors.surface)
            .text_color(theme.colors.fg)
            .font_family(theme.font_family.clone());
        if !self.active {
            return root;
        }
        if !self.session.verify() {
            return root.child(
                div()
                    .p_3()
                    .child("Files are unavailable for the current account."),
            );
        }
        let mut actions = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .p_2()
            .flex_shrink_0();
        if let Some(page) = &self.page {
            if page.number > 1 {
                actions = actions.child(page_button(
                    "pr-files-prev",
                    "Previous",
                    page.number - 1,
                    self.is_loading(),
                    cx,
                ));
            }
            if !page.files.is_empty() {
                actions = actions.child(format!(
                    "Files {}–{}",
                    page.offset() + 1,
                    page.offset() + page.files.len() as u32
                ));
            }
            if page.end == PageEnd::More {
                actions = actions.child(page_button(
                    "pr-files-next",
                    "Next",
                    page.number + 1,
                    self.is_loading(),
                    cx,
                ));
            }
            if page.end == PageEnd::FileLimit {
                actions =
                    actions.child(format!("GitHub lists at most {FILE_LIMIT} changed files."));
            }
        }
        if self.is_loading() {
            actions = actions.child(Spinner::new("pr-files-loading"));
        }
        if let Some((page, error)) = &self.failure {
            actions = actions.child(error_message(error)).child(page_button(
                "pr-files-retry",
                "Retry",
                *page,
                false,
                cx,
            ));
        }
        let root = root.child(actions);
        let Some(page) = &self.page else {
            return root;
        };
        if page.files.is_empty() {
            return root.child(div().p_3().child(
                if page.number == 1 && page.end == PageEnd::Complete {
                    "No changed files."
                } else {
                    "No changed files on this page."
                },
            ));
        }
        let files = uniform_list(
            "pr-files-list",
            page.files.len(),
            cx.processor(|view, range: Range<usize>, _, cx| {
                range
                    .map(|index| view.render_file(index, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.file_scroll)
        .size_full();
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .overflow_hidden()
                .child(
                    div()
                        .w(px(232.))
                        .max_w(gpui::relative(0.4))
                        .h_full()
                        .flex_shrink_0()
                        .border_r_1()
                        .border_color(cx.theme().colors.border)
                        .child(scroll_region(
                            files,
                            &self.file_scroll,
                            "pr-files",
                            false,
                            cx,
                        )),
                )
                .child(self.render_diff(cx)),
        )
    }
}

fn scroll_region(
    list: gpui::UniformList,
    scroll: &UniformListScrollHandle,
    name: &'static str,
    horizontal: bool,
    cx: &Context<FilesView>,
) -> gpui::Div {
    use gpui::Axis::{Horizontal, Vertical};
    let handle = scroll.0.borrow().base_handle.clone();
    let metrics =
        |handle: &gpui::ScrollHandle| (handle.bounds(), handle.max_offset(), handle.offset());
    let before = metrics(&handle);
    let after_layout = handle.clone();
    let entity = cx.entity().downgrade();
    let on_drag: OnDragging = Rc::new(move |_, _, _, cx| {
        let _ = entity.update(cx, |_, cx| cx.notify());
    });
    let placement = Placement::overlay(1.5, 8.0);
    let bar = |axis, id, placement| {
        Bar {
            target: &handle,
            id: BarId {
                owner: cx.entity_id(),
                name: id,
                axis,
            },
            placement,
            style: Style::DEFAULT,
            opacity: 1.0,
            on_drag: on_drag.clone(),
        }
        .render(cx)
    };
    let entity = cx.entity().downgrade();
    div()
        .size_full()
        .min_h_0()
        .relative()
        .overflow_hidden()
        .child(list)
        .child(bar(
            Vertical,
            name,
            if horizontal {
                placement.clear_of(&placement, 0.0)
            } else {
                placement
            },
        ))
        .when(horizontal, |el| {
            el.child(bar(Horizontal, "pr-diff-horizontal", placement))
        })
        .on_children_prepainted(move |_, _, cx| {
            if before != metrics(&after_layout) {
                let _ = entity.update(cx, |_, cx| cx.notify());
            }
        })
}

fn worker_closed() -> RepositoryError {
    GitHubError::Network {
        message: "files request worker unavailable".into(),
    }
    .into()
}

fn page_button(
    id: &'static str,
    label: &'static str,
    number: u32,
    disabled: bool,
    cx: &mut Context<FilesView>,
) -> Button {
    Button::new(id, label)
        .size(ControlSize::Sm)
        .disabled(disabled)
        .on_click(cx.listener(move |view, _, _, cx| {
            view.load(number, cx);
        }))
}

fn status(status: &Status) -> &'static str {
    match status {
        Status::Added => "Added",
        Status::Removed => "Deleted",
        Status::Renamed => "Renamed",
        Status::Modified => "Modified",
        Status::Copied => "Copied",
        Status::Unchanged => "Unchanged",
        Status::Changed | Status::Other => "Changed",
    }
}

fn error_message(error: &RepositoryError) -> &'static str {
    match error {
        RepositoryError::GitHub(GitHubError::Unauthorized) => {
            "Authentication expired. Replace the account token and retry."
        }
        RepositoryError::GitHub(GitHubError::Forbidden | GitHubError::NotFound) => {
            "This pull request could not be accessed."
        }
        RepositoryError::GitHub(GitHubError::RateLimited { .. }) => {
            "GitHub rate limit reached. Wait before retrying."
        }
        RepositoryError::GitHub(GitHubError::SsoRequired { .. })
        | RepositoryError::SsoPartial { .. } => {
            "Organization authorization is required to read these files."
        }
        RepositoryError::GitHub(GitHubError::ResponseTooLarge { .. }) => {
            "This page of changed files is too large to load."
        }
        _ => "Could not load this page of changed files.",
    }
}

fn visible_text(text: &str) -> String {
    let mut display = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\t' => display.push_str("    "),
            '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => display.extend(character.escape_unicode()),
            character if character.is_control() => display.extend(character.escape_default()),
            character => display.push(character),
        }
    }
    display
}

fn visible_width(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\t').count() * 3 + text.len()
}
