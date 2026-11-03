# sidebar-cache 开发记录

任务：为 `/user/repos` 仓库侧栏补齐账户隔离的持久缓存数据适配器，分支 `feature/sidebar-cache`，起点 main `d217116`。契约见 [repository-list-cache](../repository-list-cache.md)。本任务只做服务层，交由 cache-runtime 挂载；不修改 app/accounts、Shell、page_cache 公共契约、共享 Reader 或其他页面。

## 实现

在 `src/data/repository_list_cache.rs` 新增 `RepoService::list_repos_cached`，并在 `src/data/mod.rs` 注册模块。入口为：

```rust
list_repos_cached(mode, cancel) -> Result<Scoped<PageValue<ReposOutcome>>, PageCacheError<GitHubError>>
```

- `Online` 完整读取 `/user/repos` 并记录版本化快照；`Offline` 回放最后 Complete（回退 Partial）快照，不发网络请求。
- 缓存绑定沿用 `RepoService::cache(&context)`，不另收 store。无绑定返回 `PageCacheError::Cache(CacheError::AccessDenied)`，供消费者回退 `list_repos_cancellable`；已绑定无快照返回 `PageValue::Miss`。
- 持久 DTO `ReposSnapshotV1 { version, exchanges: Vec<Exchange> }` 复用 `github/home/cache` 的 `Exchange`/`PageTransport` 记录与回放，不新增第二套凭据/权限/分页栈；离线回放仍运行真实 `fetch_user_repos`，保留 Complete、第 2 页起失败、SSO partial-results 语义。
- 键为 `repository=None` 私有账户聚合：请求身份含 captured login、`user/repos`、固定 query、base/accept/api 与 partial 标志。Complete/Partial 分 slot；no-store 使两者同时失效；当前会话 401 记录 `CredentialRejected` 不发布；换 PAT、明确撤权、移除走既有门槛。
- 读取沿用 `RepoService::read_authenticated`（凭据加载、epoch、permit、取消），只换 transport；缓存副作用在 blocking worker 持 `credential_commit` 串行化，网络请求不持锁。离线回放后再 `lease.snapshot()` 复核，撤权/替换期间的旧值不作为存活快照返回。

## 教训应用

- L002：验证用临时账户 SQLite + 真实 CacheStore + MockTransport，不扩张为真实 GitHub/PAT/原生 GUI 验收。
- L007：缓存层不新增 URL 解析器、不保存或重放 Authorization/cookie，复用 GitHubClient/PageTransport 的脱敏与安全头过滤。
- L008：在途旧响应用真实先取 ticket、再撤权、后 put 的顺序断言 Stale，不是顺序调用冒充。
- L005：取消走既有 `CancellationToken`/`read_authenticated` 守卫，本适配器不自建取消语义。
- L014：缓存 Miss、有效空、授权拒绝分开返回，本任务不涉及可见组件。
- L012/L013：服务能力与正式 Shell 消费能力分开，唯一接线由 cache-runtime 负责。

未修改共享 LESSONS。候选证据：冻结接口在“无绑定”与“gate 拒绝”上都复用 `Cache(AccessDenied)`，消费者回退在线路径即可同时覆盖两者；无需为“无绑定”扩 `PageCacheError` 变体。已在 `no_binding_is_access_denied_so_consumer_can_fall_back` 建立回归。

## 验证

全部 Cargo 命令经 `node scripts/verify.mjs -- cargo ...`，共享 Windows mutex，独立 target。以下为本地结果，不替代 CI/review：

- `fmt --all -- --check`（`398f389b`）
- `check --locked --all-targets`（`18ccc32c`）
- `clippy --locked --all-targets -- -D warnings`（`612d52ba`、`04a010e0`）
- `build --locked`（`c1492f55`）
- `cargo test --locked repository_list_cache`（`448b1f45` 初版 7 项、`d4c709a2` 最终 13 项，0 failed）
- 完整 `cargo test --locked`（`9de970ec`）主程序 1121、其余 targets 96/573/279/250/765/849/44，0 failed。

覆盖：在线记录、重开 CacheStore 后断网回放、多页 Complete 与中途失败、SSO partial、cache miss、未绑定回退、账号切换/移除隔离、换 PAT 聚合隔离、明确撤权与在途旧 ticket、no-store、401 拒绝不持久化。测试命中真实 `RepoService::list_repos_cached` 入口，不单测序列化。

首轮聚焦测试曾有两处失败，读取原始失败日志后修正：在线请求计数把登录 `/user` 算漏（`57300929` 类比）；401 分支原先在 `read_authenticated` 返回错误处直接 `?`，导致 `transport.rejected()` 的 `CredentialRejected` 发布与 `Context(Unauthorized)` 语义被跳过，改为先捕获结果、再按 page_cache 顺序处理拒权后映射错误。

## 未接入与剩余边界

正式 Shell 离线呈现、账户聚合重新授权证明与 UI 消费未在本任务接线；无真实 PAT、外部写操作或原生 GUI 验收。CI、Codex review 与 PR 合并按工作流进行，交付后由 cache-runtime 消费本接口。
