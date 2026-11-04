# shell-global-pages 开发日志

任务：把已验收的 Notifications、全局 Issues/PR 组件正式挂载到 Githux Shell，取代全局空白入口，并为已在 Shell 接入的 Settings › Accounts 保留统一挂载模型。base `d217116`，分支 `feature/shell-global-pages`，交付 `pr`，milestone #1。

## 实现范围与架构决策

1. **页面挂载架构** (`src/app/pages/mod.rs`, `src/app/pages/{notifications,work_items}.rs`)：
   - `MountedPage` 扩充 `Notifications(Entity<NotificationsPage>)` 与 `WorkItems(Entity<WorkItemsPage>)`。
   - `NotificationsPage`：从 `AccountServices.repositories().notifications()` 获得控制器；`on_repository` 回调经 `RepoService::is_current` 校验捕获的授权上下文后，通过 `NavigationSink` 派发 `PageNavigation::Open(Route::Repository)`。无活动账户或服务不可用时显示确定性 unavailable 占位；`set_active` 转发到 `NotificationsView::set_active` 配合 tabs 生命周期停启时间刷新/动画。
   - `WorkItemsPage`：在构造前以 `repo_service().work_item_session()` 预检，按 `WorkItemKind::{Issue, PullRequest}` 构建 `WorkItemsView`；仓库链接同一复核后打开应用内 Repository；`set_active` 同步到视图。
   - `Settings`：经核查，main 分支上的正式 Shell 已经通过 `Shell::settings_accounts`（使用 `ely_settings::SettingsSection/SettingsRow` 与 `ui::accounts::accounts_section`）将生产 app-global `Accounts` 控制器挂在 `Route::Settings` 上，并有 30 项针对 `account-token-form`/`settings-accounts` 的回归测试。我们保持 Settings 现有实现及 `focus_accounts_form` 契约不变，不破坏已验收的账户测试。

2. **模块注册与双 Target 约束** (`src/ui/mod.rs`, `src/ui/notifications/mod.rs`, `src/ui/global_work_items/mod.rs`)：
   - 在 `src/ui/mod.rs` 声明 `pub mod notifications; pub mod global_work_items;`，使这两个组件正式进入产品模块树。
   - 保留组件内部的 `#![allow(dead_code)]`，并更新其注释，说明组件为产品二进制与独立开发 example 双 target 共享，example 专属 API（如 `set_now`、测试构造器等）在产品 target 属合法未调用。
   - `ui::global_work_items::tests` 改为使用相对 `#[path]` 引入 `work_items_fixture.rs`（与 `notifications/tests.rs` 保持一致），避免要求 crate 根存在 `ui::fixture`。
   - `src/ui/page.rs`：针对 `Notifications`、`Issues`、`PullRequests` 提供统一的不可用提示，PR 详情页（`Route::PullRequest`）仍按产品约定留空。

3. **测试覆盖** (`src/app/pages/tests.rs`)：
   - 扩充 `PageTransport`，支持 `/notifications`（返回未读 threads，模拟 DELETE 204 及 PUT 200 `ignored=true`）以及 `/search/issues`（辨别 `is:issue`/`is:pr` 查询与 Home 的 `involves:` 查询）。
   - 新增 `notifications_route_reads_routes_repositories_and_completes_done`：测试 Shell 打开 Notifications 路由、读取未读通知行、点击仓库链接进入应用内 RepositoryPage、返回 Notifications 标签并执行 Done（触发真实 DELETE 请求且从 inbox 移除）。
   - 新增 `issues_and_pull_requests_read_distinct_lists_and_reuse_their_tabs`：测试 Issues 与 Pull Requests 路由分别读取各自列表、状态隔离，重复导航同一资源复用已存在的 tab。

## 验证记录

- `node scripts/verify.mjs -- cargo check --locked --all-targets`：SUCCESS（无 error，无 warning）。
- `node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings`：SUCCESS（无 error，无 warning）。
- `node scripts/verify.mjs -- cargo fmt --all -- --check`：SUCCESS。
- `git diff --check` & `git diff --cached --check`：Clean。
- `node scripts/verify.mjs -- cargo test --locked app::pages::tests::`：SUCCESS（16 passed；覆盖所有页面挂载）。
- `node scripts/verify.mjs -- cargo test --locked ui::tests::`：SUCCESS（60 passed）。
- `node scripts/verify.mjs -- cargo test --locked ui::sidebar_tests::`：SUCCESS（57 passed）。
- `node scripts/verify.mjs -- cargo test --locked app::accounts::tests::`：SUCCESS（30 passed）。
- `node scripts/verify.mjs -- cargo test --locked ui::notifications::tests::`：SUCCESS（8 passed）。
- `node scripts/verify.mjs -- cargo test --locked ui::global_work_items::tests::`：SUCCESS（13 passed）。

## 人工验收与后续约束

- 本任务属于 First Release（milestone #1）。
- 提交 PR 时添加 `pending` 标签，等待负责人进行正式 Shell 原生窗口的人工验收（验证真实窗口下 Notifications、Issues、Pull Requests 的导航、读写动作与样式表现）。
- 全程无裁决事项，`arbitration_ever_required` 保持 `false`。
