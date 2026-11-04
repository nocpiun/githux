# Home/Feed 离线页面缓存与历史快照接线

- 2026-11-03 至 2026-11-04，任务：`home-offline`，执行者：基层实现 Agent。
- 目标：在正式 Home/Feed 页面宿主（`HomePage`）中消费 `CachedPageReadSession`，让已缓存的 Home 条目、Feed 首屏与后续追加页在允许的离线场景下可读，保留历史保存时间提示，在线刷新后准确替换集合；当无缓存绑定时平滑回退至原有在线行为。
- PR 目标分支：`main`，Milestone：#1（First Release），标签：`pending`（等待负责人手动验收）。

## 决策与设计边界

1. **缓存构造与平滑回退**：
   - 从既有账户会话与服务 `RepoService::cache(session.context())` 提取应用级 `CacheStore` 和当前账户的 `CacheScope`。
   - 构造 `CachedPageReadSession`；当服务未启用缓存、未注入 store 或 scope 无法匹配时，保留原有的无缓存在线读取能力，不 panic、不中断在线浏览。正式生产环境的 CacheStore 注入由 `cache-runtime` 负责。
2. **离线回退与真实身份**：
   - 仅在网络错误或请求超时且本地有合规快照时，安全回退到历史快照。
   - 401 凭据拒绝、明确撤权、请求取消（`Cancelled`）或过期上下文（`StaleContext`）绝不回退为历史成功；不把 401 伪造成断网成功。
   - 历史快照保留其原始保存时间戳（`fetched_at_ms`），并在栏目标题旁显示紧凑 `Saved` 图标与相对时间 tooltip；离线追加旧页时保留该历史标记，只有在线刷新替换集合后才清除。
   - 离线且本地无快照的首屏保持 Unavailable（“This page is not available offline.”），不渲染为错误图标或空结果。
3. **分页同源与代次隔离**：
   - Feed 追加保持单代次同源约束：历史快照集只续读历史追加页，网络快照集只续读网络追加页，绝不拼接跨代次、新旧混合的分页。
   - 当历史快照的后续页缺失（`PageValue::Miss`）时，诚实呈现 `Page N is not available offline`，并终止该快照集的自动续读，绝不将其误报为“事件时间线已到末尾”，也不用无限重试循环骚扰用户。
   - 刷新时重置去重集合（`feed_seen_ids`）、游标与 epoch，成功后完全替换整栏并清除历史保存标记。
4. **星标仓库描述富化与上下文防护**：
   - 描述富化随 Feed 来源匹配；同一 Feed 集合内的多次描述查找按独立批次并行保留，追加新页不取消上一页仍在途的描述请求，刷新/重绑取消旧批次并按 `feed_append_epoch` 拒绝迟到结果。
   - 账户切换、移除或替换 PAT 推进会话上下文后，`rebind` 与前台检查清空已渲染副本并回到 Unavailable。不推进账户 epoch 的单仓库明确撤权已由 `CachedPageReadSession` 阻断在途/后续聚合读写，但共享 `CacheStore`/`RepoService` 当前没有页面订阅或只读 aggregate revision 接口，已渲染副本需在下次读取或共享通知接口补齐后主动撤下。

## 改动清单

- `src/app/pages/home.rs`：
  - 接入 `CachedPageReadSession`，在 `start`、`load_more_feed` 和 `enrich_starred_repositories` 中根据网络/离线模式调度读取与回退。
  - 增加 `ReadSource`、`AppendError` 及带分区/代次的响应校验。
  - 在 `apply` 和 `apply_append` 中复核缓存权限；当授权失效或撤权时，清空会话并重置为中性 Unavailable 状态。
  - 为自动化测试提供 `is_ready_for_test`（精确排除刷新中状态）与受控追加入口。
- `src/ui/home/mod.rs`：
  - 新增 `PageProvenance` 与 `SnapshotPresentation`，支持传递和展示历史保存时间。
  - 面板 Header 旁增设 `Saved <relative>` 紧凑历史标识与说明 tooltip。
  - 针对离线缺页新增 `FeedAppendState::NotCached { page }`，展示中性离线不可用提示。
  - 分离网络直接发布与快照发布内部 helper，确保网络结果不会误带历史来源标记。
