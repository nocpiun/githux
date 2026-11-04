# Profile 持久缓存与 README 图片正式接线

- 日期：2026-11-03
- 任务 ID：`profile-offline`
- 分支：`feature/profile-offline`（base `d21711622ea7860c79e9a4ea8d35a78f010c162c`）
- 执行者：基层实现 Agent
- 状态：Codex 六轮意见与主 Agent 基础验收的 3 条反例均已处理并复验。等待 `cache-runtime` 发布共享权限票据接口后按其契约收尾，以及负责人原生窗口验收（`pending`）。

## 改动与决定

1. **正式 `ProfilePage` 接入 `CachedPageReadSession`**（`src/app/pages/profile.rs`）：
   - 保持 `ProfilePage::new(tab, login, services, navigation, avatars, cx)` 签名完全兼容，不修改 `src/app/pages/mod.rs` 或 `src/ui/mod.rs`。
   - 在捕获 `PageReadSession` 后，通过 `services.repositories().cache(session.context())` 获取 `(CacheStore, CacheScope)` 并构造 `CachedPageReadSession`。若应用尚未注入 `CacheStore` 或当前账户缓存门禁被隔离（`AccessDenied`），无缝回退至在线 `PageReadSession`。
   - 四个独立区块（`Basic`、`Repositories`、`Contributions`、`Readme`）先尝试 `PageReadMode::Online`，在网络断开、超时、5xx 或限流时自动回退至 `PageReadMode::Offline`。
2. **正式 Profile README 附加 `RepositoryImageService::with_cache`**（`src/app/pages/profile.rs`、`src/ui/profile/view.rs`）：
   - 当 `profile_readme` 返回 `ProfileReadme::Prepared { repository, content, .. }` 时，复用其已验证的同名公开仓库 `RepositoryIdentity`（绝不猜测数字 ID）、当前验证通过的 `RepositorySession`、`ReqwestExternalImages`（无凭据 HTTPS 通路，不对外部图片发送 PAT）及 `RepoService::cache(&context)`（`is_private = false`）构造 `RepositoryImageService::with_cache`。
   - 新增 `ProfileView::update_readme_with_provider`，在创建 `ReaderView` 时设置 `MarkdownLayout::Flow` 并挂载 `set_image_provider`，保持自然高度、无灰底、无内嵌竖向滚动和 HTML 行为。详细图片与缓存契约见 [README 图片加载与显示](../reader-images.md) 与 [Profile 视图](../profile-view.md)。
3. **历史快照、Partial 与未缓存状态的紧凑呈现**（`src/ui/profile/{model,header,render,view}.rs`）：
   - 新增 `ProfileProvenance`，在离线历史快照恢复时于资料标题、README 头部、Pinned/Popular 标题及 Contributions 标题旁渲染紧凑 Neutral `Badge`（`Cached · <relative time>`，复用 `ui::relative_time::relative`）。
   - 当 `CacheCompleteness::Partial` 时渲染紧凑 Warning `Badge`（`Partial`，tooltip 显示原因）。
   - 离线未命中（`CacheMiss::NotCached`）或损坏（`Corrupt`/`RepresentationVersion`）映射为 `PresentationError::NotCached` / `CacheCorrupt`，配合 `compact_status::failed` 黄色警告图标、tooltip 原因及点击单次重试（`RetryBasic` / `RetryRepositories` / `RetryContributions` / `RetryReadme`）。绝不将未缓存、5xx 或授权错误伪装成无 README。
   - 已缓存的确认无 README（`Missing` / `NotEligible`）离线恢复时继续静默省略整段 README 区块。
