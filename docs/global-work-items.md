# 全局 Issues / Pull Requests

本模块实现 G1–G3 已确认的只读列表与独立预览。正式 Shell 挂载已由 shell-global-pages 集成任务完成（`app::pages::WorkItemsPage`），Home 的 `involves:` 查询不变。代码入口为 `domain::work_item`、`github::work_items`、`data::work_items`、`ui::global_work_items`。

## Native Shell 原位替换（2026-11-09）

按负责人 2026-11-09 指示（`docs/native-shell-inplace.md`），本页面原位迁移为纯 Rust Native Shell 架构：
- 控制器仍位于 `src/ui/global_work_items/controller.rs`，视图与呈现位于 `src/ui/global_work_items/mod.rs`，宿主位于 `src/app/pages/work_items.rs`，遵循 `PageView` 生命周期契约。
- 视图内部通过 UI 线程事件队列与状态驱动，用户点击筛选/刷新/加载更多产生 `Intent`，`poll` 调用在 UI 线程分发异步工作至既有 BackgroundExecutor，回复通过 `Controller::finish` 门禁后再经 `PageSlot<Displayed>` 与运行时 `Sessions` 校验发布。
- 账户切换/换 PAT/移除或撤权时，单次 drain 清理已展示旧行，旧回复不可复活，`set_active(false)` 与 `retire()` 停止在途请求。
- 渲染零 I/O，时钟用 NSH 轻量定时任务驱动，失活/retire 停止。
- 视觉采用 NSH 默认主题与控件样式，保留 72px 共享行高、两行布局、相对时间与筛选语义。

## 查询与单页读取

`WorkItemSession` 从 `RepoService::work_item_session()` 捕获权威活动账户 login 和原有 `RepositoryReadContext`。UI 只能选择 `WorkItemKind::{Issue, PullRequest}`、`Relation::{Assigned, Created}`、`StateFilter::{Open, Closed, All}`。

| 选择 | 搜索限定符 |
| --- | --- |
| Issues | `is:issue` |
| Pull Requests | `is:pr` |
| Assigned（默认） | `assignee:<captured login>` |
| Created | `author:<captured login>` |
| Open（默认） | `is:open` |
| Closed | `is:closed`，PR 同时包括 merged 和未合并关闭 |
| All | 不加状态限定符 |

请求统一使用 `sort=updated&order=desc`，默认每页 50，单调用恰好请求一页。首次创建视图自动加载；之后只在筛选、显式刷新或 Load more 时请求。渲染不发请求，没有周期轮询、数据库缓存或远程写操作。

Assigned 不包含仅 author、mention、commenter 或 review-requested 的条目。Created 使用 author；两者均不使用 Home 的 involves 语义。PR state 以 `pull_request.merged_at` 区分 Merged/Closed：非空时间为 Merged、明确 null 为 Closed；字段缺失或未知 state 单独表示 Unknown。

`WorkItemCursor` 携带完整查询、page、per_page 和 endpoint 身份；不能用于不同筛选或不同服务地址。Link 只提取下一页页码，要求同源、相同 endpoint、连续前进且不重复 page 参数；实际请求用捕获查询重建。所有 URL 判定沿用 `reqwest::Url` 和 `GitHubClient::url_is_trusted`。任何 payload URL 都不会自动发起请求。

## 账户隔离与状态

新 facade 直接调用 `RepositorySession::read`，复用唯一凭据 blocking load、auth epoch、selection generation、共享后台执行器/semaphore、取消及 await 后检查。没有复制凭据读取。`verify()` 与 context 仍须在 UI 发布成功和错误之前复查。

`Controller` 将查询、cursor、请求 generation、模式（Replace/Append）及 session 保存在同一个 Request 中。切筛选/账户、取消、刷新抢占追加会推进 generation；旧成功与旧错误均拒绝。相同在途动作不会重复提交。筛选清空旧行；刷新保留旧快照直到成功；刷新或追加失败保留旧行及 typed error，重试恢复原失败模式。稳定 item id 用于跨页去重，number 只作为仓库内编号。

