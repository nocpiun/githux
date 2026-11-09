//! Element construction for the Home page: headers, panel states, the two
//! virtual lists and the Feed append footer.
//!
//! Everything here reads host-published signals and builds elements. No read is
//! started from this module; the only outgoing edges are the typed actions the
//! host drains from its own queue.

use super::*;

fn column_name(column: Column) -> &'static str {
    match column {
        Column::Home => "home",
        Column::Feed => "feed",
    }
}

/// Compact end-of-scroll append indicator for the Feed column. Loading is a
/// short status line; a failed append keeps the visible list and shows a warning
/// with click-to-retry, never a `Load more` prose button.
fn feed_append_footer(
    state: &FeedAppendState,
    request: RefreshRequest,
    on_action: &OnAction,
) -> AnyElement {
    match state {
        FeedAppendState::Idle => view().into_element(),
        FeedAppendState::Loading => view()
            .test_id("feed-append-loading")
            .flex_none()
            .flex_row()
            .items_center()
            .justify_center()
            .py_2()
            .child(crate::ui::compact_status::loading("Loading older activity…").into_element())
            .into_element(),
        FeedAppendState::NotCached { page } => view()
            .test_id("feed-append-not-cached")
            .flex_none()
            .flex_row()
            .items_center()
            .justify_center()
            .py_2()
            .child(
                text(format!("Page {page} is not available offline."))
                    .text_size(px(12.0))
                    .text_color(t::theme().muted_foreground),
            )
            .into_element(),
        FeedAppendState::Failed { page } => {
            let retry = {
                let on_action = on_action.clone();
                let request = request.clone();
                move || on_action(HomeAction::LoadMoreFeed(request.clone()))
            };
            view()
                .test_id("feed-append-failed")
                .flex_none()
                .flex_row()
                .items_center()
                .justify_center()
                .py_2()
                .child(
                    crate::ui::compact_status::failed(
                        "Could not load older activity",
                        format!("Retry page {page}"),
                        Some(Rc::new(retry)),
                    )
                    .into_element(),
                )
                .into_element()
        }
    }
}

fn historical_mark(column: Column, provenance: &PageProvenance) -> AnyElement {
    let at = std::time::UNIX_EPOCH
        + std::time::Duration::from_millis(provenance.fetched_at_ms.max(0) as u64);
    let when = crate::ui::relative_time::relative(Some(&format_rfc3339(at)), SystemTime::now());
    view()
        .test_id(&format!("{}-historical", column_name(column)))
        .flex_none()
        .flex_row()
        .items_center()
        .gap_1()
        .tooltip(controls::tooltip(format!(
            "Saved {when}. Network content has not replaced this snapshot."
        )))
        .child(
            text("Saved")
                .text_size(px(12.0))
                .text_color(t::theme().muted_foreground),
        )
        .into_element()
}

/// Millisecond cache timestamps are converted only for the shared relative
/// formatter. Invalid/future times retain the formatter's honest fallback.
fn format_rfc3339(at: SystemTime) -> String {
    let duration = at.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days conversion, independent of host locale.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        day_seconds / 3_600,
        day_seconds % 3_600 / 60,
        day_seconds % 60
    )
}

/// The compact refresh control: a real button when a refresh is allowed, and a
/// status slot while one is in flight. No countdown or poll lives here.
fn refresh_control(
    column: Column,
    busy: bool,
    refresh: &RefreshAvailability,
    request: RefreshRequest,
    on_action: &OnAction,
) -> AnyElement {
    let title = match column {
        Column::Home => "Home",
        Column::Feed => "Feed",
    };
    let enabled = !busy && refresh.available;
    let label = if busy {
        format!("Refreshing {title}")
    } else if let Some(reason) = refresh.reason.as_ref().filter(|_| !refresh.available) {
        format!("Refresh {title}: {reason}")
    } else {
        format!("Refresh {title}")
    };
    if busy {
        return view()
            .test_id(&format!("{}-refresh-busy", column_name(column)))
            .flex_none()
            .size(px(28.0))
            .items_center()
            .justify_center()
            .tooltip(controls::tooltip(label))
            .child(crate::ui::compact_status::loading("Refreshing…").into_element())
            .into_element();
    }
    let on_action = on_action.clone();
    view()
        .test_id(&format!("{}-refresh-slot", column_name(column)))
        .flex_none()
        .size(px(28.0))
        .tooltip(controls::tooltip(label.clone()))
        .child(
            nsh_base::button()
                .size(px(28.0))
                .rounded(t::theme().radius)
                .items_center()
                .justify_center()
                .accessibility_label(label)
                .disabled(!enabled)
                .cursor_pointer()
                .hover(|s| s.bg(t::theme().accent))
                .active(|s| s.bg(t::theme().accent))
                .on_press(move |_: &()| {
                    if enabled {
                        on_action(HomeAction::Refresh(request.clone()));
                    }
                })
                .child(rows::icon(rows::Icon::Refresh)),
        )
        .into_element()
}

fn panel_shell(column: Column) -> View {
    view()
        .test_id(&format!("{}-panel", column_name(column)))
        .flex_1()
        .min_w(px(320.0))
        .min_h(px(0.0))
        .flex_col()
        .gap_2()
        .overflow_hidden()
}

