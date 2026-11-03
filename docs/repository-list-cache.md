# /user/repos 仓库列表持久缓存适配器

状态：数据层已实现并通过确定性测试。**尚未接入正式 Shell**，不代表应用已支持离线仓库导航；由 cache-runtime 任务通过同一应用 `CacheStore` 和 `RepoService::cache(&context)` 挂载。

## 入口与账户绑定

仓库侧栏离线读取使用 `RepoService` 的冻结接口：

```rust
pub async fn list_repos_cached(
    &self,
    mode: crate::data::page_cache::PageReadMode,
    cancel: CancellationToken,
) -> Result<Scoped<crate::data::page_cache::PageValue<ReposOutcome>>,
             crate::data::page_cache::PageCacheError<GitHubError>>
```

- `PageReadMode::Online`：完整执行 `/user/repos` 读取并记录版本化快照，返回 `PageOrigin::Network`。
- `PageReadMode::Offline`：不发起网络请求，回放最后一次 Complete（无 Complete 时回退 Partial）快照，返回 `PageOrigin::HistoricalSnapshot` 与保存时的真实时间。
- 缓存绑定沿用 `RepoService::cache(&context)`，不另收全局 store、不自己开分区。**未绑定可辨别**：返回 `PageCacheError::Cache(CacheError::AccessDenied)`，消费者据此回退到既有 `list_repos_cancellable` 在线路径。已绑定但没有快照时返回 `PageValue::Miss(CacheMiss::NotCached)`，与有效空列表、授权拒绝分开。
- 返回的 `Scoped` 携带真实捕获的 `RepositoryReadContext`；发布前复核原会话、取消令牌与账户 epoch。

## 缓存身份与门槛

- 外层键：`CacheKey::mutable(None, true, "repository-list-v1", request, "githux-repository-list-v1", 1)`。`repository=None` 表示这是私有账户聚合，使用账户聚合 gate，不随任一单仓库 proof 解锁；请求身份包含 captured login、端点 `user/repos`、固定查询 `affiliation=owner,collaborator,organization_member&per_page=100&sort=pushed`、base URL、Accept、API 版本与 `partial` 标志。
- 账户本身由 `CacheScope` 的持久分区隔离；换 PAT、移除重加、跨账户都走既有 `AccountService`/`CacheStore` 门槛，本适配器不新建凭据或权限栈。

## 表示、完整性与时间

- 持久 DTO `ReposSnapshotV1 { version, exchanges: Vec<Exchange> }` 复用 `github/home/cache` 的响应记录/回放：只保留 method、完整 URL、固定 Accept/API 版本与响应状态、安全 headers（ETag/Last-Modified/Cache-Control/Link/X-GitHub-SSO/限流头）及清理后的 JSON body；不回放 Authorization、cookie、Set-Cookie 或未经解析的失败正文。
- 回放仍运行真实 `fetch_user_repos` 分页适配器，因此 Complete、第 2 页起失败/解析失败、`X-GitHub-SSO: partial-results` 等语义原样保留，不另写一套列表/分页算法。
- Complete 与 Partial 使用不同 slot；离线优先 Complete，新 Partial 不覆盖更完整的旧 Complete。`fetched_at_ms`/`verified_at_ms` 原样保留，离线不刷新时间、不暗称 fresh。
- 任一响应的 `no-store` 使 Complete/Partial 两个 slot 同时失效，该次内容仍可交给在线调用者；在线结果超过单条/容量预算时返回成功 Network 观察，不把容量退让变成授权错误。

## 授权与撤权边界

- 每次读取走既有 `RepoService::read_authenticated`：加载凭据、复核账户 epoch、取得并发 permit、取消检查，只是把 transport 换成记录/回放。缓存副作用（读/begin_write/put/access_event）在 blocking worker 上持有账户 `credential_commit` 锁并再次复核 context，与 PAT/selection 提交串行化；网络请求不持锁。
- 当前会话任一 401 记录 `AccessEvent::CredentialRejected` 并拒绝发布，返回 `PageCacheError::Context(GitHubError::Unauthorized)`，旧快照随后不可读。网络、超时、5xx、限流、模糊 403/404 不清缓存、不改授权账本。
- PAT 替换使账户聚合 gate 隔离，旧快照、旧 lease 与更早写 ticket 均不可回填或重开。明确仓库撤权（`revoke_repository`）会删除该账户 `repository=None` 的聚合条目并推进聚合 revision：撤权前取得的写 ticket 在 put 时得到 `Stale`，离线读随后是 `Miss(NotCached)`。回放结束后再次 `lease.snapshot()` 检查，撤权/PAT 替换期间的旧值不作为存活快照返回。

## 验证

确定性测试位于 `src/data/repository_list_cache.rs` 的 `tests` 模块，使用临时账户 SQLite、真实 `CacheStore`、内存凭据与 `MockTransport`，覆盖：在线记录、重开 CacheStore 后断网回放、多页 Complete 与中途失败、SSO partial、cache miss、未绑定回退、账号切换/移除隔离、换 PAT 聚合隔离、明确撤权与在途旧 ticket、no-store、401 拒绝不持久化。均不访问 GitHub、不读取真实 PAT。

未验证：正式 Shell 注入、真实 PAT 联调、真实四平台 CI（以 PR 检查为准）。
