# 2026-11-26 全局 PR 列表的详情入口（global-pr-detail-nav）

- 分支 `feat/global-pr-detail-nav`，基线 `951da323a114d3d2dd6e8cb2955779c12f2f1a13`（main，含 #106 Issue 详情）。
- 交付范围：`src/app/pages/work_items.rs`、`src/ui/global_work_items`、本任务定点回归与文档。

## 实际缺口与最小复现

基线（`951da32`）里 `WorkItemsPage` 只在 `WorkItemKind::Issue` 时安装标题回调（`WorkItemsView::on_issue`），`rows::row` 又以 `item.kind == WorkItemKind::Issue && has_issue_handler` 判定标题是否可激活。于是即使 PR Conversation 页面（#109）已把 `Route::PullRequest` 从空挂载改成真实 `PullRequestPage`，全局 Pull Requests 列表的行标题仍然是纯文本，Assigned/Created 两个筛选都没有入口，只能靠 Feed 的 Read more 进入。

最小复现：登录后打开 `Route::PullRequests`，点击任一 PR 行标题。基线没有任何导航，`shell.route()` 保持 `Route::PullRequests`；`Route::PullRequest` 只能由 Feed 的 Read more 产生。

必须保留的行为：

- Issues 列表标题仍打开 `Route::Issue`，不改其该页面的路由、读取或缓存。
- 仓库名链接始终独立打开 `Route::Repository`，不与标题共用点击目标或 Tab stop。
- 未接线回调的预览标题保持纯文本语义（无 Link role、无 Tab stop、无 pointer）。
- 旧账户/后台列表的标题不得导航。

## 改动

1. **组件 API 由「Issue 专用」改为「按本列表 kind」**（`src/ui/global_work_items/mod.rs`、`rows.rs`）
   - `IssueNavigation { context, repository, number }` + `OnIssue` 改为 `WorkItemNavigation` + `OnOpen`；`on_issue` / `has_issue_handler` / `navigate_issue` 相应改为 `on_open` / `has_open_handler` / `navigate_open`。结构字段相同（context + 行上的 repository/number），没有复制第二套导航结构。
   - `rows::row` 接收视图自身的 `WorkItemKind`，不再是 `item.kind == WorkItemKind::Issue && has_issue_handler`，而是 `has_open_handler && item.kind == kind`。标题的可激活性与「本列表自己的目的地」一致；payload 的 kind 与查询不符的行依旧保持纯文本，避免 `is:pr` 结果被打开成 Issue 详情。
   - `render_list` 在既有的 `cx.processor` 中直接读 `view.controller.query.kind` 传入，不新增 `read_entity`，不触发 GPUI 重入（见 `docs/devlog/2026-11-20-issue-detail.md` 的同名教训）。
   - `navigate_open` 仍在回调前检查 `active` 与 `repos.is_current(&context)`。

2. **Shell 宿主按 kind 选路线**（`src/app/pages/work_items.rs`）
   - `on_open` 只构造一次，按 `kind` 得到 `Route::Issue` 或 `Route::PullRequest`，namespace/name/number 全部取自 `WorkItemNavigation`（即被点击行），并在提交 `PageNavigation::Open` 前重复 `repos.is_current(&nav.context)`。
   - 打开/复用标签仍由 Shell 既有的 `navigate` 与 `RouteKey` 完成，本任务没有新增路由、没有修改 `Route` 定义、没有碰 `Route::PullRequest` 的挂载。

3. **测试**
   - `src/ui/global_work_items/tests.rs`：
     - `pull_request_titles_report_their_own_row_identity`：PR 列表标题点击回调带回该行自己的 repository 与 number；仅选中不导航；仓库链接不触发详情回调；移除回调后标题恢复纯文本。
     - `title_navigation_requires_the_captured_account_and_an_active_list`：当前账户可导航；`set_active(false)` 后不导航；切换到新账户后旧 context 判定为 stale，标题不导航。
     - `open_kind` helper 让 Issue/PR 两个列表复用同一装配。
   - `src/app/pages/tests.rs`：
     - `global_pull_request_title_opens_reuses_and_returns_from_the_list`：正式 Shell 里点击 `work-title-20001` 得到 `Route::PullRequest { alice, demo, 7 }`，与原 PR 列表 tab 不同；再次点击复用该 tab；同行仓库链接仍得到 `Route::Repository`；关闭详情 tab 回到原 PR 列表且行仍在。
     - `work_item_titles_keep_issue_and_pull_request_destinations_distinct`：同一 fixture 下 Issues 标题给 `Route::Issue`、PR 标题给 `Route::PullRequest`，两端不互串。
   - 既有 `unconnected_title_is_not_a_tab_stop`、`real_rows_repository_callback_and_unconnected_title`、`global_issues_title_opens_reuses_and_isolates_issue_detail_while_repo_link_stays_separate` 保持不变并继续通过。

## 验证

自动化（均在 `node scripts/verify.mjs` 记录的 run 下执行）：

```sh
node scripts/verify.mjs -- cargo fmt --all
node scripts/verify.mjs -- cargo test --locked --bins ui::global_work_items
```

组件测试 18 项通过（run `5d412a1a-88be-462d-bcdb-189356f717a1`，共享构建槽等待 130.1 秒，命令执行 256.3 秒）。Shell 集成与最终 fmt/clippy/test 的 run 记录见 PR 描述。

## 边界与后续

- 本任务只接「列表标题 → 既有详情路线」。PR Conversation 的正文、评论、图片、刷新和失权处理仍由 #109 负责；本任务不重新实现、也不改其内部。
- 断言目标是「标题点击产生正确的 `Route::PullRequest` 且复用标签」，不是 Conversation 的内部渲染：#109 合入 main 后需在集成基线上确认 `pr-conversation-*` 实际出现在该路由，本任务不预置该断言以免重复其回归。
- 原生外观（间距、相对时间、滚动观感）按负责人 2026-11-26 的决定并入 Issue/PR 阅读批次统一验收，本任务不为此重新添加 `pending`。
