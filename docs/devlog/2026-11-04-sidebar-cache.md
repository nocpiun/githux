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
- L008：初版仅覆盖存储 ticket 的先取、撤权和迟到 put。审查修复轮次补上真实入口的受控异步响应：请求暂停后推进账户/授权或撤权，再释放旧响应；另有两个实际重叠刷新验证旧值不覆盖新快照。
- L005：取消走既有 `CancellationToken`/`read_authenticated` 守卫，本适配器不自建取消语义。
- L014：缓存 Miss、有效空、授权拒绝分开返回，本任务不涉及可见组件。
- L012/L013：服务能力与正式 Shell 消费能力分开，唯一接线由 cache-runtime 负责。

未修改共享 LESSONS，沿用 main #67 新增的 L020（失败依赖 no-store）及 L021（跨存储拒绝屏障）。无 binding 与 gate 拒绝都使用 `Cache(AccessDenied)`；消费者通过 `RepoService::cache(&context)` 预先判断 binding，不能把该错误当作绕过离线门禁的依据。

## 验证

全部 Cargo 命令经 `node scripts/verify.mjs -- cargo ...`，共享 Windows mutex，独立 target。以下为本地结果，不替代 CI/review：

- `fmt --all -- --check`（`398f389b`）
- `check --locked --all-targets`（`18ccc32c`）
- `clippy --locked --all-targets -- -D warnings`（`612d52ba`、`04a010e0`）
- `build --locked`（`c1492f55`）
- `cargo test --locked repository_list_cache`（`448b1f45` 初版 7 项、`d4c709a2` 最终 13 项，0 failed）
- 完整 `cargo test --locked`（`9de970ec`）主程序 1121、其余 targets 96/573/279/250/765/849/44，0 failed。

覆盖：在线记录、重开 CacheStore 后断网回放、多页 Complete 与中途失败、SSO partial、cache miss、未绑定回退、账号切换/移除隔离、换 PAT 聚合隔离、明确撤权与在途旧 ticket、no-store、401 拒绝不持久化。测试命中真实 `RepoService::list_repos_cached` 入口，不单测序列化。

首轮聚焦测试曾有两处失败，读取原始失败日志后修正：在线请求计数把登录 `/user` 算漏（失败 run `36daf4d1`）；401 分支原先在 `read_authenticated` 返回错误处直接 `?`，导致 `transport.rejected()` 的 `CredentialRejected` 发布与 `Context(Unauthorized)` 语义被跳过，改为先捕获结果、再按 page_cache 顺序处理拒权后映射错误。

## PR #70 第一轮修复与崩溃恢复

PR [#70](https://github.com/nocpiun/githux/pull/70) 的首轮 head `145d22a` 收到两项成立反馈，均可按既有生命周期契约直接解决，没有待裁决事项，保留 `false/worker`：

- [P1](https://github.com/nocpiun/githux/pull/70#discussion_r4457456239)：直接调用缓存 `access_event` 漏掉账户 journal 与门禁写入失败时的本地 suspension。改走 `RepoService::credential_rejected`；用真实 401 入口、SQLite trigger 注入 UPDATE 失败，验证旧 lease 即时封禁，并关闭/重开账户和 CacheStore 验证持久 journal 重放。
- [P2](https://github.com/nocpiun/githux/pull/70#discussion_r4457456268)：首屏失败在处理 no-store 前被 `?` 提前返回。将 no-store 两个 slot 的失效移到错误传播前，503 + no-store 的回归覆盖在线报错后离线 Miss。

2026-11-04 Kero 崩溃后只恢复原 Pi 会话和已有 PR。检查无存活 Cargo/rustc；崩溃前的聚焦 run `099bca95` 实际在 00:50 完成，14 项通过，有完整 exit=0 JSON，非将中断状态当作通过。之后为真实在途顺序、故障 journal 及异常正文清理新增回归，故需重新验证这些变更。

新增受控 MockTransport 暂停真实 `/user/repos` 响应：覆盖切账户、换 PAT、移除/重加、明确仓库撤权、旧 401 不误拒绝新凭据、两个刷新旧值不能覆盖新值；单仓库 proof 仍不解锁账户聚合。编码复用 `src/github/repos.rs` 原 `RepoResponse`/`OwnerResponse` 类型的最小 Serialize 扩展和 `snapshot_body` 投影（共享 recorder 没有仓库 schema 清理入口），只保留 adapter 消费的字段，2xx 非法列表/代理正文保存为解析失败占位；不新增分页算法或修改原网络入口的返回契约。该清理不改变网络返回值的服务顺序或完整性，离线仍由真实 adapter 解析。

首轮 CI `41673181406` 的 code-check 和 Windows/Linux/x86 macOS 构建成功，ARM macOS 构建取消，汇总失败；旧 head 不可据此合并。后续新 head 必须重新满足全部所需 CI 和 Codex review。

初次新增故障/在途回归验证 run `c643627f`（`test --locked --bin githux repository_list_cache`）18 项通过。将手工字段投影收紧为既有 RepoResponse 类型序列化并扩充 malformed 对象/数组用例后，fmt check `2a2bc85d`、全 targets check `47193983`、clippy `eca870be` 全通过。

完整 test run `0571fbf2` 的主程序 1126、头像 96、Home 578、Profile 279、Reader 250 全通过；reader_images target 769 通过/1 失败：既有 `fixed_revision_images_persist_across_reopened_store_and_service` 在重开 CacheStore 时得到 `InUse`（测试用 50ms sleep 等异步 put），原始失败日志保留。未修改 Reader/image 文件或放宽断言；该调用退出 101，repository/overview 未执行，不能宣称完整套件通过。

随后补上 `401 + no-store` 的组合回归与同分支处理：先尝试删除两个旧 slot，再通过生命周期记录拒绝；即便 no-store 失效失败，也必须尝试记录拒绝，不让旧私有值恢复。替换凭据并显式提供聚合 proof 后仍应 Miss，避免 no-store 只因 quarantine 暂时不可见。这是实际代码变更，需重做源检查、完整 main 测试，并串行补跑 reader_images/repository/repository_overview 三个 targets 和 build；其余已通过 targets 不机械重复。

最终修复源码已通过 fmt check `71d98e62`、全 targets check `8bdb66d6`、clippy `cee265ab`（`-D warnings`）；完整主程序测试 `72f25a85`：1127 项通过，包含 19 项 repository_list_cache 回归。串行恢复 run `3817a842` 的 reader_images 771、repository 855、overview 44 全通过，先前 InUse 的图片重启用例也通过。最终 build `7579714c` 也通过。保留前一轮默认并发失败记录；最终验证覆盖这些实际修改及剩余 targets，不把失败改写为成功。所有 Cargo 操作均经共享 verify runner。通过后只整理本任务文档与 PR 记录，不重复运行未变化的源码检查。

## 未接入与剩余边界

正式 Shell 离线呈现、账户聚合重新授权证明与 UI 消费未在本任务接线；无真实 PAT、外部写操作或原生 GUI 验收。CI、Codex review 与 PR 合并按工作流进行，交付后由 cache-runtime 消费本接口。
