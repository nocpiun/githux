//! Whole-repository mode used by global Issues/PRs: blank space selects the row,
//! while text alone owns its repository tooltip and navigation.
use super::*;
use gpui::{Context, Modifiers, Render, TestAppContext, point, size};
use std::{cell::Cell, time::Duration};

struct ListRow {
    glyph: Glyph,
    selected: bool,
    selected_clicks: Rc<Cell<usize>>,
    navigations: Rc<Cell<usize>>,
}
impl Render for ListRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_clicks = self.selected_clicks.clone();
        let navigations = self.navigations.clone();
        div()
            .w_full()
            .font_family(cx.theme().font_family.clone())
            .child(render(
                Row {
                    selectors: Selectors {
                        row: "global-row".into(),
                        title_line: Some("global-title-line".into()),
                        icon: None,
                        title: "global-title".into(),
                        metadata: Some("global-metadata".into()),
                        number: Some("global-number".into()),
                        repository: Some("global-repo".into()),
                        owner: None,
                        updated: None,
                    },
                    glyph: self.glyph,
                    tone: Tone::Open,
                    number: 1136,
                    title: "Review changes".into(),
                    title_tooltip: "Review changes".into(),
                    repository: RepositoryLink::Whole {
                        label: "alibaba/open-code-review".into(),
                        action: Rc::new(move |_, _| navigations.set(navigations.get() + 1)),
                    },
                    updated_label: "Yesterday".into(),
                    updated_tooltip: "2026-11-10T00:00:00Z".into(),
                    aria: "Work item".into(),
                    selected: self.selected,
                    on_row_click: Some(Rc::new(move |_, _| {
                        selected_clicks.set(selected_clicks.get() + 1)
                    })),
                    on_title_click: None,
                },
                cx,
            ))
    }
}

#[gpui::test]
fn global_issue_and_pr_row_hit_targets_and_selected_geometry(cx: &mut TestAppContext) {
    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        cx.set_global(TextHover::default());
    });
    let clicks = Rc::new(Cell::new(0));
    let nav = Rc::new(Cell::new(0));
    let (view, cx) = cx.add_window_view(|_, _| ListRow {
        glyph: Glyph::Issue,
        selected: true,
        selected_clicks: clicks.clone(),
        navigations: nav.clone(),
    });
    cx.update(|window, _| window.activate_window());
    for glyph in [Glyph::Issue, Glyph::PullRequest] {
        view.update(cx, |v, cx| {
            v.glyph = glyph;
            cx.notify();
        });
        for width in [900.0, 400.0] {
            cx.simulate_resize(size(px(width), px(250.0)));
            cx.run_until_parked();
            let row = cx.debug_bounds("global-row").unwrap();
            let title = cx.debug_bounds("global-title-line").unwrap();
            let meta = cx.debug_bounds("global-metadata").unwrap();
            let repo = cx.debug_bounds("global-repo").unwrap();
            let state = cx.debug_bounds("global-row-state").unwrap();
            assert_eq!(row.size.height, px(56.0));
            assert!(
                ((title.top() - row.top()) + px(2.0) - (row.bottom() - meta.bottom())).abs()
                    <= px(1.0)
            );
            assert!(title.top() - row.top() <= px(8.5));
            let blank = point(repo.right() + px(4.0), repo.center().y);
            assert!(blank.x < state.left());
            cx.simulate_mouse_move(blank, None, Modifiers::none());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
            assert!(!cx.update(|_, cx| cx.global::<TextHover>().0.contains("global-repo")));
            assert!(cx.debug_bounds("global-repo-tooltip").is_none());
            let before = clicks.get();
            let nav_before = nav.get();
            cx.simulate_click(blank, Modifiers::none());
            cx.run_until_parked();
            assert_eq!(
                clicks.get(),
                before + 1,
                "row blank keeps intentional selection"
            );
            assert_eq!(nav.get(), nav_before, "row blank must not navigate");
            cx.simulate_mouse_move(repo.center(), None, Modifiers::none());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
            assert!(cx.update(|_, cx| cx.global::<TextHover>().0.contains("global-repo")));
            assert!(cx.debug_bounds("global-repo-tooltip").is_some());
            cx.simulate_click(repo.center(), Modifiers::none());
            cx.run_until_parked();
            assert_eq!(nav.get(), nav_before + 1);
            assert_eq!(
                clicks.get(),
                before + 1,
                "text navigation must stop propagation"
            );
        }
    }
}
