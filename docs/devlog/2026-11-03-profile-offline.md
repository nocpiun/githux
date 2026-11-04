# Profile 持久缓存与 README 图片正式接线

- 日期：2026-11-03
- 任务 ID：`profile-offline`
- 分支：`feature/profile-offline`（base `d21711622ea7860c79e9a4ea8d35a78f010c162c`）
- 执行者：基层实现 Agent
- 状态：独立实现与受控组合验证已完成；Codex 首轮 3 条意见与第二轮 1 条意见均已处理并复验。等待 `cache-runtime` 合入 `main` 及负责人原生窗口验收（`pending`）。

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

所有 Cargo 命令均通过共享验证器 `node scripts/verify.mjs -- cargo ...` 串行执行。审查修复后的最终一轮（工作区含本轮改动）：

- `cargo fmt --all` → exit 0（run `655dff5f`）
- `cargo fmt --all -- --check` → exit 0（run `7b140e75`）
- `cargo check --locked --all-targets` → exit 0（run `c5e529e7`）
- `cargo clippy --locked --all-targets -- -D warnings` → exit 0（run `39ee6ea0`）
- `cargo build --locked` → exit 0（run `c14d57b4`）
- `cargo test --locked` → exit 0，1116 / 96 / 560 / 284 / 250 / 752 / 836 / 44 全部 0 failed（run `374eddd9`）
- `git diff --check` → exit 0

首轮提交 `ece9f4c` 上的整轮检查同样为 exit 0（run `3e29bda8`、`7031fe10`、`f1f201d6`、`a6e4d177`）。

新增受控组合与单元回归：

- `app::pages::profile::tests::online_warmup_and_reopen_offline_restores_profile_readme_and_images_at_natural_height`：真实 `AccountService::open_with_cache` + `CacheStore` + `Accounts` 控制器 + `ProfilePage` 在线预热基本资料、pins、贡献日历、README 正文、仓库相对图片与外部图片，断网重开后从持久缓存完整恢复，展示历史快照徽章并保持自然高度与无内嵌滚动条。
- `app::pages::profile::tests::uncached_offline_and_online_recovery_work_without_masking_errors_as_absent_readme`：断网无缓存报 `NotCached` 并可重试；503 错误显示紧凑警告而非省略；404 与确认 `Missing` 省略且离线恢复仍保持省略。
- `app::pages::profile::tests::partial_pins_and_calendar_and_section_miss_preserve_distinct_states`：部分 pins、单区块离线 `NotCached` 与重试恢复。
- `app::pages::profile::tests::account_switch_pat_replace_removal_and_revocation_isolate_cache_and_stop_slow_pictures`：慢图片在途时换 PAT、移除账户与切账户立即取消在途图片并隔离旧聚合缓存。
- `app::pages::profile::tests::explicit_repository_revocation_clears_historical_profile_and_blocks_offline_revival`：具体仓库明确撤权使账户聚合快照失效并清除已展示的历史快照。
- `app::pages::profile::tests::uncached_service_preserves_online_behavior_without_regression`：无缓存绑定时在线行为与 README 图片加载无回归。
- `app::pages::profile::tests::tab_suspension_preserves_readme_image_provider_across_deactivate_and_reactivate`：普通标签切换（`set_active(false/true)`）后已挂载 README 的在途图片仍能完成，证明 `suspend` 不再卸载 provider。
- `app::pages::profile::tests::offline_reopen_on_a_later_day_restores_the_cached_contribution_interval`：在线预热后把 host 区间换成另一个滚动年再断网重开，贡献日历仍按缓存时的区间恢复，页面与组件的 `range` 都等于该区间并显示历史徽章。
- `app::pages::profile::tests::concurrently_prepared_gate_stamps_converge_on_one_value`：四个真实线程并发准备门禁 stamp，全部成功且取到同一个值。
- `ui::profile::tests::historical_and_partial_provenance_badges_and_section_retries_work`：组件级历史/Partial 徽章与单区块重试动作验证。

## 审查轮次

Codex 首轮（review `PRR_kwDOVN_Iuc8AAAABVw17Vg`，head `ece9f4c`）三条意见均成立并按以下方式修复：

1. **P2 标签切换丢失图片 provider**：原实现让 `ProfileView::suspend` 顺带 `set_image_provider(None)`，返回标签后 provider 不会恢复。现拆分为「暂停交互」(`suspend`：只取消滚动/手势，保留 provider) 与「销毁 reader」(`clear_reader`：重绑、替换 README、授权失效时调用)，并新增上述标签切换回归。
2. **P1 撤权在首轮轮询前发生仍显示旧快照**：原实现用「先看到 hit、后看到 miss」推断撤权，撤权早于首次轮询或中间重填快照时会漏判。现改为聚合门禁命名空间的 `AggregateGateStamp`：读取开始前建立、历史结果提交前复核、在屏期间由监测任务校验，版本不符即失效并清除视图；回归测试在历史快照显示后立即撤权并重新生成 stamp，仍要求清除。
3. **P2 每个 section 各起一条永久轮询**：现只有 `ProfilePage::cache_monitor` 一条任务，`rebind`/`stop`/`Drop` 取消，且不再在循环里重放 `profile(Offline)` 读取。

Codex 第二轮（head `8ff2a43`）一条意见成立并修复：

4. **P2 离线重开时贡献日历按当天滚动年查询，跨日后必然 miss**：缓存键包含区间端点，而 `ProfilePage` 每次都用当天的 `rolling_year()`。现在成功读取日历（在线或离线）后把实际区间写入聚合门禁命名空间的 `profile-calendar-range` 标记；离线回退先试当天区间，miss 后再用记录的区间重读，命中即由 host 采用该区间（`ProfileView::set_range`）并把标题与 tooltip 一起显示为实际区间，同时刷新标记。标记与 stamp 一样受聚合门禁约束（撤权/换 PAT/拒绝会一并失效）；标记缺失时保持 `NotCached` 与重试，不伪造“无贡献”。
   - 并发准备 stamp 时新增进程内 `GATE_STAMP_LOCK` 串行化创建，避免同一页面多个 section 得到不同 stamp（store 已持独占文件租约，跨进程不会并发写同一分区）。回归 `concurrently_prepared_gate_stamps_converge_on_one_value` 用四个真实线程验证收敛。

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

- 本任务未遇到需要修改共享规范的新事故；既有 L012（组件与正式页面组合验证）、L013（Reader Flow 自然高度）与 L014（可选 README 缺省与真实错误区分）均已在实现与测试中直接应用。
