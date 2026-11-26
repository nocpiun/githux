# 仓库 Pull requests 列表：页面接入（repo-pr-list）

更新：2026-11-26。任务 `repo-pr-list`，分支 `feat/repo-pr-list`。目标：在仓库 `Pull requests` Tab 中按状态分页浏览 PR，打开真实 Conversation 后返回保持筛选与位置。

## 本轮背景与依赖

上一轮交付了 PR 数据层与独立列表模型，但仓库容器与公共列表抽象属于 `repo-issue-list`，当时无法接入。2026-11-26 协调者更新：Issue 列表容器 #108（`be11df6`）与 Conversation #109（`98b9bcb`）已发布，负责人临时通过本批次的**外观**验收门槛，最终人工视觉验收统一在整批交付后合并进行。

随后两个生产者 PR 均合入 `main`（`15b3a6d` = #108，`3ca98b0` = #109），本任务先按该基线完成接线；之后 `main` 又合入 #104/#107/#111（`7e4ce73`），本分支再次 rebase 到最新 main，只保留本任务自己的改动，成为 consumer-only PR，不携带生产者或其他任务的提交。

## 实现

| 文件 | 内容 |
| --- | --- |
| `src/domain/repository_pulls.rs` | `PullFilter`、`PullState::classify`、`PullSummary`、`PullListQuery`、`PullPageCursor`（含仓库 id）、`PullListPage` |
| `src/github/repository_pulls.rs` + `tests.rs` | `GET /repos/{owner}/{repo}/pulls` 单页读取；显式 `state/sort/direction/per_page/page`；受控 body；状态/载荷校验；续链校验后按规范查询重建 |
| `src/data/repository_pulls.rs` + `_tests.rs` | `RepoService::pull_list_session(RepositoryIdentity)`、`PullListSession::page`，复用 `RepositorySession` 的凭据、epoch、并发与取消 |
| `src/ui/repository_pulls.rs` + `_tests.rs` | `PullListModel`：筛选、Replace/Append、同游标重试、真实空态、代次/上下文/仓库/筛选/页号复核、跨页 id 去重刷新、`PullScrollAnchor`、行显示字段 |
| `src/ui/repository/pulls/{mod.rs,view.rs}` + `tests.rs` | 页面接线：模型持有、后台投递与 drain、筛选控件、虚拟列表/滚动条/键盘、`suspend` 记录位置、`PageAction::OpenPullRequest` |
| `src/ui/repository/mod.rs`、`page_tabs.rs`、`view.rs` | PR Tab 有内容、字段与生命周期挂钩、`refresh_repository`/`is_refreshing`/`reset_authorization`/`select_tab`/`drain` 接入；新增 `OpenPullRequest` 动作 |
| `src/app/pages/mod.rs`、`examples/repository.rs` | 宿主动作映射到 `Route::PullRequest`（先复核 `is_current`）；预览记录该动作 |
| `src/bin_support/repository_fixture.rs` | `/pulls` 场景：open 两页、closed 四态、`NoPulls` 真空、`PullPageFailure` 一次续页失败；续链使用真实 by-id 形状 |
| `src/app/pages/tests.rs` | `/repos/alice/demo/pulls` 响应与 Shell 端到端组合测试 |
| `docs/repository-pull-requests.md` | 数据契约、续链行为、页面接线、验证边界 |

## 实际发现并修正的问题

1. **续链形状**：2026-11-26 对公开仓库只读核对 `GET /repos/rust-lang/rust/pulls` 与 `nocpiun/githux`，GitHub 返回
   `Link: <https://api.github.com/repositories/{id}/pulls?...>; rel="next"`，并省略 `sort`/`direction`。
   上一轮的校验要求同路径且四个 identity 参数齐全，会把**真实续页判成非法**，列表在第一页后静默结束。
   现在只从链接取页号，验证可信来源、集合路径、仓库 id/名称、保留参数与 +1，再用原路径与完整规范查询重建请求。
2. **仓库身份**：`PullPageCursor` 现在携带仓库 id，数据层与适配器都拒绝跨仓库游标；同号 PR 的测试用仓库 A/B 各一条。
3. **共享行扩展**：`main` 的 #111 为共享 `work_item_row::Row` 增加了 `on_identity_click`（图标与 `#number` 也可点击）。PR 行按同样的语义接线，标题仍是唯一键盘/读屏入口。
4. **页面接入处的模型归属**：模型只在同一 `RepositoryReadContext` 与同一仓库 id 下复用；账户变化清空模型并保留筛选。

## 验证