4. **授权、代次与 C1–C5 生命周期门禁**：
   - 读取启动与结果提交均校验 `generation`、`selection`、section `request`、`is_current(&context)` 与 `session.verify()`。
   - 普通标签切换（`ProfileView::suspend`）只停止滚动与手势、保留 README 已挂载的图片 provider，使返回该标签时未完成或后续滚入的图片继续正常加载；页面重绑或替换 README 时调用 `clear_reader` / `cancel_reader` 显式卸下 provider 并取消在途请求。
   - 每个页面绑定仅保留单个可取消的后台门禁监测任务（`ensure_cache_monitor`，在 `stop()` / `rebind` / `Drop` 时取消），不再按 section/重试各起一条循环。该任务每 80ms 在 blocking worker 上校验在屏历史快照的 `AggregateGateStamp`，不重放 profile 读取、不解析响应。
   - `AggregateGateStamp` 是写在 `repository=None` 聚合门禁命名空间里的随机 version stamp：每次读取任务开始时先 `prepare_gate_stamp` 建立/读取，离线历史结果提交前再 `verified_historical_stamp` 复核，因此撤权期间被删除的 stamp、换 PAT/401 拒绝后的门禁拒绝、退休 scope 都会让历史结果不被发布；已展示的历史快照同样由监测任务按同一 stamp 失效。这样即使聚合快照在首轮轮询前被撤权、或随后重新写入了新的 stamp/快照，旧历史快照也不会继续展示。

## 验证

所有 Cargo 命令均通过共享验证器 `node scripts/verify.mjs -- cargo ...` 串行执行。第二轮审查修复并 rebase 到 main `0cdb59c`（含 sidebar-cache #70）之后的最终一轮：

- `cargo fmt --all -- --check` → exit 0（run `157af114`、`f5ae7fdf`；`cargo fmt --all` run `24c801a8`）
- `cargo check --locked --all-targets` → exit 0（run `4bcec08f`、`5c20bcbe`）
- `cargo clippy --locked --all-targets -- -D warnings` → exit 0（run `ab8f7d62`）
- `cargo build --locked` → exit 0（run `5ac8fcd0`）
- `cargo test --locked` → exit 0，1146 / 96 / 579 / 284 / 250 / 771 / 855 / 44 全部 0 failed（run `e705a315`）
- `git diff --check` → exit 0

此前各轮同样全绿：`ece9f4c` 上 `3e29bda8`、`7031fe10`、`f1f201d6`、`a6e4d177`；首轮修复后 `daf4015e`–`374eddd9`。整轮两次失败均已定位为测试门限或既有并发竞态，不是产品缺陷：

- 多次 `cargo test --locked`（run `03c5c6fc`、`2451e558`、`4973b60b`、`5ab0d7a3`、`fc7d3bef`）中 `data::reader_images::tests::fixed_revision_images_persist_across_reopened_store_and_service` 报 `reopen cache store: InUse`：`CacheStore` 是进程级独占文件租约，同二进制内其他测试（例如 `page_cache`/`repository` 的临时 store）尚未释放时该用例重开同一路径会失败。该用例单独复跑通过（`60e226d0`、`272c9ba7`），且失败点在本任务未修改的既有测试与存储层；整套复跑（`8dc4483b`）通过。属于既有测试并行敏感项，不在本任务范围内修改共享 cache 测试。
- 一次整轮（run `886cc506`）中两个新增 profile 组合用例超时：`wait_for_profile` 原为固定 400 次轮询，在整套并行负载下不足以等待后台读取与图片解码。现改为 60 秒墙钟截止（run `43eac9a7` 定向通过，`8dc4483b` 整套通过）。

新增受控组合与单元回归：

