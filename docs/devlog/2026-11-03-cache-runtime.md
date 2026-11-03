# 2026-11-03：cache-runtime

任务分支 `feature/cache-runtime`，基线 main `d21711622ea7860c79e9a4ea8d35a78f010c162c`。把正式应用唯一 `CacheStore` 注入账户服务并让头像消费者绑定当前 `AccountCache`；侧栏 `list_repos_cached` 依赖本批 `sidebar-cache` 生产者，本分支暂不接入其文件。

## 实现与边界

- `src/app/accounts/runtime.rs`：`open_production` 改为在 blocking worker 打开 `CacheStore` 后调用 `AccountService::open_with_cache`；缓存打开失败回退 `AccountService::open`。抽出 `open_accounts_with_cache(config, store, client, executor, cache)` 作为唯一组装点，便于不触碰真实应用目录地测试 `Ok/Err` 两分支。缓存打开失败仅 stderr 诊断，不把在线账户功能带崩。
- `src/app/accounts/mod.rs`：头像 fetcher 从“安装一次”改为按当前 `active_summary` 绑定（`sync_avatar_binding`）。summary 变化时 `rebuild_avatar_fetcher(binding)`：取消旧 fetcher、用当前 `AccountCache` 构造 `AvatarDiskCache` 并 `with_source + with_cache`、重置内存解码缓存与请求队列、换新 `AvatarInbox` 丢弃旧迟到完成。生产 transport 仍是专用 `ReqwestAvatars`，不携带 PAT。
- 测试在 `src/app/accounts/tests.rs`：新增 `open_cached` 助手与 `CacheTestAvatarSource`（同时实现 `AvatarBytes`/`AvatarSource`），覆盖头像写入当前账户 scope、切换账户后旧 scope 不复活旧头像与内存解码缓存复位；另两条 `#[tokio::test]` 覆盖 `open_accounts_with_cache` 的缓存失败回退与启用绑定两分支。测试只用临时 SQLite/CacheStore、内存凭据与可控 transport。

没有改 `src/ui/mod.rs`、`src/app/pages/mod.rs`、`src/data/mod.rs`、页面 hosts 或存储核心；`src/data/mod.rs` 与 `repository_list_cache` 归 `sidebar-cache`，`list_repos_cached` 的生产者接线留到其合入后 rebase main 组合，当前侧栏仍是 `list_repos_cancellable`。

## 验证

全部 Cargo 调用经 `node scripts/verify.mjs -- cargo ...`，记录于 `.agents/task/runs`：

| 检查 | run_id | 结果 |
| --- | --- | --- |
| `cargo fmt --all` | `91e8079c-965b-4a68-8e3d-d6e2639d9aa8` | 0 |
| `cargo check --locked --all-targets` | `9cf8c0c8-c5b1-4151-978d-a5a04f603520` | 0 |
| `cargo clippy --locked --all-targets -- -D warnings` | `d18327ba-555a-4333-88ae-641510b19f03` | 0 |
| `cargo test --locked` | `af1fddda-6f31-40b1-8adc-6fb2323c6bda` | 0；全部 suite 通过 |

新增三条用例均在 `app::accounts::tests` 下通过：`cache_open_failure_falls_back_to_the_online_only_service`、`a_cache_enabled_service_exposes_a_binding_after_login`、`avatar_cache_binds_to_the_active_account_and_switching_accounts_drops_the_old_scope`。

**未执行**：真实应用冷启动/断网、侧栏与头像原生窗口恢复、真实 PAT 与平台凭据、真实 GitHub。这些属于本批组合验收与负责人验收，不由本机单元测试推断。

## 依赖与交接

- 依赖 `sidebar-cache` 的 `repository_list_cache` 数据适配器与 `RepoService::list_repos_cached`。生产者尚未合入（本批 base 即 main `d217116`，本地 `feature/sidebar-cache` 无领先提交）。其合入 main 后在本分支 rebase 并完成侧栏组合，不直接改写其分支/文件。
- 文档所有权：新增 `docs/app-cache-integration.md`，更新 `docs/cache-account-lifecycle.md`、`docs/avatar-cache.md` 的状态与接入说明；未改 `docs/README.md`、`product.md`、`roadmap.md`、`cache-strategy.md` 四份总文档。
- 可复用教训候选：无新增稳定规则。头像 fetcher 的不可变 binding 需要在账户变化时整体重建并丢弃旧 inbox，才能既取消在途又防止迟到完成污染新账户；已按本任务边界记录，不升级为 LESSONS 条目。
