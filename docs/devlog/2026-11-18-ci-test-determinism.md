# CI 测试确定性修复：Profile 别名 Basic 与 Home 离线续页

- 日期：2026-11-18；任务 ID：`ci-test-determinism`；执行者：本任务基层 Agent。
- 目标与状态：修复 main CI 中两处测试调度竞态，只改 `#[cfg(test)]` 与本文档。实现、本地检查与文档完成，待 PR 审查/合并；PR 与 CI 状态以 GitHub 记录为准。

## 改动与决定

### 触发事实

main `4e75ad8` 的 [CI run 44254388008](https://github.com/nocpiun/githux/actions/runs/44254388008) 在 Ubuntu Rust `code-check` 中失败：同一份 1394 个测试里只有

`app::pages::profile::tests::a_cached_alias_login_does_not_strand_the_online_basic_read`

失败，且同一测试在此前的运行中通过。失败行是等待历史快照徽标的 `wait_for_profile`（`profile.rs:3030`）：

```text
profile page did not reach expected state: view=Some((... "Ready(ProfileBasic { ... login: \"alice\" ... })",
"Loading", "Loading", "Ready(())", true,
"Some(ProfileProvenance { historical: false, ... })", "None")) work=[false, true, true, true]
```

即在线 `Basic` 已经完成（`historical: false`、work 槽已清空），历史快照徽标从未出现，等待超时。这是调度顺序依赖，不是产品逻辑回归：测试没有门控在线 `Basic`，只门控了兄弟 section。

### 根因

**Profile 别名 Basic（main CI 失败点）**：旧测试只在 `page.rebind(...)` 之前 `hold_siblings(true)`，在线 `GET /users/Alice` 未被门控。当 mock 在线回复在 SQLite 缓存快照之前或同一 drain 中到达时，`ProfilePage` 正确地发布新鲜 `Ready`（`historical: false`）并抑制陈旧历史徽标；测试等待的 `profile-basic-provenance-historical` 因此永远不会出现。产品行为正确，缺陷在测试夹具缺少对 `Basic` 独立门控与释放能力。

**Home 离线续页**：`refreshing_an_offline_feed_keeps_its_cached_continuation_pages` 在 `settle(&page, cx)` 之后直接断言 Feed 只有 `["first"]`。`settle` 每轮都 `window.refresh()`，而 Feed 第一页只有 1 条；若 Feed 比 Home 早一个 tick 完成绘制，`HomeView` 的“首屏内容过短”自动续读（[L015](../../LESSONS.md#l015虚拟列表的未知高度不能当成内容耗尽)）会在等待期间合法地加载缓存第二页，断言以 `left: ["first", "second"], right: ["first"]` 失败。断言把合法的自动续读当成了错误。

### 修复

只改 `#[cfg(test)]`，产品请求顺序、缓存策略、UI 行为与公开 API 不变。

- `ProfileFixtureTransport` 增加独立 `sibling_release` 门控，与 `release` 平行；`execute` 分别等待两个门控（`wait_for_release`，含取消分支），不再让“兄弟门控”复用总门控。新增 `release_basic()`（清 `held` 并打开 `release`），`release_all()` 同时打开两个门控。
- 别名测试在 `rebind` 前同时 `hold(true)` 与 `hold_siblings(true)`：先确定性观察到历史 `Basic` 徽标且四个 work 槽都在，再 `release_basic()` 断言在线 `Basic` 清掉历史标记、`work[Basic] == None`，而兄弟在线读取仍在飞（`work[1..]` 全为 `Some`），最后 `release_all()` 断言全部 section 完成且 `page.login == "alice"`。
- 部分/离线重试测试在离线重开稳定时同时等待 `page.work.iter().all(Option::is_none)`，并用挂载视图绑定的 `ProfileAction { request, kind: RetryContributions }` 经 `page.action(...)` 触发重试，不再在异步 Flow README 布局可能移动坐标时坐标点击 `profile-section-error-mark`。真实 UI 错误标记点击继续由 `src/ui/profile/tests.rs::historical_and_partial_provenance_badges_and_section_retries_work` 覆盖。
- Home 测试在 `settle` 后同时接受 `["first"]` 与 `["first", "second"]`：若只有第一页则显式 `load_more_for_test` 并驱动到两页，再执行离线刷新并断言集合不缩短。

### 验证

全部 Cargo 操作经 `scripts/verify.mjs`；构建槽等待时间不计入命令时长。

| 阶段 | 命令 | runner 记录 | 结果 |
| --- | --- | --- | --- |
| 基线代表用例 | `cargo test --locked --bin githux …a_cached_alias_login_does_not_strand_the_online_basic_read` | `d9e1209a-c203-4a00-9d1d-d5e3b307ec3d` | exit 0（Windows 本地顺序未复现，复现证据是上面的 CI 日志） |
| 基线代表用例 | `cargo test --locked --bin githux …refreshing_an_offline_feed_keeps_its_cached_continuation_pages` | `d65b2c9a-9ac3-492e-983b-93de70b814a3` | exit 0（同上，竞态未在本地命中） |
| 格式化 | `cargo fmt --all` | `47570fda-8739-40d0-99f0-5f9c2650a908` | exit 0 |
| 修复后 | `cargo test --locked --bin githux app::pages::profile::tests::` | `c39598ea-b909-4600-8e5c-6e1856596d15` | 28 passed / 0 failed |
| 修复后 | `cargo test --locked --bin githux …refreshing_an_offline_feed_keeps_its_cached_continuation_pages` | `e688e794-c483-43ec-a002-dedfa5d00c28` | exit 0 |
| 格式检查 | `cargo fmt --all -- --check` | `ed2e8b57-7434-4c2d-98e3-75e827a43b45` | exit 0 |
| 静态检查 | `cargo clippy --locked --bin githux --tests -- -D warnings` | `02e8a75e-ea72-48b8-8877-5c2473c42241` | exit 0 |
| 空白检查 | `git diff --check` | — | 通过 |

本地 Windows 无法稳定复现 Ubuntu 的调度交错，因此“修复前失败”的实际证据是 CI 日志，而不是本地重跑；修复后用门控让两个关键顺序确定化，而不是重复运行到绿。PR 最新 head 的 CI/Codex 审查与合并后 main CI 以 GitHub 实际记录为准。

## 后续

- 本任务不含产品代码、UI 样式或公开接口改动，也没有新增原生 UI/实网验收声明。
- 兄弟 section 的到达顺序仍由产品并发决定；测试只在需要观察中间状态时显式门控，未把产品请求串行化。
- [L010](../../LESSONS.md#l010交互预览的-fixture-必须支持反复操作) 的“并发补充”已经要求主动控制两种完成顺序、不能靠串行化或重复运行掩盖问题，本任务只是把该要求落实到 Profile 夹具的独立门控；[L015](../../LESSONS.md#l015虚拟列表的未知高度不能当成内容耗尽) 已覆盖短首屏自动续读。故未新增 LESSONS 条目（同时避免与并行 PR #89/#92 计划新增的 L029 冲突）。
