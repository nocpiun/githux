//! Noninteractive native Windows startup and memory probe for the integrated Shell.
//!
//! Uses an isolated temporary directory, in-memory credentials, a deterministic mock transport,
//! and an invisible non-activating window (`show: false, focus: false`) so it never steals focus
//! or modifies user state.
//!
//! Run with:
//! `node scripts/verify.mjs -- cargo run --locked --example reading_baseline_native`

// Keep console subsystem so noninteractive JSON report is printed to stdout.

#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;

#[path = "../src/app/mod.rs"]
#[allow(dead_code)]
mod app;
#[path = "../src/platform/mod.rs"]
#[allow(dead_code)]
mod platform;
#[path = "../src/ui/mod.rs"]
#[allow(dead_code)]
mod ui;
#[path = "../src/workspace/mod.rs"]
#[allow(dead_code)]
mod workspace;

use std::sync::Arc;
use std::time::{Duration, Instant};

use app::AccountsHandle;
use app::accounts::Accounts;
use app::accounts::runtime::{AccountRuntime, open_accounts_with_cache};
use auth::InMemoryCredentialStore;
use auth::token::Pat;
use ely_core::{Assets, theme::Theme};
use github::{GitHubClient, GitHubConfig};
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use network::executor::BackgroundExecutor;
use network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use storage::accounts::AccountDatabaseConfig;
use storage::cache::{CacheConfig, CacheStore};
use ui::Shell;

#[cfg(windows)]
fn read_process_memory() -> (usize, usize, usize) {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let mut counters = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    let size = counters.cb;
    let ok = unsafe { K32GetProcessMemoryInfo(-1, &mut counters, size) };
    if ok == 0 {
        return (0, 0, 0);
    }
    (
        counters.working_set_size,
        counters.peak_working_set_size,
        counters.pagefile_usage,
    )
}

#[cfg(not(windows))]
fn read_process_memory() -> (usize, usize, usize) {
    (0, 0, 0)
}

#[derive(Default)]
struct NativeProbeTransport;

impl Transport for NativeProbeTransport {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: network::cancel::CancellationToken,
        max_body_bytes: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max_body_bytes {
                return Err(TransportError::ResponseTooLarge {
                    limit: max_body_bytes,
                });
            }
            Ok(response)
        })
    }

    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: network::cancel::CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let url = request.url.as_str();
            let body = if url.ends_with("/user") {
                serde_json::json!({ "id": 1, "login": "alice", "avatar_url": null })
            } else if url.contains("/user/repos") {
                serde_json::json!([{
                    "id": 100, "name": "demo", "full_name": "alice/demo",
                    "visibility": "public", "private": false, "fork": false, "archived": false,
                    "default_branch": "main", "description": "Probe",
                    "html_url": "https://github.com/alice/demo",
                    "stargazers_count": 1, "forks_count": 0, "open_issues_count": 0, "size": 1,
                    "owner": { "login": "alice", "id": 1, "avatar_url": null }
                }])
            } else if url.contains("/search/issues") {
                serde_json::json!({
                    "total_count": 1,
                    "incomplete_results": false,
                    "items": [{
                        "id": 1, "number": 1, "title": "Probe item",
                        "state": "open", "html_url": "https://github.com/alice/demo/issues/1",
                        "repository_url": "https://api.github.com/repos/alice/demo",
                        "user": { "login": "alice", "id": 1, "avatar_url": null },
                        "labels": [], "comments": 0,
                        "updated_at": "2026-11-27T10:00:00Z", "created_at": "2026-11-20T10:00:00Z"
                    }]
                })
            } else {
                serde_json::json!([])
            };
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: body.to_string(),
            })
        })
    }
}

fn main() {
    let t_start = Instant::now();
    let mem_entry = read_process_memory();
    let temp = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(BackgroundExecutor::new(4));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio");
    let store = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).expect("cache");
    let service = rt
        .block_on(open_accounts_with_cache(
            AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(
                GitHubConfig::default(),
                Arc::new(NativeProbeTransport),
            )),
            executor.clone(),
            Ok(store),
        ))
        .expect("service");
    rt.block_on(service.login(Pat::new("synthetic-native-probe".to_string())))
        .expect("login");
    let service_ready_ms = t_start.elapsed().as_secs_f64() * 1000.0;
    let runtime = Arc::new(AccountRuntime::new(service, executor));

    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            let app_ready_ms = t_start.elapsed().as_secs_f64() * 1000.0;
            ely_core::init(cx).expect("Ely failed to start");
            platform::fonts::register(cx).expect("bundled fonts failed to register");
            app::theme::install(cx);
            Theme::update(cx, |theme| {
                theme.font_family = platform::fonts::UI_FAMILY.into();
                theme.mono_family = platform::fonts::CODE_FAMILY.into();
            });
            Shell::bind_keys(cx);
            let theme_fonts_ready_ms = t_start.elapsed().as_secs_f64() * 1000.0;

            let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        show: false,
                        focus: false,
                        ..Default::default()
                    },
                    move |window, cx| {
                        let controller =
                            cx.new(|cx| Accounts::new(Some(runtime.clone()), window, cx));
                        cx.set_global(AccountsHandle(controller));
                        cx.new(|cx| Shell::new(window, cx))
                    },
                )
                .expect("open hidden probe window");
            let window_opened_ms = t_start.elapsed().as_secs_f64() * 1000.0;

            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let mem_after_window = read_process_memory();
                let report = serde_json::json!({
                    "native_platform": true,
                    "window_shown": false,
                    "focus_stolen": false,
                    "service_ready_ms": service_ready_ms,
                    "app_ready_ms": app_ready_ms,
                    "theme_fonts_ready_ms": theme_fonts_ready_ms,
                    "window_opened_ms": window_opened_ms,
                    "mem_entry_bytes": [mem_entry.0, mem_entry.1, mem_entry.2],
                    "mem_after_window_bytes": [mem_after_window.0, mem_after_window.1, mem_after_window.2],
                });
                println!("NATIVE_PROBE_REPORT={report}");
                let _ = handle.update(cx, |_, window, _| window.remove_window());
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    app::accounts::wait_for_shutdown();
    drop(temp);
}
