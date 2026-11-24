# 开发日志：Home 条目与 Feed Read more 的同一工作区导航（home-work-item-nav）

- 日期：2026-11-25（Asia/Shanghai）
- 任务：`home-work-item-nav`，分支 `feat/home-work-item-nav`，基线 `f888a31`
- 目标状态：实现完成、待审查 / 待负责人验收。Issue 详情路线不在本基线，见「依赖与后续」。

## 目标与基线事实

负责人要求：Home 中的 Issue/PR 标题和 Feed 已有的 Read more 都跳到正确的同一工作区标签；可用键盘打开；同号不同仓库、同库 Issue 与 PR 不混；离开 Home 再返回保留位置；隐藏或旧账号的 Home 不得导航。

开工前核对的基线（`f888a31`）：

- Feed Read more 已接好：`HomeAction::OpenPullRequest` → `Route::PullRequest { namespace, name, number }`，同目标复用标签，测试 `offline_filtered_empty_first_page_continues_to_merged_pr_and_read_more_routes_internally` 与 `feed_read_more_opens_and_reuses_the_internal_pr_tab` 覆盖。本次只验证、不重构。
- Home 条目的 `ActivateWorkItem` 在 host 里落在「其他 Issue/PR 入口不在本次修正范围」的空分支；即使工作区已有 `Route::PullRequest`，Home 行（含 PR 行）按 Enter 不导航。这是本轮的实际缺口。
- `src/workspace/mod.rs` 没有 `Route::Issue`；`feat/issue-detail` 分支的工作区仍未提交（其 `Route::Issue` 只存在于该 worktree 的未提交改动中），也没有已合并的祖先。因此 Issue 行在本基线没有真实目的地。

## 改动

1. **新增 `src/ui/home/navigation.rs`（Home 唯一目标映射）**
   - `work_item_target(&WorkItemIdentity) -> Option<WorkItemTarget>`：`kind` 取自 typed `HomeItemKind`，repository 与 number 取自被点击行，**不从标题推断**；编号 0 或 owner/name 不能作为仓库段时返回 `None`，不构造猜测路线。
   - `WorkItemTarget::{Issue, PullRequest}` 保留 kind 区分，`kind()/repository()/number()` 只读访问器。
   - `#[cfg(test)]` 的 `navigation_tests.rs` 覆盖：同库同号 Issue≠PR、同号不同库是两个目标、保留行上大小写、不可用身份返回 `None`。
   - 通过 `src/ui/home/mod.rs` 的 `mod navigation;` 与 `pub use navigation::{WorkItemTarget, work_item_target};` 暴露；只新增本任务文件，无共享注册改动。

2. **`src/ui/home/mod.rs`：暴露导航前必须核对的两个事实**
   - `HomeView::is_active()`：本页是否仍是当前显示的标签。
   - `HomeView::has_work_item(&WorkItemIdentity)`：该身份是否仍是当前 Home 快照中的一行，同时比对全局 id、kind、number 与 owner/name（大小写不敏感）。
   - 两者都只读视图已拥有的状态，不引入请求、路由或凭据。

3. **`src/app/pages/home.rs`：host 接线与旧激活拦截**
   - `HomeAction::ActivateWorkItem(identity)` 先核对 `is_navigable`（显示中 + 捕获 session 仍有效）与 `is_current_row`（仍是当前快照的行），再调用 `open_work_item`。
   - `open_work_item` 只经 `work_item_target` 得到目标；`PullRequest` 走既有 `PageNavigation::Open(Route::PullRequest { .. })`，`Issue` 明确留空并注释依赖另一个任务的 `Route::Issue`（不注册临时路由、不从标题推断编号）。
   - Feed Read more 的 `OpenPullRequest` 分支保持不变。

