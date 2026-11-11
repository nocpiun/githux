//! Viewport regressions use the real HomeView and native list handles.
use super::*;
use crate::domain::home::*;
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point, size};

fn open(cx: &mut TestAppContext) -> (Entity<HomeView>, &mut VisualTestContext) {
    cx.update(ely_core::init_for_tests);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = HomeView::new(RequestBinding::new("viewport"), 1, window, cx);
        view.set_now(Some(SystemTime::UNIX_EPOCH), cx);
        view
    });
    cx.update(|window, _| window.activate_window());
    (view, cx)
}

fn populate(view: &Entity<HomeView>, count: usize, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| {
        let binding = view.binding.clone();
        view.apply_home(
            &binding,
            1,
            HomeSearchOutcome {
                items: (0..count)
                    .map(|index| HomeItem {
                        id: index as u64 + 1,
                        kind: HomeItemKind::Issue,
                        number: index as u64 + 1,
                        title: format!("Viewport item {index}"),
                        html_url: String::new(),
                        state: "open".into(),
                        updated_at: "2026-11-01T00:00:00Z".into(),
                        repository: HomeRepositoryRef {
                            owner: "owner".into(),
                            name: "repo".into(),
                        },
                    })
                    .collect(),
                total_count: count as u64,
                incomplete_results: false,
                readiness: HomeReadiness::Complete,
            },
            cx,
        );
        view.apply_feed(
            &binding,
            1,
            FeedOutcome {
                activities: (0..count)
                    .map(|index| {
                        let subject = FeedActor {
                            id: index as u64 + 1,
                            login: "actor".into(),
                            display_login: None,
                            avatar_url: None,
                        };
                        FeedActivity {
                            kind: FeedActivityKind::RepositoryStarred,
                            subject: subject.clone(),
                            pull_request: None,
                            repository_description: None,
                            event: FeedEvent {
                                id: index.to_string(),
                                event_type: FeedEventType::Watch,
                                actor: subject,
                                repository: FeedRepository {
                                    id: 1,
                                    name: "owner/repo".into(),
                                    url: String::new(),
                                },
                                created_at: None,
                                public: true,
                                payload: FeedPayload::Watch {
                                    action: "started".into(),
                                },
                            },
                        }
                    })
                    .collect(),
                etag: None,
                poll_interval_seconds: None,
                next_page: None,
                readiness: FeedReadiness::Complete,
            },
            cx,
        );
        cx.notify();
    });
    frames(cx);
}

fn frames(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
}
fn bounds(cx: &mut VisualTestContext, name: String) -> gpui::Bounds<Pixels> {
    cx.debug_bounds(Box::leak(name.into_boxed_str())).unwrap()
}

#[gpui::test]
fn first_and_last_rows_fit_the_real_viewport_at_both_layout_widths(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    for count in [1, 80] {
        for dimensions in [size(px(1180.0), px(800.0)), size(px(640.0), px(800.0))] {
            cx.simulate_resize(dimensions);
            populate(&view, count, cx);
            for (column, name, row_name) in [
                (Column::Home, "home", "home-item-row"),
                (Column::Feed, "feed", "feed-event-row"),
            ] {
                let header = bounds(cx, format!("{name}-header"));
                let viewport = view.read_with(cx, |v, _| match column {
                    Column::Home => v.home_view.scroll.viewport_bounds(),
                    Column::Feed => v.feed_view.scroll.viewport_bounds(),
                });
                let first = bounds(cx, format!("{row_name}-0"));
                assert!(
                    viewport.top() >= header.bottom(),
                    "{name}: {viewport:?} {header:?}"
                );
                assert_eq!(
                    first.top(),
                    viewport.top(),
                    "offset zero must show the complete first row"
                );
                assert!(
                    first.bottom() <= viewport.bottom(),
                    "{name}: {first:?} {viewport:?}"
                );
                cx.update(|window, cx| {
                    view.update(cx, |v, cx| v.panel_mut(column).focus.focus(window, cx))
                });
                cx.simulate_keystrokes("end");
                frames(cx);
                let last = bounds(cx, format!("{row_name}-{}", count - 1));
                assert!(
                    last.top() >= viewport.top() - px(1.0)
                        && last.bottom() <= viewport.bottom() + px(1.0),
                    "{name}: last={last:?}, viewport={viewport:?}"
                );
                view.update(cx, |v, cx| {
                    v.panel_mut(column).motion.cancel();
                    v.panel_mut(column).scroll.scroll_to_end();
                    cx.notify();
                });
                frames(cx);
                let last = bounds(cx, format!("{row_name}-{}", count - 1));
                assert!(
                    last.bottom() <= viewport.bottom() + px(1.0),
                    "max offset exposes the last row"
                );
                cx.simulate_keystrokes("home");
                frames(cx);
                assert_eq!(bounds(cx, format!("{row_name}-0")).top(), viewport.top());
            }
        }
    }
}

#[gpui::test]
fn scrollbar_hit_lane_is_outside_rows_and_drives_only_its_list(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_resize(size(px(1180.0), px(800.0)));
    populate(&view, 80, cx);
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select(Column::Home, 0, window, cx);
            v.select(Column::Feed, 0, window, cx);
        })
    });
    view.update(cx, |v, cx| {
        v.show_scrollbar(Column::Home);
        v.show_scrollbar(Column::Feed);
        cx.notify();
    });
    frames(cx);
    for (column, name, row_name) in [
        (Column::Home, "home", "home-item-row"),
        (Column::Feed, "feed", "feed-event-row"),
    ] {
        let row = bounds(cx, format!("{row_name}-0"));
        let region = bounds(cx, format!("{name}-rows"));
        let hit = bounds(cx, format!("{name}-scrollbar-y-thumb"));
        let line = bounds(cx, format!("{name}-scrollbar-y-line"));
        assert!(
            row.right() <= hit.left(),
            "row background must end before scrollbar hit lane: {row:?} {hit:?}"
        );
        assert_eq!(region.right() - line.right(), px(1.5));
        assert!(line.left() - row.right() <= px(8.0));
        let other = if column == Column::Home {
            Column::Feed
        } else {
            Column::Home
        };
        let other_before = view.read_with(cx, |v, _| v.offset(other));
        let start = point(hit.right() - px(0.5), hit.center().y);
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::none());
        for step in 1..=5 {
            cx.simulate_mouse_move(
                start + point(px(0.0), px(12.0 * step as f32)),
                Some(gpui::MouseButton::Left),
                Modifiers::none(),
            );
            frames(cx);
        }
        cx.simulate_mouse_up(
            start + point(px(0.0), px(60.0)),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
        frames(cx);
        assert!(view.read_with(cx, |v, _| v.offset(column).y) < px(-20.0));
        assert_eq!(view.read_with(cx, |v, _| v.offset(other)), other_before);
        view.update(cx, |v, cx| {
            v.panel_mut(column).bars.snap(false);
            cx.notify();
        });
        frames(cx);
        let viewport = bounds(cx, format!("{name}-list-viewport"));
        assert_eq!(
            viewport.right(),
            hit.left(),
            "hiding the scrollbar does not reflow rows"
        );
    }
}
