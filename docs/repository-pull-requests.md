# 仓库 Pull requests 列表

更新：2026-11-26。状态：仓库 `Pull requests` Tab 已接入正式仓库页面，Open/Closed、分页、真实空结果、续页重试与打开 `Route::PullRequest` 详情均已在离线集成测试中验证；真实窗口与真实账户尚未复测。本文记录仓库范围 PR 列表的数据契约、状态机承诺、页面接线与验证边界；该 Tab 不复用全局个人 Issues/PR 搜索语义。

## 范围

- 回答“这个仓库有哪些 PR”，查询 `GET /repos/{owner}/{repo}/pulls`，不是 `search/issues`、不是 `involves:`/`assignee:` 个人筛选，也不把同号 Issue 或通知 thread 当 PR。
- 支持 `Open` / `Closed` 两个状态筛选、服务端分页、草稿、已合并与未合并关闭的区分。
- 列表行最终打开应用内已有的 `Route::PullRequest { namespace, name, number }`；本模块不实现详情、不新增路由。
- 不属于本模块：仓库页面容器、共享行/列表抽象、Issue 列表与缓存 schema（由 `repo-issue-list` 生产）；详情内容由 `pr-conversation` 生产。本任务只做 PR 特有接线。

## 接口与字段（2026-11-24 核对官方文档）

