//! Native Settings shell. One product category (Accounts); the fixture categories exist only for
//! layout regression tests and the developer example, exactly as the previous Settings component
//! had them. Account behaviour stays in [`crate::ui::accounts`].

use crate::app::pages::NavigationSink;
use crate::app::runtime::RuntimeHandle;
use crate::ui::accounts::AccountsView;
use crate::ui::foundation::controls::Icon;
use crate::ui::foundation::{controls, theme as t};
use nsh_core::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Compact side-navigation row, matching the shell's control height rather than the old 38px tabs.
pub const SETTINGS_ROW_HEIGHT: f32 = 32.0;
/// Below this actual content width the categories become one horizontal NSH tab strip.
pub const SETTINGS_NARROW_WIDTH: f32 = 760.0;
pub const SETTINGS_RAIL_WIDTH: f32 = 168.0;
pub const SETTINGS_CONTENT_MAX_WIDTH: f32 = 720.0;

/// The actual production category list. Fixture categories are opt-in for layout previews/tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Accounts,
    Fixture(usize),
}

struct Inner {
    accounts: RefCell<AccountsView>,
    selected: Signal<usize>,
    width: Signal<f32>,
    fixtures: usize,
    retired: RefCell<bool>,
}

#[derive(Clone)]
pub struct SettingsView(Rc<Inner>);

impl SettingsView {
    /// Create inside an NSH component's reactive scope. Categories are view-local; the shared
    /// account runtime is polled by the shell through [`SettingsView::poll`].
    pub fn new(navigation: NavigationSink) -> Self {
        Self(Rc::new(Inner {
            accounts: RefCell::new(AccountsView::new(navigation)),
            selected: signal(0),
            width: signal(f32::from(window().width)),
            fixtures: 0,
            retired: RefCell::new(false),
        }))
    }

    /// Developer preview/test only: adds labelled fixture categories beside Accounts. Production
    /// keeps one category.
    pub fn with_fixture_categories(mut self, fixtures: usize) -> Self {
        if let Some(inner) = Rc::get_mut(&mut self.0) {
            inner.fixtures = fixtures;
        }
        self
    }

    pub fn install_avatar_provider(
        &mut self,
        provider: Rc<dyn crate::ui::accounts::AccountAvatarProvider>,
    ) {
        self.0
            .accounts
            .borrow_mut()
            .install_avatar_provider(provider);
    }

