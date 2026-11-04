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
   - 历史快照保留其原始保存时间戳（`fetched_at_ms`），并在栏目标题旁显示紧凑的 `Saved <time>` 提示与 tooltip 说明，绝不把历史快照当成当前网络新鲜结果。
3. **分页同源与代次隔离**：
   - Feed 追加保持单代次同源约束：历史快照集只续读历史追加页，网络快照集只续读网络追加页，绝不拼接跨代次、新旧混合的分页。
   - 当历史快照的后续页缺失（`PageValue::Miss`）时，诚实呈现 `Page N is not available offline`，并终止该快照集的自动续读，绝不将其误报为“事件时间线已到末尾”，也不用无限重试循环骚扰用户。
   - 刷新时重置去重集合（`feed_seen_ids`）、游标与 epoch，成功后完全替换整栏并清除历史保存标记。
4. **星标仓库描述富化与上下文防护**：
   - 描述富化随 Feed 来源匹配（网络 Feed 查网络或当前缓存，历史 Feed 读历史描述快照）。
   - 描述响应携带请求代次与分区标识，迟到或跨代次响应安全丢弃。
   - 撤权或账户移除时，已发布的内存视图在收到屏障时立即清空，不遗留已撤权的私有数据展示。

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
  - 增加确定性组合用例：
    - `online_cache_reopens_and_offline_reads_the_same_home_feed_and_older_page`：预热后跨重启冷启动，断网回放 Home/Feed 首屏与多页追加，保留 `Saved` 标记与星标描述。
    - `offline_miss_partial_and_refresh_replacement_are_distinct`：第 3 页未缓存时命中 `NotCached { page: 3 }`；刷新后网络结果完全替换并清除历史标记。
    - `authentication_failure_cancellation_and_uncached_pages_are_not_historical_success`：401 撤权与未缓存项绝不显示为历史成功；取消请求不产生快照。
    - `no_cache_binding_keeps_the_original_online_path`：无缓存绑定的常规在线服务不发生功能回归。
- `docs/home-view.md`：
  - 记录 2026-11-03 离线页面缓存、历史快照呈现、分页同源与无缓存回退契约。

## 验证记录

所有 Cargo 检查与测试均经由 `node scripts/verify.mjs` 串行执行，确保遵循平台互斥锁：
1. `node scripts/verify.mjs -- cargo fmt --all -- --check`：通过（exit 0）。
2. `node scripts/verify.mjs -- cargo check --locked --all-targets`：通过（exit 0）。
3. `node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings`：通过（exit 0）。
4. `node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::home::tests -- --test-threads=1`：4 项通过（exit 0）。
5. `node scripts/verify.mjs -- cargo test --locked --test home`：84 项通过（exit 0）。
6. `node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::tests -- --test-threads=1`：6 项通过（exit 0）。
7. `git diff --check`：无空白或行尾违例。

## 验证边界与限制

- 真实原生 Shell 窗口下的端到端离线查看仍依赖 `cache-runtime` 统一注入 `CacheStore`。在本任务的隔离分支上，通过真实 `AccountService::open_with_cache`、真实 SQLite 账户目录、真实 `CacheStore`、真实 `CachedPageReadSession` 和真实 `HomePage` 宿主完成组合断言。
- 自动化测试运行在 GPUI 测试平台，不覆盖真实的 Windows 原生窗口绘制、DPI 切换手感或真实 GitHub 网络抖动。
- 本 PR 添加 `pending` 标签，等待协调者或负责人进行原生界面验收后方可合并。
