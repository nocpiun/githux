# 开发日志：cache-first 首次内容呈现

- 日期：2026-11-11；任务 ID：`cache-first-paint`；分支：`fix/cache-first-paint`；base：`f9e31a0`。
- 目标：已有可读缓存时，先把快照内容呈现给用户，再在后台完成网络重验并替换；不改动既有缓存存储/授权架构，不新建离线产品，不改读者图片/SVG 管线。

## 起点核查

阅读了 `AGENTS.md`、`docs/README.md`、`docs/code-standards.md`、`LESSONS.md`（L018–L022）与缓存/页面主题文档，并逐处核对实际代码。真实起点是：所有正式页面都是“先网络、失败后才回放快照”。

- Home/Feed：`src/app/pages/home.rs::read_home` / `read_feed` 先 `PageReadMode::Online`，仅当网络/超时失败才 `Offline`。
- Profile：`read_profile_basic/pinned/contributions/readme` 同样是 Online 优先、允许的瞬时失败才回放。
- 侧栏：`src/app/accounts/mod.rs::read_repositories` 通过 `list_repos_cached(Online)` 在线读取，仅允许的瞬时故障回放历史列表。
- Repository：`src/ui/repository/**` 以 `RepositoryCacheMode::OnlinePreferred` 顺序推进 摘要→分支→revision→根树/README→正文；固定对象（tree/blob）已能直接命中缓存，但可变的 refresh/summary/ref 仍要等网络返回或失败。`locator_index` 提供无需网络的数字仓库 ID 提示。

因此本次不是“缩短超时”，而是把每个独立 section 的呈现顺序改成 **缓存先行、网络随后替换**。

## 实现

### 1. 共享的呈现顺序规则

每处读取拆成两个并发作业（同一 `Work` 取消令牌、同一 generation/request 身份）：

- **快照半边**：只做 `PageReadMode::Offline`（纯本地）读取，命中就以 `HistoricalSnapshot` 来源/时间戳发布一次。
- **网络半边**：保持原有在线读取与既有的“允许的瞬时失败回放快照”逻辑不变。

前台（UI/前台 drain）负责最终顺序，规则写在各页文的 `PublishMark`/`preliminary` 判定里：

1. 快照回复只在“本 generation 尚未发布任何网络值、且未发布过同一快照”时生效；
2. 网络回复永远覆盖快照（含网络先到、缓存迟到的竞态，缓存被丢弃）；
3. 同一 `(partition, fetched_at_ms)` 的历史快照不重复发布（重复发布只会重置滚动位置与选择）；
4. 快照回复不释放 work 槽、不恢复刷新可用性、不触发 Feed 轮询冷却——这些仍由在线读取收尾；
5. 缓存未命中/损坏/权限拒绝/存储故障一律不发布任何内容，保留原有 online loading。

`PublishMark`（Home）与 `preliminary: bool`（Home/Profile/侧栏/Repository Message）是显式的顺序身份，而不是依赖任务完成顺序。

### 2. Home / Feed（`src/app/pages/home.rs`）

- 新增 `read_cached_first`（Offline-only，返回 `Option<ColumnResult>`），与 `read_home`/`read_feed` 在同一任务内 `tokio::join!` 并发执行；网络半边先开始，缓存半边不阻塞它。
- `Reply.preliminary` 区分两阶段；`PublishMark` 记录 Empty/Historical/Network。
- Feed 快照发布时同样设置 `feed_source`/`feed_next_page`/`feed_seen_ids`，因此快照期间的分页仍属于同一个 `CollectionGeneration`；网络首页到达后整体替换集合（append epoch 已由 `start` 推进），不会混排旧页。
- 网络失败且快照已在屏上时保留快照（真实 offline 回放路径不变，仍由 `read_*` 的既有 fallback 提供）。

### 3. Profile（`src/app/pages/profile.rs`）

