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

// Inspect the scene actually submitted to the renderer, including its inherited clip.
// The probe sits in the middle of a solid row, away from rounded corners and borders.
fn assert_edge_painted(
    cx: &mut VisualTestContext,
    row: gpui::Bounds<Pixels>,
    edge: Pixels,
    top: bool,
) {
    cx.update(|window, _| {
        let scale = window.scale_factor();
        let quad = window.painted_quads().into_iter().find(|quad| {
            quad.bounds == row.scale(scale) && !quad.background.is_transparent()
        }).expect("the edge row must submit a background quad");
        let visible = quad.bounds.intersect(&quad.content_mask.bounds);
        let probe = gpui::Bounds::new(
            point(row.center().x - px(1.0), edge + if top { px(0.25) } else { px(-0.75) }),
            size(px(2.0), px(0.5)),
        ).scale(scale);
        assert!(visible.contains(&probe.origin) && visible.contains(&probe.bottom_right()),
            "opaque inset band at intended edge: row={:?}, clip={:?}, visible={visible:?}, probe={probe:?}",
            quad.bounds, quad.content_mask.bounds);
    });
}

fn intermediate_home_feed_edge(cx: &mut TestAppContext, top: bool, partial: bool) {
    let (view, cx) = open(cx);
    for (width, height, font_scale, display_scale) in [
        (1180.0, 1000.0, 1.0, 1.0),
        (640.0, 1400.0, 1.0, 1.0),
        (1180.0, 1400.0, 1.5, 1.5),
        (640.0, 1800.0, 1.5, 1.5),
    ] {
        cx.simulate_scale_factor_change(display_scale);
        cx.update(|_, cx| {
            ely_core::theme::Theme::update(cx, |theme| theme.font_scale = font_scale);
        });
        cx.simulate_resize(size(px(width), px(height)));
        populate(&view, 80, cx);
        if partial {
            view.update(cx, |view, cx| {
                let binding = view.binding.clone();
                let mut home = view.home.state.data().unwrap().clone();
                home.readiness = HomeReadiness::Partial {
                    reasons: vec![HomePartialReason::ResultLimit],
                };
                view.apply_home(&binding, 1, home, cx);
                let mut feed = view.feed.state.data().unwrap().clone();
                feed.readiness = FeedReadiness::Partial {
                    reasons: vec![FeedPartialReason::PageLimit],
                };
                view.apply_feed(&binding, 1, feed, cx);
            });
            frames(cx);
        }
        for (column, name, row_name, paint_name) in [
            (Column::Home, "home", "home-item-row", "home-item-row"),
            (Column::Feed, "feed", "feed-event-row", "feed-card"),
        ] {
            view.update(cx, |view, cx| {
                let panel = view.panel_mut(column);
                panel.motion.cancel();
                panel.pending_reveal = None;
                panel.selected = None;
                panel.scroll.scroll_to(gpui::ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.0),
                });
                cx.notify();
            });
            frames(cx);
            // Advance through real input so unmeasured list estimates cannot clamp a jump.
            let position = bounds(cx, format!("{name}-rows")).center();
            cx.simulate_mouse_move(position, None, Modifiers::none());
            for _ in 0..80 {
                cx.simulate_event(gpui::ScrollEvent {
                    position,
                    delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-17.0))),
                    modifiers: Modifiers::none(),
                    touch_phase: Default::default(),
                    source: gpui::ScrollSource::Pointer,
                });
                frames(cx);
                let anchor = view.read_with(cx, |view, _| match column {
                    Column::Home => view.home_view.scroll.logical_scroll_top(),
                    Column::Feed => view.feed_view.scroll.logical_scroll_top(),
                });
                if anchor.item_ix >= 2 && anchor.offset_in_item >= px(12.0) {
                    break;
                }
            }
            let anchor = view.read_with(cx, |view, _| match column {
                Column::Home => view.home_view.scroll.logical_scroll_top(),
                Column::Feed => view.feed_view.scroll.logical_scroll_top(),
            });
            assert!(
                anchor.item_ix >= 2 && anchor.offset_in_item >= px(12.0),
                "real input establishes an intermediate offset: {anchor:?}"
            );
            view.update(cx, |view, cx| {
                view.panel_mut(column).selected = Some(anchor.item_ix);
                cx.notify();
            });
            frames(cx);
            let root = bounds(cx, "home-view".into());
            let panel = bounds(cx, format!("{name}-panel"));
            let header = bounds(cx, format!("{name}-header"));
            let side_by_side =
                bounds(cx, "home-header".into()).top() == bounds(cx, "feed-header".into()).top();
            let edge = if top {
                if partial {
                    bounds(cx, format!("{name}-partial")).bottom()
                } else {
                    header.bottom()
                }
            } else if side_by_side || column == Column::Feed {
                root.bottom()
            } else {
                panel.bottom()
            };
            let index = if top {
                anchor.item_ix
            } else {
                let native = view.read_with(cx, |view, _| match column {
                    Column::Home => view.home_view.scroll.viewport_bounds(),
                    Column::Feed => view.feed_view.scroll.viewport_bounds(),
                });
                (anchor.item_ix..80)
                    .find(|index| {
                        cx.debug_bounds(Box::leak(format!("{row_name}-{index}").into_boxed_str()))
                            .is_some_and(|row| {
                                row.top() < native.bottom() && row.bottom() >= native.bottom()
                            })
                    })
                    .expect("an intermediate row crosses the viewport bottom")
            };
            if !top {
                let row = bounds(cx, format!("{paint_name}-{index}"));
                view.update(cx, |view, cx| {
                    let panel = view.panel_mut(column);
                    panel.selected = Some(index);
                    let offset =
                        scroll::ScrollTarget::offset(&panel.scroll).y + edge - row.center().y;
                    scroll::ScrollTarget::set_axis_offset(
                        &panel.scroll,
                        gpui::Axis::Vertical,
                        offset,
                    );
                    cx.notify();
                });
                frames(cx);
            }
            let row = bounds(cx, format!("{paint_name}-{index}"));
            assert!(
                row.top() < edge && row.bottom() > edge,
                "the probe must be inside a partially scrolled row: {row:?} edge={edge:?}"
            );
            assert_edge_painted(cx, row, edge, top);
            // Input in the former blank band must reach this column exactly once.
            let position = point(row.center().x, edge + if top { px(0.5) } else { px(-0.5) });
            cx.simulate_mouse_move(position, None, Modifiers::none());
            frames(cx);
            let before = view.read_with(cx, |view, _| view.offset(column).y);
            cx.simulate_event(gpui::ScrollEvent {
                position,
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-9.0))),
                modifiers: Modifiers::none(),
                touch_phase: Default::default(),
                source: gpui::ScrollSource::Pointer,
            });
            frames(cx);
            let after = view.read_with(cx, |view, _| view.offset(column).y);
            assert!(
                (after - before + px(9.0)).abs() < px(1.0),
                "edge input must scroll the real list once"
            );
        }
    }
}