4. **测试**
   - `src/ui/home/tests.rs`：`has_work_item` 的单字段错误（kind/number/repository/id）都不算当前行；`set_active(false)` 后 `is_active()` 为假；替换快照后旧身份不再是当前行。
   - `src/app/pages/home/tests.rs`：新增 `work_item_list_response` fixture（四行：同库同号的 Issue 与 PR、另一个库的同号 Issue、同库 #8，且前三行标题相同），以及
     - `home_work_item_rows_navigate_by_identity_and_reuse_one_tab`：逐行激活，只有 PR 行产生路线，Issue 行保持 inert；
     - `repeated_home_work_item_activation_requests_the_same_route`：同一行重复激活请求同一路线；
     - `keyboard_activation_opens_the_rows_own_destination`：焦点在列表上、`down` 选中 PR 行、`enter` 触发一次并停在所选行；
     - `a_hidden_home_drops_work_item_navigation`、`a_stale_home_row_does_not_navigate`：隐藏或已被替换快照的行不导航；
     - `the_emitted_work_item_route_reuses_one_workspace_tab`：真实 `Workspace` 上同资源复用、同号不同库是另一标签、大小写别名复用；
     - `returning_to_home_keeps_the_selected_row_navigable`：`set_active(false/true)` 后 selected 与 offset 不变且仍可导航。
   - 新增 `src/app/pages/home/shell_nav_tests.rs`（本任务文件，未改共享 `src/app/pages/tests.rs`）：真实 `Shell` + `Accounts` + `HomePage` + `Workspace`，点击 `home-title-1` 打开 `alice/demo #7` PR 标签、返回 Home 后同资源复用同一标签、Issue 行不建替身标签也不落到同号 PR 标签。

## 反向验证

把 host 的 `ActivateWorkItem` 分支临时改为空实现（保持其余模块接线），`home_work_item_rows_navigate_by_identity_and_reuse_one_tab` 立即以
`left: [] / right: [PullRequest { namespace: "octo", name: "repo", number: 7 }]` 失败，确认新增回归确实咬住原缺口而不是恒真断言。恢复实现后通过。

## 验证

全部通过 `node scripts/verify.mjs -- cargo ...`：

| 命令 | 结果 | run_id |
| --- | --- | --- |
| `cargo fmt --all` | SUCCESS | `59b3e3b5-4d0a-419f-93c7-789f610be1a3` 等多轮 |
| `cargo check --locked --all-targets` | SUCCESS | `7cd5bebc-f799-4185-9e10-a493fab87e9b` |
| `cargo test --locked --bin githux -- app::pages::home::tests:: app::pages::home::shell_nav_tests:: ui::home::` | SUCCESS，102 passed | `b7fafa1b-c54f-45e0-acc2-994a98158e37` |
| `cargo test ... keyboard_activation_opens_the_rows_own_destination` | SUCCESS | `4cfb7a48-e8cc-4371-8569-f5e0219b530d` |
| 反向验证（空实现） | 预期 FAILED | `c092c393-ca0c-48aa-afa0-9e5a47a5cf64` |

- 期间一次 `append_keeps_a_delayed_first_page_description_and_refresh_rejects_old_batches` 在整组并行下超时（描述请求 30s 内未到 transport 边界），单独重跑与随后整组重跑均通过；该用例的截止时间是既有设计，与本轮改动无关。
- 未运行：真实 Windows 窗口内的原生键盘/焦点路径（GPUI 测试平台的 keystroke 不经过 Windows 原生输入分发，见 L002）。原生观感与验收仍需负责人确认。

## 依赖与后续

- **阻塞项（交协调者）**：Issue 行需要 `Route::Issue { namespace, name, number }`（生产者：`feat/issue-detail`，该 worktree 中此 route 尚未提交，也无 PR）。合入 main 后，本任务只需在 `open_work_item` 的 `WorkItemTarget::Issue` 分支调用 `PageNavigation::Open(Route::Issue { .. })` 并补一条 Shell 组合断言；映射、守卫与测试已在位。
- 本任务未改：`src/ui/work_item_row.rs`、全局 Issues/PR 列表、Notifications、workspace/route 定义、shell 宿主、Feed 数据拉取。
- Feed Read more 本次只补回归验证，未改行为与目的地。
