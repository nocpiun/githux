# 头像磁盘缓存适配器

更新：2026-11-01。本文档记录头像加载链路接入普通可淘汰磁盘缓存（`CacheStore` / `CacheScope`）的工程设计、接口契约与验证边界。

## 架构与数据流

头像显示由 `ui::avatar::avatar` 组件负责呈现（圆形、tinted initials 兜底、固定尺寸、滚动空 mask 防护），加载与解码由 `app::avatar::AvatarFetcher` 与 `app::avatar::AvatarCache` 驱动。

本项改动接入底层 `storage::cache::CacheStore`，通过 `data::avatar_cache::AvatarDiskCache` 适配器将头像持久化至本地业务缓存，实现断网冷启动命中已缓存头像、同 URL 更新与 HTTP 304 续期。

```mermaid
flowchart TD
    UI[ui::avatar / Shell 组件] -->|AvatarKey| Cache[app::avatar::AvatarCache (内存解码 LRU)]
    Cache -->|未命中 / 启动拉取| Fetcher[app::avatar::AvatarFetcher]
    Fetcher -->|1. 尝试读取| Disk[data::avatar_cache::AvatarDiskCache]
    Disk -->|spawn_blocking| Store[storage::cache::CacheStore]
    Fetcher -->|2. 在线条件请求| Net[AvatarSource / ReqwestAvatars]
    Net -->|304 Not Modified| Update[revalidate_sync 续期时间戳]
    Net -->|200 OK| Put[put_sync 写入磁盘缓存]
    Net -->|no-store| Inval[invalidate_sync 失效旧缓存]
    Net -->|网络失败 / 离线| Fallback[读取已有本地快照或 initials 兜底]
    Put --> Store
    Update --> Store
    Inval --> Store
```

## 核心接口契约

### 1. 作用域与身份隔离
- **查看账户作用域**：调用方注入当前查看账户的 `CacheScope`。头像所属用户的私有/公开图片存放在当前授权账户的缓存空间内，不同登录账户之间的缓存相互隔离。
- **缓存键（CacheKey）**：
  - `resource = "avatar"`
  - `request = "subject={subject};url={clean_url}"`
  - `representation = "avatar-image-v1"`
  - `version = 1`
  - `repository = None`，`private = false`。

头像是无凭据的公开 CDN 图片，`private = false` 使它在 PAT 替换后仍可读写，同时隔离仍由 `CacheScope` 的 partition 保证（每个登录账户独立分区，退休/移除后仍拒绝读写）。若设为 `private = true`，头像会与账户级私有聚合共用 `repository=None` 门槛，PAT 替换后必须等待账户级证明才能写，而头像的匿名 GET 无法产生该证明，离线重启将无法恢复头像。详见 [app-cache-integration](app-cache-integration.md)。
- **URL 敏感参数脱敏**：`sanitize_avatar_url` 会剥离包含签名（`token`, `sig`, `signature`, `auth`, `key`, `secret`, `cred`, `expires`）的查询字符串，绝不将含有敏感凭据签名的完整链接作为键或暴露在日志中。

### 2. HTTP 缓存新鲜度与条件请求
- **AvatarSource trait**：支持发送 `If-None-Match`（ETag）和 `If-Modified-Since`（Last-Modified）。
- **写入票据先于网络**：`begin_write_sync` 先于读取快照与下载执行，返回响应用同一票据 `commit_write_sync` / `commit_revalidation_sync` 发布。期间发生 clear、invalidate 或更新写入时，`CacheStore::put` 以 `Stale` 拒绝，旧响应不会回填或覆盖新值。
- **304 Not Modified**：仅用请求前快照 + 同一票据续期 `verified_at_ms`；无本地表示时不捏造空字节。
- **200 OK**：先做 `MAX_AVATAR_BYTES`（2 MiB）上限与完整解码校验，再写 SQLite/文件对象并保存 `ETag`、`Last-Modified`、`Cache-Control`。
- **Cache-Control**：多个 `Cache-Control` 字段由 `collect_cache_control` 合并；缺失时返回 `None`，所以无头的 304 不会抹掉原 `no-cache` 策略。`no-store` 经同一票据交给 `CacheStore::put`，由其在票据校验后删除旧表示。
- **离线回退限制**：`is_snapshot_reusable_offline` 阻止 `no-cache` / `must-revalidate` 表示在未验证时被回退；回退与 304 分支都在 blocking worker 上重新 `read_sync` 复核 scope 授权，账户退休或凭据被拒后不再投递旧图片。
- **元数据必需**：启用磁盘缓存却只注入无元数据的 `AvatarBytes` 时，不执行任何磁盘读写；需 `AvatarSource` 才能遵守响应头语义。

### 3. 解码校验与安全防线
- 磁盘缓存读取的字节必须经过 `image::load_from_memory_with_format` 完整解码校验，才包装为 GPUI `Image`。损坏、截断或伪造签名的文件无法通过校验，安全回退到 initials fallback，绝不在 UI 产生破图。
- 所有磁盘 I/O（`store.read`, `store.put`, `store.invalidate`, 票据与快照读取）严格在 Tokio 的 `spawn_blocking` 中执行，绝不在 UI 线程或事件循环主任务执行。
- 未注入 `AvatarDiskCache` 或未登录时，保持原本纯内存缓存与无磁盘行为，完全兼容既有调用点。

## 正式 Shell 接入与失效

生产控制器 `src/app/accounts/mod.rs` 在每次权威账户状态刷新时核对当前 `active_summary`（`sync_avatar_binding`），summary 不变则不重建 fetcher；变化时：

1. `cancel()` 旧 `AvatarFetcher`，停止在途下载；
2. 用当前 `AccountCache` 的 `store`/`scope` 构造 `AvatarDiskCache`，并通过 `AvatarFetcher::with_source(...).with_cache(disk)` 重建 fetcher；
3. 重置内存解码 `AvatarCache`、清空头像请求队列，并换新 `AvatarInbox`，使旧 fetcher 的迟到完成不再写入新账户展示。

因此选择、替换、重验或移除账户后，已解码显示、在途读取和旧 scope 副本都不会在新账户下被复用。生产组装、回退与验证边界见 [app-cache-integration](app-cache-integration.md)。