#[gpui::test]
fn intermediate_home_feed_paint_reaches_fixed_header(cx: &mut TestAppContext) {
    intermediate_home_feed_edge(cx, true, false);
}

#[gpui::test]
fn intermediate_home_feed_paint_reaches_panel_bottom(cx: &mut TestAppContext) {
    intermediate_home_feed_edge(cx, false, false);
}

#[gpui::test]
fn intermediate_home_feed_paint_reaches_partial_notice(cx: &mut TestAppContext) {
    intermediate_home_feed_edge(cx, true, true);
}

fn endpoint_spacing(cx: &mut TestAppContext, end: bool, partial: bool) {
    let (view, cx) = open(cx);
    for (width, height, font_scale) in [
        (1180.0, 1000.0, 1.0),
        (640.0, 1400.0, 1.0),
        (1180.0, 1400.0, 1.5),
        (640.0, 1800.0, 1.5),
    ] {
        cx.update(|_, cx| {
            ely_core::theme::Theme::update(cx, |theme| theme.font_scale = font_scale)
        });
        cx.simulate_resize(size(px(width), px(height)));
        for count in [1, 80, 0] {
            populate(&view, count, cx);
            if partial {
                view.update(cx, |v, cx| {
                    let binding = v.binding.clone();
                    let mut home = v.home.state.data().unwrap().clone();
                    home.readiness = HomeReadiness::Partial {
                        reasons: vec![HomePartialReason::ResultLimit],
                    };
                    v.apply_home(&binding, 1, home, cx);
                    let mut feed = v.feed.state.data().unwrap().clone();
                    feed.readiness = FeedReadiness::Partial {
                        reasons: vec![FeedPartialReason::PageLimit],
                    };
                    v.apply_feed(&binding, 1, feed, cx);
                });
                frames(cx);
            }
            for (column, name, row_name) in [
                (Column::Home, "home", "home-item-row"),
                (Column::Feed, "feed", "feed-event-row"),
            ] {
                let header = bounds(cx, format!("{name}-header"));
                let content_start = if partial {
                    let notice = bounds(cx, format!("{name}-partial"));
                    assert!((notice.top() - header.bottom() - px(8.0)).abs() < px(0.01));
                    notice.bottom()
                } else {
                    header.bottom()
                };
                let panel = bounds(cx, format!("{name}-panel"));
                if count == 0 {
                    let empty = bounds(cx, format!("{name}-empty"));
                    assert_eq!(view.read_with(cx, |v, _| v.item_count(column)), 0);
                    assert!(
                        cx.debug_bounds(Box::leak(format!("{row_name}-0").into_boxed_str()))
                            .is_none()
                    );
                    assert!((empty.top() - content_start - px(8.0)).abs() < px(0.01));
                    assert!((panel.bottom() - empty.bottom() - px(8.0)).abs() < px(0.01));
                    continue;
                }
                cx.update(|window, cx| view.update(cx, |v, cx| v.select(column, 0, window, cx)));
                frames(cx);
                let first = bounds(cx, format!("{row_name}-0"));
                if !end {
                    assert!(
                        (first.top() - content_start - px(8.0)).abs() < px(0.01),
                        "mandatory initial spacing: {name} first={first:?} header={header:?}"
                    );
                    if column == Column::Home {
                        assert_eq!(first.size.height, px(rows::HOME_ROW_HEIGHT));
                    }
                } else {
                    cx.update(|window, cx| {
                        view.update(cx, |v, cx| v.panel_mut(column).focus.focus(window, cx))
                    });
                    cx.simulate_keystrokes("end");
                    frames(cx);
                    let native = view.read_with(cx, |v, _| match column {
                        Column::Home => v.home_view.scroll.viewport_bounds(),
                        Column::Feed => v.feed_view.scroll.viewport_bounds(),
                    });
                    let last = bounds(cx, format!("{row_name}-{}", count - 1));
                    let gap = native.bottom() - last.bottom();
                    if count > 1 {
                        assert!(
                            (gap - px(8.0)).abs() < px(0.02),
                            "mandatory end spacing: {name} gap={gap:?}"
                        );
                    } else {
                        assert!(gap >= px(8.0));
                    }
                    view.update(cx, |v, cx| {
                        let p = v.panel_mut(column);
                        p.motion.cancel();
                        p.scroll.scroll_to_end();
                        cx.notify();
                    });
                    frames(cx);
                    let last = bounds(cx, format!("{row_name}-{}", count - 1));
                    assert!(native.bottom() - last.bottom() >= px(7.98));
                }
            }
        }
    }
}