- `app::pages::profile::tests::online_warmup_and_reopen_offline_restores_profile_readme_and_images_at_natural_height`：真实 `AccountService::open_with_cache` + `CacheStore` + `Accounts` 控制器 + `ProfilePage` 在线预热基本资料、pins、贡献日历、README 正文、仓库相对图片与外部图片，断网后 **`ProfilePage::rebind`**（不销毁账户服务/CacheStore）恢复四类内容、内外图片与历史徽章，保持自然高度与无内嵌滚动条。这是同一进程内的页面级离线回放；真正的服务/存储冷启动由主 Agent 的 `acceptance_profile_reopens_service_and_store_without_online_revalidation` 覆盖，本文件不把它称为冷启动。
- `app::pages::profile::tests::uncached_offline_and_online_recovery_work_without_masking_errors_as_absent_readme`：断网无缓存报 `NotCached` 并可重试；503 错误显示紧凑警告而非省略；404 与确认 `Missing` 省略且离线恢复仍保持省略。
- `app::pages::profile::tests::partial_pins_and_calendar_and_section_miss_preserve_distinct_states`：部分 pins、单区块离线 `NotCached` 与重试恢复。
- `app::pages::profile::tests::account_switch_pat_replace_removal_and_revocation_isolate_cache_and_stop_slow_pictures`：慢图片在途时换 PAT、移除账户与切账户立即取消在途图片并隔离旧聚合缓存。
- `app::pages::profile::tests::explicit_repository_revocation_clears_historical_profile_and_blocks_offline_revival`：具体仓库明确撤权使账户聚合快照失效并清除已展示的历史快照。
- `app::pages::profile::tests::uncached_service_preserves_online_behavior_without_regression`：无缓存绑定时在线行为与 README 图片加载无回归。
- `app::pages::profile::tests::tab_suspension_preserves_readme_image_provider_across_deactivate_and_reactivate`：普通标签切换（`set_active(false/true)`）后已挂载 README 的在途图片仍能完成，证明 `suspend` 不再卸载 provider。
- `app::pages::profile::tests::offline_reopen_on_a_later_day_restores_the_cached_contribution_interval`：在线预热后把 host 区间换成另一个滚动年再断网重开，贡献日历仍按缓存时的区间恢复，页面与组件的 `range` 都等于该区间并显示历史徽章。
- `app::pages::profile::tests::concurrently_prepared_gate_stamps_converge_on_one_value`：四个真实线程并发准备门禁 stamp，全部成功且取到同一个值。
- `app::pages::profile::tests::aggregate_gate_stamp_survives_capacity_eviction_but_not_revocation`：小预算 store 写入足量普通条目触发 LRU 淘汰（断言最早条目确被删除），stamp 仍有效；显式撤权后立即失效。
- `ui::profile::tests::historical_and_partial_provenance_badges_and_section_retries_work`：组件级历史/Partial 徽章与单区块重试动作验证。

## 审查轮次

Codex 首轮（review `PRR_kwDOVN_Iuc8AAAABVw17Vg`，head `ece9f4c`）三条意见均成立并按以下方式修复：

1. **P2 标签切换丢失图片 provider**：原实现让 `ProfileView::suspend` 顺带 `set_image_provider(None)`，返回标签后 provider 不会恢复。现拆分为「暂停交互」(`suspend`：只取消滚动/手势，保留 provider) 与「销毁 reader」(`clear_reader`：重绑、替换 README、授权失效时调用)，并新增上述标签切换回归。
2. **P1 撤权在首轮轮询前发生仍显示旧快照**：原实现用「先看到 hit、后看到 miss」推断撤权，撤权早于首次轮询或中间重填快照时会漏判。现改为聚合门禁命名空间的 `AggregateGateStamp`：读取开始前建立、历史结果提交前复核、在屏期间由监测任务校验，版本不符即失效并清除视图；回归测试在历史快照显示后立即撤权并重新生成 stamp，仍要求清除。
3. **P2 每个 section 各起一条永久轮询**：现只有 `ProfilePage::cache_monitor` 一条任务，`rebind`/`stop`/`Drop` 取消，且不再在循环里重放 `profile(Offline)` 读取。

Codex 第二轮（head `8ff2a43`）一条意见成立并修复：

4. **P2 离线重开时贡献日历按当天滚动年查询，跨日后必然 miss**：缓存键包含区间端点，而 `ProfilePage` 每次都用当天的 `rolling_year()`。现在成功读取日历（在线或离线）后把实际区间写入聚合门禁命名空间的 `profile-calendar-range` 标记；离线回退先试当天区间，miss 后再用记录的区间重读，命中即由 host 采用该区间（`ProfileView::set_range`）并把标题与 tooltip 一起显示为实际区间，同时刷新标记。标记与 stamp 一样受聚合门禁约束（撤权/换 PAT/拒绝会一并失效）；标记缺失时保持 `NotCached` 与重试，不伪造“无贡献”。
   - 并发准备 stamp 时新增进程内 `GATE_STAMP_LOCK` 串行化创建，避免同一页面多个 section 得到不同 stamp（store 已持独占文件租约，跨进程不会并发写同一分区）。回归 `concurrently_prepared_gate_stamps_converge_on_one_value` 用四个真实线程验证收敛。

