# 应用缓存运行时接线

更新：2026-11-03。本文记录正式应用把唯一业务 `CacheStore` 注入账户服务、并把头像与页面消费者绑定到当前账户 `CacheScope` 的组装位置与生命周期。存储/政策语义见 [cache-core](cache-core.md)，账户事件与门禁见 [cache-account-lifecycle.md](cache-account-lifecycle.md)，头像磁盘适配器见 [avatar-cache.md](avatar-cache.md)。

## 状态

- 生产启动已从 `AccountService::open` 改为先打开应用 `CacheStore`，再调用 `AccountService::open_with_cache`（`src/app/accounts/runtime.rs`）。缓存打开失败回退到原有在线-only 服务，不使登录、切换、仓库读取不可用。
- 头像加载已在生产控制器中绑定当前账户的 `AccountCache`，账户/凭据变化时重建 fetcher 并清空已解码显示、在途请求与旧权限副本。
- 页面消费者通过既有 `RepoService::cache(context)` / `AccountService::cache(summary)` 取得绑定；Home/Profile/Repository 的页面快照适配器（`CachedPageReadSession`）已合并，它们与账户事件接线不属于本任务。
- 侧栏的 `RepoService::list_repos_cached(PageReadMode, CancellationToken)` 依赖本批 `sidebar-cache` 生产者的 `repository_list_cache` 数据适配器；生产者尚未合入，本任务在 rebase 后接入，不修改其分支/文件。当前侧栏仍是 `list_repos_cancellable` 在线读取。

## 组装位置与注入

`src/app/accounts/runtime.rs::open_production` 是唯一生产组装点：

1. 打开账户目录配置与后台 executor（既有路径不变）。
2. 在 blocking worker 打开 `CacheStore::open(CacheConfig::application_cache())`，失败时保留 `Err`。
3. `open_accounts_with_cache(config, store, client, executor, cache)`：
   - `Ok(cache)` → `AccountService::open_with_cache`，账户目录与缓存账本对账后发绑定；
   - `Err` → `AccountService::open`，在线功能照常，`RepoService::cache`/`AccountService::cache` 返回 `None`（fail-closed，不是空缓存命中）。

`CacheStore` 被 `open_with_cache` 移入 `AccountService` 内的 `AccountCaches`，随 `AccountRuntime → AccountService` 一起存活；`AccountService::close` 关闭账户 DB，缓存租约随最后一个 `CacheStore` clone 释放。缓存没有独立的 `close` 等待，不需要额外的关闭顺序；账户目录的既有 `credential_commit` 串行化仍覆盖缓存门禁提交。

## 头像绑定

`Accounts` 控制器持有 `avatar_fetcher: Arc<AvatarFetcher>`，由 `sync_avatar_binding` 在每次权威账户状态刷新时核对当前 `active_summary`：

- summary 不变时不重建，避免普通重绘/无变化结算反复取消在途头像。
- summary 变化（选择、替换、重验、移除后无账户）时调用 `rebuild_avatar_fetcher(binding)`：
  - 先 `cancel()` 旧 fetcher，使在途 `AvatarSource`/`AvatarBytes` 读取停止；
  - 用当前 `AccountCache` 的 `store`/`scope` 构造 `AvatarDiskCache`，`AvatarFetcher::with_source(生产 ReqwestAvatars).with_cache(disk)`；
  - 重置内存 `AvatarCache`、清空头像队列，并换新 `AvatarInbox`，使旧 fetcher 的迟到完成不再写入新账户展示。

头像缓存键 `private = false`：头像是无凭据的公开 CDN 图片，账户隔离由 `CacheScope` partition 保证；若标为 `private = true` 会与账户级私有聚合共用 `repository=None` 门槛，PAT 替换后被隔离且匿名头像 GET 无法产生账户级证明，离线重启将无法恢复。详见 [avatar-cache](avatar-cache.md)。

所有磁盘 I/O 都发生在 `AvatarFetcher` 既有的 `spawn_blocking` 路径，不在 GPUI render/事件线程。头像仍按 owner 身份键去重、圆形绘制与 initials 兜底不变。

## 页面绑定

页面/仓库消费者通过固定读会话取绑定，不持有跨账户的 scope：

```rust
let binding = repo_service.cache(session.context()); // Option<AccountCache>
```

返回 `None` 表示无可用私有缓存（未启用、对账未完成、账户不匹配或已退休），不是空内容。消费者必须继续复核现有 session/context/资源代次/取消，并把 `scope.account()` 与会话账户对齐。Home/Profile/Repository 的持久页面快照适配器已在 `data/page_cache` 中按此模式实现；它们不是本任务的接入对象。

## 验证边界

- 本任务覆盖：真实生产组装回退（`open_accounts_with_cache` 的 `Ok/Err` 两分支）、账户服务注入同一 store、头像绑定到当前账户 scope、切换账户后旧 scope 不复活旧头像、全部既有账户/页面测试保持通过。测试使用临时 SQLite/CacheStore、内存凭据与可控 transport，不访问 GitHub 或真实 PAT。
- 未覆盖：侧栏离线历史列表（依赖 `sidebar-cache` 生产者）、真实应用冷启动/断网验收、真实平台凭据与真实窗口观感。这些在 PR 描述中明确为 pending/组合依赖，不把单元测试当原生验收。