只有成功、无未取页、无 partial 且没有错误的零行结果显示 No results。初载失败、取消、SSO/截断返回零行都不冒充空集合。`HomePartialReason` 复用共享完整性词汇；ResultLimit、IncompleteResults、SsoPartial 和分页异常保留在列表状态，页面用小图标及 tooltip 呈现。后续页错误保留 `WorkItemError` / `GitHubError` 原类型，不降为普通字符串或空页。

Search 最多可访问 1,000 个位置，包括非整除 page size 的最后一页；不会自动预取。updated 排序会随服务端变化移动，去重不能保证无漏项。total_count 是服务端快照元数据，不在 UI 承诺完整历史。

## 共享工作条目组件

条目呈现遵循与 Home 的 Involved 列共享的 `src/ui/work_item_row.rs` 约定。在 NSH 迁移中，Home 的共享行由集成作者统一移植为 `src/ui/foundation/controls::work_item_row`，本列表复用该共享行：72px 固定行高、图标槽、选中底、元信息顺序 `#number → 仓库名 → 状态 → 相对时间`、状态色按 Open/Closed/Merged/Unknown 分别映射，均保持一致。本列表只提供展示值与回调，自身保留数据模型、账户上下文、typed navigation 与键盘选择，不重复近似实现。

## Shell 接入契约

```rust,ignore
let page = WorkItemsView::new(context, WorkItemKind::Issue, on_repository);
// 保留标签离开前台：page.set_active(false)
// 重新进入：page.set_active(true)
// 活动账户/授权变化：由宿主以新的 PageContext 重建页面
```

`WorkRepository { host, owner, name }` 只由仓库名链接触发；Search 不提供数字 repository id，因此不伪造 id。宿主验证 workspace incarnation、TabId 和 context 后打开应用内 Repository。链接自身再次检查捕获授权 context，旧账户链接不触发回调。

正式 Shell 宿主见 [`src/app/pages/work_items.rs`](../src/app/pages/work_items.rs)：构造前以 `work_item_session()` 预检，无账户/控制器不可用时显示 unavailable 占位；仓库链接触发复核后转 `Route::Repository`。

时间显示：主行只显示相对年龄（`Just now`、`3 minutes ago`、`2 hours ago`、`Yesterday`、`4 days ago`），不再显示 `Updated` 前缀或 ISO 日期；精确 RFC3339 值只保留在 tooltip 与无障碍说明。解析与格式化复用共享 `relative_time`。页面可见且有行时，本地 wall clock 低频更新以刷新相对文本；窗口失活、隐藏、切离前台或视图释放时停止计时，不产生 GitHub 刷新或后台轮询。

标题和行没有详情动作、浏览器跳转或替代仓库跳转，也不建立空详情路由。列表 Up/Down/Home/End 改变选择并 reveal；仓库名是独立键盘/点击入口，刷新、筛选和 Load more 使用原生控件。

## 开发预览与验证边界

运行 `node scripts/verify.mjs -- cargo run --locked --example global_work_items`（`--pr` 切 Pull Requests）。预览真实使用 AccountService → WorkItemSession → Search adapter → Controller → 真实行，只有 transport、临时账户目录及合成 PAT 是 fixture。场景包括 Mixed（跨页重复 id）、Initial failure、Append failure、Empty、Partial、Long、Delayed。fixture 按请求身份应答，重复刷新、切换及重试不耗尽响应队列；未建模请求为 599 开发错误。

离线回归测试位于 `tests/global_work_items.rs`，通过 `.agents/task/probe` 独立装配，运行 `node scripts/verify.mjs -- cargo test --manifest-path .agents/task/probe/Cargo.toml --test global_work_items`。

## 官方来源与公开响应核验

2026-11-01 读取 [Search 官方文档](https://docs.github.com/en/rest/search/search?apiVersion=2022-11-28) 与 [搜索限定符](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests)，核对 1,000 上限、incomplete_results 与 merged_at schema。另以无凭据 GET `/search/issues?q=repo:rust-lang/rust is:pr is:closed&sort=updated&order=desc&per_page=2` 核对公开代表响应：state=closed，pull_request.merged_at 非空，repository_url 为 `/repos/rust-lang/rust`。没有读取私人内容或真实 PAT；此公开样本不替代确定性 mock 测试，也不证明 Assigned 私有权限/SSO 实网验证完成。