fn panel_header(
    column: Column,
    busy: bool,
    refresh: &RefreshAvailability,
    provenance: Option<&PageProvenance>,
    request: RefreshRequest,
    on_action: &OnAction,
) -> AnyElement {
    let title = match column {
        Column::Home => "Home",
        Column::Feed => "Feed",
    };
    view()
        .test_id(&format!("{}-header", column_name(column)))
        .flex_none()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(t::PAGE_HEADER_HEIGHT))
        .px(px(t::PAGE_HEADER_INSET))
        .child(
            view().flex_1().min_w(px(0.0)).child(
                text(title)
                    .test_id(&format!("{}-title", column_name(column)))
                    .text_size(px(18.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t::theme().foreground),
            ),
        )
        .children(provenance.map(|provenance| historical_mark(column, provenance)))
        .child(refresh_control(column, busy, refresh, request, on_action))
        .into_element()
}

fn centered_status(
    column: Column,
    mark: AnyElement,
    title: &str,
    detail: Option<String>,
) -> AnyElement {
    view()
        .test_id(&format!("{}-status", column_name(column)))
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_5()
        .child(mark)
        .child(
            text(title.to_string())
                .text_center()
                .text_color(t::theme().muted_foreground),
        )
        .children(detail.map(|detail| {
            text(detail)
                .text_size(px(12.0))
                .text_center()
                .text_color(t::theme().muted_foreground)
        }))
        .into_element()
}

fn partial_notice(column: Column, message: String) -> AnyElement {
    view()
        .test_id(&format!("{}-partial", column_name(column)))
        .flex_none()
        .mx_3()
        .mb_2()
        .p_2()
        .rounded(t::theme().radius)
        .bg(t::theme().muted)
        .child(text(message).text_size(px(12.0)).text_color(t::WARNING))
        .into_element()
}

impl HomeView {
    /// Builds the two-column page. Pure presentation: every value comes from a
    /// signal the host has already published, and no request starts here.
    pub fn element(&self, on_action: &OnAction, avatar: Option<&AvatarImage>) -> AnyElement {
        *self.sink.borrow_mut() = Some(on_action.clone());
        let on_action = on_action.clone();
        let avatar = avatar.cloned();
        let this = self.clone();
        view()
            .test_id("home-view")
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex_col()
            .bg(t::theme().background)
            .p(px(t::PAGE_GUTTER))
            .pt(px(t::PAGE_TOP))
            .pb(px(t::PAGE_BOTTOM))
            .child(
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex_row()
                    .flex_wrap()
                    .gap(px(32.0))
                    .child(this.home_panel(on_action.clone()))
                    .child(this.feed_panel(&on_action, avatar.as_ref())),
            )
            .into_element()
    }

    pub(super) fn request(&self, column: Column) -> RefreshRequest {
        RefreshRequest {
            column,
            binding: self.binding.get_untracked(),
            generation: self.generation.get_untracked(),
        }
    }

    fn home_panel(&self, on_action: OnAction) -> AnyElement {
        let this = self.clone();
        dynamic(move || {
            let model = this.home.get();
            let request = this.request(Column::Home);
            let busy = model.state.is_busy();
            let shell = panel_shell(Column::Home).child(panel_header(
                Column::Home,
                busy,
                &model.refresh,
                model.provenance.as_ref(),
                request,
                &on_action,
            ));
            match &model.state {
                PanelState::Loading => shell.child(centered_status(
                    Column::Home,
                    crate::ui::compact_status::loading("Loading this column…").into_element(),
                    "Loading…",
                    None,
                )),
                PanelState::Ready {
                    content: PanelContent::Unavailable { message },
                    ..
                } => shell.child(centered_status(
                    Column::Home,
                    crate::ui::compact_status::empty_dot().into_element(),
                    "No current data.",
                    Some(message.clone()),
                )),
                PanelState::Ready {
                    content: PanelContent::Error(error),
                    ..
                } => shell.child(centered_status(
                    Column::Home,
                    crate::ui::compact_status::failed(
                        "This column could not be loaded",
                        presentation::error_message(error),
                        None,
                    )
                    .into_element(),
                    "This column could not be loaded",
                    Some(presentation::error_message(error)),
                )),
                PanelState::Ready {
                    content: PanelContent::Ready(outcome),
                    ..
                } => {
                    let notice = home_summary(outcome).notice;
                    let list = this.home_list(on_action.clone());
                    shell
                        .children(notice.map(|notice| partial_notice(Column::Home, notice)))
                        .child(list)
                }
            }
            .into_element()
        })
        .into_element()
    }

