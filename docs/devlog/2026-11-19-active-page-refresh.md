# 开发日志：顶部 `tab-refresh` 接入当前活动页面刷新

日期：2026-11-19
分支：`feat/active-page-refresh`
目标：将顶部 tab 条最右的 `tab-refresh` 接到当前活动页面所有已挂载的数据来源，刷新期间在同槽显示加载动画，同时保持各页原有状态、缓存、滚动与跨页签/跨账户隔离。

## 实现摘要

1. **独立 Shell 刷新模块**：新增 [`src/ui/shell_refresh.rs`](../../src/ui/shell_refresh.rs)，显式枚举全部 `Route` 变体，提供 `RefreshState`（`Disabled` / `Ready` / `Refreshing`）、`state_for_active_page`、`refresh_active_page` 与按 `TabId` 作用域过滤的 `observe_page`。
2. **复用各页真实刷新 owner**：
   - **Home**（[`src/app/pages/home.rs`](../../src/app/pages/home.rs)、[`src/ui/home/mod.rs`](../../src/ui/home/mod.rs)）：`refresh_all` 同步重验证 Home 工作项列与 Feed 列并同步列头刷新态，保留现有内容/滚动/选择；Feed 处于 `X-Poll-Interval` 冷却期时不违背冷却，仍允许 Home 列刷新。
   - **Profile**（[`src/app/pages/profile.rs`](../../src/app/pages/profile.rs)）：`refresh_all` 同步重验证 Basic、Pinned/Popular、Contributions、README 四个分区；已挂载视图在刷新期遇到瞬态 Basic 错误（如 503）时原地报在 Basic 分区，不拆毁兄弟分区与滚动位置。
   - **Repository**（[`src/ui/repository/mod.rs`](../../src/ui/repository/mod.rs)、[`src/ui/repository/extras.rs`](../../src/ui/repository/extras.rs)）：为 `Work` 增加 RAII `InFlightJob` 计数（随 `Message` 存活至 `apply` 结束后释放，防止 `Repository → Branches/Revision/Replacement/Overview` 链式阶段之间短暂归零）与 `extras.has_in_flight()`，新增 `can_refresh`、`is_refreshing`、`refresh_if_idle`，并修复 `counts_work` 在 spawn 失败时的清理。
   - **Notifications & Global Work Items**（[`src/ui/notifications/mod.rs`](../../src/ui/notifications/mod.rs)、[`src/app/pages/notifications.rs`](../../src/app/pages/notifications.rs)、[`src/ui/global_work_items/mod.rs`](../../src/ui/global_work_items/mod.rs)、[`src/app/pages/work_items.rs`](../../src/app/pages/work_items.rs)）：暴露真实只读刷新状态与幂等刷新入口，尊重 Notifications 的服务端冷却期。
   - **Settings / PullRequest / 未登录**：显式禁用且点击零请求。
3. **Toolbar 与 Shell 接线**：[`src/ui/tabs.rs`](../../src/ui/tabs.rs) 在刷新期间于固定方槽渲染 Ely `Spinner`（`tab-refresh-spinner`），保留 `tab-refresh` 调试选择器；[`src/ui/mod.rs`](../../src/ui/mod.rs) 管理 `page_observers` 并在关闭 tab/切换账户时自动清理。
4. **预览入口**：新增 [`examples/shell_refresh.rs`](../../examples/shell_refresh.rs)（`cargo run --locked --example shell_refresh`），以 650ms 延迟 fixture 驱动真实 `Shell` 供全路由视觉与交互验收。

## 验证记录

- 聚焦集成回归 [`src/app/pages/refresh_tests.rs`](../../src/app/pages/refresh_tests.rs)（6 项 GPUI 集成测试）：覆盖未登录/Settings/PR 禁用零请求、Home 双列在途合并/部分失败重试/Feed 冷却、Profile 四分区刷新与瞬态 Basic 失败保留视图、Repository 跨阶段在途追踪/不遍历未开目录/零写操作/部分失败重试、Notifications 与全局 Issues/PR 刷新及冷却、多 tab 连续切换/关闭刷新中 tab/切换账户晚到隔离。
- 最终完整检查（`cargo fmt`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked --bins --tests`、`cargo build --locked`、`git diff --check`）结果见本轮交付记录与 PR 检查。
- **验证边界**：GPUI 离线测试平台与 fixture 预览覆盖路由、请求计数、并发门闩与状态隔离；真实 Windows 原生窗口观感与真人交互仍待负责人验收（PR 保持 `pending`）。
