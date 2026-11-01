# 页面持久缓存适配器

状态：数据层已实现，验证结果见 [开发日志](devlog/2026-11-01-page-cache.md)。**尚未接入正式 Shell，不代表应用离线阅读已交付。**

`data::page_cache::CachedPageReadSession::new(PageReadSession, CacheStore, CacheScope)` 只组合调用方已有的页面会话、同一个 app 级 store clone 和已保存账户的 scope。构造时比较 `scope.account()` 与 `session.context().auth().account`，保留原有服务实例、账户选择/授权代次、凭据读取、请求并发 permit 与取消检查。消费者不得自行打开另一数据库或创建分区。

接口覆盖 Home、Feed 首屏/旧页、Profile basic/pinned/calendar/可选 README、Releases/latest/Contributors/Languages 和仓库 OPEN counts。原网络接口继续可用，Home/Profile UI 与 Shell 未改。

## 表示、完整性与时间

缓存 DTO `SnapshotV1` 保存一份有界页面读取的原始响应集合；离线仍运行真实 GitHub adapters 和原 Feed policy，不再实现一套卡片筛选、排序、作者解析或分页算法。每个交换按 method、完整 URL、固定 GraphQL query/variables、Accept/API version 匹配。外层键包括授权账户分区、当前 captured login、目标 profile login/id、查询、分页 cursor、预算及表示版本；仓库 extras/counts 另带数字 repo ID。原始响应集随完成后的 outcome 原子写入，未完成的请求不会写成完整空。

所有条目按私有门槛读取，即便其中有公开字段。Home/Feed/Profile composite 使用 `repository=None` 的账户聚合 gate；一个仓库重新验证不能解锁它们。Profile README 的 repo ID 要在发现后才能知道，整个组合也留在账户聚合 gate。适配器不会自行调用 `permission_confirmed`，PAT 替换后由账户生命周期和后续组装方按明确范围提供证明。

返回 `Scoped<PageValue<T>>`，包含原 `RepositoryReadContext`。`Value` 明确给出 `Network` 或 `HistoricalSnapshot`、`fetched_at_ms`、`verified_at_ms` 及 `CacheCompleteness`。离线时间原样保留，不刷新时间、不暗称 fresh。`Miss` 与有效空的 `Value` 不同；无 README 的 `Missing`、根树不完整的 `Unavailable` 和请求失败仍分别保留。离线 snapshot 没有 TTL 自动晋升，调用者必须标识历史快照。

完整与 Partial 使用不同 slot。离线优先上次 Complete，没有 Complete 时才使用 Partial；因此新 Partial 不覆盖更完整的旧时间。Partial 内保留已取得响应、Link/cursor 及失败类别，重放仍产生真实的缺失页/作者/GraphQL errors，下一次在线读无条件重试。首屏与旧页分别保存，不能推导“完整历史”；列表跨刷新代次的拼接仍归现有 host 管理。Feed 的去重集合仅在读取成功及最终检查后回写，失败不能推进 cursor 或污染 seen IDs。

## HTTP 与授权边界

当前采用**无条件整次验证**，不发送持久验证器；每个 REST 原始响应仍保留 ETag/Last-Modified/Cache-Control。聚合无单一 validator，GraphQL 不造 ETag。意外 304 只允许一次无条件重取，若仍 304 则返回原 adapter 错误，绝不生成空 body。未来条件优化必须逐表示验证，不能用 Feed 第一页 ETag 代表 following/author 及旧页。

任一依赖响应（包括失败状态和首次意外 304）的 `no-store` 都使完整与 Partial 两个旧 slot 失效；该次内容仍可交给在线调用者。写 ticket 在请求前领取，较旧刷新不能覆盖新快照或用 no-store 删除新一轮值。当前有效会话的任一 HTTP 401 会记录 `CredentialRejected` 并拒绝发布，包括被 Feed 当作作者未解析的依赖失败；旧会话的 401 先被 context 检查丢弃。常规网络/超时/服务失败/限流及模糊 403/404 不会清缓存或更改授权账本。latest release 404 保留 NotFound-or-inaccessible 含义，标 Partial，不当作“确定不存在”。

持久 DTO 不保存请求 Authorization/cookie/PAT；响应仅保留解析/验证所需 headers，不存 Set-Cookie。失败响应不保存服务端正文；GraphQL errors 只保存 type/path，丢弃可回显输入的 message。未使用的 `download_url` 字段递归剔除。数据库、编解码和 README 内容准备在现有 Tokio blocking pool 执行；不创建 runtime，不在 UI render 中读取。

离线读持有 CacheLease，并在 adapter 解析结束后再次 `snapshot()` 检查；完成网络、blocking I/O 和发布前继续检查原 session。blocking worker 复用现有账户提交锁，把最后的 session 检查与缓存副作用串行到 PAT/selection 提交；网络请求不持锁，旧 401 不得隔离替换后的新凭据。缓存清理/退休/撤权会使 lease 或 ticket 失效；页面仍须在真正应用结果前复核 context、TabId、路由与 section generation，已复制的字节无法由 storage 撤回。

## 唯一后续组装点

由协调者在账户 lifecycle 与仓库缓存合入后指派**唯一正式 Shell 缓存接线任务**：在现有账户服务/executor 组装处注入同一个 CacheStore 与保存账户 scope，给 Home/Profile/Repository host 选择 online/offline 模式，消费来源/旧时间/Partial/Miss，并处理页面清空和 account aggregate 重新验证。该任务拥有 `app/pages` 等正式 host 接口。本任务没有添加第二个 Shell 入口或调整首屏/自动旧页触发行为。
