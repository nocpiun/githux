//! Standalone native Accounts window using the application's one production runtime.
//! Uses the normal account database and platform credential store, as the previous account example
//! did. All initialization and mutations are scheduled by RuntimeHandle off the UI thread.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use githux::app::runtime::ExitGate;
    use githux::ui::foundation;
    use nsh_engine::Root;
    use nsh_platform_windows::{AppOptions, AppSource, run_app};
    let gate = ExitGate::default();
    EXAMPLE_GATE
        .set(gate.clone())
        .ok()
        .expect("one example root");
    nsh_platform_windows::set_ui_sets(vec![nsh_base::registry()]);
    let result = run_app(
        AppSource::Rust(
            foundation::fonts::install(Root::of::<AccountExample>())
                .tokens(foundation::theme::tokens())
                .app_id("dev.githux.accounts"),
        ),
        AppOptions {
            title: "Githux · Accounts".into(),
            size: (920, 760),
            ..Default::default()
        },
    );
    if !gate.wait(std::time::Duration::from_secs(10)) {
        return std::process::ExitCode::FAILURE;
    }
    result
}

#[cfg(windows)]
static EXAMPLE_GATE: std::sync::OnceLock<githux::app::runtime::ExitGate> =
    std::sync::OnceLock::new();

#[cfg(windows)]
#[nsh_core::prelude::component]
fn AccountExample() -> impl nsh_core::prelude::IntoElement {
    use githux::app::{pages::PageNavigation, runtime::RuntimeHandle};
    use githux::ui::accounts::AccountsView;
    use nsh_core::prelude::*;
    use std::{cell::RefCell, rc::Rc, time::Duration};
    let runtime = Rc::new(RefCell::new(RuntimeHandle::start(
        EXAMPLE_GATE.get().expect("example gate").clone(),
        None,
    )));
    let accounts = Rc::new(RefCell::new(
        AccountsView::new(Rc::new(|navigation| {
            if let PageNavigation::External(url) = navigation {
                let _answer = nsh_core::element::open_url(&url);
            }
        }))
        .with_repository_preview(),
    ));
    let node = accounts.borrow().element();
    let (polling, handle) = (accounts.clone(), runtime.clone());
    let _timer = set_interval(Duration::from_millis(50), move || {
        polling.borrow_mut().poll(&mut handle.borrow_mut());
    });
    cleanup(move || {
        accounts.borrow_mut().retire();
        runtime.borrow_mut().shutdown();
    });
    scroll()
        .size_full()
        .scrollbar(true)
        .child(view().p_4().child(node))
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("The NSH account window currently targets Windows.");
    std::process::ExitCode::FAILURE
}