- `src/app/pages/home/tests.rs`：
  - 增加 6 项确定性组合用例：
    - `online_cache_reopens_and_offline_reads_the_same_home_feed_and_older_page`：预热后跨重启冷启动，断网回放 Home/Feed 首屏与多页追加，追加后继续保留 `Saved` 历史标记与星标描述。
    - `offline_miss_partial_and_refresh_replacement_are_distinct`：第 3 页未缓存时命中 `NotCached { page: 3 }`；刷新后网络结果完全替换并清除历史标记。
    - `authentication_failure_cancellation_and_uncached_pages_are_not_historical_success`：401 撤权、未缓存离线首屏、真实在途请求取消与移除账户清空已渲染副本。
    - `append_keeps_a_delayed_first_page_description_and_refresh_rejects_old_batches`：首屏描述暂停时追加第二页不丢失首屏描述，刷新拒绝旧 epoch 描述响应。
    - `offline_filtered_empty_first_page_continues_to_merged_pr_and_read_more_routes_internally`：离线全噪声空首屏自动续读到合并 PR 页，点击 `Read more` 走内部 `Route::PullRequest`。
    - `no_cache_binding_keeps_the_original_online_path`：无缓存绑定的常规在线服务不发生功能回归。
- `docs/home-view.md`：
  - 记录 2026-11-03 离线页面缓存、历史快照呈现、分页同源与无缓存回退契约。

## 2026-11-04 F1/F2 返工与权限代次票据

负责人要求继续原 PR #72，修复主 Agent 组合验收反例。复用其 `D:/githux-cache-home-acceptance/src/app/pages/home/tests.rs` 最后四条验收用例，保留原断言，不改缓存生产者接口。

### F1：缓存拒绝不再关闭有效在线会话

- 原实现把 `CacheError::AccessDenied`/`Stale` 映射为 `GitHubError::StaleContext`，导致同一身份新 PAT 已验证成功时 Home/Feed 被关闭。
- 新增 `CachedReadFailure::{Network, AuthOrContext, UncachedOnline}` 分流：网络/超时回退历史；缓存 quarantine、lease、I/O 故障回退普通在线读取；真实 401、取消、stale 上下文保持原义。
- 401 清空旧页面副本但不把当前 `PageReadSession` 误标 stale；无缓存对照与真实在线读取保持成功。
- `acceptance_valid_replacement_pat_keeps_home_online`、`acceptance_valid_replacement_pat_keeps_feed_online`、`operational_cache_failure_degrades_to_online_without_unlocking_old_snapshots`、`replacement_pat_rejection_never_replays_the_quarantined_snapshot` 均通过。

### F2：撤权后已显示副本与迟到响应失效

- 采用 cache-runtime 独立核心提交 `9f87f47` 的 `CacheAuthorization`：`CacheStore::authorization(scope, None)` 捕获账户聚合票据，内存原子读取，不授予权限、不访问磁盘。
- `HomePage` 在绑定/读取前捕获票据，Home/Feed 首屏、分页、描述回复都携带同一票据；后台入队前与 UI apply 前复核，旧票据永久失效后停止并清空 Home/Feed/描述副本。
- 原 `acceptance_confirmed_revocation_clears_already_rendered_home` 通过，扩展覆盖撤权后迟到 append/enrichment 不复活旧内容。
- `acceptance_cold_start_without_network_revalidation_replays_saved_home` 通过：真实关闭并重开服务/缓存、PendingVerification、无网络 revalidate 仍可回放。
- 核心接口仍由 cache-runtime 唯一负责；本任务没有改 storage/data 生产者，后续按明确提交/协调基线完成正式组合。

### 验证

`node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::home::tests -- --test-threads=1`：12/12 通过（run `b5b15498-3a06-4d7a-8d3e-a6fed99c5099`）。
`node scripts/verify.mjs -- cargo fmt --all`：通过（run `b19c1194-b873-46b6-bcee-aa4ba9cb073d`）。

