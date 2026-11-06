# 2026-11-03 开发日志：RepositoryPage 持久缓存与跨重启离线阅读 (repository-offline)

## 任务目标与范围

在正式 `RepositoryPage`（`src/ui/repository/**`、`src/ui/repository_overview/**`）中接上已合并的 `CachedRepositorySession`、`CachedPageReadSession` 与 `RepositoryImageService::with_cache`：
- 保持 `RepositoryPageServices::new(accounts, executor)` 等既有构造兼容；页面不自行打开 `CacheStore`，只从 `RepoService::cache(session.context())` 取得当前已保存账户的 `(CacheStore, CacheScope)` 绑定，无绑定时保留原有在线路径。
- 连接仓库摘要、分支列表、分支 revision、目录树、README 发现、文件正文、overview（Releases/Latest release/Contributors/Languages）与 OPEN Issue/PR 计数缓存路径。
- 明确区分在线最新验证（`Network`）与离线历史快照（`Snapshot`/`HistoricalSnapshot`）：历史快照展示 `Cached` 胶囊（悬停显示同步时间）与手动 `Refresh` 按钮，历史 Releases 不伪造 `Latest` 绿色认证徽章或完整数量胶囊，未缓存目录/文件诚实报 miss 而不编造空内容。
- 将 `RepositoryImageService::with_cache` 通过 `ReaderView::set_image_provider` 挂到 `RepositoryPage` 的真实 `ReaderView` 上（传入可信 `RepositoryDetails` 的数字仓库 ID 与私有属性），复用固定 commit 的解析映射与图片字节缓存，外链继续走无凭据 `ReqwestExternalImages`（详见 [README 图片](../reader-images.md)）。
- 离线历史快照下 Star 不发送状态/写请求、不乐观改数，在线 `Refresh` 后恢复权威读/写；Fork 保持禁用。
- 复核缓存门禁与账户上下文：换 PAT 隔离、凭据拒绝或 `revoke_repository` 立即清空页面已展示的摘要、树、正文、图片 provider 与 overview。

## 关键实现

- **`src/ui/repository/locator_index.rs`（新增）**：
  - `RepositoryPage` 以 `RepositoryLocator`（`owner/name`）打开，而 `CachedRepositorySession` 以数字仓库 ID（`RepositoryIdentity`）做键。新增账户聚合级（`repository = None, private = true`）的 `repository-locator-index` 辅助映射：在线成功解析仓库详情后记录 `locator -> RepositoryIdentity`，跨重启离线激活时先在后台 blocking worker 读取该映射，再调用 `CachedRepositorySession::repository`。
  - 该映射仅作定位提示，不绕过任何仓库门禁或 DTO 数字 ID 校验；账户聚合被隔离或因某仓库明确撤权失效时一并清除。
- **`src/ui/repository/mod.rs` & `view.rs` & `navigation.rs`**：
  - `activate` 捕获 `RepositorySession` 后查询 `RepoService::cache(session.context())` 构建 `CachedRepositorySession`。
  - 当摘要或 revision 返回 `RepositoryCacheOrigin::Snapshot` 时，`current_read_mode()` 自动切换为 `OfflineOnly`，使后续根目录、已展开子目录、README 发现与文件正文直接从本地持久缓存读取，未访问过的目录/文件立即返回诚实的 `PageError::from_cache(&RepositoryCacheError::Miss(CacheMiss::NotCached))`。
  - 顶栏新增 `cached_marker`：在 `is_showing_cached_snapshot()` 为真时渲染 `Cached` 描边胶囊（tooltip 标注同步时间）与 `Refresh` 控件；点击调用 `refresh_repository` 在线重验并在返回 `Network` 后移除历史标记。
  - `apply_content` 创建 `ReaderView` 时调用 `image_provider_for` 注入 `RepositoryImageService::with_cache`，并在 `deactivate` / 权限失效时显式卸下 provider。
  - `check_authorization` 与 `apply` 额外检查 `cache_allowed(repo_id)`，在仓库撤权或缓存分区失效时立即清空页面数据。
- **`src/ui/repository/extras.rs` & `src/ui/repository_overview/**`**：
  - `start_overview` 在有缓存绑定时构建 `CachedPageReadSession`；当页面处于离线快照态时直接以 `PageReadMode::Offline` 读取，在线态优先 `PageReadMode::Online` 并在网络/超时/限流/5xx 时回退 `PageReadMode::Offline`。
  - `SectionState` 新增 `Historical { value, sync_label, reason }`：Releases 与 Contributors 在历史快照下展示 `Cached` 胶囊与 scoped retry 图标，历史快照不渲染 `Latest` 绿色徽章和完整数量胶囊；Languages 底条在历史快照下于 tooltip 中标注 `cached`。

