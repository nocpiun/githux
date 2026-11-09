//! Shared offline harness for the Native Shell Notifications / global work-list pages.
//!
//! It wires the real runtime (`RuntimeHandle` over real `AccountService`/SQLite/`CacheStore`), a
//! mock HTTP transport, the real page host under test and an NSH headless root. Only the transport,
//! the temporary account/cache directories and the synthetic PAT are fixtures; every page request,
//! authorization gate, cursor and controller reply is the production path.
//!
//! The harness also exposes a few development controls (add/select account, revoke the account
//! cache authorization) so account-scoped clearing is exercised through the same runtime commands
//! the Shell uses. It is a test/bin_support module, never part of the product UI.
#![allow(dead_code)]
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::element::engine_context;
use nsh_core::prelude::*;

use crate::app::pages::{NavigationSink, PageNavigation, PageView};
use crate::app::runtime::{Backend, Command, ExitGate, RuntimeHandle};
use crate::auth::credential::CredentialStore;
use crate::auth::memory::InMemoryCredentialStore;
use crate::auth::token::Pat;
use crate::data::AccountService;
use crate::github::client::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::Transport;
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};
use crate::ui::foundation::{controls, theme as t};
use crate::workspace::Route;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Notifications,
    Issues,
    PullRequests,
}

/// What the root mounts. The backend is handed over once, on the engine thread.
pub struct Mount {
    pub backend: Mutex<Option<Backend>>,
    pub kind: Kind,
    /// A fixed wall clock for deterministic relative-time assertions.
    pub now: Option<u64>,
    /// Start the page as a retained, inactive tab (no read is dispatched until it is shown).
    pub start_inactive: bool,
}

/// A real backend on a temporary account/cache directory. Keep it alive for the test's lifetime.
pub struct FixtureBackend {
    pub transport: Arc<dyn Transport>,
    pub mount: Mount,
    temp: tempfile::TempDir,
}

impl FixtureBackend {
    pub fn build(transport: Arc<dyn Transport>, token: &str, kind: Kind) -> Self {
        let temp = tempfile::tempdir().expect("temporary harness directory");
        let executor = Arc::new(BackgroundExecutor::new(4));
        let client = Arc::new(GitHubClient::new(
            GitHubConfig::default(),
            transport.clone(),
        ));
        let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache")))
            .expect("temporary cache opens");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("harness runtime");
        let accounts = runtime.block_on(async {
            let accounts = AccountService::open_with_cache(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
                client,
                executor.clone(),
                cache,
            )
            .await
            .expect("accounts open");
            accounts
                .login(Pat::from_str(token))
                .await
                .expect("synthetic account signs in");
            accounts
        });
        Self {
            transport,
            mount: Mount {
                backend: Mutex::new(Some(Backend::new(accounts, executor, ExitGate::default()))),
                kind,
                now: None,
                start_inactive: false,
            },
            temp,
        }
    }

    /// Hands the backend to a fresh headless engine and starts it.
    pub fn start(&mut self) -> Result<nsh_testing::Headless, String> {
        let backend = self.mount.backend.lock().expect("harness mount").take();
        start(Mount {
            backend: Mutex::new(backend),
            kind: self.mount.kind,
            now: self.mount.now,
            start_inactive: self.mount.start_inactive,
        })
    }

    /// The temporary directory stays alive with the backend (the account/cache files live there).
    pub fn path(&self) -> &std::path::Path {
        self.temp.path()
    }
}

#[allow(clippy::large_enum_variant)]
enum Page {
    Notifications(crate::app::pages::notifications::NotificationsPage),
    WorkItems(crate::app::pages::work_items::WorkItemsPage),
}

impl Page {
    fn element(&self) -> AnyElement {
        match self {
            Self::Notifications(page) => page.element(),
            Self::WorkItems(page) => page.element(),
        }
    }

    fn poll(&mut self, runtime: &RuntimeHandle) {
        match self {
            Self::Notifications(page) => page.poll(runtime),
            Self::WorkItems(page) => page.poll(runtime),
        }
    }

    fn refresh(&mut self, runtime: &RuntimeHandle) {
        match self {
            Self::Notifications(page) => page.refresh(runtime),
            Self::WorkItems(page) => page.refresh(runtime),
        }
    }

    fn set_active(&mut self, active: bool) {
        match self {
            Self::Notifications(page) => page.set_active(active),
            Self::WorkItems(page) => page.set_active(active),
        }
    }