    fn home_list(&self, on_action: OnAction) -> AnyElement {
        let this = self.clone();
        let items = self
            .home
            .get_untracked()
            .state
            .data()
            .map(|outcome| outcome.items.clone())
            .unwrap_or_default();
        let now = self.now.get_untracked();
        let selected = self.selected_home.get_untracked();
        if items.is_empty() {
            return view()
                .test_id("home-empty")
                .flex_1()
                .min_h(px(0.0))
                .items_center()
                .justify_center()
                .p_5()
                .child(
                    text("No open Issues or pull requests matched this search.")
                        .text_center()
                        .text_color(t::theme().muted_foreground),
                )
                .into_element();
        }
        let count = items.len();
        virtual_list(
            move || count,
            move |at: Position| {
                let index = at.index();
                let Some(item) = items.get(index) else {
                    return view().into_element();
                };
                home_row(
                    item,
                    index,
                    selected == Some(index),
                    now,
                    this.selected_home,
                    on_action.clone(),
                )
            },
        )
        .test_id("home-rows")
        .flex_1()
        .min_h(px(0.0))
        .scrollbar(true)
        .into_element()
    }

    fn feed_panel(&self, on_action: &OnAction, avatar: Option<&AvatarImage>) -> AnyElement {
        let this = self.clone();
        let on_action = on_action.clone();
        let avatar = avatar.cloned();
        dynamic(move || {
            let model = this.feed.get();
            let append = this.feed_append.get();
            let request = this.request(Column::Feed);
            let busy = model.state.is_busy();
            let shell = panel_shell(Column::Feed).child(panel_header(
                Column::Feed,
                busy,
                &model.refresh,
                model.provenance.as_ref(),
                request.clone(),
                &on_action,
            ));
            match &model.state {
                PanelState::Loading => shell.child(centered_status(
                    Column::Feed,
                    crate::ui::compact_status::loading("Loading this column…").into_element(),
                    "Loading…",
                    None,
                )),
                PanelState::Ready {
                    content: PanelContent::Unavailable { message },
                    ..
                } => shell.child(centered_status(
                    Column::Feed,
                    crate::ui::compact_status::empty_dot().into_element(),
                    "No current data.",
                    Some(message.clone()),
                )),
                PanelState::Ready {
                    content: PanelContent::Error(error),
                    ..
                } => shell.child(centered_status(
                    Column::Feed,
                    crate::ui::compact_status::failed(
                        "This column could not be loaded",
                        presentation::error_message(error),
                        None,
                    )
                    .into_element(),
                    "This column could not be loaded",
                    Some(presentation::error_message(error)),
                )),
                PanelState::Ready {
                    content: PanelContent::Ready(outcome),
                    ..
                } => {
                    let notice = presentation::feed_summary(outcome).notice;
                    let list = this.feed_list(on_action.clone(), avatar.clone());
                    shell
                        .children(notice.map(|notice| partial_notice(Column::Feed, notice)))
                        .child(list)
                        .child(feed_append_footer(&append, request, &on_action))
                }
            }
            .into_element()
        })
        .into_element()
    }

    fn feed_list(&self, on_action: OnAction, avatar: Option<AvatarImage>) -> AnyElement {
        let this = self.clone();
        let activities = self
            .feed
            .get_untracked()
            .state
            .data()
            .map(|outcome| outcome.activities.clone())
            .unwrap_or_default();
        let now = self.now.get_untracked();
        let selected = self.selected_feed.get_untracked();
        let descriptions = self.descriptions.get_untracked();
        // A scroll view rather than a recycling list: every retained activity is
        // laid out, so the page's near-end detection sees real content bounds, and
        // a card's varying height never depends on a measurement pass. The Feed is
        // bounded by the data layer (30 days / 300 events), so this stays small.
        let mut rows: Vec<AnyElement> = Vec::with_capacity(activities.len());
        for (index, activity) in activities.iter().enumerate() {
            let repo_key = activity.event.repository.name.trim().to_ascii_lowercase();
            let description = activity
                .repository_description
                .clone()
                .or_else(|| descriptions.get(&repo_key).cloned());
            let image = avatar
                .as_ref()
                .and_then(|resolve| resolve(&activity.subject));
            rows.push(feed_row(
                activity,
                index,
                selected == Some(index),
                now,
                image,
                description.as_deref(),
                this.selected_feed,
                on_action.clone(),
            ));
        }
        let empty = rows.is_empty();
        let list = scroll()
            .test_id("feed-rows")
            .element_ref(&self.feed_scroll)
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .scrollbar(true)
            .child(view().flex_col().w_full().min_w(px(0.0)).children(rows));
        if empty {
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_col()
                .child(list)
                .child(
                    view()
                        .test_id("feed-empty")
                        .flex_1()
                        .min_h(px(0.0))
                        .items_center()
                        .justify_center()
                        .p_5()
                        .child(
                            text("No activities from accounts you follow in this Feed snapshot.")
                                .text_center()
                                .text_color(t::theme().muted_foreground),
                        ),
                )
                .into_element()
        } else {
            list.into_element()
        }
    }

    /// The current selected row in the requested panel.
    pub fn selected(&self, column: Column) -> Option<usize> {
        match column {
            Column::Home => self.selected_home.get_untracked(),
            Column::Feed => self.selected_feed.get_untracked(),
        }
    }

    /// Emits a fresh manual refresh for `column`. The host decides whether the
    /// request is allowed and publishes the resulting state.
    pub fn refresh(&self, column: Column, on_action: &OnAction) {
        on_action(HomeAction::Refresh(self.request(column)));
    }
}
