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

## PR #85 第五轮审查修复

Codex 对 `05e416b` 提出 1 项 P2，成立且可直接修复；仍无待裁决事项。

**P2 分支枚举失败时不再重验已显示快照**：bundle 已上屏且在线摘要成功后，若分支枚举失败且没有可用缓存回退，旧实现得到 `chosen=None` 同时清掉 `snapshot_preliminary`，于是既不重验已显示 commit，后续成功重试也不会重验。修复：枚举失败时用保存意图或刚观察到的 `default_branch` 作为可解析 ref；两者都没有时保留 `snapshot_preliminary`，让后续成功重试仍能触发重验。回归 `a_failed_branch_enumeration_still_revalidates_the_cached_revision`（恢复旧语义后失败）。

本轮验证（同一源码）：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked` 全部通过。

**已知与本任务无关的既有偶发**：全量 `cargo test --locked` 期间 `data::reader_images::tests::fixed_revision_images_persist_across_reopened_store_and_service` 偶发在重开 `CacheStore` 时得到 `InUse`（该用例用 50ms sleep 等异步写入）。本任务未修改 `src/data/reader_images*`，定向重跑与主程序目标均通过；记录为既有顺序/资源偶发，不归因本任务。

## PR #85 第六轮审查修复

Codex 对 `6b6a8d5` 提出 1 项 P2，成立且可直接修复；仍无待裁决事项。

**P2 同 commit 重验后保存的展开目录不恢复**：bundle 只还原根树（以及保存文件所在路径的祖先），保存的 `intent.expanded` 目录不会由 bundle 读取；同 commit 捷径保留已加载内容后直接返回，没有 Root/Readme 回复来触发 `load_expanded`，于是这些目录保持展开但为空。修复：捷径在保留内容且没有补发 Root/Readme 时显式 `load_expanded` + `resolve_selection`（此时已按在线模式读取）。回归 `saved_expansions_resume_after_same_commit_revalidation`（移除该调用后失败）。

本轮验证（同一源码）：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（主程序与各目标通过；`data::reader_images` 既有偶发另记）、`cargo build --locked` 全部通过。

## PR #85 第七轮审查修复

Codex 对 `f549e23` 提出 1 项 P2，成立且可直接修复；仍无待裁决事项。

**P2 缓存分支回退会抑制 revision 重验**：`network_verified` 只看本次分支枚举的 provenance；当摘要在线成功、分支枚举瞬时失败并回退到缓存（Snapshot provenance）时，该判断为假，已显示的 revision 不再重验。修复：把 `details_provenance` 的 Network 结果也计入（摘要在线成功即证明网络可达），仅分支列表保持历史标记。回归 `a_cached_branch_fallback_still_revalidates_the_shown_revision`（移除该条件后失败；同时测试夹具新增 `fail_branches` 开关模拟枚举瞬时失败）。

本轮验证（同一源码）：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked` 全部通过。

## PR #85 第八轮审查修复

Codex 对 `d16ac05` 提出 1 项 P2，成立且可直接修复；仍无待裁决事项。

**P2 canonical login 改名后未重启 README discovery**：Profile README 来自与登录同名的公开仓库，缓存 Basic 先用旧 login 启动了 README；在线 Basic 报告新 canonical login 时，已完成的 README 区块不会被替换、在途槽位也不会重启，于是页面继续显示（或因 404 永久省略）旧 `old/old` README。修复：`changed_login` 时取消在途 README work、推进其 request 身份、把该区块置回 Loading 并重新读取（新请求使用新 login 的仓库），旧回复由 request 身份拒绝。回归 `a_canonical_login_rename_restarts_the_cached_readme_read`（移除重启后失败；测试夹具的 `/users/<login>` 现在返回可配置的 canonical login 以复现改名）。

本轮验证（同一源码）：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked` 全部通过。

## PR #85 第九轮审查修复

Codex 对 `8549fa2` 提出 2 项 P2，均成立且可直接修复；仍无待裁决事项。

1. **P2 撤权后预发布列表仍留在屏上**：缓存先行列表在 `/user/repos` 仍在途时已经显示，而 `clear_revoked_repositories` 会因 `operation.is_some()` 直接返回，撤权后旧行继续可见直到请求结束。修复：当前显示的是缓存先行（`Historical` + `Ready`）列表时，撤权检查照常清理已显示行，但不结束在途操作；最终回复仍按原票据/上下文校验。回归 `a_revocation_retires_a_preliminary_list_while_the_online_read_is_pending`（恢复旧早退后失败）。
2. **P2 仅缺 README 发现时不恢复保存的文件解析**：同 commit 捷径在只补发 `Job::Readme` 时跳过了 `load_expanded`/`resolve_selection`，而 `apply_readme` 只会在 `Selection::Readme` 时请求正文，于是保存的文件永远停在 Loading。修复：只要根树已保留（`!missing_root`）就恢复展开与选择解析，与是否补发 README 无关。回归 `a_saved_file_resolves_when_only_readme_discovery_was_missing`（恢复旧条件后失败）。

实现细节补充：清理期间在途读取仍拥有最终呈现，因此缓存先行行被清除为中性 Loading（不发布与之竞争的失败文案），且不清 `loaded` 标记——否则 401 之后会在结算时再触发一次自动读取，把凭据拒绝文案覆盖成网络失败（既有 `a_401_is_not_shown_as_offline_history` 回归暴露出这一点）。

本轮验证（同一源码）：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked` 全部通过。