Codex 第三轮（head `5e4916a`）一条意见成立并修复：

5. **P2 gate stamp 会被普通容量淘汰误判为撤权**：stamp 原为普通可变条目，`CacheStore::put` 的 LRU 淘汰会在授权未变时清掉它，导致有效历史内容被丢弃成 `Forbidden`。现 `AggregateGateStamp` 持有该条目的 `CacheLease` 并在整个绑定生命周期内保留：容量淘汰（`evict` 跳过 pinned 条目）无法删除它；而撤权、换 PAT、凭据拒绝、退休仍会删除条目，使 `lease.snapshot()` 返回空。校验同时比对 lease 内字节与「当前键仍解析为同一 stamp」，避免删除后重建同分区 stamp 让旧历史复活。回归 `aggregate_gate_stamp_survives_capacity_eviction_but_not_revocation` 先证明最早的普通条目确实被淘汰，再断言 stamp 仍有效、撤权后立即失效。

Codex 第四轮（head `6bdaf7b`）一条意见成立并修复：

6. **P2 撤权后 `cache_invalidated` 永久卡住，重试无法采纳新授权的快照**：`cache_invalidated` 原为一次性闩锁，只在 `rebind()` 清除；而具体仓库撤权刻意保留聚合门禁可用，其他页面重新在线写入快照后，本页重试得到的仍是被拒绝的历史结果，且 `apply()` 在 `accepts()` 失败前没有释放 section 的 `work` 句柄，导致后续重试彻底不动。现 `check_authorization` 在清理完在屏历史内容后**清除**该闩锁（撤权仅使当时展示的快照退休，不永久拒绝新证明），并释放该绑定下所有在途 section 句柄与收件箱；监测任务通过 `cache_monitor_running` 暴露存活状态，退出后允许为新的 stamp 重新启动。回归 `retry_after_revocation_adopts_a_newly_authorized_cache_stamp`：撤权清空在屏快照后，由另一读会话在线重填聚合快照，再在离线状态点击重试，页面正常恢复历史快照；反向验证（把 `swap(false)` 改回 `load()`）该用例超时失败。

Codex 第五轮（head `3828ac7`）一条意见成立并修复：

7. **P2 大小写不同的路由在离线重开时无法取回 Basic 快照**：缓存键包含请求时的 login 原始拼写，而 `apply_basic` 会用服务器返回的 canonical login 覆盖 `self.login`，Shell 也会把路由改写成 canonical。于是以 `ALICE` 首次在线访问后，离线重开 canonical 路由（`alice`）会 miss，Profile 直接无法挂载。现 `read_profile_basic` 在在线成功且 canonical 与请求拼写不同时，用 canonical 拼写再读一次（同一会话、同一授权），把同一观察写入 canonical 键；常见已 canonical 的情形不产生额外请求。回归 `canonical_login_reopen_offline_restores_the_profile_before_canonicalization` 以 `ALICE` 首次访问、断网后按 canonical login 重开，仍恢复历史快照；反向验证（去掉 canonical 预热读）该用例超时失败。

## 2026-11-04 主 Agent 基础验收后的返工

主 Agent 冻结 `ebc8dea` 组合进独立验收 worktree（含 cache-runtime `e2c2230`、home-offline `2af61aa`），用 `profile_acceptance_tests.rs` 经本文件 `tests` 模块 `include!` 引入，得到 14 passed / 3 failed（run `5f274477`）。三条失败均为源码缺口，已在本分支修复：

1. **P1 在线显示的聚合内容不随明确仓库撤权失效。** 原实现只给历史结果保存门禁 stamp，`check_authorization` 也只在 `any_historical` 时清屏；而 `revoke_repository` 刻意不推进 `AuthEpoch`，在线读取显示的旧聚合会留在屏幕上。
   - 现在 `unpack_page_value` 给**所有**受授权约束的观察附带它读取时的聚合门禁 stamp（在线 `Network` 与历史 `HistoricalSnapshot` 等价），字段由 `historical_stamps` 改名为 `gate_stamps`。
   - `check_authorization` 增加 `displayed_sections_are_stale()`：只要任一已显示 section 的 stamp 不再有效就视为失效，先取消监测任务、释放所有 section 在途句柄与收件箱，再通过 `retire_displayed_sections` 清空 `ProfileView`、README `ReaderView` 与图片 provider，留下可重试占位。撤销在线结果与撤销历史结果走同一路径。