| 项目 | 采用值/处理 | 依据 |
| --- | --- | --- |
| 端点 | `GET /repos/{owner}/{repo}/pulls` | [List pull requests](https://docs.github.com/en/rest/pulls/pulls?apiVersion=2022-11-28#list-pull-requests) |
| 状态筛选 | `state=open` / `state=closed`，不发 `all` | 同页 `state` 参数，默认 `open`，可 `open/closed/all` |
| 排序 | 显式 `sort=created&direction=desc` | 默认即 `created` + `desc`；显式发送避免默认值变化改变分页语义 |
| 分页 | `per_page`（默认 30，上限 100）+ `page`；`Link: rel="next"` 只取页号并重新构造请求 | 同页 `per_page`/`page`；[分页说明](https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api) |
| 总数量 | 不假设存在；只以“是否有下一页”表达完整性 | 该端点响应为数组，没有 `total_count` |
| 行状态 | 由 `state` + `draft` + `merged_at` 推导 | 响应 schema：`state` 必填、`draft` 可选布尔、`merged_at` 必填可空 |

分页续链的实测形状（2026-11-26，公开仓库只读请求）：GitHub 把 `Link: rel="next"` 改写成
`/repositories/{id}/pulls?...`，并会省略 `sort`/`direction`。因此适配器不按链接直接请求，而是要求：
可信来源、PR 集合路径且 id 等于当前仓库 id（或名称路径完全一致）、无未知查询参数、链接保留的
`state`/`sort`/`direction`/`per_page` 必须与本次请求一致、页号恰好 +1；随后按原路径与完整规范查询
重建下一页。这样续链既不会被误判为“已到末页”，也不可能改变仓库、筛选、排序或页大小。

状态映射：

| REST 观测 | 列表状态 |
| --- | --- |
| `state=open`，`draft=false` | Open |
| `state=open`，`draft=true` | Draft |
| `state=closed`，`merged_at` 非空 | Merged |
| `state=closed`，`merged_at` 为空 | Closed（关闭未合并） |

未知 `state`、`state=open` 同时带 `merged_at`、单页内重复 PR id 均按无效响应拒绝，不把未知状态显示成 Open。`Closed` 筛选的结果集中同时包含 Merged 与 Closed，不在客户端按状态二次过滤，否则会破坏服务端分页。

## 模块与契约

| 层 | 文件 | 职责 |
| --- | --- | --- |
| 领域模型 | [`src/domain/repository_pulls.rs`](../src/domain/repository_pulls.rs) | `PullFilter`、`PullState`、`PullSummary`、`PullListQuery`、`PullPageCursor`、`PullListPage`；无 I/O、无凭据 |
| GitHub 适配 | [`src/github/repository_pulls.rs`](../src/github/repository_pulls.rs) | 精确 URL/参数、受控 body 上限、`Link` 校验、SSO `partial-results`、状态推导、错误分类 |
| 账户会话 | [`src/data/repository_pulls.rs`](../src/data/repository_pulls.rs) | `RepoService::pull_list_session(locator)`；复用 `RepositorySession` 的凭据、epoch、并发与取消路径 |
| 列表模型 | [`src/ui/repository_pulls.rs`](../src/ui/repository_pulls.rs) | 筛选、分页/追加、重试、真实空态、旧响应拒绝、保留筛选/已加载页/滚动锚点；无渲染 |
| 页面接线 | [`src/ui/repository/pulls/`](../src/ui/repository/pulls/mod.rs)（`mod.rs` + `view.rs`） | 持有模型、后台投递、筛选控件、虚拟列表、滚动条、键盘选择，并把行激活转成 `PageAction::OpenPullRequest` |
| 宿主动作 | [`src/app/pages/mod.rs`](../src/app/pages/mod.rs) | 复核账户上下文后打开 `Route::PullRequest`，不新开浏览器或 Issue 详情 |

列表模型的约定：

1. **筛选**：`Open`/`Closed` 切换会取消在途请求并清空当前行、游标与滚动锚点；重复选择同一筛选不重新请求。
2. **追加失败**：保留已加载行、`next` 游标与 `loaded`，仅记录错误与失败模式；`retry()` 用同一游标继续，不重新拉第一页。
3. **真实空结果**：只有整页成功、零行、无错误且完整性为 `Complete` 时才是 `is_empty_result()`。SSO partial 的空页、失败页和加载中都不是空结果。
4. **响应归属**：每个请求携带 session 上下文、筛选、页号与代次。完成时同时校验代次、账户上下文、页号和 query；任何不匹配都丢弃成功与错误。A→B→A、PAT 替换、移除重登与关闭服务都由共享会话返回 `StaleContext`。
5. **返回保持位置**：模型由 Tab host 持有。打开详情再回来时 `resume()` 不自动重读，筛选、已加载页和 `PullScrollAnchor { pull_id, offset }` 保持；`rebind`（账户/授权变化）保留筛选但清空内容与锚点。
6. **仓库身份**：行号只在仓库内唯一；游标绑定 `RepositoryLocator` + 筛选，来自另一个仓库或另一个筛选的页会被拒绝。

## 页面接线

`RepositoryTab::PullRequests` 现在有内容，复用 Issue 列表已经建立的页面约定，而不是另起一套框架：

- **读取**：`PullListModel` 绑定 `PullListSession`（`RepositorySession` 的凭据、epoch、并发与取消路径）。页面在 Tab 激活、刷新、筛选变化时投递请求；回复经 `mpsc` 回到 GPUI 线程后由模型复核代次、账户上下文、仓库 id、筛选与页号。
- **呈现**：过滤控件 `repository-pull-state`（Open/Closed），行使用共享的 `work_item_row`（`Glyph::PullRequest`）。状态文字由行携带的 `state_label` 决定而不是由颜色推导：Draft 保持 open 色但**可见文字为 Draft**，Merged 为 accent，Closed 为 muted。元信息显示作者与相对更新时间，tooltip/aria 带 `base ← head`。
- **分页**：只有服务端给出合法下一段时才显示 “Load more”；续页失败保留已加载行与游标，Retry 继续同一游标，不重启列表。追加页的完整性（SSO/非法分页）与前一页合并，只有成功替换整表时才重置，避免后一页成功把前页缺失“洗掉”。
- **空态**：只有整页成功、零行且完整性为 `Complete` 才显示 “No pull requests”。失败、SSO/非法分页 partial 与加载中各有自己的呈现，不冒充空结果。
- **打开**：行或 Enter 产生 `PageAction::OpenPullRequest { context, locator, number }`；宿主在 `RepoService::is_current` 通过后打开应用内 `Route::PullRequest`（Conversation）。同号 Issue 不会被当成 PR，反向亦然。筛选切换、替换页与刷新会清空宿主行选中；Enter 只作用于仍能索引当前行的选中项，过期索引被丢弃而不是打开任意一行。
- **返回**：仓库页保持挂载，虚拟列表保留自身偏移，模型保留筛选与已加载页；`suspend` 只结束手势并记录锚点，不重读。
- **刷新/授权**：`tab-refresh` 复用 `refresh_repository` 的 `is_refreshing`/`request_pulls` 路径；账户或授权变化清空模型（保留筛选），旧回复不能再进入页面。
- **缓存**：本 Tab 目前只在线读取，没有像 Issues 那样的持久缓存适配；离线首屏属于后续独立工作，现有 `CacheStore` 契约未被修改。

## 已知边界

- 真实窗口、真实 PAT、限流与半页滚动尚未复测；现有证据是离线页面集成测试。
- 行内的 `base ← head` 只出现在 tooltip 与无障碍名称中，没有独立列，避免与共享行布局漂移。
- PR 列表没有持久缓存，这是与 Issues Tab 的已知差异，不是“已缓存但离线不可读”。

## 验证

离线 fixture 覆盖：

- 请求只到 `/repos/{owner}/{repo}/pulls`，显式发送 `state/sort/direction/per_page/page`，不访问 search；仓库 A/B 同号 PR 行与游标互不串用。
- Open/Draft/Merged/Closed 四态；未知 state 与 open+merged_at 拒绝。
- 有效 `Link` 续页；另一主机、另一仓库、另一筛选、回退页、跳页都变成 partial 而不是丢行或改变查询；SSO partial 保留行。
- 401/404/5xx/限流分类；超限 body、非 JSON、超大页、单页重复 id 拒绝；取消不消耗响应。
- 账户 A→B→A、PAT 替换、移除重登、关闭服务后的延迟成功与延迟 401 都是 `StaleContext`。
- 追加失败保留前页并以同一游标重试；真实空结果与失败/SSO partial 空页区分；跨仓库页与跨筛选页被拒绝；跨页重复 id 去重并在原位置刷新状态；返回后筛选/行/滚动锚点保持。

页面集成测试（真实 `RepositoryPage` + 共享 fixture）覆盖：Open 首屏两页、Draft/Open/Merged/Closed 四态、
续页失败保留第一页并用同一游标 Retry、确认空结果、筛选切换丢弃旧行、账户变化清空旧行并保留筛选、
离开 Tab 返回后筛选/已加载页/偏移保持且不重读、行激活产生带验证仓库身份的 `OpenPullRequest` 动作。
Shell 组合测试额外覆盖：仓库页 PR Tab → 行 → 真实 `Route::PullRequest` Conversation 正文 → 关闭详情
回到同一仓库 Tab 且页数与偏移不变。

命令（在任务 worktree，共享验证 runner）：

```sh
node scripts/verify.mjs -- cargo fmt --all
node scripts/verify.mjs -- cargo test --locked --bins repository_pulls
node scripts/verify.mjs -- cargo test --locked --bins repository::pulls
node scripts/verify.mjs -- cargo test --locked --bins repository_pull_list
node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings
```

验证边界：以上均为离线证据。真实窗口滚动/筛选手感、真实账户与限流、中文 IME、多 DPI 仍需负责人
在真实窗口验收；本批次的最终视觉验收按负责人 2026-11-26 决定统一合并后进行。