## PR #85 第十轮审查修复

Codex 对 `b6a3c11` 提出 1 项 P1 与 1 项 P2，均成立；仍无待裁决事项。

1. **P1 去重快照会截断已追加的离线 Feed**：离线加载续页后再次刷新（仍离线）时，回放的第一页与已显示标记相同，走捷径把第一页交给 `apply_feed_page`，而该 API 只扩不缩虚拟列表，保留行会索引到新内容之外并 panic，同时绕过 `feed_next_page`/`feed_seen_ids` 重置。修复：只有当已显示条目数不大于本快照时才做追加态归一，否则保留同一缓存代际的合并集合不动。回归 `refreshing_an_offline_feed_keeps_its_cached_continuation_pages`（去掉守卫后 panic）。
2. **P2 缓存阶段失败在复核后不重试**：预览快照阶段打开的未缓存目录/文件被记为目录失败或 `ContentFailed`，同 commit 复核成功后 `load_expanded` 跳过失败目录、`resolve_selection` 对 `ContentFailed` 直接返回，错误一直留在屏上。修复：捷径里若已显示修订仍带 snapshot provenance，就清空目录失败并重置 `ContentFailed`/`SelectionUnavailable` 为 Loading 后重试。回归 `a_cache_miss_directory_recovers_after_same_commit_revalidation`（关闭清理后失败）。

注意：不能用 `snapshot_preliminary` 判断该 переход——`apply_branches` 在派发复核任务时已提前清除该标记，因而改用已显示修订的 provenance 判断。

## PR #85 第十一轮审查修复

Codex 对 `3b56e12` 提出 1 项 P2，成立。

- **P2 复核未验证时缺失资源永久 Loading**：摘要与分支列表都回落到缓存（`network_verified` 为假）时，`apply_branches` 既不重新解析也不补读，快照里缺失的根树、README 发现与正文就一直是加载态。修复：该情形清掉 preview 标记并走离线路径补读（根树、README 发现、展开目录与选择解析），让每个资源到达确定状态（内容或可重试错误），保留下来的可读部分与历史标记不变。回归 `an_unverified_revalidation_settles_the_missing_snapshot_resources`（关闭该分支后失败）。

## 基线整合（2026-11-12）

负责人于 2026-11-12 接受 head `54f5632`，`pending` 已移除；本轮只做基线整合，不重做实现、不改已接受的呈现。

- 用普通 `--no-ff` merge 合入 `origin/main` = `a2c5ade`（#82 reader-svg、#84 scroll geometry、#87 workflow），保留已审查历史，未 cherry-pick 任何兄弟分支 WIP，未强推。合并提交 `9f48c4f`，无代码冲突；唯一同文件改动是 `docs/home-view.md`（两侧分别新增 cache-first 与滚动几何段落，自动合并后两段都在）。
- 组合面核查：
  - `src/ui/home/mod.rs`（main 侧新增 `ContentInsets`/`InsetListTarget`/尾侧 8px 通道）与 `src/app/pages/home.rs` 的发布顺序互不相干：快照/网络仍在 host 内按 `PublishMark` 决定覆盖，UI 只负责几何。`apply_feed_page` 仍然只在“更长”时 `splice`，因此第十轮“去重不得截断已追加集合”的守卫依旧必要且有效。
  - `src/ui/repository/view.rs` 的 tree 行尾侧 overdraw 不改行内容与选择语义；快照整包发布仍按同一 commit 同时更新 header/tree/reader。
  - `src/ui/reader/**`、`src/content/images/**`、`src/data/reader_images.rs`（SVG/质量/时间线、`start_with_options`）未改动；固定对象缓存读取与同 commit 保留 reader 实体、回调身份的逻辑不受影响。
- 组合验证（同一合并提交）：`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked`。
  - `--bin githux` 1342 通过；`--test home` 116；`--test repository` 952；`--test repository_overview` 44；`--test profile` 344；`--test reader` 309；`--test scroll_viewports` 1343；`--test notifications` 全通过。
  - cache-first 关键用例在合并后逐一复跑并通过：Home/Feed 快照先行→网络替换、迟到缓存不回滚、Profile 分节快照先行、侧栏预发布（含撤权清理）、Repository 快照整包（同 commit 复核/缺 README 发现恢复文件解析/缓存阶段失败重试/复核未验证时补读）、`offline_replay_does_not_wait_for_a_network_permit`。