2. **P1 后台历史成功到 UI 发布之间漏掉撤权。** `verified_historical_stamp` 只在后台校验，`accepts` 只查尚未更新的 `cache_invalidated` 与 context/binding。
   - `apply` 现在在**出队之后、`view.update` 之前**对随回复携带的同一 stamp 再做一次 `is_current()`（持 `CacheLease` 的进程内检查，不读 SQLite），失败即丢弃该回复并退休已显示内容；新捕获的 stamp 不会为旧回复背书。`accepts` 额外要求已列出的缓存绑定仍然存在。
   - 该提交点检查正是 cache-runtime 正在发布的共享权限票据接口要承担的位置。**依赖记录**：其契约（`D:/githux-cache-runtime/.agents/output/cache-recovery-contract.md`）规定 `CacheStore::authorization(scope, None) -> Option<CacheAuthorization>` 与 `CacheAuthorization::is_current()`（无锁、无磁盘）供 UI 发布前调用，并要求回复携带原票据、禁止在 `apply` 另取新票据。接口合入后本页应把 `AggregateGateStamp` 迁移到该票据，不再自制持久 stamp；在此之前保留现有 lease 方案，二者语义一致（撤权/换 PAT/拒绝/退休失效，普通淘汰与写入不失效）。
3. **P2 运行期缓存故障让健康在线 Profile 失败。** 四个 `read_profile_*` 原来只对 `CacheError::AccessDenied` 回退普通在线读取，SQLite/I/O 故障被映射成 `Unavailable`。
   - 新增 `is_uncached_fallback`：除 `CacheError::Stale`（授权/邻近代次结果，保留原义）外的缓存错误都回退普通 `PageReadSession` 读取，仍走原有取消、context 与授权检查，不读取旧私有缓存；外层离线回放遇到同类缓存故障时改为返回原始网络错误而不是缓存错误。
   - 回退得到的在线结果同样携带 stamp，保持第 1 条的撤权义务。

### 反例接入与反向验证

- `src/app/pages/profile_acceptance_tests.rs` 原样纳入本分支，经 `tests` 模块末尾 `include!` 引入（不在 `app/pages/mod.rs` 注册）。
- 修复后 `cargo test --locked --bin githux app::pages::profile::tests -- --test-threads=1` 为 19 passed / 0 failed（run `d6f3ba28`、`7dc8b7e0`），三条反例与两条新回归全部通过。
- 反向验证（run `1e8ef5cf`）：把 stamp 恢复为「仅历史」、去掉发布前检查、把 `is_uncached_fallback` 收窄回仅 `AccessDenied` 后，三条反例重新全部失败（`acceptance_revocation_also_clears_online_profile`、`acceptance_queued_historical_profile_is_rejected_after_revocation`、`acceptance_profile_cache_write_failure_keeps_online_result`），冷启动用例仍通过——证明三条用例确实锁住这三处缺口。

### 第六轮 Codex 意见（head `4428c61`）

三条均成立于本轮新增代码，已修复：

1. **P2 `CacheLease::snapshot()` 在 UI 线程做 SQLite 读取。** `displayed_sections_are_stale()` 原本在每 30ms 的 `drain` 里对每个已显示 stamp 取一次锁并查库，非活动标签也跑。
   - 删除了该前台函数与调用：前台只消费后台监测任务已经得出的 `cache_invalidated` 结论，每拍不再有任何 store 访问。
   - `apply` 中**每次回复一次**的发布前校验保留（它正是要关闭的窗口），并在源码注释里写明这是 cache-runtime 共享票据接口要替换的那一处，不得移回后台。该单次查询是有界的，且不随 tick 频率放大。
2. **P2 Basic 在线成功时丢掉了刚安装的 gate stamp。** 若三个兄弟 section 都失败，`gate_stamps` 全为 `None`，监测与前台都无物可验，在线 Basic 会在撤权后无限保留。
   - `apply_basic` 现在无论来源都保留 Basic 的 stamp（`*stamps = [gate_stamp, None, None, None]`），与其他 section 一致。