## 应用的教训 (LESSONS)

- **L002（模拟验证写清边界）**：离线测试使用真实 `RepositoryPage`、`AccountService::open_with_cache`、真实 `CacheStore` 与受控传输，不宣称已完成真实 Shell 原生窗口或真实 PAT 验收。
- **L010（fixture 支持反复操作）**：测试中的离线开关和图片分支通过包装原 `FixtureTransport` 按请求身份重复应答，未修改共享 fixture。
- **L012（组件与整页联合验证）**：在同一 `RepositoryPage` 实例内同时验证摘要、分支、树、README 正文、源码、README 图片、overview 与 Star 按钮的离线/刷新行为。
- **L014（可选内容与诚实状态）**：历史快照以紧凑 `Cached` 胶囊与 tooltip 表达，不堆砌长段落；未缓存目录/文件明确报告未缓存，不伪装成空目录。
- **授权判定不要靠轮询**：把授权状态做成“后台定时探测最近结果”的布尔量，会让撤权正确性依赖探测间隔——既是延迟窗口，也可能整个漏掉（撤权与重新授权落在同一窗口内）。撤权必须由被读取数据自己携带的失效令牌判定：读取前取令牌、回复带令牌、发布前校验令牌，失效后永不复原。这条同时满足 UI 线程零存储 I/O（令牌校验只是原子 load），见 `cache-runtime` 的 `CacheAuthorization`。

## 验证记录

所有 Cargo 命令均通过 `node scripts/verify.mjs -- cargo ...` 执行（共享 Windows mutex，独立 `target/`）。基线：rebase 到 `origin/main` `0cdb59c`（含 #67 文档校准与 #70 sidebar-cache）。

- 聚焦离线组合测试 `cargo test --locked --test repository offline`：全部通过（含 `locator_index` 单元测试及 7 项整页 GPUI 离线组合测试：跨重启离线浏览仓库/分支/目录/README/源码与未读目录 miss、手动 `Refresh` 恢复在线 provenance、固定 commit README 图片跨重启离线复用、换 PAT 隔离阻止旧缓存读取、extras 历史快照标记、离线 Star 禁用不发请求且在线刷新后恢复写操作、明确 `revoke_repository` 清空已展示页面并跨重启阻断）。
- `cargo test --locked --test repository`（864 项）与 `cargo test --locked --test repository_overview`（44 项）全部通过。
- `cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked` 均通过。
- 完整 `cargo test --locked` 通过：主程序 1136、home 96、notifications 579、profile 279、reader 250、reader_images 771、repository 864、repository_overview 44，0 failed。

期间观察到一次与本任务无关的既有偶发失败：默认并发下 `data::reader_images::tests::fixed_revision_images_persist_across_reopened_store_and_service` 重开 CacheStore 时得到 `InUse`（该用例用 50ms sleep 等异步写入，不显式关闭服务）。本任务未修改 `src/data/reader_images*` 或放宽断言；定向重跑、`--test reader_images` 与 `--test notifications --test-threads=1` 均通过，随后完整套件也通过。记录为既有测试的顺序/资源偶发，不归因本任务，也不掩盖原始失败日志。

## 剩余依赖与原生验收边界

- `cache-runtime`（应用启动时打开唯一 `CacheStore` 并通过 `AccountService::open_with_cache` 注入正式 Shell）目前仍在等待上游 `sidebar-cache`（PR #70），尚未合入 `main`。因此当前 `main` 的正式二进制启动仍走无缓存 `AccountService::open`，本页面在此状态下保持原有在线路径；待 `cache-runtime` 合入 `main` 并 rebase 后即可在正式 Shell 中端到端启用跨重启离线仓库浏览。
- PR 标记 `pending`，等待负责人进行真实 Windows 原生窗口验收。

## PR #73 第一轮审查修复（28436cd）

Codex 提出 3 项 P2，均成立且可直接修复；无待裁决事项，`merge_owner` 保持 worker。

