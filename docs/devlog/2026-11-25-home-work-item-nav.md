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

## 2026-11-26 负责人验收返工（可见入口）

负责人否决了第一版：图三圈出的 Feed「PR 图标＋标题（含 `#number`）」与「Read more」必须都能单击/键盘进入同一个 PR；Home 的标题、可见编号与图标也要指向同一详情，且不得吞掉 owner/repo 链接；不能用原地选中、空白页或临时路由冒充成功。

根因（第一版遗漏）：

1. Feed 合并卡片的 PR 标题本来走 `HomeView::activate` → `ActivateFeed`，host 忽略；只有 `Read more` 走 `OpenPullRequest`。短正文没有 `Read more` 时，鼠标与键盘都进不去。
2. Home 行只有标题可激活；旁边的图标和 metadata 里的 `#number` 是惰性的。`Issue` 行也因 `Route::Issue` 未接入而 inert。
3. 我给 `work_item_row` 的整条 title line 加了点击层，结果扩到了标题右侧的空白区——既有回归 `blank title activated` 立刻失败，说明热区超出了「可见身份元素」。改为只包住图标本身。

本轮改动：

- `src/ui/home/rows.rs`：新增 `feed_pull_request_title` 与 `feed_pull_request_icon`，与 `Read more` 共用同一个 `feed_pull_request_action`；标题包含 `#number`，短正文没有 `Read more` 时标题仍可单击/键盘 Enter 进入；无法解析目标时标题退回纯文本（无 Link 角色/Tab stop）。Home 行新增 `identity_action`：图标与 `#number` 做 pointer 激活（select + activate），选中/键盘入口仍是标题。
- `src/ui/home/mod.rs`：`feed_action` 对可寻址的合并 PR 返回 `OpenPullRequest`，所以列表级 Enter 与卡片内入口一致；星标/建库卡片保留 `ActivateFeed`。
- `src/ui/work_item_row.rs`：新增可选 `on_identity_click`，仅作用于图标与 `#number` 两个可见身份元素；全局 Issues/PR 列表传 `None`。空白区域保持惰性（既有 `blank title activated` 回归继续通过）。
- `src/app/pages/home.rs`：`open_work_item` 现在把 `WorkItemTarget::Issue` 映射到 `Route::Issue { namespace, name, number }`，`PullRequest` 保持 `Route::PullRequest`；隐藏/旧账号/旧快照守卫不变。
- 依赖与 stacked PR：#106 未获用户验收，不得合入 main。本分支 rebase 到生产者 `feat/issue-detail`。生产者在本轮实施期间从 `1ac899e22a4e7f1da42c71ecd85b87ff5705666a` 前进到 `ab24dd949424d392400abc5b5cb23828b662039d`（`fix(issues): open issue detail once after async account restore`），已重新核对并再次 rebase；PR #107 base 改为 `feat/issue-detail` 并注明依赖；未推改他人分支、未复制其实现。生产者变动或落 main 后需再次核对，落 main 后转回 `main`、消除重复提交并重查 CI/review。
- `src/app/pages/home/shell_nav_tests.rs`：改为一份代表性离线 fixture（Home 混合列表 Issue #21 / PR #7 同库、另一库 Issue #21 + Feed 合并 PR #5 截断正文），用真实 UI 点击 Home Issue 标题、Home PR 标题、Home `#number`、Home 图标、Feed PR 图标、Feed 标题、Feed `Read more`，并核对：Issue 打开 `Route::Issue` 后由 issue-detail 真实渲染 `issue-detail-body`/评论；PR 打开/复用同一 `Route::PullRequest`；owner/repo 链接保持各自目标；返回 Home 复用原标签。

### 本轮验证

- `cargo test --locked --bin githux -- ui::home::`：72 passed（含新增图标/标题/Read more 一致性、无 Read more 的键盘路径、不可寻址标题纯文本、Home 图标+编号、列表级 Enter）。
- `cargo test --locked --bin githux -- app::pages::home::`：含更新后的 `home_work_item_rows_navigate_by_identity_and_reuse_one_tab` 与真实 Shell 组合 `home_and_feed_work_item_surfaces_open_their_own_workspace_tab`。
- `cargo test --locked --bins --tests`：1576 passed, 8 ignored（`09c9dff7-92f0-4c5a-ab7d-d388b2d48ed2`）。
- `cargo clippy --locked --all-targets -- -D warnings`（`679cac21-3eb5-4428-980d-06184c289fe5`）、`cargo build --locked`（`1921598f-c970-446f-8570-925a84e817a1`）、`cargo fmt --all -- --check`（`bb7d806c-d934-4f3f-80f6-f2955b9255ce`）均通过。

### 返工后的验证边界

