# 应用缓存运行时接线

更新：2026-11-04。本文记录正式应用把唯一业务 `CacheStore` 注入账户服务、并把头像、侧栏与页面消费者绑定到当前账户 `CacheScope` 的组装位置与生命周期。存储/政策语义见 [cache-core](cache-core.md)，账户事件与门禁见 [cache-account-lifecycle.md](cache-account-lifecycle.md)，头像磁盘适配器见 [avatar-cache.md](avatar-cache.md)，仓库列表适配器见 [repository-list-cache](repository-list-cache.md)。

## 状态

- 生产启动已从 `AccountService::open` 改为先打开应用 `CacheStore`，再调用 `AccountService::open_with_cache`（`src/app/accounts/runtime.rs`）。缓存打开失败回退到原有在线-only 服务，不使登录、切换、仓库读取不可用。
- 头像加载已在生产控制器中绑定当前账户的 `AccountCache`，账户/凭据变化时重建 fetcher 并清空已解码显示、在途请求与旧权限副本。
- 正式侧栏自动读取/重试已消费 `RepoService::list_repos_cached`：在线读取记录快照；只有已允许的瞬时网络故障回退到历史快照，并以 `RepositoryListOrigin::Historical` 标注离线/同步时间；未绑定缓存时保留原在线读取。
- 页面消费者通过既有 `RepoService::cache(context)` / `AccountService::cache(summary)` 取得绑定；Home/Profile/Repository 的页面快照适配器（`CachedPageReadSession`）已合并，它们与账户事件接线不属于本任务。

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

## 侧栏仓库列表

`Accounts` 的自动加载与重试（`Operation::Repositories`）统一走 `read_repositories`：

1. `RepoService::list_repos_cached(PageReadMode::Online, cancel)` 在线读取并记录账户聚合快照，发布 `RepositoryListOrigin::Network`。
2. 返回任意 `PageCacheError::Cache(_)`（未绑定缓存、对账未完成、私有聚合门禁被隔离，或运行期 SQLite/文件 I/O 故障）时回退 `list_repos_cancellable`，保持原在线能力；该回退仍保留取消、过期与授权检查，绝不让缓存问题移除在线读取。
3. 只有 `Network`、`Timeout`、`RateLimited` 与 5xx `UnexpectedStatus` 这类已允许的瞬时故障才用 `PageReadMode::Offline` 回放最后一次快照，发布 `RepositoryListOrigin::Historical { verified_at_ms }`。
4. `Unauthorized`(401)、`Forbidden`/`NotFound`/SSO、`Cancelled`、`StaleContext` 与缓存错误保持原义，不回退历史；网络失败后无快照时显示可重试的失败，而不是空列表。

`Repositories::historical_status()` 把历史来源转为一条简短行（`Offline snapshot · last synced <相对时间>`）；侧栏 `src/ui/mod.rs` 只消费这个方法，并在历史状态保留同样的 scoped Retry，历史/部分数据不会被渲染成在线完整结果。前轮曾直接修改 `src/ui/mod.rs` 的历史提示接线，并错误声称 shell-global-pages 已合入、无并发写者；协调验收已证实当时 #71 仍 OPEN 且在返工。该所有权判断错误，本轮不再编辑 `ui/mod.rs`、页面 hosts 或 Home rows；Shell 组合由其原执行者处理。

**PAT 替换恢复（验收返工 F3）**：替换前仍推进持久 epoch、隔离旧凭据；新凭据目录提交时同时保存现有 `replaced` recovery journal。目录发布后，生命周期原子删除旧账户聚合 entries/slots，并建立新 aggregate revision，只接收新凭据的新结果，成功后才确认 journal。重启发现 journal 会保守重做隔离/清除/新代次，再发绑定。未经重新读取的旧 Home/Profile/Feed 聚合不会恢复，单仓库旧内容仍须该仓库证明。`CredentialRejected` 的持久 journal 不会被普通 recover、GET /user 或列表成功清除；只有成功发布替换凭据的目录事务可取代它。侧栏和既有页面适配器无需调用 `permission_confirmed(None)`，正常在线刷新即可保存新快照。

