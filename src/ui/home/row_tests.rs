//! Renderer regressions for the rejected #83 acceptance cases.
use super::*;
use ely_core::theme::{ActiveTheme, TextSize};
use std::time::Duration;

// This suite is also compiled by the standalone Home integration target, whose
// crate root deliberately has no platform module. Embed the same production assets
// here instead of loading fonts.rs a second time in the application test target.
mod bundled_fonts {
    pub const UI_FAMILY: &str = "Githux UI";
    pub const CODE_FAMILY: &str = "Google Sans Code";

    pub fn register(cx: &gpui::App) -> gpui::Result<()> {
        let faces: [&[u8]; 5] = [
            include_bytes!("../../../assets/fonts/githux-ui/GithuxUI-Regular.ttf"),
            include_bytes!("../../../assets/fonts/githux-ui/GithuxUI-Medium.ttf"),
            include_bytes!("../../../assets/fonts/githux-ui/GithuxUI-SemiBold.ttf"),
            include_bytes!("../../../assets/fonts/githux-ui/GithuxUI-Bold.ttf"),
            include_bytes!("../../../assets/fonts/google-sans-code/GoogleSansCode.ttf"),
        ];
        cx.text_system()
            .add_fonts(faces.into_iter().map(std::borrow::Cow::Borrowed).collect())
    }
}

fn hover_settle(at: gpui::Point<gpui::Pixels>, cx: &mut VisualTestContext) {
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
}

struct ThemedHome(Entity<HomeView>);
impl gpui::Render for ThemedHome {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
            .size_full()
            .font_family(cx.theme().font_family.clone())
            .child(self.0.clone())
    }
}