3. **P2 过期回复可能退休刚重绑的新页面。** 原顺序是先做 stamp 检查、后做 `accepts()`；`rebind()` 之后再到达的上一代回复若 stamp 已失效，会清掉新一代页面。
   - 现在先 `accepts()`（generation/selection/request/login/context/binding）再校验 stamp，旧绑定的回复直接安静丢弃，无权改动新页面。

新增回归（均已反向验证：撤掉对应修复即失败）：

- `online_sections_keep_the_gate_stamp_they_were_read_under`：四个 section 在线成功后 `gate_stamps` 全为 `Some`，随后显式撤权清除整页。
- `a_late_reply_from_a_previous_binding_cannot_retire_the_new_page`：后台历史读取成功→重绑→新页面就绪→撤权→投递旧回复，新页面与其内容必须保留。

## 原生验收步骤与边界（`pending`）

- **Runtime 依赖说明**：应用启动处打开唯一 `CacheStore` 并调用 `AccountService::open_with_cache` 属于并行任务 `cache-runtime` 的文件所有权（`src/app/accounts/**`）。在 `cache-runtime` 合入 `main` 前，正式二进制启动默认走无缓存 `AccountService::open`（在线模式不回归）；合入并 rebase 后，正式二进制自动向 `ProfilePage` 提供 `RepoService::cache(context)` 绑定。
- **正式 Shell 原生 Profile 组合验收步骤**（合入 `cache-runtime` 后）：
  1. 启动 `cargo run`，使用有效 Classic PAT 登录，点击侧栏账户行打开本人 Profile（或从 Feed/仓库贡献者打开具有同名公开仓库 README 及图片的他人 Profile）。
  2. 确认公开基本资料、README（含仓库相对图片与 HTTPS 徽章/外链图片）、Pinned/Popular 卡片（最多三行简介、底部统计左对齐）与贡献热力图正常渲染；README 以自然高度展开，仅外层页面滚动，无内嵌竖向滚动条或灰色背景。
  3. 断开网络，切换标签或重启应用后再次进入该 Profile：确认基本资料、README 正文与已缓存图片、Pinned 及贡献日历从本地缓存恢复，各区块标题旁显示紧凑 `Cached · <relative time>` 标记。
  4. 断网状态下访问从未缓存过的用户 Profile：确认显示未缓存提示与黄色警告重试按钮；恢复联网后点击重试，确认恢复在线数据且 `Cached` 标记消失。
  5. 在 Settings › Accounts 中替换 PAT 或移除账户：确认旧 Profile 离线缓存立即被隔离/清理，不会跨账户或在新 PAT 未重验前展示。
- **未验证边界**：GPUI 测试平台不经过 Windows 原生输入分发与真实 GPU 合成；真实窗口下的多 DPI 缩放、字体回退与原生滚动手感待负责人在真实窗口验收。

## 候选可复用教训（供 `docs-current-state` 评估）

- **发布检查必须在 UI 真正提交之前覆盖权限代次。** 后台读完再校验、把「80ms 后可能补清理」当作保证，都会留下「后台成功已返回、UI 尚未 apply」的窗口；校验要作用在回复携带的**同一**代次票据上，并在 `view.update` 之前执行。重新捕获票据不能为旧回复背书。
- **数据来源（在线/历史）不改变撤权义务。** 明确撤权不推进 `AuthEpoch`，所以「当前 session 仍有效」不能作为保留已显示内容的理由；在线读取的聚合、派生描述、README 及其图片都要与历史快照使用同一失效判据。
- **缓存是能力增强，不是前置条件。** 运行期缓存故障（SQLite/I-O）必须降级到原在线路径；只有真正的授权拒绝、取消与过期上下文才保持原义。

既有 L002（模拟与真实分层）、L012（组件与正式页面组合验收）、L013（Reader Flow 自然高度）与 L014（可选 README 缺省与真实错误区分）在本轮继续适用；LESSONS 文件由其他任务维护，本轮只在此记录证据。