在线/历史副本共用 `CacheAuthorization`：请求前通过同一 binding 的 `store.authorization(scope, None)` 捕获，后台完成及 UI apply 前检查同一 token 的 `is_current()`，显示期间由 drain 继续检查。`None` 资源表示聚合，独立仓库使用 `Some(repo_id)`。Token 捕获只取独立内存锁、检查只做原子读取，不等待 SQLite；撤权或新 gate 恢复后旧 token 永远无效。侧栏已经消费此接口，Home/Profile/Repository 由各自原执行者消费。详见 [cache-core](cache-core.md#已显示副本的权限失效接口)。

## Cache-first 首次内容呈现（2026-11-11）

已有可读快照时，正式页面不再等网络返回或失败才显示内容：每个独立 section 都会在后台先读本地快照并立即呈现，再让在线结果覆盖。实现细节与逐面覆盖见 [缓存先行开发日志](devlog/2026-11-11-cache-first-paint.md)。

**顺序规则（各宿主一致）**

1. 快照读取只做 `PageReadMode::Offline`/`RepositoryCacheMode::OfflineOnly`（纯本地），与在线读取**并发**启动；在线读取先开始，缓存 I/O 不前置、不替代网络。
2. 宿主为每个 section 保存显式发布标记（Home 的 `PublishMark`；其余页面的 `Reply`/`Message::preliminary`）。快照回复仅在“本 generation/request 尚未发布网络值、且未发布过同一快照”时生效；网络回复永远覆盖快照，迟到缓存不会回滚在线成功。
3. 快照阶段不释放 work 槽、不恢复刷新可用性、不推进 Feed 轮询冷却；这些仍由在线读取收尾，因此“快照在屏 + 刷新不可用”不会变成重复刷新。
4. 未命中、损坏、权限拒绝、存储 I/O 故障一律不发布内容，保留原有 online loading；旧快照/损坏内容不会被伪造成空成功。
5. 同一 `(partition, fetched_at_ms)` 的历史快照不重复发布（重复发布只会重置滚动与选择）。
6. 权限仍由既有 `CacheAuthorization` 票据与 session/context 复核把关：快照与在线结果携带同一请求前票据，发布前与显示期间都检查，撤权/换 PAT/移除账户照旧清除已显示副本。

**覆盖范围**：启动侧栏仓库列表、Home/Feed、Profile basic/pinned/calendar/README、Repository 摘要/分支/revision/根树/README/正文与 overview extras。Notifications 与全局 Issues/PR 列表没有可复用持久快照，本批不做新持久缓存产品。

**本地回放不占网络并发配额**：缓存复用的仍是既有在线适配器与真实 transport 回放，但回放不发请求，因此 `RepositorySession::local` / `RepoService::read_authenticated_local` 让 Offline 分支跳过 `BackgroundExecutor` 的有界网络 permit（凭据、epoch/context、取消、gate/lease 检查不变）。否则所有网络槽位都被挂起的重验占用时，缓存首帧会被饿死。

**Repository 一致性**：入口用一次 `OfflineOnly` 作业读完整快照（locator index → 摘要 → 分支 → ref revision → 根树 → README → 初始正文）后作为单个消息发布，header/tree/README 永远指向同一 commit；在线链随后重验，同一 commit 时保留按 commit 寻址的 tree/README/reader（含滚动位置），commit 变化才整体替换。

## 页面绑定

页面/仓库消费者通过固定读会话取绑定，不持有跨账户的 scope：

```rust
let binding = repo_service.cache(session.context()); // Option<AccountCache>
```

返回 `None` 表示无可用私有缓存（未启用、对账未完成、账户不匹配或已退休），不是空内容。消费者必须继续复核现有 session/context/资源代次/取消，并把 `scope.account()` 与会话账户对齐。Home/Profile/Repository 的持久页面快照适配器已在 `data/page_cache` 中按此模式实现；它们不是本任务的接入对象。

## 验证边界

- 本任务覆盖：真实生产组装回退（`open_accounts_with_cache` 的 `Ok/Err` 两分支）、账户服务注入同一 store、头像绑定到当前账户 scope、切换账户后旧 scope 不复活旧头像；侧栏在线读取发布 `Network`、允许的网络故障回退 `Historical` 并显示离线行、401 不冒充离线历史。全部既有账户/页面/壳测试保持通过。测试使用临时 SQLite/CacheStore、内存凭据与可控 transport，不访问 GitHub 或真实 PAT。
- 未覆盖：真实应用冷启动/断网、真实平台凭据与真实窗口观感；这些在 PR 描述中作为人工验收步骤列出，不把单元测试当原生验收。