fn check_rendered_rows(
    cx: &mut TestAppContext,
    native_text: Option<std::sync::Arc<dyn gpui::PlatformTextSystem>>,
) {
    setup(cx);
    cx.update(|cx| {
        cx.set_global(crate::ui::work_item_row::TextHover::default());
        bundled_fonts::register(cx).unwrap();
        ely_core::theme::Theme::update(cx, |t| {
            t.font_family = bundled_fonts::UI_FAMILY.into();
            t.mono_family = bundled_fonts::CODE_FAMILY.into();
        });
    });
    let (host, cx) = cx.add_window_view(|window, cx| {
        ThemedHome(cx.new(|cx| HomeView::new(binding(), 1, window, cx)))
    });
    let view = host.read_with(cx, |host, _| host.0.clone());
    cx.update(|window, _| window.activate_window());
    view.update(cx, |v, cx| {
        v.set_now(
            Some(std::time::UNIX_EPOCH + Duration::from_secs(1_793_188_800)),
            cx,
        )
    });
    let mut item = home_item(0, HomeItemKind::PullRequest);
    item.title = "Review changes".into();
    item.number = 1136;
    item.repository.owner = "alibaba".into();
    item.repository.name = "open-code-review".into();
    let mut long = home_item(1, HomeItemKind::Issue);
    long.title = "中文 English 标题与很长的工作条目标题 ".repeat(10);
    long.repository.owner = "organization-with-a-long-name".into();
    long.repository.name = "中文-English-long-repository-name-".repeat(8);
    let mut feed = merged_activity(0, author(901, "contributor"));
    feed.pull_request.as_mut().unwrap().body_preview = Some(BodyPreview {
        excerpt: "Preview…".into(),
        is_truncated: true,
    });
    view.update(cx, |v, cx| {
        assert!(v.apply_home(
            &binding(),
            1,
            home_outcome(vec![item, long], HomeReadiness::Complete),
            cx
        ));
        assert!(v.apply_feed(
            &binding(),
            1,
            feed_outcome(vec![feed], FeedReadiness::Complete),
            cx
        ));
    });
    let actions = action_sink(&view, cx);
    for scale in [1.0, 1.5, 2.0] {
        cx.simulate_scale_factor_change(scale);
        for width in [1180.0, 880.0, 640.0, 320.0] {
            cx.simulate_resize(size(px(width), px(900.0)));
            cx.update(|window, cx| view.update(cx, |v, cx| v.select(Column::Home, 0, window, cx)));
            cx.run_until_parked();
            let row = bounds("home-item-row-0", cx);
            let title = bounds("home-title-0", cx);
            let meta = bounds("home-metadata-0", cx);
            let number = bounds("home-0-number", cx);
            let owner = bounds("home-0-owner", cx);
            let slash = bounds("home-item-row-0-separator", cx);
            let repo = bounds("home-0-repository", cx);
            let state = bounds("home-item-row-0-state", cx);
            // Check the actual selected item, not a constant or justify_center style.
            assert!(
                row.size.height <= px(56.0),
                "excessive selected fill: {row:?}"
            );
            let top = title.top() - row.top();
            let bottom = row.bottom() - meta.bottom();
            assert!(
                top <= px(8.5) && bottom <= px(10.5),
                "excessive padding: {top:?}/{bottom:?}"
            );
            assert!(
                (top + px(2.0) - bottom).abs() <= px(1.0),
                "line boxes lost optical lift: {top:?}/{bottom:?}"
            );
            assert!(
                (number.left() - title.left()).abs() <= px(1.0),
                "metadata starts under title text"
            );
            assert!(
                (slash.left() - owner.right()).abs() <= px(1.0),
                "layout gap before slash"
            );
            assert!(
                (repo.left() - slash.right()).abs() <= px(1.0),
                "layout gap after slash"
            );
            for cell in [number, owner, slash, repo] {
                assert!((cell.center().y - meta.center().y).abs() <= px(1.0));
                assert!(cell.size.width > px(0.0) && cell.size.height > px(0.0));
            }
            if width >= 640.0
                && let Some(native_text) = &native_text
            {
                let (ink_top, ink_bottom) = cx.update(|window, cx| {
                    let mut top = f32::INFINITY;
                    let mut bottom = f32::NEG_INFINITY;
                    for (label, cell, text_size, weight, family) in [
                        (
                            "Review changes",
                            title,
                            TextSize::Base,
                            gpui::FontWeight::MEDIUM,
                            bundled_fonts::UI_FAMILY,
                        ),
                        (
                            "#1136",
                            number,
                            TextSize::Xs,
                            gpui::FontWeight::NORMAL,
                            bundled_fonts::CODE_FAMILY,
                        ),
                        (
                            "alibaba",
                            owner,
                            TextSize::Xs,
                            gpui::FontWeight::NORMAL,
                            bundled_fonts::UI_FAMILY,
                        ),
                        (
                            "open-code-review",
                            repo,
                            TextSize::Xs,
                            gpui::FontWeight::NORMAL,
                            bundled_fonts::UI_FAMILY,
                        ),
                    ] {
                        let font = gpui::Font {
                            family: family.into(),
                            weight,
                            ..Default::default()
                        };
                        let run = gpui::TextRun {
                            len: label.len(),
                            font,
                            ..Default::default()
                        };
                        let font_size =
                            cx.theme().text_size(text_size).to_pixels(window.rem_size());
                        let line = window
                            .text_system()
                            .layout_line(label, font_size, &[run], None);
                        let baseline = f32::from(
                            cell.top()
                                + (cell.size.height - line.ascent - line.descent) / 2.0
                                + line.ascent,
                        );
                        for run in &line.runs {
                            for glyph in &run.glyphs {
                                let params = gpui::RenderGlyphParams {
                                    font_id: run.font_id,
                                    glyph_id: glyph.id,
                                    font_size,
                                    subpixel_variant: point(0, 0),
                                    scale_factor: scale,
                                    is_emoji: false,
                                    subpixel_rendering: false,
                                    dilation: 0,
                                };
                                let raster = native_text.glyph_raster_bounds(&params).unwrap();
                                if raster.size.width.0 == 0 || raster.size.height.0 == 0 {
                                    continue;
                                }
                                let (_, pixels) =
                                    native_text.rasterize_glyph(&params, raster).unwrap();
                                let raster_width = raster.size.width.0 as usize;
                                for (y, scan) in pixels.chunks(raster_width).enumerate() {
                                    if scan.iter().any(|alpha| *alpha > 32) {
                                        let ink_y = baseline
                                            + f32::from(glyph.position.y)
                                            + (raster.origin.y.0 as f32 + y as f32) / scale;
                                        top = top.min(ink_y);
                                        bottom = bottom.max(ink_y + 1.0 / scale);
                                    }
                                }
                            }
                        }
                    }
                    (top, bottom)
                });
                let above_ink = ink_top - f32::from(row.top());
                let below_ink = f32::from(row.bottom()) - ink_bottom;
                eprintln!(
                    "row ink: scale={scale}, width={width}, above={above_ink:.2}, below={below_ink:.2}"
                );
                assert!(
                    (above_ink - below_ink).abs() <= 2.5,
                    "painted padding imbalance: {above_ink}/{below_ink}"
                );
                assert!(
                    above_ink <= 15.0 && below_ink <= 15.0,
                    "excessive painted padding"
                );
            }
            // Independent shaped text measurement catches the old block div stretching.
            let measured = cx.update(|window, cx| {
                let font = gpui::Font {
                    family: cx.theme().font_family.clone(),
                    ..Default::default()
                };
                let label = "open-code-review";
                let run = gpui::TextRun {
                    len: label.len(),
                    font,
                    ..Default::default()
                };
                window
                    .text_system()
                    .layout_line(
                        label,
                        cx.theme()
                            .text_size(TextSize::Xs)
                            .to_pixels(window.rem_size()),
                        &[run],
                        None,
                    )
                    .width
            });
            assert!(
                repo.right() + px(2.0) < state.left(),
                "fixture needs real trailing blank space"
            );
            actions.borrow_mut().clear();
            let blank = if width >= 640.0 {
                point(repo.left() + measured + px(4.0), repo.center().y)
            } else {
                point((repo.right() + state.left()) / 2.0, repo.center().y)
            };
            hover_settle(blank, cx);
            assert!(
                !cx.update(|_, cx| cx
                    .global::<crate::ui::work_item_row::TextHover>()
                    .0
                    .contains("home-0-repository")),
                "repo hovered on blank"
            );
            assert!(
                cx.debug_bounds("home-0-repository-tooltip").is_none(),
                "repo tooltip on trailing blank space"
            );
            cx.simulate_click(blank, Modifiers::none());
            cx.run_until_parked();
            assert!(actions.borrow().is_empty(), "blank metadata navigated");
            if width >= 640.0 {
                assert!(
                    (repo.size.width - measured).abs() <= px(1.0),
                    "repo hit target wider than shaped text: {:?} vs {measured:?}",
                    repo.size.width
                );
            } else {
                assert!(repo.size.width > px(0.0) && repo.size.width <= measured);
            }
            hover_settle(repo.center(), cx);
            assert!(cx.update(|_, cx| {
                cx.global::<crate::ui::work_item_row::TextHover>()
                    .0
                    .contains("home-0-repository")
            }));
            assert!(
                cx.debug_bounds("home-0-repository-tooltip").is_some(),
                "text tooltip never appeared"
            );
            click("home-0-repository", cx);
            assert!(
                matches!(actions.borrow().last(), Some(HomeAction::Repository(r)) if r.full_name()=="alibaba/open-code-review")
            );
            hover_settle(blank, cx);
            assert!(
                cx.debug_bounds("home-0-repository-tooltip").is_none(),
                "tooltip survived leaving text"
            );
            actions.borrow_mut().clear();
            let blank_title = point(title.right() + px(8.0), title.center().y);
            assert!(blank_title.x < row.right());
            hover_settle(blank_title, cx);
            assert!(!cx.update(|_, cx| {
                cx.global::<crate::ui::work_item_row::TextHover>()
                    .0
                    .contains("home-title-0")
            }));
            assert!(cx.debug_bounds("home-title-0-tooltip").is_none());
            cx.simulate_click(blank_title, Modifiers::none());
            cx.run_until_parked();
            assert!(actions.borrow().is_empty(), "blank title activated");
            hover_settle(title.center(), cx);
            assert!(cx.debug_bounds("home-title-0-tooltip").is_some());
            click("home-title-0", cx);
            assert!(matches!(
                actions.borrow().last(),
                Some(HomeAction::ActivateWorkItem(_))
            ));
            let long_row = bounds("home-item-row-1", cx);
            assert!(
                (long_row.top() - row.bottom()).abs() <= px(1.0),
                "selection fill must match the real contiguous item bounds"
            );
            let long_title = bounds("home-title-1", cx);
            let long_repo = bounds("home-1-repository", cx);
            assert!(long_title.right() <= long_row.right() - px(7.0));
            assert!(
                long_repo.size.width > px(0.0),
                "long owner must not eliminate repo target"
            );
            assert!(long_repo.right() < long_row.right());
            actions.borrow_mut().clear();
            hover_settle(long_repo.center(), cx);
            assert!(
                cx.debug_bounds("home-1-repository-tooltip").is_some(),
                "truncated repo lost tooltip"
            );
            click("home-1-repository", cx);
            assert!(
                matches!(actions.borrow().last(), Some(HomeAction::Repository(r)) if r.owner=="organization-with-a-long-name" && r.name.starts_with("中文-English"))
            );
            actions.borrow_mut().clear();
            click("home-0-owner", cx);
            assert!(
                matches!(actions.borrow().last(), Some(HomeAction::Profile(p)) if p.login=="alibaba")
            );
        }
    }
    // Feed column/card ancestors have different flex directions: test their real targets too.
    cx.simulate_resize(size(px(1180.0), px(900.0)));
    cx.run_until_parked();
    let time = bounds("feed-time-0", cx);
    let card = bounds("feed-card-0", cx);
    let time_blank = point(card.right() - px(20.0), time.center().y);
    assert!(time_blank.x > time.right() + px(4.0));
    hover_settle(time_blank, cx);
    assert!(cx.debug_bounds("feed-time-0-tooltip").is_none());
    hover_settle(time.center(), cx);
    assert!(cx.debug_bounds("feed-time-0-tooltip").is_some());
    for target in [
        "feed-actor-0",
        "feed-0-repository",
        "feed-title-0",
        "feed-pr-read-more-0",
    ] {
        let text = bounds(target, cx);
        let card = bounds("feed-card-0", cx);
        let blank = point(card.right() - px(20.0), text.center().y);
        assert!(
            blank.x > text.right() + px(4.0),
            "{target} still stretched: {text:?}"
        );
        actions.borrow_mut().clear();
        hover_settle(blank, cx);
        assert!(
            !cx.update(|_, cx| cx
                .global::<crate::ui::work_item_row::TextHover>()
                .0
                .contains(target)),
            "{target} hovered on blank"
        );
        let tip = selector(format!("{target}-tooltip"));
        assert!(cx.debug_bounds(tip).is_none(), "{target} tooltip on blank");
        cx.simulate_click(blank, Modifiers::none());
        cx.run_until_parked();
        assert!(
            !actions.borrow().iter().any(|a| matches!(
                a,
                HomeAction::Repository(_)
                    | HomeAction::Profile(_)
                    | HomeAction::OpenPullRequest { .. }
            )),
            "blank Feed text navigated"
        );
        hover_settle(text.center(), cx);
        assert!(
            cx.update(|_, cx| cx
                .global::<crate::ui::work_item_row::TextHover>()
                .0
                .contains(target)),
            "{target} text cannot hover"
        );
        if target != "feed-pr-read-more-0" {
            assert!(
                cx.debug_bounds(tip).is_some(),
                "{target} text tooltip never appeared"
            );
        }
        click(target, cx);
        assert!(
            !actions.borrow().is_empty(),
            "{target} text cannot activate"
        );
    }
}

#[gpui::test]
fn row_acceptance_real_render_tree(cx: &mut TestAppContext) {
    check_rendered_rows(cx, None);
}

/// Native DirectWrite shaping on Windows, with exactly the bundled production font bytes.
/// This still uses simulated pointer dispatch and does not claim native-window acceptance.
#[cfg(target_os = "windows")]
#[test]
fn row_acceptance_bundled_native_fonts() {
    // Windows' headless platform deliberately supplies TestTextSystem, so request the
    // native platform here even though the window/event harness remains simulated.
    let native_text = gpui_platform::current_platform(false).text_system();
    let mut cx = TestAppContext::build_with_text_system(
        gpui::TestDispatcher::new(0),
        None,
        native_text.clone(),
    );
    check_rendered_rows(&mut cx, Some(native_text));
}
