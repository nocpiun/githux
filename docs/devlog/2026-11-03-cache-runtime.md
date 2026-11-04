# 2026-11-03：cache-runtime

任务分支 `feature/cache-runtime`，基线 main `d21711622ea7860c79e9a4ea8d35a78f010c162c`。把正式应用唯一 `CacheStore` 注入账户服务、让头像消费者绑定当前 `AccountCache`，并在 `sidebar-cache`（#70，0cdb59c）合入后 rebase main 完成侧栏 `list_repos_cached` 接入。

## 实现与边界

- `src/app/accounts/runtime.rs`：`open_production` 改为在 blocking worker 打开 `CacheStore` 后调用 `AccountService::open_with_cache`；缓存打开失败回退 `AccountService::open`。抽出 `open_accounts_with_cache(config, store, client, executor, cache)` 作为唯一组装点，便于不触碰真实应用目录地测试 `Ok/Err` 两分支。缓存打开失败仅 stderr 诊断，不把在线账户功能带崩。
- `src/app/accounts/mod.rs`：头像 fetcher 从“安装一次”改为按当前 `active_summary` 绑定（`sync_avatar_binding`）。summary 变化时 `rebuild_avatar_fetcher(binding)`：取消旧 fetcher、用当前 `AccountCache` 构造 `AvatarDiskCache` 并 `with_source + with_cache`、重置内存解码缓存与请求队列、换新 `AvatarInbox` 丢弃旧迟到完成。生产 transport 仍是专用 `ReqwestAvatars`，不携带 PAT。
- 测试在 `src/app/accounts/tests.rs`：新增 `open_cached` 助手与 `CacheTestAvatarSource`（同时实现 `AvatarBytes`/`AvatarSource`），覆盖头像写入当前账户 scope、切换账户后旧 scope 不复活旧头像与内存解码缓存复位；另两条 `#[tokio::test]` 覆盖 `open_accounts_with_cache` 的缓存失败回退与启用绑定两分支。测试只用临时 SQLite/CacheStore、内存凭据与可控 transport。

没有改 `src/app/pages/mod.rs`、`src/data/mod.rs`、页面 hosts 或存储核心；`src/data/mod.rs` 与 `repository_list_cache` 由 `sidebar-cache` 拥有。侧栏接入后对 `src/ui/mod.rs` 只做了渲染历史来源所需的最小改动（详见下文）。

## 侧栏接入（2026-11-04）

`sidebar-cache` 以 squash 合入 main（`0cdb59c`）后，在原分支 rebase main，未改写其文件。

- `src/app/accounts/mod.rs`：新增 `read_repositories`，自动加载/重试统一走 `RepoService::list_repos_cached`。在线读取发布 `RepositoryListOrigin::Network`；任意 `PageCacheError::Cache(_)`（未绑定、对账未完成、私有聚合门禁隔离或运行期 I/O 故障）回退原 `list_repos_cancellable` 保持在线能力；仅 `Network`/`Timeout`/`RateLimited`/5xx 这四类瞬时故障回退 `PageReadMode::Offline`，发布 `Historical { verified_at_ms }`；401/403/404/SSO/取消/过期保持原义。网络失败但无快照时显示可重试失败，不伪造空列表。
- `Repositories` 增加 `origin` 与 `historical_status()`；侧栏消费该方法，把历史来源显示为 `Offline snapshot · last synced <相对时间>` 并保留 scoped Retry；历史空列表不再显示“当前无仓库”的旧文案。**前轮曾直接改 `src/ui/mod.rs` 并错误声称 shell-global-pages 已合入、无并发写者；协调验收已指正该所有权错误。本轮不再编辑 `ui/mod.rs`、Home/Profile/rows 或页面 hosts，Shell 组合由原执行者处理。**
- 新用例：`a_network_failure_falls_back_to_the_cached_list_as_history`（网络失败→历史行/侧栏状态可见）、`a_401_is_not_shown_as_offline_history`（401 不冒充离线）、`only_a_historical_ready_list_reports_an_offline_label`、`epoch_milliseconds_convert_to_the_relative_formatter_input`。

