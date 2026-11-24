# 2026-11-20 · Single Issue Detail & Route (`feat/issue-detail`)

## 交付内容

实现了 Githux 内只读单条 Issue 详情阅读链路，并接通全局 Issues 列表标题点击入口：

- **Workspace 路由（本任务独占契约）**：在 `src/workspace/mod.rs` 新增 `Route::Issue { namespace, name, number }` 与 `RouteKey::Issue { namespace, name, number }`，以 ASCII 小写 `owner/repo` + 独立变体类型 + 仓库内编号区分与复用标签，不与同库同号 `Route::PullRequest` 或不同仓库同号冲突；`src/ui/tabs.rs`、`src/ui/page.rs` 与 `src/ui/shell_refresh.rs` 同步接入图标、未登录回退与顶部 `tab-refresh`。
- **Domain / GitHub / Data 层**：
  - `src/domain/issue_detail.rs`：`IssueLocator`、`IssueDetail`、`IssueComment`、`IssueCommentsCursor`、`IssueCommentsPage`、`IssueDetailError`（显式区分 `NotAnIssue`、`Gone`/410、`NotFound`/404、失权 401/403/SSO 与瞬态错误）。
  - `src/github/issue_detail.rs`：只读 `GET /repos/{owner}/{repo}/issues/{number}` 与分页 `GET /repos/{owner}/{repo}/issues/{number}/comments`，严格以 `reqwest::Url` + `GitHubClient::url_is_trusted` 校验请求与 `Link: rel="next"`，拒绝将返回 `pull_request` 字段的 PR 误当作 Issue，解析 `Cache-Control: no-store`。
  - `src/data/issue_detail.rs`：`RepoService::issue_detail_session()` 复用 `RepositorySession` 凭据/epoch/semaphore/取消链路，并接入 `AccountCache`（`CacheStore`）：同账户已授权快照绑定 `CacheAuthorization`（L022），`no-store` 与 404/410/401/403/`NotAnIssue` 失效旧缓存，瞬态错误保留快照供重试。
- **UI 与正式宿主**：
  - `src/ui/issue_detail/`：`IssueDetailView` 与 `Controller` 复用 `ReaderView`（`MarkdownLayout::Flow` + `on_layout_changed`）渲染正文与评论，共用 `ui::scroll`、`ui::page_layout`、`ui::relative_time`、`ui::avatar`；无正文（`No description provided.`）、无评论（`No comments yet.`）、评论首屏/翻页失败（保留正文与已加载评论并可重试）、缺失/失权/非 Issue 错误均可明确区分。使用 `Arc<Mutex<VecDeque<Reply>>>` 收件箱 + 轻量前台 drain，保持 GPUI `TestScheduler` 确定性。
  - `src/app/pages/issue_detail.rs` & `src/app/pages/work_items.rs`：`IssueDetailPage` 挂载到正式 `Shell`；全局 Issues 列表仅对 `WorkItemKind::Issue` 接线 `WorkItemsView::on_issue`（仓库名链接与 PR 列表保持不变）。
- **离线人工验收预览**：新增 `examples/issue_detail.rs`（`cargo run --locked --example issue_detail`），在真实 `Shell` 中预置 5 条确定性 Issue（富 Markdown + 两页评论、跨库同号 `#42` 无正文无评论、评论首次 503 重试恢复、404 缺失、403 失权），便于无 PAT 验收。

## 发现与修正

1. `WorkItemsView::render_list` 在 `cx.processor` 中已可变借用 `WorkItemsView`，子函数 `rows::row` 再调 `cx.read_entity` 会触发 GPUI 重入 panic；改为由 `render_list` 直接传入 `view.has_issue_handler()`。
2. 并发缓存预读与在线刷新竞态：当瞬态 503 早于 `spawn_blocking` 缓存读取完成时，原先 `apply_cached_issue` 未将 `revalidation_failed` 置为 `true`（且未拦截 404/403 后晚到的旧缓存）；已修正 `Controller::apply_cached_issue` / `apply_cached_comments` 在同代次按当前错误语义结算。
3. 后台 Tokio 线程直接通过 `tokio::sync::oneshot` 唤醒 GPUI `cx.spawn` 会在 `TestScheduler` 触发跨线程非确定性 panic；改为项目统一的 `Mutex<VecDeque<Reply>>` 收件箱 + 前台轻量 drain。
4. 首轮 Codex 审查修复：修复 `read_cached_comments_first_page` 在单页评论（`has_next == false`）时提前返回 `None` 导致缓存未命中的问题；新增 `pending_cache_reads` 计数使前台 drain 持续至并行缓存读取全部落袋；为 `IssueDetailPage` / `IssueDetailView` 的正文与评论 `ReaderView` 接入账户作用域 `RepositoryImageService`（复用外部图片无凭据读取与缓存）。

## 验证记录与边界

- 离线自动化验证（均经 `node scripts/verify.mjs`）：
  - `cargo fmt --all -- --check`
  - `cargo clippy --locked --all-targets -- -D warnings`
  - `cargo test --locked --bins --tests`
  - `cargo build --locked --example issue_detail`
  - 覆盖领域校验、GitHub 适配与分页安全、跨账户/跨仓库缓存隔离与失效、`IssueDetailView` 状态与错误恢复、`Workspace` 路由身份及正式 `Shell` 标题点击/复用/仓库链接/关闭回列表/`tab-refresh` 接线。
- **未覆盖边界（需负责人原生验收，PR 保持 `pending`）**：真实 Windows 窗口中的视觉排版、滚动手感、Markdown 选区复制与真实 PAT 在线读取，需通过正式应用或 `cargo run --locked --example issue_detail` 人工验收。