- 一个非本任务失败：`network::transport::bounded_tests::chunked_and_no_length_stop_before_eof_on_success_and_errors` 在一次全量并发运行中报 `Err(Network{"error sending request"})` 而非 `ResponseTooLarge`（本地 HTTP 测试服务在高并发下连接失败），单独运行该模块通过；与 cache-first 无交集（transport 未改动）。
- 无新增语义选择：合并没有改变已接受行为，因此不改 UI 呈现、不重写 Reader/SVG/表格图片，也不改 `merge_owner`。

## 基线整合后的审查修复（第十二轮）

Codex 对 `4d2c729`（合并 `a2c5ade` 后的 head）提出 1 项 P2，成立。

- **P2 复核成功时未作废仍在途的缓存阶段读取**：第十轮只清理了“已到达的失败”。若用户在预览快照阶段展开未缓存目录或打开文件、其 `OfflineOnly` 读取在复核成功时仍未返回，`loading` 标记会挡住重试（`load_expanded` 跳过 loading、`resolve_path` 直接返回、`pending_content` 挡住重新请求），随后迟到回复按未变的 branch/commit 校验通过，把缓存未命中错误发布到已验证的页面上且不再自动重试。
- 修复：
  1. `TreeModel.loading` 改为 `BTreeMap<path, RepositoryCacheMode>`，新增 `matches_pending(path, mode)`；`Job::Directory`/`Body::Directory` 携带 `mode`，`apply_directory` 只应用“当前仍在等待的那个模式”的回复，其余（被作废的离线读取）直接丢弃。
  2. 新增 `supersede_pending_loads()`：复核成功（且屏上修订仍带 snapshot provenance）时作废全部在途目录读取，`load_expanded`/`resolve_selection` 随即以在线模式重新请求。
  3. 文档读取：作废时清空 `pending_content` 并推进 `selection_request`，迟到正文回复因身份不符被丢弃，同时立刻发起新的正文读取。
  4. 新增 `online_verified` 纪元：该 generation 的修订一旦由网络验证，用户后续发起的读取（展开目录/打开文件）走 `OnlinePreferred`，不会在摘要或分支列表仍来自缓存时重复一次纯缓存读取、再报同样的未缓存错误；`refresh_rebind`/换分支等代次边界重置。
- 回归 `a_pending_cache_phase_read_is_superseded_by_online_verification`：构造“目录与 README 正文读取仍在途”的预览状态，复核同一 commit 后断言在途目录被重新请求、正文读取换了身份、迟到的离线未命中结果不产生可见错误，最终目录内容与正文都在线到达（仅回退本修复后该用例失败）。
- 顺带修正本任务两处测试夹具的等待方式：`src/app/pages/home/tests.rs::settle` 与 `src/ui/repository/offline_tests.rs::settle` 由固定迭代次数改为 60 秒截止时间。原因是一次全量并发运行中 `app::pages::home::tests::a_cache_miss_keeps_the_ordinary_online_loading_path` 因 ~0.8s 固定预算在高负载下超时（状态仍是 Loading，非产品行为变化）。改为截止时间后全量运行通过。

本轮验证（同一提交）：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（bin 1343、home 116、notifications 642、profile 344、reader 309、reader_images 846、repository 953、repository_overview 44、scroll_viewports 1344，0 失败）、`cargo build --locked`。

## 测试等待方式与 CI 稳定性（同一提交）

新 head 的 CI 在 `--test scroll_viewports` 里失败过一次：`app::pages::profile::tests::account_switch_pat_replace_removal_and_revocation_isolate_cache_and_stop_slow_pictures` 在“离线 + 已换 PAT”阶段等不到 `profile-page-basic-error`。

- 复现：本地连续跑该套件，约每 3–6 次出现一次。诊断打印显示失败时**主机状态已正确**（`view=None`、`unavailable=Some(Unauthorized)`、`request=false`），但渲染出的 `profile-page-basic-error` 标记在该帧不存在，于是等待一直不结束。
- 判断：该断言等的是渲染标记（几何/帧时机），而这一阶段的合同是“旧视图被清除 + 主机发布失败”。二者在并发负载下不总在同一帧对齐，属测试等待方式问题，非 cache-first 行为变化（该路径在 base `f9e31a0` 已存在，且本地单跑/全量多次通过）。
- 修复：该处改为等待主机状态（`view_for_test().is_none() && unavailable.is_some()`），与同文件其他断言一致；`Home` 套件里若干固定迭代次数（100/200/300 次）的等待改为统一的 `pump_until` 截止时间等待（60s），`settle`/`wait_for_profile`/`offline_tests::settle` 早已是截止时间等待。
- 结果：`--test scroll_viewports` 连续 3 次全量通过，`, full cargo test --locked` 一次通过（1343+116+642+344+309+846+953+44+1344，0 失败）。

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