    fn retire(&mut self) {
        match self {
            Self::Notifications(page) => page.retire(),
            Self::WorkItems(page) => page.retire(),
        }
    }

    fn set_now(&mut self, now: Option<u64>) {
        match self {
            Self::Notifications(page) => page.set_now(now),
            Self::WorkItems(page) => page.set_now(now),
        }
    }

    fn loaded_rows(&self) -> usize {
        match self {
            Self::Notifications(page) => page.loaded_rows(),
            Self::WorkItems(page) => page.loaded_rows(),
        }
    }
}

/// Runtime commands the development controls ask for. They are serialized on the foreground tick
/// like the Shell's own submit gate, so a read that is still in flight cannot reject them.
#[derive(Clone, Copy)]
enum HarnessCommand {
    AddSecond,
    SelectSecond,
    SelectFirst,
}

struct Live {
    runtime: RuntimeHandle,
    page: Page,
    pending: std::collections::VecDeque<HarnessCommand>,
}

/// The headless root: mounts the chosen page on a real runtime and polls it like the Shell tick.
#[component]
pub fn PagesHarness() -> impl IntoElement {
    let status = signal(String::from("starting"));
    let rows = signal(0usize);
    let log = signal(String::new());
    let live: Stored<Option<Rc<RefCell<Live>>>> = stored(None);
    let mut toolbar: Vec<AnyElement> = Vec::new();
    let mut page_element = view().into_element();

    match engine_context::<Mount>() {
        None => status.set(String::from("no-mount")),
        Some(mount) => match mount.backend.lock().expect("harness mount").take() {
            None => status.set(String::from("no-backend")),
            Some(backend) => {
                let mut runtime = RuntimeHandle::from_backend(backend);
                let route = match mount.kind {
                    Kind::Notifications => Route::Notifications,
                    Kind::Issues => Route::Issues,
                    Kind::PullRequests => Route::PullRequests,
                };
                let tab = runtime.sessions_mut().open(route.clone());
                let context = runtime
                    .services()
                    .and_then(|services| services.page_context(runtime.sessions(), tab, None).ok());
                match context {
                    None => status.set(String::from("no-context")),
                    Some(context) => {
                        let sink_log = log;
                        let sink: NavigationSink = Rc::new(move |page| {
                            let message = match page {
                                PageNavigation::Open(route) => format!("open:{}", route.title()),
                                PageNavigation::Repository { route, .. } => {
                                    format!("repository:{}", route.title())
                                }
                                PageNavigation::External(url) => format!("external:{url}"),
                                PageNavigation::CanonicalProfile { login, .. } => {
                                    format!("profile:{login}")
                                }
                            };
                            sink_log.update(|log| {
                                if log.is_empty() {
                                    *log = message;
                                } else {
                                    log.push('\n');
                                    log.push_str(&message);
                                }
                            });
                        });
                        let mut mounted = match mount.kind {
                            Kind::Notifications => Page::Notifications(
                                crate::app::pages::notifications::NotificationsPage::new(
                                    context, sink,
                                ),
                            ),
                            Kind::Issues | Kind::PullRequests => {
                                Page::WorkItems(crate::app::pages::work_items::WorkItemsPage::new(
                                    context, route, sink,
                                ))
                            }
                        };
                        mounted.set_now(mount.now);
                        if mount.start_inactive {
                            mounted.set_active(false);
                        }
                        page_element = mounted.element();
                        let live_state = Rc::new(RefCell::new(Live {
                            runtime,
                            page: mounted,
                            pending: std::collections::VecDeque::new(),
                        }));
                        status.set(String::from("ready"));
                        // The Shell polls mounted pages (including inactive ones) on its
                        // foreground tick; 10 ms keeps the headless steps deterministic.
                        let tick = live_state.clone();
                        set_interval(Duration::from_millis(10), move || {
                            let mut live = tick.borrow_mut();
                            let Live {
                                runtime,
                                page,
                                pending,
                            } = &mut *live;
                            page.poll(runtime);
                            let count = page.loaded_rows();
                            if rows.get() != count {
                                rows.set(count);
                            }
                            if let Some(command) = pending.front().copied()
                                && runtime.snapshot().busy.is_none()
                            {
                                pending.pop_front();
                                submit(runtime, command);
                            }
                        });
                        toolbar = dev_controls(live_state.clone());
                        live.replace(Some(live_state));
                    }
                }
            }
        },
    }

    view()
        .size_full()
        .flex_col()
        .bg(t::theme().background)
        .text_color(t::theme().foreground)
        .child(
            view()
                .flex_col()
                .px_2()
                .flex_none()
                .test_id("harness-toolbar")
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .h(px(30.0))
                        .child(
                            text(move || status.get())
                                .text_size(px(12.0))
                                .test_id("harness-status"),
                        )
                        .child(
                            text(move || format!("rows:{}", rows.get()))
                                .text_size(px(12.0))
                                .test_id("harness-rows"),
                        )
                        .child(view().flex_1()),
                )
                .children(toolbar),
        )
        .child(
            text(move || log.get())
                .text_size(px(11.0))
                .text_color(t::theme().muted_foreground)
                .px_2()
                .test_id("harness-nav"),
        )
        .child(
            view()
                .flex_1()
                .min_h_0()
                .test_id("harness-page")
                .child(page_element),
        )
}

