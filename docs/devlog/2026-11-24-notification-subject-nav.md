# Notifications 主题直达（notification-subject-nav）

基线 `f888a31f2be22296b40a9ec5feeec81bb3d36fad`，分支 `feat/notification-subject-nav`。产品契约更新见 [Notifications](../notifications.md#主题直达2026-11-24)。

## 行为

- domain 新增 `NotificationTarget` 与 `NotificationThread::target()`：只把 `https://api.github.com/repos/{owner}/{repo}/(issues|pulls)/{number}` 中 host、owner/repo、subject 类型和正整数编号全部匹配的 Issue/Pull Request 识别为可导航目标。缺 URL、错 host/仓库/路径/类型、query/fragment/userinfo、0/前导零/溢出都返回 `None`。
- 匹配逻辑只解析本地字符串，不请求 subject/comment URL，也不读取 GitHub 页面。
- 通知标题只在匹配成功且宿主声明支持该类型时成为独立 link；点击发出 typed `SubjectNavigation(context, target)`。仓库链接与 Done/Unsubscribe/Save for later 仍是独立动作，标题不会吞掉它们。宿主未声明的类型保持普通文本，不制造无效链接。
- 宿主在路由前重复 `RepoService::is_current(context)` 检查；组件自身先用 controller context 拒绝账户切换后的旧行。Issue 与 PullRequest 分别接到 `Route::Issue` / `Route::PullRequest`，宿主能力声明为两者都可打开。
- 不匹配、缺失或不支持的 subject 保持原有“标题无动作”行为。

## 依赖与未完成（2026-11-26 更新）

2026-11-24 那一轮交付时本基线没有 `Route::Issue`，所以 Issue 标题保持普通文本。负责人验收反馈要求打通 Issue，因此本分支现在以 `feat/issue-detail`（`1ac899e22a4e7f1da42c71ecd85b87ff5705666a`，实施时重新核对过远端）为开发基础 rebase，宿主消费该生产者的 `Route::Issue`；`#106` 自身仍以 `main` 为 base。

- 未新增/修改 `src/workspace/mod.rs` 路由注册、`src/ui/mod.rs` 页面挂载、共享网络层、全局列表或 Home；Issue 详情页、`Route::Issue`、`IssueDetailPage` 全部来自生产者分支。
- PR 对话正文由 pr-conversation 任务负责挂载；本任务只验证通知能到达正确的 `Route::PullRequest`（同仓库同号与 Issue 是不同路由/tab），组合后再验 PR 正文。

## 文件

- `src/domain/notification.rs`、`src/domain/notification/target.rs`、`src/domain/notification/target_tests.rs`
- `src/ui/notifications/mod.rs`、`src/ui/notifications/tests.rs`
- `src/app/pages/notifications.rs`、`src/app/pages/tests.rs`
- `src/bin_support/notifications_fixture.rs`（notification 专属 `Scene::Targets`）
- `examples/notifications.rs`（预览中显示 target 动作）

## 验证

所有 Cargo 命令经 `node scripts/verify.mjs -- cargo ...`：

- `cargo fmt --all`：`8a08d96c`；宿主 borrow 修复后 `24900163`；首轮最终 `ae874255`；审查修复后 `c61e586c`。
- `cargo check --locked --all-targets`：`043c6688` 通过。
- `cargo test --locked --bins subject`：`b3ecd892` 通过，含 domain 反例与 UI 主题/账号测试。
- `cargo test --locked --bins notification::target`：`15a591f3` 通过（3 项）。
- `cargo test --locked --bins notifications_route_reads_routes_repositories_and_completes_done`：2026-11-24 首次 `dbbeb5dd` 通过（当时 Issue 保持原位）；2026-11-26 打通后 `4aaff0df` 通过，断言通知 Issue #42 打开并渲染 `issue-detail-body` + `issue-comment-401`、同通知复用同一 tab、同仓库同号 PR 走 `Route::PullRequest`、另一个仓库的同号 Issue 另开 tab、`github.com` HTML subject 保持原位、仓库链接与 Done 独立。
- `cargo clippy --locked --all-targets -- -D warnings`：首轮 `c72e112d`；审查修复后 `4dd6c9d8` 通过。
- `cargo test --locked --bins --tests`：首轮 `14f3d162`；审查修复后 `b120a735` 通过（1542 passed，0 failed，含新增 unsupported-kind 回归）。

覆盖：host/owner/repo/type/number 正例与错误 host、错仓库、错路径、错类型、缺 URL、userinfo、端口、query/fragment、0/符号/前导零/溢出；UI 中可靠行打开、不可靠行不打开、未声明支持的 Issue 保持普通文本、仓库链接独立、Done 不触发主题导航、切 PAT 后直接调用旧行也不导航。审查修复后只改本次涉及源码与文档；未做真实 PAT 或原生窗口手动点击验收。

## 2026-11-26 负责人返工：Issue 也打通

负责人反馈 `#105` 未通过：PR 可跳但可靠的 Issue 类型被宿主禁用。本轮在原任务/原分支继续：

- 分支 rebase 到 `feat/issue-detail`（`1ac899e`），宿主 `src/app/pages/notifications.rs` 新增 `Route::Issue` 分支，能力声明改为 Issue/PR 都支持。`CanOpenSubject` 机制保留，仍是宿主声明能力的唯一入口。
- 正式 Shell 回归重写为真实宿主点击：通知 Issue #42 打开 `Route::Issue{alice,demo,42}` 并渲染该 issue 的正文与评论（fixture 只为 #42 提供 issue/comments 端点，route 字符串正确但未读到正文会失败）；重复点击同一通知复用同一 tab；同仓库同号 PR 打开 `Route::PullRequest` 且 tab 不同；另一个仓库的同号 Issue 打开自己的 tab；`https://github.com/...` 形状的 subject 仍不匹配并保持在 Notifications。
- `PageTransport` 的通知 fixture 扩为 4 条（demo Issue #42、demo PR #42、other Issue #42、HTML URL 行）并新增两条 issue/一条 comments 端点；Done 断言改为等待 `notification-row-3` 消失。

验证（经 `scripts/verify.mjs`）：rebase 到生产者最新 head `ab24dd9` 后重新执行，`cargo fmt --all -- --check` `34298b2e`；`cargo clippy --locked --all-targets -- -D warnings` `f7db54cb`；`cargo test --locked --bins --tests` `3817677b`（1563 passed / 0 failed）；`cargo build --locked --all-targets` `8c8f27b2`；聚焦回归 `cargo test --locked --bins notifications_route_reads_routes_repositories_and_completes_done` 在 rebase 前 `4aaff0df`、rebase 后 `c858b0d2` 均通过。

## 审查反馈

Codex P2：生产页面把匹配的 Issue 标题渲染成 link，但宿主明确忽略 Issue，造成 dead link。修复为视图要求宿主通过 `CanOpenSubject` 声明可打开类型；生产宿主只声明 PullRequest，Issue 保持普通文本，等 `Route::Issue` 落 main 后再声明。新增 `unsupported_subject_kinds_stay_plain_text` 回归：同一场景下 Issue 点击无回调，PR 点击正常发出 target。