| 轮次 | 命令 | 结果 |
| --- | --- | --- |
| 定向模型/GitHub/数据 | `node scripts/verify.mjs -- cargo test --locked --bins repository_pulls` | 27 passed |
| 页面接线 | `node scripts/verify.mjs -- cargo test --locked --bins repository::pulls` | 6 passed |
| Shell 组合 | `node scripts/verify.mjs -- cargo test --locked --bins repository_pull_list_filters_pages` | 1 passed |
| 全量（rebase 到 `7e4ce73`，含审查修复前） | `node scripts/verify.mjs -- cargo test --locked --bins --tests` | 1628 passed / 0 failed / 8 ignored |
| 静态 | `node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings` | 通过 |

页面集成测试覆盖：Open 首屏分页与 Load more、Draft/Open/Merged/Closed 四态、续页失败保留第一页并同游标重试、
确认空列表、筛选切换清除旧行、账户变化清除旧行且保留筛选、离开 Tab 返回后筛选/页数/偏移保持且不重读、
行激活携带验证过的仓库身份。Shell 组合测试覆盖：仓库页 PR Tab → 行 → 真实 `Route::PullRequest`
Conversation 正文 → 关闭详情回到同一仓库 Tab、页数与偏移不变，且未走个人搜索。

## 审查轮次（2026-11-26）

首轮 Codex 审查提出两项，均已修复并补回归：

1. **过期行选中可能 panic（P1）**：筛选切换或替换页缩短列表后，宿主保留的 `selected` 索引会在 Enter
   时越界索引。现在筛选切换、替换请求都会清空选中；方向键把索引夹在当前范围内，Enter 只接受仍能索引
   当前行的选中项，否则丢弃。回归 `a_stale_selection_cannot_crash_or_open_a_missing_row`（先注入越界索引，
   再确认筛选切换清空选中，最后用 `end` + Enter 打开真实关闭行）。
2. **Draft 可见文字（P2）**：共享行原来只用 `tone.label()`，导致 draft 在可见状态位显示 Open。现在
   `work_item_row::Row` 增加可选 `state_label`（`state_text()` 决定可见文字，默认仍用 tone），PR 行传入真实
   四态标签；Draft 保留 open 色但显示 “Draft”。回归覆盖 `state_text` 单测、draft 行的 `state_label` 与
   页面 `repository-pull-11-state` 存在性。共享行新增字段对其余调用方是 `None`，Issue/Home/全局列表行为不变。
3. **筛选切换未复位真实滚动（P2）**：模型只清了锚点，虚拟列表仍沿用上一筛选的滚动句柄与缓动，长列表会从上一筛选的偏移开始。
   现在替换/清空时取消手势并把滚动句柄复位到顶部。回归 `switching_the_filter_returns_the_real_list_to_the_top`。
4. **续页覆盖完整性警告（P2）**：前页 SSO/非法分页 partial 会被后一页的 Complete 覆盖，Partial 徽标消失但缺失行不会重取。
   Append 现在合并原因，只有成功 Replace 才重置。回归 `a_later_complete_page_keeps_an_earlier_partial_warning_until_a_replacement`。

5. **关闭页面未取消在途读取（P2）**：`PullsRequest`/模型在 drop 时不会取消，关闭仓库 Tab 后请求仍可能占用共享请求槽直到超时。
   现在 `PullListRequest::abort()` 显式取消，`Drop for PullListModel` 调用 `cancel()`；回归 `dropping_the_model_aborts_its_in_flight_read`。

6. **授权被拒后仍保留旧行（P2）**：401/403/404/SSO 只会记录错误，已加载的私有行仍可见、可点击。现在这些失败按访问失败清空行/游标/loaded，
   只保留筛选并改为整表重试；网络/5xx 仍保留已加载页。回归 `a_rejected_credential_clears_rows_while_a_network_failure_keeps_them`。

## CI 记录

- 首轮 head `1b89704` 的 CI 在 Ubuntu `Test` 失败于既有缓存页测试
  `ui::repository::tests::offline_tests::visited_repository_content_reads_after_restart_without_network`
  （`repository-cached-badge` 未出现），该测试本机连续 3 次通过，且同一基线 main 的 CI 为绿色。修复审查意见后另行重跑；
  若再次失败，需要按跨平台时序问题单独排查，不能当作已通过。

## 未覆盖与剩余问题

- 真实窗口、真实 PAT、限流、半页滚动、中文 IME、多 DPI 未复测；按负责人 2026-11-26 决定，最终视觉验收在整批交付后统一进行。不得把这些写成已通过。
- PR 列表暂无持久缓存（Issues Tab 有），属于后续独立工作；`CacheStore` 契约未改。
- 行内 `base ← head` 仅出现在 tooltip 与无障碍名称，没有独立列，避免共享行布局漂移。
- CI 与 Codex review 结论以 PR 最新 head 为准，不在本文推断。
