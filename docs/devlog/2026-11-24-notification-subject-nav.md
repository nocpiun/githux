# Notifications 主题直达（notification-subject-nav）

基线 `f888a31f2be22296b40a9ec5feeec81bb3d36fad`，分支 `feat/notification-subject-nav`。产品契约更新见 [Notifications](../notifications.md#主题直达2026-11-24)。

## 行为

- domain 新增 `NotificationTarget` 与 `NotificationThread::target()`：只把 `https://api.github.com/repos/{owner}/{repo}/(issues|pulls)/{number}` 中 host、owner/repo、subject 类型和正整数编号全部匹配的 Issue/Pull Request 识别为可导航目标。缺 URL、错 host/仓库/路径/类型、query/fragment/userinfo、0/前导零/溢出都返回 `None`。
- 匹配逻辑只解析本地字符串，不请求 subject/comment URL，也不读取 GitHub 页面。
- 通知标题只在匹配成功时成为独立 link；点击发出 typed `SubjectNavigation(context, target)`。仓库链接与 Done/Unsubscribe/Save for later 仍是独立动作，标题不会吞掉它们。
- 宿主在路由前重复 `RepoService::is_current(context)` 检查；组件自身先用 controller context 拒绝账户切换后的旧行。PullRequest 接到已有 `Route::PullRequest`。
- 不匹配、缺失或不支持的 subject 保持原有“标题无动作”行为。

## 依赖与未完成

当前基线没有 `Route::Issue`。domain/UI 已能识别 Issue，正式宿主暂不消费，点击保持原位；issue-detail 的 `Route::Issue` 落 main 后在该宿主补上 `PageHost` 分支即可。没有新增路由注册、共享网络层、全局列表或 Home 改动。

## 文件

- `src/domain/notification.rs`、`src/domain/notification/target.rs`、`src/domain/notification/target_tests.rs`
- `src/ui/notifications/mod.rs`、`src/ui/notifications/tests.rs`
- `src/app/pages/notifications.rs`、`src/app/pages/tests.rs`
- `src/bin_support/notifications_fixture.rs`（notification 专属 `Scene::Targets`）
- `examples/notifications.rs`（预览中显示 target 动作）

## 验证

所有 Cargo 命令经 `node scripts/verify.mjs -- cargo ...`：

- `cargo fmt --all`：`8a08d96c`；宿主 borrow 修复后再次 `24900163`；最终 `ae874255`。
- `cargo check --locked --all-targets`：`043c6688` 通过。
- `cargo test --locked --bins subject`：`b3ecd892` 通过，含 domain 反例与 UI 主题/账号测试。
- `cargo test --locked --bins notification::target`：`15a591f3` 通过（3 项）。
- `cargo test --locked --bins notifications_route_reads_routes_repositories_and_completes_done`：`dbbeb5dd` 通过，正式 Shell 中 Issue 标题保持原位、PR 标题打开 `Route::PullRequest`。
- `cargo clippy --locked --all-targets -- -D warnings`：`c72e112d` 通过。
- `cargo test --locked --bins --tests`：`14f3d162` 通过（1541 passed，0 failed）。

覆盖：host/owner/repo/type/number 正例与错误 host、错仓库、错路径、错类型、缺 URL、userinfo、端口、query/fragment、0/符号/前导零/溢出；UI 中可靠行打开、不可靠行不打开、仓库链接独立、Done 不触发主题导航、切 PAT 后直接调用旧行也不导航。最终质量门后只改本开发日志，未再跑 Cargo（文档不参与编译）；未做真实 PAT 或原生窗口手动点击验收。