#[gpui::test]
fn endpoint_initial_spacing_is_preserved(cx: &mut TestAppContext) {
    endpoint_spacing(cx, false, false);
}

#[gpui::test]
fn endpoint_final_spacing_is_preserved(cx: &mut TestAppContext) {
    endpoint_spacing(cx, true, false);
}

#[gpui::test]
fn endpoint_partial_initial_spacing_is_preserved(cx: &mut TestAppContext) {
    endpoint_spacing(cx, false, true);
}
#[gpui::test]
fn endpoint_partial_final_spacing_is_preserved(cx: &mut TestAppContext) {
    endpoint_spacing(cx, true, true);
}

#[gpui::test]
fn endpoint_thumb_max_keeps_unmeasured_tail_spacing(cx: &mut TestAppContext) {
    use crate::ui::scroll::ScrollTarget;
    use gpui::Axis;
    let (view, cx) = open(cx);
    cx.simulate_resize(size(px(1180.0), px(1000.0)));
    populate(&view, 80, cx);
    assert!(view.read_with(cx, |v, _| v.home_view.scroll.bounds_for_item(79).is_none()));
    view.update(cx, |v, cx| {
        let motion = &v.home_view.motion;
        motion.drag_started(Axis::Vertical);
        motion.set_axis_offset(Axis::Vertical, -motion.max_offset().y);
        cx.notify();
    });
    frames(cx);
    let last = cx.debug_bounds("home-item-row-79").unwrap_or_else(|| {
        panic!(
            "tail missing after max drag: {:?}",
            view.read_with(cx, |v, _| (
                v.home_view.scroll.logical_scroll_top(),
                v.home_view.motion.offset(),
                v.home_view.motion.max_offset(),
                v.home_view.scroll.max_offset_for_scrollbar(),
                v.home_view.scroll.viewport_bounds()
            ))
        )
    });
    let viewport = view.read_with(cx, |v, _| v.home_view.scroll.viewport_bounds());
    assert!(
        (viewport.bottom() - last.bottom() - px(8.0)).abs() < px(0.02),
        "a frozen scrollbar range must include the required tail gap"
    );
    view.update(cx, |v, cx| {
        v.home_view.motion.drag_ended(Axis::Vertical);
        cx.notify();
    });
    frames(cx);
    assert!(
        (viewport.bottom() - bounds(cx, "home-item-row-79".into()).bottom() - px(8.0)).abs()
            < px(0.02)
    );
}

