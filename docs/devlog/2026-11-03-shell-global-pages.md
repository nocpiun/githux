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

## 2026-11-04 负责人视觉验收返工

负责人对 PR #71 的人工验收提出两点：Notifications 背景/标题/内外边距与其它正式页面不统一；全局 Issues/PR 与 Home 的条目必须是同一组件（Home 同步改用全局的彩色状态图标）。本轮在原分支/原 PR、原会话完成，保留 `pending`。

### 改动

1. **共享工作条目组件** `src/ui/work_item_row.rs`（新增，`ui::work_item_row`）：
   - 单一 `WORK_ITEM_ROW_HEIGHT = 72`，替代 Home 的 68 与全局的 72 两套行高；Home 的 `HOME_ROW_HEIGHT` 改为引用它，导出名与调用签名保持不变。
   - 统一标题（`TextSize::Base` + `MEDIUM` + GPUI 原生 `truncate()`）、图标槽竖直居中、`Radius::Md` 圆角、`colors.active` 选中底、`TextSize::Xs` 元信息、窄屏截断。
   - 元信息顺序统一为 `#number → repository → state → relative time`，编号仍在前（负责人此前要求），编号用主题 `mono_family`（Google Sans Code）。
   - 图标颜色按 `Tone::{Open, Closed, Merged, Unknown}` 映射到 `success / fg_subtle / accent / warning`，Home 因此从原来的固定 `fg_muted` 图标改为彩色状态图标。
   - 组件只接收展示值与回调（`RowAction` / `TitleAction`），两端继续各自持有数据模型、账户上下文与 typed navigation。
2. **Home 消费共享行** `src/ui/home/rows.rs`：`home_row` 改为构造共享 `Row`（owner 链接、repository 链接、标题激活仍走原 `HomeAction` / `view.select` + `view.activate`），保留 `home-item-row-*`、`home-title-*`、`home-icon-*`、`home-metadata-*`、`home-0-owner`、`home-0-repository`、`home-time-*` 这些既有 selector。Feed 卡片、数据获取与缓存语义未改。
3. **全局列表消费共享行** `src/ui/global_work_items/rows.rs`：`ROW_HEIGHT` 引用共享常量；`work-item-*`、`work-title-*`、`work-metadata-*`、`work-number-*`、`work-repo-*`、`work-updated-*` selector 与仓库回调、键盘选择语义保持不变；新增 `work-*-state` selector。标题仍无详情动作。
4. **页面背景/标题/内边距统一**：
   - 新增 `src/ui/page_layout.rs` 作为四个正式页面共用的内容几何（`PAGE_GUTTER=16`、`PAGE_TOP=16`、`PAGE_BOTTOM=8`、`PAGE_HEADER_HEIGHT=40`），与 Home 现有的 `px_4 / pt_4 / pb_2` 和 40px 列头一致。
   - Notifications：根背景由 `colors.bg` 改为 `colors.surface`（与 Home/Settings/全局列表一致），移除 header 自带的 `px_3/py_2`，改由页面 body 统一提供 gutter/top/bottom；标题补 `SEMIBOLD`、header 高 40；已读行底色由 `colors.bg` 改为 `colors.surface`，只保留未读行的 `info_subtle` 底色与圆点/字重差别。通知行排版、右下角动作与相对时间未改。
   - 全局 Issues/PR：根 `p_4` 改为统一 gutter/top/bottom，header 固定 40 高；筛选行与列表随同一 gutter 对齐。
5. **fixture/example 最小接线**：`home_modules.rs`、`work_items_modules.rs`、`notifications_modules.rs` 增加 `page_layout` / `work_item_row`（及 `tooltip`）的 `#[path]` 声明，使既有 preview 与组件测试继续编译同一份源文件。

### 验证

- `cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo fmt --all -- --check`：SUCCESS。
- `cargo test --locked --bin githux`：SUCCESS，1151 passed。
- 定向：`ui::home::tests::`（含新增 `home_work_item_row_is_the_shared_row_presentation`）、`ui::global_work_items::tests::`、`ui::notifications::tests::`、`app::pages::tests::` 全部通过。

### 未覆盖

- 原生 Windows 窗口的四页对照（宽/窄、图标与标题对齐、长标题/长仓库名、四种状态色彩）仍需负责人复验；GPUI 测试平台的 bounds 断言不替代原生视觉确认（L002/L017）。PR 保持 `pending`，由负责人重新验收后移除。