    pub fn element(&self) -> AnyElement {
        let state = self.0.clone();
        let width = state.width;
        let body_state = state.clone();
        let body = dynamic(move || {
            let narrow = width.get() < SETTINGS_NARROW_WIDTH;
            let selected = body_state.selected;
            let nav = navigation(&body_state, narrow);
            let content_state = body_state.clone();
            let content = dynamic(move || content(&content_state, selected.get()));
            if narrow {
                view()
                    .flex_col()
                    .gap_6()
                    .child(nav)
                    .child(content)
                    .into_element()
            } else {
                view()
                    .flex_row()
                    .gap_8()
                    .items_start()
                    .child(nav)
                    .child(content)
                    .into_element()
            }
        });
        view()
            .size_full()
            .min_h_0()
            .bg(t::theme().background)
            .test_id("settings-root")
            .on_layout({
                move |bounds: &Bounds<Points>| {
                    let next = bounds.size.width.0;
                    if next.is_finite()
                        && next > 0.0
                        && (next < SETTINGS_NARROW_WIDTH)
                            != (width.get_untracked() < SETTINGS_NARROW_WIDTH)
                    {
                        width.set(next);
                    }
                }
            })
            .child(
                scroll().flex_1().min_h_0().scrollbar(true).child(
                    view()
                        .w_full()
                        .px_6()
                        .py_6()
                        .flex_col()
                        .gap_6()
                        .child(
                            t::label("Settings")
                                .text_size(px(24.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .test_id("settings-title"),
                        )
                        .child(body),
                ),
            )
            .into_element()
    }

    pub fn poll(&mut self, runtime: &mut RuntimeHandle) {
        if !*self.0.retired.borrow() {
            self.0.accounts.borrow_mut().poll(runtime);
        }
    }

    pub fn retire(&mut self) {
        *self.0.retired.borrow_mut() = true;
        self.0.accounts.borrow_mut().retire();
    }

    pub fn select(&mut self, index: usize) {
        if index <= self.0.fixtures {
            self.0.selected.set(index);
        }
    }
}

fn category(index: usize) -> Category {
    if index == 0 {
        Category::Accounts
    } else {
        Category::Fixture(index)
    }
}

fn category_label(index: usize) -> String {
    if index == 0 {
        "Accounts".to_string()
    } else {
        format!("Fixture section {index}")
    }
}

fn navigation(state: &Rc<Inner>, narrow: bool) -> AnyElement {
    if narrow {
        narrow_tabs(state)
    } else {
        wide_rail(state)
    }
}

/// The wide side navigation: compact rows with one left indicator on the current row. The row is a
/// native press target with a tab role, so it keeps keyboard activation and screen-reader state.
fn wide_rail(state: &Rc<Inner>) -> AnyElement {
    let mut list = nsh_base::tabs()
        .w(px(SETTINGS_RAIL_WIDTH))
        .flex_none()
        .flex_col()
        .gap_1();
    for index in 0..=state.fixtures {
        let selected = state.selected;
        let current = state.selected;
        let label = category_label(index);
        list = list.child(
            nsh_base::tab()
                .selected(move || selected.get() == index)
                .h(px(SETTINGS_ROW_HEIGHT))
                .w_full()
                .px_2()
                .rounded(t::theme().radius)
                .flex_row()
                .relative()
                .items_center()
                .gap_2()
                .accessibility_label(label.clone())
                .selected_style(|s| s.bg(t::theme().accent))
                .hover(|s| s.bg(t::theme().accent))
                .focus(|s| s.bg(t::theme().accent))
                .on_press({
                    let state = state.clone();
                    move |_: &()| state.selected.set(index)
                })
                .child(controls::icon(Icon::Settings))
                .child(
                    view()
                        .test_id(&format!("settings-category-{index}"))
                        .child(t::label(label).text_size(px(13.0))),
                )
                .child(dynamic(move || {
                    if current.get() != index {
                        return view().into_element();
                    }
                    view()
                        .absolute()
                        .left_0()
                        .top(px(6.0))
                        .w(px(2.0))
                        .h(px(20.0))
                        .rounded_tr(px(4.0))
                        .rounded_br(px(4.0))
                        .bg(t::theme().muted_foreground)
                        .test_id("settings-category-indicator")
                        .into_element()
                })),
        );
    }
    view()
        .test_id("settings-category-list")
        .child(list)
        .into_element()
}

/// The narrow layout: the default NSH tab list and its native underline indicator.
fn narrow_tabs(state: &Rc<Inner>) -> AnyElement {
    let mut tabs = nsh_base::tabs()
        .flex_row()
        .gap_4()
        .border_b_1()
        .border_color(t::theme().border)
        .indicator_style(|s| s.bottom(px(0.0)).h(px(2.0)).bg(t::theme().primary));
    for index in 0..=state.fixtures {
        let selected = state.selected;
        tabs = tabs.child(
            nsh_base::tab()
                .flex_none()
                .selected(move || selected.get() == index)
                .accessibility_label(category_label(index))
                .px_2()
                .py_2()
                .on_press({
                    let state = state.clone();
                    move |_: &()| state.selected.set(index)
                })
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(controls::icon(Icon::Settings))
                        .child(
                            view()
                                .test_id(&format!("settings-category-{index}"))
                                .child(t::label(category_label(index)).text_size(px(13.0))),
                        ),
                ),
        );
    }
    view()
        .flex_col()
        .w_full()
        .min_w_0()
        .test_id("settings-tabs-region")
        .child(scroll().horizontal().scrollbar(true).child(tabs))
        .into_element()
}

fn content(state: &Rc<Inner>, index: usize) -> AnyElement {
    match category(index) {
        Category::Accounts => view()
            .flex_1()
            .min_w_0()
            .flex_col()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .test_id("settings-accounts-content")
            .child(state.accounts.borrow().element())
            .into_element(),
        Category::Fixture(index) => view()
            .flex_1()
            .min_w_0()
            .flex_col()
            .gap_2()
            .test_id("settings-fixture-content")
            .child(
                t::label(format!("Fixture section {index}"))
                    .text_size(px(18.0))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                t::label("Layout fixture only. This is not a product setting.")
                    .text_size(px(13.0))
                    .text_color(t::theme().muted_foreground),
            )
            .into_element(),
    }
}