- **可实际点击**：`feat/home-work-item-nav`（stacked 在 `feat/issue-detail` 上）。Home Issue/PR 标题与图标、`#number`、Feed PR 图标/标题/`Read more`/列表 Enter 在真实 Shell 组合中均已点击验证。
- **真实正文已验证**：Issue #21 打开后渲染 issue-detail 的真实正文与评论（生产者 #106 的读取路径）。
- **尚未验证真实正文**：`Route::PullRequest` 目前仍是未设计空白页；PR 正文由 pr-conversation 负责，需在其正式详情整合后复验目标正文，不能以「打开了正确标签」代替。
- **原生边界**：GPUI 测试平台的 keystroke 不经过 Windows 原生输入分发（L002），真实窗口的键鼠手感与观感仍由负责人验收。

## 2026-11-26 负责人批次决定与合入 main

负责人对本轮 Issue/PR 阅读批次的决定：**观感（appearance）整批临时放行**，最终人工视觉验收在整批交付后合并进行；该决定取代此前「每个任务保持 pending、等负责人看图后才可合并」的指令。因此本任务不再为观感添加 `pending`，也不以「等负责人看图」为合并前提；同时不得声称做过未执行的人工或真实账户验证。协调者已处理 #106 并移除五个仅因观感设置的 pending 标签。

集成状态与动作：

- #106 已 squash 合入 main（`951da32`）。本分支用 `git rebase --onto origin/main ab24dd94` 只把本任务三个提交重放到 main 上，**最终 diff 只包含本任务文件**（`git diff --stat origin/main HEAD` 无生产者文件）；生产者快照 `ab24dd94` 仅用于 rebase 参照，未推改他人分支、未合并进依赖分支。重放前的分支保留为本地 `backup/home-nav-pre-restack`。
- PR #107 base 改回 `main`，同一个 PR、同一分支、同一会话继续交付。
- PR 正文（`Route::PullRequest` 内容）由 #109（`feat/pr-conversation`，`bf3854d`，尚未合入 main）负责。为不把空白页当完成，在**本地** `tmp/home-nav-plus-pr-conversation` 分支（`bf3854d` + 本任务提交）上准备了组合验证并实际运行：Home PR 标题、Feed PR 图标、Feed 标题、Feed `Read more` 都进入正确的 `Route::PullRequest` 标签，并渲染出 `pr-conversation-body` 与 `pr-comment-501`；Feed 三处复用同一标签且指向 Feed 合并的 #5，Home 行指向 #7，两者不同。该组合测试不进入交付提交（它依赖未合入的 #109），分支保留供 #109 落 main 后复验。

验证证据（本机 `scripts/verify.mjs`）：

- 交付分支（main base）：`cargo test --locked --bins --tests`、`clippy -D warnings`、`build --locked`、`fmt --check` 全绿。
- 组合分支（+ #109）：`combined_with_pr_conversation_every_entry_point_reaches_the_pull_request_body` 通过（`81ba4cac-e08c-436e-9850-828bf9daa76f`）。

剩余观感验收（按批次合并进行，不在本任务单列 pending）：

- Feed 合并卡片：图标、标题、`Read more` 与选中卡片 Enter 的单击/键盘手感。
- Home 行：标题、图标、`#number` 的指针热区与选中反馈；标题旁空白保持惰性。
- Issue 详情与 PR Conversation 的真实窗口观感、字体、滚动与多 DPI；本任务只用 GPUI 测试平台与离线 fixture 验证，未做真实账户/原生输入验证（L002）。
- PR 正文的最终验证归属 #109：#109 落 main 后应在 main 上复跑组合场景（本地组合分支可直接对照）。

## 2026-11-26 终段：main 前进后的重排与真实正文验证

main 在等待审查期间前进：`15b3a6d`（#108 仓库 Issues 列表）与 `3ca98b0`（#109 PR Conversation）先后合入。处理如下：

- 再次 `git rebase origin/main` 重放到 `3ca98b0`，PR #107 base 保持 `main`；重放前的分支保留为本地 `backup/home-nav-pre-main-advance`。
- #108 新增的 `src/ui/repository/issues/view.rs` 也构造共享 `work_item_row::Row`，本任务新增的可选字段在那里补 `on_identity_click: None`（该列表仍以标题为唯一激活入口，未扩大其热区）。这是 main 前进带来的编译修复，不改 #108 的行为。
- #109 已在 main，因此把组合验证并入交付测试：`shell_nav_tests.rs` 的 fixture 增加 `query PRConversation` 与 `/repos/alice/demo/issues/{5,7}/comments`，断言 Home PR 标题打开 `Route::PullRequest` 后渲染 `pr-conversation-body` 与 `pr-comment-501`；Feed 图标/标题/`Read more` 共享同一 PR 标签并渲染 `pr-conversation-body` 与 `pr-comment-601`。至此“左栏 Home 标题 + 右栏 Feed 图标/标题/Read more 到达正确 Issue/PR 正文”在 main 基线上有真实 UI 点击证据。
- 本机最终检查（main base `3ca98b0`）：`cargo test --locked --bins --tests`、`clippy -D warnings`、`build --locked`、`fmt --check` 全部通过；head 变化后重新请求 Codex 审查并核对 CI。

观感验收仍按批次合并进行：本轮未做真实窗口/真实账户/原生输入验证，不声称完成人工观感验收。