1. **门禁探测占据 UI 线程**：原 `cache_allowed(repo_id)` 在 `check_authorization`（render 每帧 + receiver 每 30ms）里同步调用 `CacheStore::read`，即 UI 线程上取 store mutex 并执行 SQLite 查询，缓存发布/淘汰/恢复时可卡住滚动与输入。修复：`cache_allowed()` 改为纯内存判定（最近一次后台探测结果 + 绑定时 `scope` 语义），新增 `request_gate_probe`：最多每 2 秒调度一次、同一时刻只有一个在途，真正的 store 探测跑在 `BackgroundExecutor` 的 blocking worker，结果经新的 `Body::Gate { allowed }` 回到页面并复用同一条 context/generation 发布检查。
2. **Refresh 未作废旧代次回复**：`refresh_repository` 只取消 `work`（只停未来 I/O），既不推进 `generation` 也不推进 `branch_request`，已排队的旧 revision 回复仍能通过 `apply`/`apply_revision` 回填旧分支快照，并使后续读取落入 `OfflineOnly`。修复：刷新前推进 `generation` 与 `branch_request`、清空在途队列、重置 `details/branches/provenance`，并把 `gate_denied`/探测状态归位。
3. **历史计数冒充最新**：summary/branches/revision 在线成功而计数超时/限流时，旧 OPEN 计数被直接画进 tab badge，`counts_historical` 无人读取，`is_showing_cached_snapshot` 也不覆盖该情形。修复：历史计数只置 `counts_historical = true` 并隐藏 badge，不与刚验证的数值同形呈现（该处没有 per-count provenance 标记）。

回归：`manual_refresh_invalidates_queued_replies_from_the_previous_generation`（先注入旧代次排队 revision 回复再 `refresh_repository`，断言 generation/branch_request 前进且页面未被回填，随后刷新正常完成）；`cached_extras_report_a_historical_snapshot` 追加断言历史计数被隐藏且 `counts_historical` 为真。撤销修复后该刷新回归稳定失败（run `6fe4f051`），恢复后通过。

本轮验证（同一源码）：`cargo test --locked --test repository offline` 全通过（17 项过滤命中）、`--test repository` 865 通过、`--test repository_overview` 44 通过、`cargo test --locked` 主程序 1137 / home 96 / notifications 579 / profile 279 / reader 250 / reader_images 771 / repository 865 / overview 44 全部通过；`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked` 均通过。期间默认并发 `--test repository` 一次出现与本任务无关的既有 `reader_images::fixed_revision_images_persist_across_reopened_store_and_service` `InUse` 偶发失败（未修改该文件、未放宽断言），定向与随后完整重跑均通过，原始失败日志保留。

## PR #73 第二轮审查修复（c21f58e）

Codex 对 `c21f58e` 提出 1 项 P1，成立且可直接修复；仍无待裁决事项，`merge_owner` 保持 worker。

**激活/刷新与在途门禁探测竞争**：`activate`/`rebind`/`refresh_repository` 推进 `generation` 时没有重置 `gate_probe_in_flight`。上一轮 `Body::Gate` 回复会先在 `apply` 的 generation 检查处被丢弃，永远走不到 `apply_gate`，于是 `gate_probe_in_flight` 恒为真，后续不再发起探测；结果是新加载仓库内容在 `revoke_repository` 之后仍长期显示（`RepositorySession::verify` 只看服务上下文，不看 storage gate）。`gate_denied` 同样会残留，使激活后无法恢复。

修复：新增 `reset_gate_probe()`（推进 `gate_probe_request`、清 `gate_probe_in_flight`/`gate_probe_at`/`gate_denied`），在 `activate`、`rebind`、`refresh_repository` 调用；`Body::Gate` 携带该次探测的 `request`，`apply_gate` 只在 `request == gate_probe_request` 时落定，旧回复因此既不清除新探测的在途标志，也不发布过期仓库的判定。

回归：`activation_resets_gate_probe_bookkeeping_and_recovers_from_denial` 构造“在途 + 已拒绝 + 旧代次回复”状态后 `activate`，断言在途标志与 `gate_denied` 被清除、探测身份推进、旧回复无法清除新探测的在途标志，且新探测结果可正常落定。

本轮验证（同一源码）：`cargo test --locked --test repository offline` 全通过（含上述新回归）、`--test repository` 866 通过、`--test repository_overview` 44 通过、`cargo test --locked` 主程序 1138 / home 96 / notifications 579 / profile 279 / reader 250 / reader_images 771 / repository 866 / overview 44 全通过；`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked` 均通过。默认并发下 `--test notifications` 再次出现与本任务无关的既有 `reader_images::fixed_revision_images_persist_across_reopened_store_and_service` `InUse` 偶发失败，`--test-threads=1` 定向与随后完整重跑均通过；未修改该文件、未放宽断言，原始失败日志保留。