- 新增 `read_cached_first_section`（`SnapshotRequest` 参数聚合），按 Basic / Repositories / Contributions / Readme 各自离线读取；日历先试当前 rolling year，再试记录的区间。
- `Reply.preliminary` + `accept_preliminary`：只在 section 仍为空时接受快照；`SectionState::Loading` 才允许被填充。
- Basic 快照可创建 `ProfileView` 并立即启动其余三个 section 的读取（各自缓存先行），因此 pinned/calendar/README 不再等待网络 Basic。
- 网络 Basic 到达后**原地更新**同一 view（同 target/binding），不再重建 view、不再重置兄弟 section 的滚动/README reader；只有 login 变化才发 canonical 导航。
- 保留原有 `CacheAuthorization` 票据、monitor、`retire_displayed_sections` 语义：快照与网络回复都带同一票据，发布前与显示期间仍然复核。

### 4. 侧栏仓库列表（`src/app/accounts/mod.rs`）

- `Operation::Repositories` 额外启动一个 preliminary 作业：`read_cached_repositories`（`list_repos_cached(Offline)`）；命中则以 `RepositoryListOrigin::Historical` 发布。
- `Reply.preliminary` + `apply_preliminary`：仅在当前 preview 仍为 `Loading` 时发布，不结束 operation、不解锁输入、不改 `loaded` 标记；最终回复照旧覆盖。
- 历史行提示沿用既有 `historical_status()`（“Offline snapshot · last synced …” + scoped Retry），未新增文案类型。
- `cancel()` 等既有路径不变。

### 5. Repository 入口（`src/ui/repository/mod.rs`、`extras.rs`）

- 新增 `Job::Snapshot`：一次后台作业用 `OfflineOnly` 读完 locator index → 摘要 → 分支 → ref revision → 根树 → README discovery → 初始正文，全部来自同一持久分区与同一 fixed commit，作为**单个** `Body::Snapshot` 发布（header/tree/README 不会指向不同 commit）。
- `apply_snapshot` 仅填充仍为空的页面（`self.details.is_none()`），设置 `Snapshot` provenance（既有 `Cached` 胶囊/`Refresh`），并调用既有 `start_overview(identity, preliminary=true)`；extras 用同一快照读，缓存未命中的 section 保持 Loading 而不是报“未缓存”（`unanswered(preliminary)`）。
- 在线链完全不变并在同一 generation 内继续：网络摘要到达 → 分支（此时以“网络已证明可达”为准用 `OnlinePreferred`）→ 若屏上 revision 仍是快照，则用同一 `branch_request` 重新在线解析 ref；同 commit 时保留已加载的 tree/README/reader（按 commit 寻址的内容不变，也保住 reader 滚动位置），commit 变化时才整体替换。
- `start_overview` 对同一 repository/generation 复用已有 overview，只重新发起读取；已显示的历史 extras 在重新读取期间不再被重置为 Loading。
- extras 仍保留既有 `Historical` 段位、`Cached` 胶囊与隐藏历史 counts 的语义。

### 6. 本地回放不占网络并发配额（真实缺陷修复）

联调时发现：页面缓存回放 `PageReadMode::Offline` 仍走 `RepositorySession::read_authenticated`，因此会排队等待 `BackgroundExecutor` 的**有界网络 permit**（生产与测试都是 4）。当 Home/Feed/Profile 的全部网络读取都挂起时，缓存半边拿不到 permit，首次内容反而要等网络空出槽位——正是本任务要消除的等待。

修复（小接口、不放松任何检查）：

- `RepositorySession` 增加 `local: bool`；`RepoService::read_authenticated_local` 与 `read_authenticated` 共用实现，`local` 只跳过 permit。
- `PageReadSession::with_local_page_client`；`page_cache::read` 的 Offline 分支使用它。
- `repository_list_cache::list_repos_cached` 的 Offline 分支使用 `read_authenticated_local`。
- 凭据加载、账户 epoch/context 复核、取消检查、持久 gate/lease 全部保留；只是不再占用网络并发槽。

回归：`offline_replay_does_not_wait_for_a_network_permit`（持有唯一 permit 时离线回放仍需在 5s 内完成）。

## 验证

全部 Cargo 检查经 `node scripts/verify.mjs -- cargo …`（共享 Windows mutex，独立 `target/`）。