**验收返工 F3 已实现（2026-11-04）**：PAT 替换后侧栏仍可在线读，且新凭据的新结果可保存。旧账户聚合被丢弃，不再等待“其他消费者 permission_confirmed(None)”；旧 Home/Profile/Feed 未经重读不恢复，单仓库旧内容仍须该仓库证明；`CredentialRejected` 持久 journal 不因 recover/GET /user/列表成功解除。

## 验证

全部 Cargo 调用经 `node scripts/verify.mjs -- cargo ...`，记录于 `.agents/task/runs`：

| 检查 | run_id | 结果 |
| --- | --- | --- |
| `cargo fmt --all` | `91e8079c-965b-4a68-8e3d-d6e2639d9aa8` | 0 |
| `cargo check --locked --all-targets` | `9cf8c0c8-c5b1-4151-978d-a5a04f603520` | 0 |
| `cargo clippy --locked --all-targets -- -D warnings` | `d18327ba-555a-4333-88ae-641510b19f03` | 0 |
| `cargo test --locked` | `af1fddda-6f31-40b1-8adc-6fb2323c6bda` | 0；全部 suite 通过 |

侧栏接入后的最新 head 验证（rebase main `0cdb59c` 后）：

| 检查 | run_id | 结果 |
| --- | --- | --- |
| `cargo fmt --all` | `fd9cd9a0-d18b-4fe2-b0d9-4fa93b7ab72c` | 0 |
| `cargo check --locked --all-targets` | `7a0aa4ac-4ed0-44a2-b920-d1264ce58dc1` | 0 |
| `cargo clippy --locked --all-targets -- -D warnings` | `2a8d237d-7b12-4fd3-a0f0-f9dd77717ef2` | 0 |
| `cargo test --locked` | `a2b59478-299c-43ca-bdce-60f895be6569` | 0；全部 suite 通过 |

聚焦账户测试 `cargo test --locked app::accounts` 为 `20ffd883-e04a-4166-bb73-f7c3fc7d1aee`（37/37），其中新增网络回退/401/历史标注均通过。

新增三条用例均在 `app::accounts::tests` 下通过：`cache_open_failure_falls_back_to_the_online_only_service`、`a_cache_enabled_service_exposes_a_binding_after_login`、`avatar_cache_binds_to_the_active_account_and_switching_accounts_drops_the_old_scope`。

**未执行**：真实应用冷启动/断网、侧栏与头像原生窗口恢复、真实 PAT 与平台凭据、真实 GitHub。这些属于本批组合验收与负责人验收，不由本机单元测试推断。

## 审查修复

Codex 在 PR #68 指出：PAT 替换后 `commit_login` 施加 `AccessEvent::PatReplaced`，使 `repository=None` 的私有聚合被隔离；而 `avatar_cache_key` 原先把头像标成 `private = true`，匿名头像 GET 又无法产生账户级证明，导致替换后头像仍能在线渲染但写不进磁盘，离线重启无法恢复。

修复为把头像缓存键改为 `private = false`（`src/data/avatar_cache.rs` 的一处最小改动，本文件非本任务默认所有权，已在 PR 记录原因与最小接口）：头像是无凭据公开 CDN 图片，账户隔离由 `CacheScope` partition 保证，退休/移除仍拒绝读写；改为 public 后头像跨 PAT 替换仍可读写，同时不会解锁无关的 Home/Profile/Feed 私有聚合快照。新增 `avatar_writes_survive_pat_replacement_but_the_private_aggregate_stays_quarantined` 回归：替换后头像可写、私有聚合仍拒绝，确认账户级证明后才解锁私有聚合。

## 依赖与交接

- `sidebar-cache` 已以 squash 合入 main（#70，`0cdb59c`），侧栏 `list_repos_cached` 已在本分支消费；未直接改写其文件。
- 文档所有权：新增 `docs/app-cache-integration.md`，更新 `docs/cache-account-lifecycle.md`、`docs/avatar-cache.md`、`docs/repository-list-cache.md` 的状态与接入说明；未改 `docs/README.md`、`product.md`、`roadmap.md`、`cache-strategy.md` 四份总文档。
- 可复用教训候选：无新增稳定规则。头像 fetcher 的不可变 binding 需要在账户变化时整体重建并丢弃旧 inbox；侧栏历史回退只对已允许的瞬时故障生效，其余错误保持原义。均按本任务边界记录，不升级为 LESSONS 条目。
