# Native Shell 原位替换：全局页面（nsh-global-pages）

- 日期：2026-11-09。任务 ID：nsh-global-pages（`migration/nsh-global-pages`），本地迁移，不推送/不建 PR/不合入 main。
- 状态：本地实现完成并离线验证；待集成作者按 `docs/native-shell-inplace.md` 组合真实契约。

## 改动

- `src/ui/notifications/mod.rs`：原位替换为 NSH Notifications 视图。`NotificationsView::new(PageContext, OnRepository)` 遵循 `PageView` 生命周期；UI 线程 `Intent` 队列 + `poll` 分发到既有 `BackgroundExecutor`，回复经 `PageSlot<InboxSnapshot>` / `PageTicket` 门禁发布；账户切换/替换/移除/撤权单次 drain 清理已展示行。保留收件箱分页、read/unread 背景+圆点+字重、相对时间、Done/Unsubscribe 语义（`UnsubscribedPendingDone` 中间事实保留）、Save for later 禁用；仓库链接走内部 `Route::Repository`。原 GPUI 行几何与读状态行为回归映射到新行（见 implementation-result）。
- `src/ui/global_work_items/mod.rs` + `controller.rs`：原位替换为 NSH 全局 Issues/PR 视图。`WorkItemsView::new(PageContext, WorkItemKind, OnRepository)`，保留 Assigned/Created、Open/Closed/All 筛选，`Controller` 的 generation 门禁与 `PageSlot` 账户门禁；共享行复用 `foundation::controls::work_item_row`，元信息顺序 `#number → 仓库名 → 状态 → 相对时间` 与状态色保持一致。删除了仅服务 GPUI 的 `rows.rs` 与 `tests.rs`（回归迁移到 `tests/global_work_items.rs`）。
- `src/app/pages/notifications.rs`、`src/app/pages/work_items.rs`：原位替换为 `PageView` 实现，调用上述视图并转 typed 仓库导航。
- `examples/notifications.rs`、`examples/global_work_items.rs`：原位替换为 NSH 原生窗口预览（复用真实 runtime/fixture，无 GPUI/Ely）。
- `tests/notifications.rs`、`tests/global_work_items.rs`：NSH headless 回归，用真实 `RuntimeHandle` + `AccountService` + 真实 transport fixture 验证行为；新增 `src/bin_support/nsh_pages_pages.rs`（最小 `PageView` 契约，供独立装配）与 `src/bin_support/nsh_pages_harness.rs`（headless 装配/开发控件）。
- 主题文档 `docs/notifications.md`、`docs/global-work-items.md` 更新 Native Shell 原位替换段。

## 验证

- `.agents/task/probe` 独立装配（`.agents` 被忽略，不提交）编译真实源文件：`cargo check --manifest-path .agents/task/probe/Cargo.toml --all-targets` 通过。
- `cargo test --manifest-path .agents/task/probe/Cargo.toml --all-targets -- --test-threads 4` 通过：notifications 576、global_work_items 571 项全部通过（含 runtime/业务层回归，真实服务+内存凭据+mock transport）。
- `cargo clippy --manifest-path .agents/task/probe/Cargo.toml --all-targets -- -D warnings` 通过；`cargo fmt --manifest-path .agents/task/probe/Cargo.toml` 通过。
- 覆盖场景：收件箱真实读取/相对时间/内部仓库导航、read/unread 字重区分、Done 真实 DELETE、Unsubscribe 两步失败保留确认、Save 禁用零请求、加载更多真实分页、首页失败非空、确认空收件箱、账户变更/撤权清屏、隐藏冷启动不读、加载后不重读、retire 清屏；Issues/PR 查询限定符、筛选、加载更多去重、追加失败保留+重试、确认空、撤权清屏、retire 清屏。

## 剩余与边界

- 正式窗口的 NSH 视觉/字体/交互最终由负责人验收；无 Computer Use、无截图。
- 集成作者组合真实 `src/app/pages/mod.rs`、`crate::ui::foundation` 与共享 `work_item_row` 后，需确认本视图的 `foundation::controls::work_item_row` 与 Home 侧共享行一致；行内相对时间与仓库文本竖直居中的最终几何需以真实 NSH 帧为准。
- 页面服务层未修改，复用既有 `data::notifications` / `data::work_items` 与缓存/账户隔离；独立探针只服务验证，不是第二个生产包。