- 真实生产 host 路径上的确定性 gate 测试（release 网络前断言缓存内容已在屏，release 后断言替换）：
  - `cached_home_and_feed_paint_before_the_network_read_finishes_then_are_replaced`（Home+Feed，重启进程存储、传输层挂起）。
  - `cached_sections_paint_before_the_network_reads_finish_and_are_then_replaced`（Profile 四 section 历史标记出现再消失）。
  - `a_cached_repository_list_paints_before_the_online_read_finishes_then_is_replaced`（侧栏历史列表 → 网络列表）。
  - `cached_repository_paints_before_the_online_chain_finishes_then_is_replaced`（Repository 摘要/tree/README 先出现，随后 `Cached` 标记消失且 commit 一致）。
- 竞态 / 回滚：`a_late_cached_reply_never_rolls_back_a_newer_network_value`、`a_late_profile_snapshot_reply_never_rolls_back_a_newer_network_section`、`a_late_cached_repository_reply_never_rolls_back_the_network_page`。
- 失败与缺失：`network_failure_after_the_cached_paint_keeps_the_readable_snapshot`、`a_cache_miss_keeps_the_ordinary_online_loading_path`、`a_corrupt_snapshot_does_not_fabricate_an_empty_page`、`a_profile_cache_miss_keeps_online_loading_and_then_shows_the_network_result`。
- 并发配额：`offline_replay_does_not_wait_for_a_network_permit`。
- 既有 Home/Feed、Profile、Repository、accounts 套件全部保留并通过；`authentication_failure_cancellation_and_uncached_pages_are_not_historical_success` 的固定迭代等待改为 deadline 等待（断言不变，仅消除负载下的时序竞态）。

最终命令与结果：见交付评论/PR（`cargo fmt --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked`）。

## PR #85 第一轮审查修复

Codex 对 `9665f19` 提出 1 项 P1 与 2 项 P2，均成立且可直接修复；无待裁决事项，`merge_owner` 保持 coordinator（本任务按派发不自行合并）。

1. **P1 后续网络刷新被误去重**：`publish_mark` 把所有网络结果都映射为 `PublishMark::Network`，因此第二次成功的 Home 刷新与屏上标记相等，被 `same_as_shown` 跳过、不再 `apply_home`，新条目要到页面重绑才出现。修复：去重只针对**历史快照**（`PublishMark::Historical` + 相同 `fetched_at_ms`），网络结果一律照常应用。回归 `a_second_network_refresh_replaces_the_first_network_result`（撤销修复后失败，恢复后通过）。
2. **P2 同 commit 重验漏补快照缺失资源**：快照 bundle 可能只有 revision 而没有根树/README 发现/正文，此时 `apply_revision` 的“同 commit 保留”捷径直接返回，缺失部分永远停在 loading。修复：捷径中按缺失项补发 `Job::Root`/`Job::Readme`（`OnlinePreferred`）。回归 `a_same_commit_revalidation_fetches_the_resources_the_snapshot_lacked`（禁用补发后失败）。
3. **P2 保留 reader 的回调身份被重验打断**：缓存 README 的 reader 回调捕获创建时的 `branch_request`/`selection_request`；重新解析该分支时自增 `branch_request` 会让保留下来的 reader 所有链接动作被静默丢弃。修复：重验沿用原 `branch_request`（`begin_branch` 等真正的换分支仍会推进并淘汰旧回复）。回归 `the_retained_cached_reader_keeps_its_page_identity_after_revalidation`。

本轮验证（同一源码）：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（主程序 1249、其余目标全部）通过。

## PR #85 第二轮审查修复

Codex 对 `4550f17` 提出 2 项 P2，均成立且可直接修复；仍无待裁决事项。

1. **P2 缓存快照改写 login 会作废同一次在线 Basic 回复**：非规范拼写的路由（例如 `/Alice`）打开时，缓存 Basic 带的是上次保存的 canonical login；preliminary 回复若把 `self.login` 改成 canonical，在线半边的回复仍带请求时的 login，`accepts()` 会拒绝它且不释放 work 槽，于是 Basic 一直停在历史态、Retry 也失效。修复：快照回复不改写 `self.login`，也不触发 canonical 导航；只有在线结果可以重命名路由。回归 `a_cached_alias_login_does_not_strand_the_online_basic_read`（撤销修复后失败，恢复后通过）。
2. **P2 保存的嵌套文件选择被快照改成 README**：`file_in_root` 只接受根目录文件，保存 `src/main.rs` 时 bundle 退回 README 并覆盖页面已恢复的选择，后续重验也不会按 `intent.path` 重建。修复：bundle 现在按 `RepositoryNavigation::ancestors` 逐级离线读取祖先目录（新增 `SnapshotBundle.children`，应用到 `TreeModel` 并展开祖先），能解析时直接用该文件内容；解析失败时保留 `Selection::File(path)` 让在线链继续解析，而不是静默改成 README。回归 `a_saved_nested_file_reopens_from_the_cache_first_bundle`。