## PR #73 第三轮审查修复（a4e8433）

Codex 对 `a4e8433` 提出 1 项 P2，成立且可直接修复；仍无待裁决事项，`merge_owner` 保持 worker。

**混合来源的时间归属错误**：当仓库摘要在线成功、而分支枚举或 revision 回退到旧快照时，`is_showing_cached_snapshot()` 正确点亮 `Cached` 胶囊，但 `cached_marker` 用 `details_provenance().or_else(revision).or_else(branches)` 取时间，摘要的 `Network` provenance 会先命中，于是把可能数天前的分支内容描述成 "cached from just now"。

修复：`RepositoryPage` 新增 `historical_verified_at_ms()`，只在 `origin == Snapshot` 的摘要/分支/revision provenance 中取**最旧**的 `verified_at_ms`（无历史来源时为 `None`）；`is_showing_cached_snapshot()` 复用该判定，`cached_marker` 只用它生成同步时间文案。

回归：`cached_marker_reports_the_oldest_historical_source_not_a_fresh_summary` 构造 `Network` 摘要 + 两个不同时间的 `Snapshot` 分支/revision，断言胶囊出现、报告最旧时间且绝不等于摘要的当前时间。

本轮验证（同一源码）：`cargo test --locked --test repository offline` 全通过、`--test repository` 867 通过、`--test repository_overview` 44 通过、`cargo test --locked` 主程序 1139 / home 96 / notifications 579 / profile 279 / reader 250 / reader_images 771 / repository 867 / overview 44 全通过；`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked` 均通过。

## PR #73 第四轮审查修复：共享撤权令牌取代定时门禁探测（final base `abae289`）

Codex 对 `a391612` 提出 1 项 P1，成立且必须修复；仍无待裁决事项，`merge_owner` 保持 worker。

**撤权延迟窗口**：原实现把授权判定写成 `context.is_some() && !gate_denied`，`gate_denied` 只由 `request_gate_probe` 的后台探测填充，且两次探测之间至少间隔 2 秒、同一时刻仅一个在途。因此 `revoke_repository(scope, Some(id))` 在 AuthEpoch 不变时（明确仓库撤权不清账户代次）不会立刻反映到页面：私有内容会继续显示最长 2 秒；若撤权与重新验证都发生在同一个 2 秒窗口内，探测只看到“当前允许”，这个撤权会被完全漏掉。任何“缩短间隔”都只是缩小窗口，不能消除它。

**修复**：改用 `cache-runtime`（#68，已并入 main）提供的共享 `CacheAuthorization` 令牌，删除 `request_gate_probe`/`apply_gate`/`Body::Gate`/`GATE_PROBE_INTERVAL` 以及 `gate_denied`/`gate_probe_in_flight`/`gate_probe_at`/`gate_probe_request` 四个字段。

- 每次后台读取**开始前**用 `CacheStore::authorization(scope, repository)` 取一个令牌，随回复回到页面：`Message.authorization`、extras 的 `Reply.authorization` / `CountsReply.authorization`。
- `apply` 在写入任何字段前先做 `CacheAuthorization::is_current()`：撤权前完成、撤权后才被排空的回复直接被拒，不会把内容写回，也不会因随后的重新授权而复活（令牌一旦失效不会再次有效）。
- 页面按资源分槽保留已展示令牌 `displayed_authorizations: [Option<CacheAuthorization>; 4]`（摘要/分支/revision/内容），overview+counts 的令牌留在 `Extras::displayed_authorization`。`cache_allowed()` 只对它们做原子 load，`render` 与 30ms receiver 节拍依然零存储 I/O，也不再需要节流。
- 只有通过自身令牌校验的回复才能替换**它自己那个槽**的令牌，因此某资源的新读取不能替另一资源（可能已失效）的内容重新授权。
- `activate`/`rebind`/`deactivate`/`refresh_repository`/授权丢失路径都 `reset_authorization()`，丢弃旧代次令牌，避免新代次把旧代次的撤权掩盖掉。

**确定性回归**（均为真实 `RepositoryPage` + `AccountService::open_with_cache` + 临时 `CacheStore`）：

