# 2026-11-26 全局 PR 列表的详情入口（global-pr-detail-nav）

- 分支 `feat/global-pr-detail-nav`。基线 `951da323a114d3d2dd6e8cb2955779c12f2f1a13`（main，含 #106 Issue 详情）；之后陆续 rebase 过含 #109 PR Conversation、#108、#107、#105 的 main，最终交付树基于 main `7e4ce73f`（含 #111 PR Files changed），即集成基线本身，没有待合入的前置 PR。
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

自动化（均在 `node scripts/verify.mjs` 记录的 run 下执行，最终树基于 main `7e4ce73f`）：

```sh
node scripts/verify.mjs -- cargo fmt --all -- --check   # d9d36b9b
node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings   # dc6ff6fb（槽等待 106.0s，执行 132.7s）
node scripts/verify.mjs -- cargo test --locked --bins --tests   # 6db605ac, 1598 passed / 8 ignored
node scripts/verify.mjs -- cargo build --locked --example global_work_items   # 81abaa26
```

- 组件测试（`cargo test --locked --bins ui::global_work_items`）18 项，含两条新增用例；随后每个 rebase 后的 `--bins --tests` 全量运行都覆盖同一代码。
- Shell 集成（`cargo test --locked --bins app::pages::tests`）含两条新增用例，在最后一次 `6db605ac` 全量运行中通过。
- **集成路径的非空 Conversation 证据**：Shell fixture 的全局 PR 行与 Feed 使用同一 `alice/demo #5`，`global_pull_request_title_opens_reuses_and_returns_from_the_list` 点击行标题后在断言 `Route::PullRequest` 与标签复用之外，等待 `pr-conversation-body`、`markdown-heading-0` 与 `pr-comment-501`，即列表入口真的渲染了该 PR 的正文和评论，而不是仅切换到一个标签。随后再次点击复用同一标签、同行仓库链接仍去 `Route::Repository`，关闭详情后回到原 PR 列表且行仍在。
- 同一 fixture 中 Issues 行仍为 `alice/demo #21`，`work_item_titles_keep_issue_and_pull_request_destinations_distinct` 与既有 `global_issues_title_...` 用例校验 Issues 行为未变。

## 边界与后续

- 本任务只接「列表标题 → 既有详情路线」。PR Conversation 的正文、评论、图片、刷新和失权处理仍由 #109 负责；本任务不重新实现、也不改其内部。
- 自动化覆盖的是路由、标签复用和 Conversation 的实际挂载与非空内容（事务 fixture），不是原生外观。原生外观（间距、相对时间、滚动观感）与外部视觉检查按负责人 2026-11-26 的决定并入 Issue/PR 阅读批次统一验收，本任务不为此重新添加 `pending`，也未在真实 PAT 账户下重跑。
- 失权/旧账户：组件已在存在号回调前检查 `active` 与 `repos.is_current(&context)`，Shell 宿主在发出 `PageNavigation::Open` 前重复同一检查；新用例覆盖了 `set_active(false)` 和切换账户后旧 context 为 stale 两种情况。

## 一轮无关的 CI 失败

首轮 PR CI（run `45825983783`，head `a8f00a5`）在 Ubuntu `code-check` 中失败于
`ui::repository::tests::offline_tests::visited_repository_content_reads_after_restart_without_network`
的 `the cached marker is visible` 断言（`src/ui/repository/offline_tests.rs:1489`）。该文件与用例不在本任务 diff 内，本任务修改的只有全局 Issues/PR 行的标题回调与两个页面宿主用例。

本地证据：同一 head 上该用例单独执行通过、`offline_tests` 整组连续 5 次通过、`cargo test --locked --bin githux -- --test-threads=1` 全量通过；PR 的 CI 在同 head 上 `gh run rerun --failed` 后通过（attempt 2）。因此把它记录为一次与本改动无关的并发/时间敏感失败，不声称已修复该用例，也不把重跑通过当作对它的长期保证；真实原因仍需在 repository 离线用例上单独诊断。