fn submit(runtime: &mut RuntimeHandle, command: HarnessCommand) {
    match command {
        HarnessCommand::AddSecond => {
            let _ = runtime.submit(Command::Add(Pat::from_str("ghp_synthetic-second")));
        }
        HarnessCommand::SelectSecond => {
            if let Some(account) = runtime
                .snapshot()
                .accounts
                .iter()
                .find(|account| account.identity.login == "second-user")
                .map(|account| account.identity.key.clone())
            {
                let _ = runtime.submit(Command::Select(account));
            }
        }
        HarnessCommand::SelectFirst => {
            if let Some(account) = runtime
                .snapshot()
                .accounts
                .iter()
                .find(|account| account.identity.login != "second-user")
                .map(|account| account.identity.key.clone())
            {
                let _ = runtime.submit(Command::Select(account));
            }
        }
    }
}

fn dev_controls(live: Rc<RefCell<Live>>) -> Vec<AnyElement> {
    let mut rows: Vec<AnyElement> = Vec::new();
    let queue = |command: HarnessCommand| {
        let live = live.clone();
        move || live.borrow_mut().pending.push_back(command)
    };
    let add = {
        let queue = queue(HarnessCommand::AddSecond);
        controls::button("Add second", queue)
    };
    let select_second = {
        let queue = queue(HarnessCommand::SelectSecond);
        controls::button("Select second", queue)
    };
    let select_first = {
        let queue = queue(HarnessCommand::SelectFirst);
        controls::button("Select first", queue)
    };
    let revoke = {
        let live = live.clone();
        controls::button("Revoke", move || {
            let live = live.borrow();
            let Some(services) = live.runtime.services() else {
                return;
            };
            let repositories = services.repositories();
            let Ok(session) = repositories.session() else {
                return;
            };
            if let Some(cache) = repositories.cache(session.context()) {
                let _ = cache.store().revoke_repository(cache.scope(), None);
            }
        })
    };
    let refresh = {
        let live = live.clone();
        controls::button("Refresh", move || {
            let mut live = live.borrow_mut();
            let Live { runtime, page, .. } = &mut *live;
            page.refresh(runtime);
        })
    };
    let inactive = {
        let live = live.clone();
        controls::button("Inactive", move || {
            live.borrow_mut().page.set_active(false);
        })
    };
    let active = {
        let live = live.clone();
        controls::button("Active", move || {
            live.borrow_mut().page.set_active(true);
        })
    };
    let retire = {
        let live = live.clone();
        controls::button("Retire", move || {
            live.borrow_mut().page.retire();
        })
    };
    let chunks: [Vec<AnyElement>; 2] = [
        vec![
            add.into_element(),
            select_second.into_element(),
            select_first.into_element(),
            revoke.into_element(),
        ],
        vec![
            refresh.into_element(),
            inactive.into_element(),
            active.into_element(),
            retire.into_element(),
        ],
    ];
    for chunk in chunks {
        rows.push(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(30.0))
                .children(chunk)
                .into_element(),
        );
    }
    rows
}

/// The NSH root used by both page tests; fonts come from the production registration.
pub fn root() -> nsh_engine::Root {
    crate::ui::foundation::fonts::install(nsh_engine::Root::of::<PagesHarness>())
}

/// Starts a headless app with the page described by `mount`.
pub fn start(mount: Mount) -> Result<nsh_testing::Headless, String> {
    let mut config = nsh_engine::EngineConfig::new(root());
    config.context = nsh_core::element::EngineContext::default().with(mount);
    nsh_testing::Headless::start(config).map_err(|error| error.to_string())
}