Codex 本轮另指出 `stop()` 必须清除保留的权限票据，避免撤权后反复重绘；已补充 `cache_authorization = None` 与断言，12/12 测试通过（run `938e2569-27fb-4737-bdb6-57b157e37495`），clippy 通过（run `28ab071f-ad2d-4dc0-ba4a-a48a72f265a8`）。共享核心测试的 Clippy 修正同步自 cache-runtime 76815c0 的已修内容，未搬入其他页面文件。

原生窗口与真实 GitHub PAT 仍未验证，保留 `pending`。


所有 Cargo 检查与测试均经由 `node scripts/verify.mjs` 串行执行，确保遵循平台互斥锁：
1. `node scripts/verify.mjs -- cargo fmt --all -- --check`：通过（exit 0）。
2. `node scripts/verify.mjs -- cargo check --locked --all-targets`：通过（exit 0）。
3. `node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings`：通过（exit 0）。
4. `node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::home::tests -- --test-threads=1`：6 项通过（exit 0）。
5. `node scripts/verify.mjs -- cargo test --locked --test home`：84 项通过（exit 0）。
6. `node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::tests -- --test-threads=1`：6 项通过（exit 0）。
7. `git diff --check`：无空白或行尾违例。

## 验证边界与限制

- 正式生产启动注入 `CacheStore` 仍依赖 `cache-runtime`（PR #68，当前未合入 `main`；其前置 `sidebar-cache` PR #70 已合入 `main`）。在本任务分支上，通过真实 `AccountService::open_with_cache`、真实 SQLite 账户目录、真实 `CacheStore`、真实 `CachedPageReadSession` 和真实 `HomePage` 宿主完成受控组合断言，尚未做 `cache-runtime` 合入后的正式二进制冷启动组合验证。
- 若未来要求单仓库明确撤权在不推进账户 `AuthEpoch`、也不触发新一轮读取的情况下立即撤下已渲染的 Home/Feed 副本，需协调共享层（如 `RepoService` 暴露当前账户聚合 gate 状态或失效序号）；本任务按文件所有权未修改 `src/data/**` 或 `src/storage/**`。
- 自动化测试运行在 GPUI 测试平台，不覆盖真实 Windows 原生窗口绘制、DPI/输入法交互或真实 GitHub PAT/API 响应。
- 候选教训（供 `docs-current-state` 评估，不改 `LESSONS.md`）：分页追加共用 `apply_*_page` 时不要复用“首屏网络替换清空 provenance / 取消前批异步丰富”的状态重置路径；缓存门禁拒绝不等于当前账户认证失败，需保留普通在线读取；已显示副本必须跟随权限代次票据失效，存储 lease 失效不等于 UI 副本已清除。测试中验证缓存跨重启恢复时须共享模拟凭据存储但先关闭旧 `CacheStore` 文件锁。
- 本 PR 保持 `pending` 标签，等待 `cache-runtime` 合入后的组合核验及负责人手动 UI 验收。

## 正式 Shell 验收步骤（待 `cache-runtime` 合入后执行）

1. 在线登录有效 Classic PAT，打开默认 Home 标签，确认左栏仅展示当前账户 `involves` 的 open Issue/PR，右栏仅展示已关注账号的合并 PR（作者归因 + 摘要/Read more）、星标仓库（含可选描述）与新建仓库，滚到底部自动加载更旧一页。
2. 关闭应用并断开网络后重新启动，确认已访问的 Home 与 Feed 页从本地缓存恢复，栏标题旁显示 `Saved` 提示及保存时间 tooltip；继续滚到未缓存的更旧页时显示 `Page N is not available offline` 且不误称没有更多活动；合并 PR 的 `Read more` 仍打开内部 PR 标签。
3. 恢复网络后点击两栏刷新按钮，确认新集合替换旧快照、清除 `Saved` 标记，且不把旧分页拼入新结果。
4. 切换账户、替换 PAT 或移除账户后，确认原账户已展示的 Home/Feed 内容立即清空，不会在离线或未重验状态下继续显示。
