# 2026-11-09 Native Shell 仓库页原位替换（nsh-repository）

任务：`nsh-repository`（migration/nsh-repository，基线 `01decd2`）。范围：`src/ui/repository/**`、
`src/ui/repository_overview/**`、`tests/repository*.rs`、`examples/repository*.rs`、
`src/bin_support/repository_fixture.rs`。交付：本地提交，不合入 main、不推送、不改 PR #75。

## 决定与实现

负责人 2026-11-09 的校正（见 [原位替换契约](../native-shell-inplace.md)）要求替换现有 Ely 页面，
而不是保留第二套应用。本任务因此删除旧的 GPUI 页面实现，保留其业务状态机与真实服务接线，改为：

- **控制器与视图分离**：`RepositoryController` 不持有窗口/信号，可在测试线程直接 poll；NSH
  `RepositoryPage` 包一层 `Rc<PageRuntime>`，持有 `Signal<ViewModel>`、`Signal<Option<ReaderHandle>>`
  和导航 sink。`PageView::element()` 返回稳定 props 的 `RepositoryView` 组件，组件 `view` 只运行一次，
  变更通过信号绑定更新；Reader 只在自己的 `dynamic` 中挂载，普通状态变化不会卸载它或 retire 图片授权。
- **发布门禁**：四个顶层资源（摘要、分支、revision、content）各用一个 `PageSlot`；请求 ticket 在
  开始前 `begin`，回复经 `publish(ticket, value, runtime.sessions())` 校验。目录树/README 发现作为
  revision 子资源，另核对页面代次、分支请求、固定 commit、路径身份与 `cache_allowed()`。
  `poll` 每轮 `clear_invalid` 已展示值；撤权/账户变化在一次 drain 内清空。
- **捕获令牌**：摘要/分支/revision/content 各保存读取前捕获的 `CacheAuthorization`；overview 单独
  保存一份。检查全部是原子 load，不在渲染或 drain 做存储 I/O（LESSONS L022）。
- **Reader**：复用 `crate::ui::reader`（`ReaderHandle`/`ReaderDocument`/`ImageBridge`/
  `ReaderLayout::Repository`）。内容在后台准备，UI 线程只创建句柄；页内 Tab 切出 Code 时卸载 Reader
  元素并清空句柄，切回时用同一文档 + 新授权桥重新挂载（不重读 blob）。
- **Star**：真实 `RepositoryActionService`，写确认后只发 GET 摘要；超时只对账不重放；离线快照禁用；
  Fork 无写入口。NSH 入口 `star_entry` 在失败态重试只读。
- **Overview**：保留 `SectionState`/binding/历史与 partial 语义；NSH 视图渲染 Releases/Contributors
  紧凑条和 4px 语言底条。Contributor 头像当前用首字母回退，`AvatarImage` 注入点保留给账户头像加载器。
- **文件树**：`TreeModel` 原样保留（惰性目录、行上限、目录优先排序、失败分层）；NSH 用
  `virtual_list` 虚拟化，行高/缩进/chevron 槽位/选中与焦点区分沿用已确认规则。
- **导航**：跨仓库/跨 commit 链接只产生 `PageAction::LinkNotFollowed`；外部 URL 交壳；
  `OpenContributor` 走 `PageNavigation::CanonicalProfile`；`OpenRelease` 走 `External`。

## 旧行为/测试映射（摘要）

| 旧覆盖 | 新覆盖 |
| --- | --- |
| `the_normal_fixture_renders_its_readme_at_a_real_path_and_single_selection` | 同名：摘要/分支/commit/README 路径/行排序/无伪 README 行 |
| `expanding_nested_directories_and_reopening_the_same_file_always_succeeds` | `nested_directories_load_once_and_reopening_the_same_file_works` |
| 分支真实差异、A→B→A | `branch_switches_resolve_real_differences_and_a_to_b_to_a_ignores_the_old_reply` |
| 切分支不重取 overview | `a_branch_switch_does_not_restart_the_repository_overview` |
| README 缺失/不确定/409/空分支 | `readme_absence_uncertainty_empty_repository_and_empty_branch_list_stay_distinct` |
| 目录失败作用域与单次重试 | `a_scoped_directory_failure_keeps_the_rest_of_the_page_and_retries_once` |
| 空树/截断树 | `a_confirmed_empty_root_is_not_a_failure_and_a_truncated_tree_says_so` |
| 页内 Tab 清空/恢复 Code | `switching_the_in_page_tab_keeps_the_code_intent_and_returns_to_the_same_file` + headless 测试 |
| 账户切换、失活、旧回复 | `an_authorization_change_clears_every_displayed_value_and_the_queued_reply` |
| 跨仓库链接只报告 | `a_reader_link_to_another_repository_is_reported_and_never_followed` |
| overview typed action 复核 | `overview_actions_are_revalidated_against_the_displayed_values` |
| 键盘树导航 | `tree_keyboard_navigation_moves_by_visible_rows_and_activates` |
| Star 初始/双向/计数 | `star_observes_then_toggles_both_directions_with_authoritative_counts` |
| Star 403/未知超时/不重放 | `a_rejected_write_keeps_the_server_observation_and_retry_only_reads`、`an_unknown_write_outcome_is_only_reconciled_by_get` |
| 离线重启同 commit/目录/README/源码 | `an_offline_restart_reads_the_same_commit_paths_and_directories_from_cache` |
| 未缓存 miss 与网络拒绝区分 | 同上（未读目录断言 cache-miss 文案） |
| Refresh 恢复在线 + 离线 Star 禁用 | `manual_refresh_restores_online_reading_and_enables_star` |
| 撤权单次 drain 清空 / 旧回复拒绝 | `revocation_clears_the_displayed_page_in_one_drain`、`a_reply_captured_before_revocation_is_rejected_and_never_revived` |
| 历史计数不冒充最新、Cached 标记 | `historical_counts_and_overview_are_never_presented_as_fresh` |
| overview 组件状态/历史/partial/语言比例 | `src/ui/repository_overview/tests.rs` |
| NSH 视图真实渲染 | headless `the_page_renders_real_rows_and_switches_in_page_tabs` |

