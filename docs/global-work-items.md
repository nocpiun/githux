# 全局 Issues / Pull Requests

本模块实现 G1–G3 已确认的只读列表与独立预览。正式 Shell 挂载已由 shell-global-pages 集成任务完成（`app::pages::WorkItemsPage`），Home 的 `involves:` 查询不变。代码入口为 `domain::work_item`、`github::work_items`、`data::work_items`、`ui::global_work_items`。

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

请求统一使用 `sort=updated&order=desc`，默认每页 50，单调用恰好请求一页。首次创建视图自动加载；之后只在筛选、显式刷新或 Load more 时请求。render 不发请求，没有周期轮询、数据库缓存或远程写操作。

Assigned 不包含仅 author、mention、commenter 或 review-requested 的条目。Created 使用 author；两者均不使用 Home 的 involves 语义。PR state 以 `pull_request.merged_at` 区分 Merged/Closed：非空时间为 Merged、明确 null 为 Closed；字段缺失或未知 state 单独表示 Unknown。

`WorkItemCursor` 携带完整查询、page、per_page 和 endpoint 身份；不能用于不同筛选或不同服务地址。Link 只提取下一页页码，要求同源、相同 endpoint、连续前进且不重复 page 参数；实际请求用捕获查询重建。所有 URL 判定沿用 `reqwest::Url` 和 `GitHubClient::url_is_trusted`。任何 payload URL 都不会自动发起请求。

## 账户隔离与状态

新 facade 直接调用 `RepositorySession::read`，复用唯一凭据 blocking load、auth epoch、selection generation、共享后台执行器/semaphore、取消及 await 后检查。没有复制凭据读取。`verify()` 与 context 仍须在 UI 发布成功和错误之前复查。

`Controller` 将查询、cursor、请求 generation、模式（Replace/Append）及 session 保存在同一个 Request 中。切筛选/账户、取消、刷新抢占追加会推进 generation；旧成功与旧错误均拒绝。相同在途动作不会重复提交。筛选清空旧行；刷新保留旧快照直到成功；刷新或追加失败保留旧行及 typed error，重试恢复原失败模式。稳定 item id 用于跨页去重，number 只作为仓库内编号。

只有成功、无未取页、无 partial 且没有错误的零行结果显示 No results。初载失败、取消、SSO/截断返回零行都不冒充空集合。`HomePartialReason` 复用共享完整性词汇；ResultLimit、IncompleteResults、SsoPartial 和分页异常保留在列表状态，页面用小图标及 tooltip 呈现。后续页错误保留 `WorkItemError` / `GitHubError` 原类型，不降为普通字符串或空页。

Search 最多可访问 1,000 个位置，包括非整除 page size 的最后一页；不会自动预取。updated 排序会随服务端变化移动，去重不能保证无漏项。total_count 是服务端快照元数据，不在 UI 承诺完整历史。

## 共享工作条目组件（2026-11-04 返工）

条目呈现由 `src/ui/work_item_row.rs` 提供，Home 的 Involved 列与全局 Issues/PR 列表消费同一实现，不再各写近似样式：
行高固定 `WORK_ITEM_ROW_HEIGHT = 56`（2026-11-11 返工由 72 收紧），水平 padding 8px，两行间距 3px；背景与虚拟列表实际 item 使用同一边界。标题 `TextSize::Base`/`MEDIUM`、元信息 `TextSize::Xs`，均原生截断；`Radius::Md` 圆角、`colors.active` 选中底、状态图标颜色保持共享。两行显式行高 `rems(字号 × 1.5)`，metadata 的编号起点与标题文字左沿对齐（避开图标槽和 gap）。Noto Sans SC 墨迹相对行盒偏低，内容块整体上提 1 logical px（顶部 padding 7、底部 9），不是逐个字体/文字补相对偏移。行盒留白和 bundled font 的实际 glyph raster 墨迹边界分别核验，不能仅凭 `justify_center` 宣称视觉平衡。

元信息顺序为 `#number → 仓库名 → 状态 → 相对时间`，编号用主题 `mono_family`；Home 的 owner 与 repo 独立导航、斜杠两侧不加布局 gap。只有非交互横向 flex slot 占据余宽，短链接按 intrinsic 文本宽度排版，长链接按显示范围截断；链接 hover、tooltip、pointer、click 不覆盖 trailing blank。标题 tooltip 同样按文字范围，无动作标题不增加 Link role 或 Tab stop。全局列表保留整行选择，仓库文字单独导航并停止冒泡。两端继续持有各自的数据模型、账户上下文和 typed navigation。

## Shell 接入契约

