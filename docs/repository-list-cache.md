# /user/repos 仓库列表持久缓存适配器

状态：数据层已实现并通过确定性测试；**已由 cache-runtime 通过同一应用 `CacheStore` 和 `RepoService::cache(&context)` 接入正式侧栏**（Online 记录 + 允许的网络故障回退 Offline 历史，`Historical` 来源在侧栏标示）。组装与回退策略见 [app-cache-integration](app-cache-integration.md)。

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

- 持久 DTO `ReposSnapshotV1 { version, exchanges: Vec<Exchange> }` 复用 `github/home/cache` 的响应记录/回放：只保留 method、完整 URL、固定 Accept/API 版本与响应状态、安全 headers（ETag/Last-Modified/Cache-Control/Link/X-GitHub-SSO/限流头）及清理后的 JSON body；不回放 Authorization、cookie、Set-Cookie 或未经解析的失败正文。仓库专用编码只保留现有 adapter 消费的仓库/owner 字段；2xx 非列表或不可解析正文替换为 `null`，回放仍得到解析失败，不持久化原始代理错误页。
- 回放仍运行真实 `fetch_user_repos` 分页适配器，因此 Complete、第 2 页起失败/解析失败、`X-GitHub-SSO: partial-results` 等语义原样保留，不另写一套列表/分页算法。
- Complete 与 Partial 使用不同 slot；离线优先 Complete，新 Partial 不覆盖更完整的旧 Complete。`fetched_at_ms`/`verified_at_ms` 原样保留，离线不刷新时间、不暗称 fresh。
- 任一响应的 `no-store`（含第一页 503/解析失败和 401）在传播读取错误前使 Complete/Partial 两个 slot 同时失效；成功内容仍可交给在线调用者；在线结果超过单条/容量预算时返回成功 Network 观察，不把容量退让变成授权错误。

## 授权与撤权边界

- 每次读取走既有 `RepoService::read_authenticated`：加载凭据、复核账户 epoch、取得并发 permit、取消检查，只是把 transport 换成记录/回放。缓存副作用（读/begin_write/put）在 blocking worker 上持有账户 `credential_commit` 锁并再次复核 context，与 PAT/selection 提交串行化；网络请求不持锁。
- 当前会话任一 401 通过 `RepoService::credential_rejected(&context)` 写账户拒绝 journal，再记录缓存门禁并拒绝发布，成功记录后返回 `PageCacheError::Context(GitHubError::Unauthorized)`。401 同时带 no-store 时，在推进拒权 epoch 前先尝试删除旧 slot；无论删除是否成功仍尝试记录拒权。门禁 SQL 失败时返回生命周期错误，已 clone 的 scope/lease 立即被本地 suspension 封禁；重启按 journal 重新拒绝，不因数据库恢复可写而复活旧私有快照。网络、超时、5xx、限流、模糊 403/404 不清缓存、不改授权账本。
- PAT 替换先隔离并推进 epoch，新凭据目录/journal 发布后由生命周期原子丢弃旧聚合并建立新代次；旧快照/lease/writer/proof 不复活，新凭据在线列表可直接保存，断网/重启可回放新快照。明确仓库撤权（`revoke_repository`）会删除该账户 `repository=None` 的聚合条目并推进聚合 revision：撤权前取得的写 ticket 在 put 时得到 `Stale`，离线读随后是 `Miss(NotCached)`。回放结束后再次 `lease.snapshot()` 检查，撤权/PAT 替换期间的旧值不作为存活快照返回。

## 验证

确定性测试位于 `src/data/repository_list_cache.rs` 的 `tests` 模块及其 `lifecycle_tests.rs`，使用临时账户 SQLite、真实 `CacheStore`、内存凭据与 `MockTransport`，覆盖：在线记录、重开 CacheStore 后断网回放、多页 Complete 与中途失败、SSO partial、cache miss、未绑定回退、账号切换/移除隔离、换 PAT 聚合隔离、明确撤权与真实在途旧响应、旧刷新不能覆盖新值、迟到 401 不污染替换后的凭据、no-store 错误路径、401 门禁 SQL 失败后的 lease 封禁和重启 journal 重放，以及异常正文/敏感头不持久化。均不访问 GitHub、不读取真实 PAT。

正式 Shell 注入与真实 PAT 联调未在本任务验证；CI/四平台构建结果以 PR 当前 head 检查为准。

## cache-runtime 使用片段

以下运行在既有后台 executor。无 binding 时保持原在线能力；`AccessDenied` 也可能表示私有门禁被拒绝，不能因此放开离线访问或为账户聚合发布单仓库 proof。

```rust
let repos = accounts.repo_service();
let context = repos.session()?.context().clone();
if repos.cache(&context).is_none() {
    let online = repos.list_repos_cancellable(cancel).await;
    // 在线结果仍按原账户/请求 generation 守卫应用。
} else {
    let result = repos.list_repos_cached(mode, cancel).await;
    // Ok(scoped)：先 repos.is_current(&scoped.context)，再检查宿主 generation。
    // PageValue::Value { value, origin, metadata }：保留 readiness、来源和旧时间。
    // PageValue::Miss(reason)：未缓存，不是完整空列表。
    // Err：保持读取/账户/缓存错误含义，不将其转为空列表或自动授权。
}
```

已复制到消费者的值仍须由宿主在移除/撤权时清空；本适配器不修改 UI，也不提供持续有效的展示许可。

## Cache-first 侧栏（2026-11-11）

正式侧栏在 `Operation::Repositories` 中额外启动一个只读快照作业（`read_cached_repositories`，`list_repos_cached(Offline)`）：命中即以 `RepositoryListOrigin::Historical` 发布既有历史行，`Reply.preliminary` 使该回复不结束操作、不解锁输入、不改自动加载标记；在线读取的结果随后覆盖为 `Network`。未命中、损坏或缓存故障不发布内容，侧栏保持原有 loading。历史行提示与 scoped Retry 沿用 `historical_status()`，未新增文案类型。回放分支使用 `read_authenticated_local`，不占网络并发配额。

`list_repos_cached` 对在线与历史读取都捕获并校验 `CacheAuthorization`。正式侧栏还在请求前捕获自己的票据，在 UI apply 前复核并在 drain 清掉已显示的失效副本；单仓库撤权不推进 AuthEpoch 也会清旧行，新 gate 恢复不让旧 reply 混入。接口线程与消费要求见 [cache-core](cache-core.md#已显示副本的权限失效接口)。
