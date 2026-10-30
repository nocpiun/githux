# Home 仅 open、Feed 三类活动筛选

- 2026-10-30，任务 `home-feed-focus`，执行者：基层实现 Agent。
- 目标与状态：实现负责人 2026-10-30 的 Home/Feed 信息筛选，等待 PR 审查；PR 目标分支为 `feature/shell-pages`，保持 `pending`，不自行合并。

## 背景与决定

负责人 2026-10-30 确认：

1. Home 保持 `involves:<当前登录>`、无时间窗口、`updated` 降序，但**实际查询必须加 `is:open`**；最终可见条目只能是 open 的 Issue/PR，closed/merged/未知状态不得泄漏。不能只请求无限制页面再在本地隐藏 closed 行。
2. Feed 只保留明确允许的活动，去掉 Push 等技术噪声；**先实现三类，关注两类暂缓**（负责人回答）：
   - 合并的 PR：标题 `<user> contributed to <owner>/<repo>`，`<user>` 取 payload 中真实 PR 作者，而不是点 merge 的维护者 actor；
   - 星标仓库：`WatchEvent action=started`；
   - 创建仓库：`CreateEvent ref_type=repository`。
3. Follow 两类（“Followed someone / Followed me”）暂缓：当前官方 Events API 事件类型列表没有 `FollowEvent`，followers/following 端点只返回关系、没有事件时间戳（2026-10-30 核对 [GitHub event types](https://docs.github.com/en/rest/using-the-rest-api/github-event-types)、[followers](https://docs.github.com/en/rest/users/followers)）。本轮不实现、不占位、不伪造时间戳、不抓取私有网页端点。保存的确认见 `.agents/task/follow-decision.md`。

## 改动

- **Home 查询**（`src/github/home/search.rs`）：`q=involves:<login> is:open [updated:…]`，顺序、排序与分页不变；`state` 改为可选并在转换时归一化为 `open`，closed/merged/未知/缺失一律不返回，作为缓存/mock/非合规数据路径的防御。筛选不是截断，`readiness` 与 `total_count` 语义不变。
- **Feed 策略**（`src/domain/home.rs`）：新增纯函数 `classify_feed_activity`，三类活动的唯一判定。合并证据为 `action=closed` + `pull_request.merged=true`；官方 `PullRequestEvent.action` 没有 `merged`，`closed` 单独不算合并。作者来自同一 payload 的 `pull_request.user`（`FeedAuthor`），不额外请求。未知事件仍解析为 `Unknown`/`Unknown` payload，不 panic 也不保留大 payload。
- **Feed 适配器**（`src/github/home/events.rs`）：解析后按策略过滤，保留服务顺序与真实事件 id；`FeedLimits::max_events` 统计解析到的事件，过滤不会触发额外分页/请求来填满页面。整页噪声仍是 `Complete` 空 Feed；真实上游分页失败仍为对应 Partial。ETag/`X-Poll-Interval`/304 语义不变。
- **视图**（`src/ui/home/**`）：`apply_feed` 对每个 `FeedOutcome` 再过滤一次，`apply_home` 丢弃非 open 条目，作为 retained/partial/fixture/未来数据路径的防御；选择索引、滚动与激活都基于过滤后的列表。Feed 卡片改为紧凑的 credited 用户 + 动作句 + 目标仓库/PR + 相对时间；删除 Push/ref/SHA/commit 数等 prose 与所有未知类型回退句。空态文案区分正常空 Feed 与真实错误。
- **头像归因**（`src/app/pages/home.rs`、`src/ui/home/mod.rs`）：`AvatarRenderer` 接收 credited subject（合并 PR 时为 PR 作者）；同时移除原先重复安装 renderer 的死代码与不再使用的 `avatars` 字段。
- **fixture**（`examples/home.rs`、`src/ui/home/tests.rs`、`src/app/pages/tests.rs`）：更新为 open-only Home 与三类 Feed；新增 `Filtered out` 预览场景；Shell 端到端 transport 混入 closed issue、Push 与合并 PR，断言只显示合法行。
- **文档**：`product.md`（Home 查询、Feed 三类与 Follow 暂缓、Profile README 确认意图）、`home-view.md`、`home-query-service.md`、`page-data-service.md`、`github-api-capabilities.md` 同步；旧的“Feed 不过滤/未知回退/Home 可含 closed”表述已被本轮负责人指令取代。

## 验证

- 串行 Cargo helper（`.agents/task/cargo-serialized.ps1`，共享 build mutex、独立 `target/`）：`fmt --all --check`、`check --locked --all-targets`、`clippy --locked --all-targets -- -D warnings`、`test --locked`、`build --locked` 全部通过（退出码 0）。
- 测试计数：主 binary 805、home 60、profile 194、reader 168、repository 565、repository_overview 20。
- 新增回归：
  - 搜索：`query_is_encoded_and_fixed_to_involves`、`the_open_filter_is_not_an_unrestricted_limit_page_hidden_afterwards`、`non_open_items_are_dropped_from_the_returned_list`（closed/merged/未知/缺失 state 均不返回，且仍是 Complete）。
  - Feed 适配器：`retained_activities_keep_source_order_and_identity`（顺序/身份/PR 作者归因）、`wrong_actions_ref_types_and_missing_merge_evidence_are_excluded`、`an_all_filtered_feed_stays_complete_and_empty`、`filtered_activities_do_not_trigger_extra_fetching_to_fill_the_page`。
  - 视图：`closed_merged_and_unknown_home_states_never_become_rows_or_targets`、`a_refresh_that_returns_only_closed_items_clears_the_previous_open_snapshot`、`only_retained_activities_make_feed_cards_and_no_placeholder_appears`、`a_merged_pull_request_credits_the_author_not_the_merging_actor`、`feed_merge_evidence_follows_the_official_event_shape`。
  - 正式 Shell 端到端：`the_shell_mounts_real_home_and_reads_its_two_columns` 在真实 adapter/data/view 路径下断言 closed Home 行与 Push 事件都没有行。
- 测试执行期间遇到一次与本次改动无关的既有 flaky 用例 `network::transport::bounded_tests::chunked_and_no_length_stop_before_eof_on_success_and_errors`（本地 socket 时序，单独或重跑通过）；不影响本轮结论，未修改该模块。

### 原生预览证据

本机 Windows 上构建并运行真实组件的独立预览，用 `PrintWindow` 抓取原生帧：

```sh
cargo build --locked --example home
powershell -File .agents/task/capture.ps1 -Out .agents/task/home-preview.png          # Mixed
powershell -File .agents/task/capture.ps1 -Scenario 3 -Out .agents/task/home-filtered.png  # Filtered out
```

- Mixed 帧（1195×837 物理像素）：Home 六项全部显示 `#N · open`；Feed 只有三张卡片——`synthetic-contributor` 的合并 PR（inset 显示 PR 标题）、`synthetic-user` 星标仓库、`synthetic-user` 创建仓库。没有 Push/issue/评论/Release/Fork/未知卡片。
- Filtered-out 帧：同一预览场景把全部 Push/issue/评论/Release/Fork/分支/未合并 PR/未知事件交给真实 `apply_feed`；Home 三项正常，Feed 只显示正常最小空态文案（`No starred repositories, new repositories, or merged pull requests in this Feed snapshot.`），没有占位卡。
- `examples/home.rs` 增加仅预览用的可选首场景参数（标签或 0-based 索引），不进入产品代码。

## 未覆盖

- 未对 `api.github.com` 发真实请求，未用真实 PAT 验证真实合并事件、Follow 关系或 Events 延迟；`action=merged` vs `closed+merged=true` 为官方文档静态核对，没有真实事件回放。原生预览使用合成 fixture，不代表真实账户数据。
- 原生帧是 `PrintWindow` 抓取，窗口宽度大于可见区域，右侧被裁切；未做真实鼠标/键盘/IME/多 DPI/触控板交互验收。验证边界仍遵循 [L002](../../LESSONS.md#l002模拟验证要写清边界)。
- Follow 两类仍待可靠来源；出现受支持的权威来源时先报告证据再扩大范围。

## 后续

- 审查反馈按 [githux-pr-review](../../.agents/skills/githux-pr-review/SKILL.md) 处理；任何一轮出现待裁决事项后只能由负责人或主 Agent 合并。