#[gpui::test]
fn endpoint_append_moves_only_the_content_tail_gap(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_resize(size(px(1180.0), px(1000.0)));
    populate(&view, 3, cx);
    let old = bounds(cx, "feed-event-row-2".into());
    let measured = view.read_with(cx, |v, _| v.feed_view.scroll.bounds_for_item(2).unwrap());
    assert!((measured.size.height - old.size.height).abs() < px(0.01));
    let anchor = view.read_with(cx, |v, _| v.feed_view.scroll.logical_scroll_top());
    view.update(cx, |v, cx| {
        let mut out = v.feed.state.data().unwrap().clone();
        for i in 3..30 {
            let mut item = out.activities[0].clone();
            item.event.id = format!("append-{i}");
            out.activities.push(item);
        }
        assert!(v.apply_feed_page(&v.binding.clone(), 1, out, cx));
    });
    frames(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.feed_view.scroll.logical_scroll_top()),
        anchor
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.feed_view.scroll.item_count()),
        30
    );
    let row = bounds(cx, "feed-event-row-2".into());
    let measured = view.read_with(cx, |v, _| v.feed_view.scroll.bounds_for_item(2).unwrap());
    assert!(
        (measured.size.height - row.size.height).abs() < px(0.01),
        "the former tail must become an ordinary interior item"
    );
    cx.update(|w, cx| view.update(cx, |v, cx| v.select(Column::Feed, 29, w, cx)));
    frames(cx);
    let last = bounds(cx, "feed-event-row-29".into());
    let viewport = view.read_with(cx, |v, _| v.feed_view.scroll.viewport_bounds());
    assert!((viewport.bottom() - last.bottom() - px(8.0)).abs() < px(0.02));
}