本轮验证（同一源码）：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked` 全部通过。

## PR #85 第三轮审查修复

Codex 对 `6e69090` 提出 1 项 P2（上一轮修复暴露的相邻问题），成立且可直接修复；仍无待裁决事项。

**P2 canonical login 改名会作废已在途的兄弟 section 回复**：缓存 Basic 先用原始拼写启动 Repositories/Contributions/Readme；若在线 Basic 先返回并把 `self.login` 改成 canonical，兄弟回复（仍带原始拼写）会被 `accepts()` 拒绝且不释放 work 槽，section 停在历史/加载态且 retry 无法开始。修复：`ProfilePage` 保留打开时的拼写 `opened_login`；`accepts()` 接受当前 canonical 或打开时拼写（两者由同一路由解析到同一 profile，target 数字 ID 未变）。回归 `a_cached_alias_login_does_not_strand_the_online_basic_read`（扩展到四个 section 全部转在线且 work 槽清空；撤销修复后失败，恢复后通过）。

同时把三个测试夹具的“hold/release”从 `Notify::notify_waiters` 改为 `watch<bool>`：请求可能先读到 hold 标志、在 release 之后才注册等待者，`notify_waiters` 会漏掉它；`watch` 的当前值不会漏。

## PR #85 第四轮审查修复

Codex 对 `49a57ef` 提出 1 项 P2，成立且可直接修复；仍无待裁决事项。

**P2 默认分支迁移后仍解析缓存的旧默认**：无保存分支时，bundle 选了快照里的 default，并把它写进了 `intent.branch`；在线摘要若报告了新的默认分支，重验只会重新解析旧分支（该分支被删还会直接失败）。修复：`apply_snapshot` 不再把快照的隐式选择写入导航意图；重验时对最新 `details.default_branch` 重新执行 `branch_choice`——同分支时按原身份重解析（保留 tree/reader），不同则 `begin_branch` 重新解析新默认。回归 `a_moved_default_branch_replaces_the_cached_default`（撤销修复后失败，恢复后通过）。

本轮验证（同一源码）：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked` 全部通过。

## 顺序/边界说明

- 未新增毫秒级性能指标；验证依据是“网络未释放时已渲染缓存内容”和“释放后替换”的可观察顺序，而非合成计时。
- 未使用真实 PAT / 真实 GitHub；未做 Windows 原生窗口验收（属负责人）。
- 明确未覆盖：Notifications、全局 Issues/PR 列表没有可复用的持久快照（Notifications 有独立数据层但未有页面缓存适配器），本次不做新持久缓存产品。
- 已知限制：
  - Repository 快照只还原根目录树与初始文档；保存的展开子目录由在线链在后续读取时补齐（快照期间展开未缓存目录会按 `OfflineOnly` 报未缓存，刷新/在线链完成即可恢复）。
  - Home/Feed 的缓存首帧标记为 `Saved`（历史快照），刷新控件在在线读取完成前保持不可用；未新增“快照 + 进行中 spinner”专用呈现 API（`src/ui/home/mod.rs` 由 ui-scroll-geometry 负责，未改动）。
  - Profile 的 Readme 离线快照需要页面已创建 view，因此它随 Basic 快照一起出现，而不是先于 Basic。
- 候选 LESSONS（本任务未写共享 LESSONS，按派发要求记录备用）：
  - **本地回放不得占用网络并发配额**：把“读缓存”接到既有在线读取路径时，要检查该路径是否顺带排队等网络 permit/信号量；否则缓存首帧会在网络饱和时被饿死。
  - **两阶段呈现要有显式顺序身份**：缓存与网络来自不同作业，完成顺序不可依赖；用 per-section 的发布标记（Empty/Historical/Network）而非任务顺序决定谁能覆盖谁，并让缓存阶段不拥有 work 槽/刷新状态。