- `repository_revocation_clears_the_displayed_page_in_one_drain`：在线展示摘要+overview 后 `revoke_repository(&scope, Some(42))`，仅一次 `drain`（不 sleep、不 settle）即断言页面失活、`details`/`branch`/`reader`/overview 全清、错误就位、`context` 为 `None`。该测试在交付 head 上稳定失败（run `653e5a3f`，panic `the revoked page stays active in the same drain`）；把 `cache_allowed()` 临时改回恒真后再次失败（run `d2603345`），恢复修复后通过（run `af02e39b`、`3616ac00`、`d3908bdf`）。
- `reply_captured_before_revocation_is_rejected_afterwards_and_never_revived`：捕获撤权前的真实令牌，把“已完成但仍在队列”的 revision 回复推入 inbox，撤权后单次 `drain` 必须拒绝它且页面保持清空；随后 `begin_revalidation`/`permission_confirmed` 重新授权，断言旧令牌**仍不** current（旧回复永不复活），而重新捕获的新令牌 current（真实重载可以继续）。
- `activation_and_refresh_reset_retained_authorizations`：展示态确有保留令牌且 `cache_allowed()` 为真；`refresh_repository` 与 `activate` 后所有槽归空，再稳定回到可显示态。
- 删除原先描述节流探测内部状态的 `activation_resets_gate_probe_bookkeeping_and_recovers_from_denial`（对应实现已不存在）。

**基底变化**：本轮开始时 `feature/cache-runtime` 尚未合并，曾以 cherry-pick 方式引入令牌核心；#68 合并后该提交在 rebase 中按要求丢弃，`src/storage/` 与 main 完全一致（`CacheStore::authorization`、`CacheAuthorization`、`start_fresh_aggregate` 均来自 main）。随后 #72 Home、#69 Profile 合并，最终 rebase 到 `abae289`，本 PR 保持仓库范围独占。

**文档状态修正**（本 PR 一并负责）：`docs/README.md`、`docs/roadmap.md`、`docs/cache-strategy.md`、`docs/architecture.md` 中“正式 Shell 尚未打开应用级 `CacheStore`／消费者未接线／全局页面未挂载”的过时段落改为按事实分层：已合并的应用级 `CacheStore` 组装与 Home/Profile/Repository 消费、已挂载的 Notifications 与全局 Issues/PR、仍未挂载的 `SettingsView` 最终组件、仍属建议值的容量与验证间隔、以及仍未完成的真实 Windows 原生/dogfooding 验收。`docs/repository-browser.md` 的授权段落同步改写为 carried-token 机制。

本轮验证（同一源码，最终 head）：见下方"最终验证"小节，命令均为 `node scripts/verify.mjs -- cargo ...`，exit 0 与 run 日志为准。

### 最终验证（final head on `abae289`）

全部通过 `node scripts/verify.mjs -- cargo ...` 执行，以 runner 的 exit code 与 run 日志为准（不使用结尾管道判断 Cargo 成功）：

| 检查 | runner 结果 | run id |
| --- | --- | --- |
| `cargo check --locked --all-targets` | SUCCESS exit 0 | `771358e1` |
| `cargo fmt --all -- --check` | SUCCESS exit 0 | `5a96d404` |
| `cargo clippy --locked --all-targets -- -D warnings` | SUCCESS exit 0 | `69796b85` |
| `cargo test --locked --test repository offline` | SUCCESS exit 0，22 passed / 853 filtered | `9c2e902c` |
| `cargo test --locked --test repository` | SUCCESS exit 0，875 passed | `5a37c92e` |
| `cargo test --locked --test repository_overview` | SUCCESS exit 0，44 passed | `6e750fbe` |
| `cargo test --locked`（全量） | SUCCESS exit 0 | `49368563` |

全量分项：main 1224、home 98、notifications 589、profile 284、reader 250、reader_images 777、repository 875、repository_overview 44，合计 4141 通过，0 失败。

失败-通过证据：撤权回归在交付 head 上失败（run `653e5a3f`）；把 `cache_allowed()` 临时改回恒真后再次失败（run `d2603345`）；恢复 carried-token 实现后通过（`af02e39b`、`3616ac00`、`d3908bdf`、`9c2e902c`）。

原生边界不变：`pending` 仍需真实 Shell 窗口下的人工验收（冷启动离线打开已读仓库、断网切换目录、撤权后页面即时清空、固定 commit 图片复用）。
