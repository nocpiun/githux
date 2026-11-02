# 仓库持久缓存适配层

状态：已实现可复用数据服务及离线组合测试；**尚未接入正式 Shell**。账户 lifecycle 的正式事件生产、CacheStore 应用组装和离线 UI 由后续唯一接线任务完成。本模块不自行打开数据库、创建账户分区或读取 PAT。

## 接口与策略

[`CachedRepositorySession`](../src/data/repository_cache.rs) 显式包装现有 `RepositorySession`，构造时注入同一个 `CacheStore` clone 与已保存账户的 `CacheScope`，并核对 `scope.account == session.context().auth().account`。原 `RepositorySession` 的无缓存方法保持在线行为。缓存错误使用独立 `RepositoryCacheError`，不要求既有 UI 的 `RepositoryError` 匹配增加缓存分支。

```rust
let cached = CachedRepositorySession::new(session, cache.clone(), scope)?;
let details = cached.repository(&repository, &limits,
    RepositoryCacheMode::OnlinePreferred, cancel).await?;
let snapshot = cached.revision(&repository, RevisionSpec::Ref(main), &limits,
    RepositoryCacheMode::OfflineOnly, cancel).await?;
```

支持 repository metadata、branches、revision、directory、README discovery、raw blob 与 prepared content。读取模式：

- `OnlinePreferred`：可变仓库信息、分支列表与 ref 指针在线验证；网络、超时、限流或 5xx 失败且有缓存时，返回明确 `Snapshot` 来源的旧值。401、取消、上下文失效、403/404 等不以缓存冒充成功。
- `OfflineOnly`：不发网络请求；未缓存返回 `Miss(NotCached)`，损坏/版本不符单独保留 miss 原因。
- `ForceRefresh`：跳过固定对象快速命中和可变 validator，发无条件网络请求。

固定 commit/tree/README/blob 的已缓存对象可直接复用，但每次读取仍复核 session context 和 cache gate。ref 快照只是上次观察，不表示当前 main。`CachedRepositoryRead` 携带 `Network`/`Snapshot`、fetched/verified 毫秒时间及完整性，供后续 UI 标注“上次同步/固定 commit”；不会将旧值包装成 fresh online。

所有缓存读、ticket、权限 proof 和写入均在 Tokio blocking worker 执行，等待前后检查原 session context 与取消状态。`content` 在缓存 bytes 读出后仍调用现有 `prepare_content`，保留有效空、二进制、非 UTF-8 和大小限制。

## 表示与身份

`githux-repository-v1` 的显式 DTO 记录 repository numeric id 以及 commit/root-tree/tree-entry/blob 的关联，读取重新验证请求身份；不 serde Rust 内部句柄。blob 只保存原始 bytes，固定键含 repo id 与 blob SHA，账户/partition 由 CacheStore 隔离，不保存 PAT 或 download URL。目录键包含影响完整性的 entry limit，branches 键包含分页/条目/响应上限；SSO、分页失败、截断等原因分类随 Partial 保留。

仓库详情与可变 ref 支持精确表示的 ETag/Last-Modified 条件读取。304 只有在本地表示存在且 validator 匹配时才复用原 bytes，保留 fetched、更新 verified 并原子写回；缺失/不同 validator 会自动无条件重取。聚合 branches、tree 和 README 不伪造 validator。`no-store` 走 CacheStore 的失效旧值路径。

在线请求前取得 revalidation proof 和普通写入 ticket。只有真实成功的对应仓库网络响应才确认权限；固定 SHA、本地 locator 或本地 oversized 拒绝不能解锁。隔离后成功响应再获得写入 ticket。旧响应由 CacheStore ticket 与原 `RepositoryReadContext` 双重拒绝，网络/超时/服务故障/限流和模糊 403/404 不触发撤权。

## 验证与边界

`src/data/repository_cache_tests.rs` 使用临时账户目录、临时 CacheStore、合成凭据和 mock transport，经过真实 AccountService/RepositorySession/GitHub 适配链，覆盖在线读取→关闭存储/服务→重建→离线 repository/tree/README/content、未读目录 NotCached、main A→B、同 SHA 跨账户、304 匹配与重取、no-store、网络旧值回退、Partial/空/二进制、取消/旧 ticket、权限 proof 和坏 DTO。

这些离线组合测试不是正式 Shell 的 GUI 验收，也未使用真实 PAT 或访问 GitHub。正式页面离线标记、Home 冷启动导航和账户 lifecycle 事件组合仍需后续接线与相应验收。