```rust,ignore
let page = cx.new(|cx| WorkItemsView::new(
    accounts.repo_service(), executor.clone(), WorkItemKind::Issue, window, cx,
))?;
page.update(cx, |view, _| view.on_repository(callback));
// 保留标签离开前台：set_active(false, cx)
// 重新进入：set_active(true, cx)
// 活动账户/授权变化：rebind(cx)，失败时同样清除旧行。
```

`RepositoryNavigation { context, repository }` 只由仓库名链接触发，含稳定 host + canonical owner/name；Search 不提供数字 repository id，因此不伪造 id。host 再验证 workspace incarnation、TabId 和 context 后打开应用内 Repository。链接自身再次检查捕获授权 context，旧账户链接不触发回调。

正式 Shell 宿主见 [`src/app/pages/work_items.rs`](../src/app/pages/work_items.rs)：构造前以 `work_item_session()` 预检，无账户/控制器不可用时显示 unavailable 占位；仓库链接触发同一 `RepoService::is_current` 复核后转 `Route::Repository`。

元数据行顺序和对齐以上述共享组件为准，不再使用旧的 baseline + 1px 相对上移方案；长仓库名在受限 slot 内截断，编号、状态与时间保留各自空间。

时间显示：主行只显示相对年龄（`Just now`、`3 minutes ago`、`2 hours ago`、`Yesterday`、`4 days ago`），不再显示 `Updated` 前缀或 ISO 日期；精确 RFC3339 值只保留在 tooltip 与无障碍说明。解析与格式化复用 `src/ui/home/time.rs` 的同一纯函数（本模块以 `#[path]` 引用该源文件，不复制 parser），因此 offset、小数秒、无效和未来值得到与 Home 一致的诚实结果。页面可见且有行时，本地 wall clock 每 30 秒更新一次以刷新相对文本；窗口失活、隐藏、切离前台或视图释放时停止计时，不产生 GitHub 刷新或后台轮询。这是过渡消费者接点，Notifications 的共享 helper 落地后由协调者统一为单一公共模块。

标题和行没有详情动作、浏览器跳转或替代仓库跳转，也不建立空详情路由。列表 Up/Down/Home/End 改变选择并 reveal；仓库名是独立 Tab/Enter 键盘入口，刷新、筛选和 Load more 使用 Ely 控件。每个视图使用有界 GPUI ListState、共享 Bars/Motion 和 FocusHandle，按可见行渲染；追加不重置滚动，失活/窗口 activation 变化清理 motion/drag。

**已修复：** 早先原生长标题越界问题已定位为 Ely `Ellipsis` 隐藏全文污染 `ListState` intrinsic width 加旧 viewport 兜底，现改为 GPUI 原生 `truncate()` 且行由列表实际可用宽度约束；DPI 感知原生帧与 460px 受限 pane 回归均通过。交付 PR 仍保持 `pending`，等待负责人本轮相对时间改动的原生验收。

## 开发预览与验证边界

运行 `node scripts/verify.mjs -- cargo build --locked --example global_work_items` 后打开 `target/debug/examples/global_work_items.exe`。预览真实使用 AccountService → WorkItemSession → Search adapter → Controller → 真实行，只有 transport、临时账户目录及合成 PAT 是 fixture。页面上方开发工具条切 Issues/PR 和场景，产品本身仅有两个筛选。fixture 按请求身份应答，重复刷新、切换及重试不耗尽响应队列；未建模请求为 599 开发错误。

场景包括 Mixed（三页且跨页重复 id）、Initial failure（一次 503 后重试成功）、Append failure（第二页一次失败）、Empty、Partial（incomplete + SSO）、Long（超过 1,000）、Delayed（Assigned 延迟，Created 快速；快速切筛选验证旧响应丢弃）。PR Closed/All 展示 merged 和 unmerged。预览回调只显示仓库目标，不需要另一页面实现。

测试放在适配器/data 模块及 example 编译的视图模块中，没有新增复制整套应用的 tests target。视图测试需要显式执行 `node scripts/verify.mjs -- cargo test --locked --example global_work_items ui::global_work_items`；普通 cargo test 不执行 example 内测试。原生输入、真人手感和正式 Shell 接线不能由 GPUI 测试或 fixture 推导。最终命令、原生证据和验收边界见本任务开发日志。

## 官方来源与公开响应核验

2026-11-01 读取 [Search 官方文档](https://docs.github.com/en/rest/search/search?apiVersion=2022-11-28) 与 [搜索限定符](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests)，核对 1,000 上限、incomplete_results 与 merged_at schema。另以无凭据 GET `/search/issues?q=repo:rust-lang/rust is:pr is:closed&sort=updated&order=desc&per_page=2` 核对公开代表响应：state=closed，pull_request.merged_at 非空，repository_url 为 `/repos/rust-lang/rust`。没有读取私人内容或真实 PAT；此公开样本不替代确定性 mock 测试，也不证明 Assigned 私有权限/SSO 实网验证完成。