旧测试中依赖 GPUI 布局/像素/Ely 控件的断言没有逐条平移（无对应 NSH 元素树）；由 headless 可观察
状态与 controller 行为断言替代，视觉与输入仍由负责人验收。`tests/repository.rs` 与
`examples/repository.rs` 已改为 NSH 装配（真实服务 + 控制器 + fixture 数据预览），需要在集成后的
根 package（NSH 依赖 + 最终模块路径）中编译，本任务的隔离 probe 不编译这两个入口。

## 验证

全部经 `node scripts/verify.mjs -- cargo ...`，共享 Windows mutex，`CARGO_PROFILE_DEV_DEBUG=0`、
`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_INCREMENTAL=0`（runner 设置），独立 probe target：

| 目的 | 命令（manifest `.agents/task/probe/Cargo.toml`） | 结果 |
| --- | --- | --- |
| 生产源码编译 | `cargo check --lib` | SUCCESS（run `0e178349`） |
| 页面/overview 测试 | `cargo test --lib ui::repository` | SUCCESS 33/33（run `b5f6a695`） |
| 严格 Clippy | `cargo clippy --lib --tests -- -D warnings` | SUCCESS（run `7c2d8675`） |
| 格式（自有文件） | `rustfmt --edition 2024` + `cargo fmt --check` | 仅 probe 装配与未拥有的 `src/nsh/runtime/mod.rs` 保留既有差异 |
| 全库回归（probe 内业务层 + 本页） | `cargo test --lib` | 终轮 SUCCESS 606/606（run `6f0c4f11`）；首轮 600 passed / 6 failed 均为本页测试断言或重启时序，修正后全绿 |
| 严格 Clippy（终轮） | `cargo clippy --lib --tests -- -D warnings` | SUCCESS（run `84c2fd39`） |
| 格式（终轮） | `cargo fmt --check` | 仅 probe 装配与未拥有的 `src/nsh/runtime/mod.rs` 保留既有差异（run `5c0e4a39`） |

测试边界：probe 用最小适配复制 `app::pages` 契约并包含真实 `src/app`/`src/ui`/runtime/reader 源码；
它不是最终应用组合，也不能替代集成作者的真实契约编译。Probe 位于被忽略的 `.agents/task/probe`，
不是第二个生产 package。README 图片 provider 需要宿主注入执行器（见 integration-notes）；未注入时
页面如实降级为无 provider，不新建执行器/网络栈。

## 剩余问题

1. **执行器注入**：`RepositoryPage::set_image_executor` 需壳/集成作者在构造后调用；否则 README 图片
   只显示占位。`Services` 没有公开 executor 访问器，本任务未改 runtime。
2. **Contributor 头像**：需要账户头像加载器提供 decoded image；当前首字母回退是诚实降级，不是最终视觉。
3. **展开目录跨页恢复**：`workspace_intent` 只保存 branch/file；展开集合是页面本地状态（旧页面可保存）。
4. **树滚动位置**：页内 Tab 切出/回到 Code 会重建虚拟列表，滚动位置回到顶部；行、选中与文档保留。
5. **视觉/输入验收**：未运行真实 Windows 窗口；headless 不覆盖 IME、焦点、像素、tooltip 拟合、
   NSH 原生滚动条外观。
6. **examples/集成测试装配**：需要集成后的 NSH 依赖与最终路径；本任务 probe 未编译它们。
7. **真实 GitHub 写入**：Star/Unstar 未对真实仓库执行，保留负责人验收。

## 教训候选

- 页面在 NSH 中必须把 Reader 挂载在只读自身句柄信号的 `dynamic` 里；把它放进“任意状态变化都会重跑”
  的 dynamic 会触发 `reader()` 的 cleanup 提前 retire 图片授权（本任务实现的直接原因）。
- `PageSlot` 的 `publish` 只在 slot 有 pending ticket 时成功；取消/重建必须先 `begin` 再发请求，
  否则合法回复会被当成旧响应丢弃。见测试 `manual_refresh_restores_online_reading_and_enables_star`。