#[gpui::test]
fn endpoint_feed_footer_keeps_its_original_spacing(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_resize(size(px(1180.0), px(1000.0)));
    populate(&view, 80, cx);
    view.update(cx, |v, cx| {
        assert!(v.feed_append_loading(&v.binding.clone(), 1, cx));
    });
    frames(cx);
    let footer = bounds(cx, "feed-append-loading".into());
    let panel = bounds(cx, "feed-panel".into());
    assert!((panel.bottom() - footer.bottom() - px(8.0)).abs() < px(0.01));
    cx.update(|w, cx| view.update(cx, |v, cx| v.select(Column::Feed, 79, w, cx)));
    frames(cx);
    let last = bounds(cx, "feed-event-row-79".into());
    assert!(
        (footer.top() - last.bottom()).abs() < px(0.02),
        "real footer spacing must not be doubled by a second end inset"
    );
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
                    viewport.top() + px(8.0),
                    "offset zero preserves the original heading-to-content spacing"
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
                assert_eq!(
                    bounds(cx, format!("{row_name}-0")).top(),
                    viewport.top() + px(8.0)
                );
            }
        }
    }
}

#[gpui::test]
fn inset_scrollbar_thumb_stays_in_track_and_drag_reaches_end(cx: &mut TestAppContext) {
    use crate::ui::scroll::ScrollTarget;
    let (view, cx) = open(cx);
    for (width, height, font_scale, count) in [(1180.0, 800.0, 1.0, 80), (640.0, 1400.0, 1.5, 1000)]
    {
        cx.update(|_, cx| ely_core::theme::Theme::update(cx, |t| t.font_scale = font_scale));
        cx.simulate_resize(size(px(width), px(height)));
        populate(&view, count, cx);
        view.update(cx, |v, cx| {
            v.home_view.pinned_clock = Some(0);
            v.show_scrollbar(Column::Home);
            let m = &v.home_view.motion;
            m.set_axis_offset(gpui::Axis::Vertical, -m.max_offset().y);
            cx.notify();
        });
        frames(cx);
        let track = bounds(cx, "home-scrollbar-y".into());
        let thumb = bounds(cx, "home-scrollbar-y-thumb".into());
        assert!(
            (thumb.bottom() - track.bottom()).abs() < px(0.02),
            "maximum thumb must end inside its physical track: thumb={thumb:?} track={track:?}"
        );
        cx.update(|w, cx| view.update(cx, |v, cx| v.select(Column::Home, 0, w, cx)));
        frames(cx);
        let thumb = bounds(cx, "home-scrollbar-y-thumb".into());
        let start = thumb.center();
        cx.simulate_mouse_move(start, None, Modifiers::none());
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::none());
        for step in 1..=8 {
            cx.simulate_mouse_move(
                start + point(px(0.0), px(step as f32)),
                Some(gpui::MouseButton::Left),
                Modifiers::none(),
            );
            frames(cx);
        }
        let end = point(start.x, track.bottom() - px(0.25));
        cx.simulate_mouse_move(end, Some(gpui::MouseButton::Left), Modifiers::none());
        frames(cx);
        cx.simulate_mouse_up(end, gpui::MouseButton::Left, Modifiers::none());
        frames(cx);
        view.read_with(cx, |v, _| {
            assert!(
                (v.home_view.motion.offset().y + v.home_view.motion.max_offset().y).abs()
                    < px(0.02),
                "drag to physical track bottom reaches native range end"
            )
        });
        let native = view.read_with(cx, |v, _| v.home_view.scroll.viewport_bounds());
        let last = bounds(cx, format!("home-item-row-{}", count - 1));
        assert!((native.bottom() - last.bottom() - px(8.0)).abs() < px(0.02));
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
