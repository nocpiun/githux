# Home 仅 open、Feed 三类活动（主语必须被关注）

- 2026-10-30，任务 `home-feed-focus`，执行者：基层实现 Agent。文件覆盖三轮：首轮信息筛选、第二轮真实数据更正、第三轮触底分页。
- 目标与状态：实现负责人 2026-10-30 的 Home/Feed 信息筛选与触底加载更旧活动，等待 PR 审查；PR 目标分支为 `feature/shell-pages`，保持 `pending`，不自行合并。

## 背景与决定

负责人 2026-10-30 确认：

1. Home 保持 `involves:<当前登录>`、无时间窗口、`updated` 降序，但**实际查询必须加 `is:open`**；最终可见条目只能是 open 的 Issue/PR，closed/merged/未知状态不得泄漏。不能只请求无限制页面再在本地隐藏 closed 行。
2. Feed 只保留明确允许的活动，去掉 Push 等技术噪声；**先实现三类，关注两类暂缓**（负责人回答）：
   - 合并的 PR：标题 `<user> contributed to <owner>/<repo>`，`<user>` 是**真实的 PR 作者**，不是点 merge 的维护者 actor；
   - 星标仓库：`WatchEvent action=started`；
   - 创建仓库：`CreateEvent ref_type=repository`。
3. **第二轮补充（负责人）**：三条卡片的主语**都必须是当前账户关注的人**；合并贡献的主语是 PR 作者（需可靠解析），星标/建库主语是事件 actor。关注关系与作者解析失败不得显示为完整空 Feed。
4. Follow 两类（“Followed someone / Followed me”）暂缓：当前官方 Events API 事件类型列表没有 `FollowEvent`，followers/following 端点只返回关系、没有事件时间戳（2026-10-30 核对 [GitHub event types](https://docs.github.com/en/rest/using-the-rest-api/github-event-types)、[followers](https://docs.github.com/en/rest/users/followers)）。本轮不实现、不占位、不伪造时间戳、不抓取私有网页端点。保存的确认见 `.agents/task/follow-decision.md`。

## 数据源更正（第二轮）

首轮实现有一个被真实公开数据推翻的假设，必须先记录下来：

- 首轮源码注释、`docs/github-api-capabilities.md` 与一条回归都断言 **`PullRequestEvent.action` 没有 `merged`**，只接受 `action=closed` + `pull_request.merged=true`，并把作者读自 payload 内嵌的 `pull_request.user`。
- 官方 [GitHub event types](https://docs.github.com/en/rest/using-the-rest-api/github-event-types?apiVersion=2022-11-28) 明确列出 `opened`、`closed`、**`merged`**、`reopened`、`assigned`、`unassigned`、`labeled`、`unlabeled`。
- 用无凭据公开端点（`X-GitHub-Api-Version: 2022-11-28`，与应用声明一致）核对 `GET /users/torvalds/received_events/public`：70 条事件里有 4 条 `action=merged`，其 `payload.pull_request` 只有 `url`、`id`、`number`、`head`、`base`——**既没有 `merged` 标记也没有 `user`**。
- 因此首轮判定在真实数据上等于“一条合并都收不到”，而合成 fixture 恰好按错误假设构造，测试全绿也发现不了。
- 修复：合并证据改为 `action == "merged"`（或旧形状 `closed` + `merged=true`），作者改读 `GET /repos/{owner}/{repo}/pulls/{number}` 的 `user`（公开核对示例：[subsurface/libdc#144](https://github.com/subsurface/libdc/pull/144) 返回 `merged: true` 与 `user.login`）。
- 教训写入 [L002](../../LESSONS.md#l002模拟验证要写清边界) 的补充证据，不新增重复条目。

同样地，协调者 2026-10-30T14:41:52Z 的公开探针发现 `received_events` 混合“关注的人”与“watch 的仓库”，13 条 `WatchEvent` 里有 8 条主语未被关注。初次派发没有写明三类活动统一的关注主语约束，这是派发缺口，不是负责人意图含糊。现在按补充指令显式读取 `/user/following`，优先按稳定 ID 过滤。

## 改动

- **Home 查询**（`src/github/home/search.rs`）：`q=involves:<login> is:open [updated:…]`，顺序、排序与分页不变；`state` 改为可选并在转换时归一化为 `open`，closed/merged/未知/缺失一律不返回。筛选不是截断，`readiness` 与 `total_count` 语义不变。
- **Feed 策略**（`src/domain/home.rs` + `src/domain/home/feed_policy_tests.rs`）：`build_feed` 是唯一策略点，输入是源事件、关注关系与作者解析结果。
  - `merge_candidate`：`action=merged` 或 `closed`+`merged=true`；`closed` 单独不算合并，也不触发详情读取。
  - `local_activity`：星标（`started`）与仓库创建（`ref_type=repository`）的主语是事件 actor。
  - 顺序：先取作者（内嵌优先，否则用 `AuthorResolutions` 命中的详情结果），再校验主语是否被关注。作者缺失/失败/详情未确认合并 ⇒ 扣留并计入 `AuthorUnresolved`；关注列表不完整且主语不在其中 ⇒ 扣留并计入 `FollowingIncomplete`；两者都不添加“过滤导致”的 Partial。
  - 真实事件 `id`、时间戳、仓库与源顺序原样保留（`FeedActivity { kind, subject, pull_request, event }`）。
- **关注关系适配器**（`src/github/home/following.rs`）：`GET /user/following`，完整跟随 `Link`（官方 next 写作 `/user/{id}/following`，只取页码并按 `/user/following` 重建），校验同源、拒绝循环/无效链接，页数/账户数/字节预算；`FollowingReadiness::Complete` 与 `Partial` 区分“明确未关注”和“未知”。
- **PR 作者适配器**（`src/github/home/pull_request.rs`）：`GET /repos/{owner}/{repo}/pulls/{number}`，owner/name/number 取自解析后的事件，URL 由受信 base 重建（拒绝 `.`/`..`、路径分隔符、`?`、`#`、`@`），从不请求 payload 的 `url`；编号不匹配视为不可用证据；无 `user` 返回 `Unattributable`，绝不回退到 merger。
- **数据层**（`src/data/page_reads.rs`）：`PageReadSession::feed` 只取第一源页并返回 `FeedOutcome.next_page` 游标；`feed_source_page` 按游标取一页，`feed_page` 完成“源页 + 关系 + 作者”并返回策略应用后的 `FeedPage`（按调用方传入的 `seen_ids` 跨页去重）。关注关系第一页失败即返回 `Err`（不是空 Feed）；作者详情失败只扣留该条。切换账户由既有 `is_current` 判为 `StaleContext`，没有跨账户缓存。
- **视图**（`src/ui/home/**`）：只渲染 `FeedOutcome.activities`；不再在 `apply_*` 里重算策略。合并卡片 header 是 `<author> contributed to <owner>/<repo>`，inset 放 PR 标题/#号；星标/建库卡片 inset 放仓库。空态与 Partial 文案区分“正常空”“关注关系不完整”“作者未解析”。选择/激活使用卡片来源事件的真实 id。第三轮新增 `LoadMoreFeed` 动作与 `apply_feed_page`/`feed_append_loading`/`feed_append_failed`：触底（或首屏不满一屏）单次发出，在途去重，`splice` 追加保留滚动锚点与键盘选择，失败告警 + 点击重试，真正耗尽停止。
- **host**（`src/app/pages/home.rs`）：传入 `FeedLimits`/`FollowingLimits`/`PullRequestDetailLimits` 与 `FEED_AUTHOR_LOOKUP_LIMIT = 30`（每次刷新/追加最多 30 次作者详情读取，按 PR 去重），并把 renderer 改为接收 credited subject；`LoadMoreFeed` 经独立 append 通道按游标追加，`apply_append` 合并 seen 集合、游标与活动，账户切换/刷新/延迟响应不会混代次。
- **fixture 与预览**（`examples/home.rs`、`src/ui/home/tests.rs`、`src/app/pages/tests.rs`、`src/data/page_read_tests.rs`）：全部按真实形状重建——瘦 `action=merged` + 详情端点作者、被关注/未被关注的星标与建库、关注列表分页与失败；Shell 端到端 transport 也提供 `/user/following` 与 `/pulls/{n}`。
- **文档**：`product.md`、`home-view.md`、`home-query-service.md`、`page-data-service.md`、`github-api-capabilities.md`、`roadmap.md`、`shell-page-integration.md` 同步；旧的“官方 action 没有 merged / 作者来自内嵌 payload”表述已删除。

## 第三轮：触底追加旧活动

负责人要求 Feed 滚到底部时自动取更旧的活动。实现要点：

- 源事件适配器新增 `fetch_received_events_page`（一次一页，返回经同源校验的 `FeedSourceCursor` 页码游标）；`feed` 只取第一页，`FeedOutcome.next_page` 暴露下一页。数据层 `feed_source_page`/`feed_page` 按游标逐页读，每页重读关注关系并按 PR 去重解析作者，再用 `build_feed_page` + 跨页 `seen_ids` 去重。
- 视图触底（或首屏不满一屏、无法滚动）发一次 `LoadMoreFeed`；每账户/代次至多一次在途追加，渲染/resize/重复回调/刷新都不能重复。`apply_feed_page` 用 `ListState::splice` 尾部增长，保留滚动锚点与键盘选择；重复/重叠页按源事件 id 去重。加载中显示紧凑动画图标，失败保留内容并显示黄色告警 + 点击重试，真正耗尽（无 next 链接）停止。
- 官方 Events 文档确认时间线只含**最近 300 条/30 天**事件，不是无限归档；逐页追加仍受同一源预算约束，不假装有更旧的历史。

回归：`feed_page_appends_without_duplicating_overlapping_events`、`a_filtered_empty_page_keeps_the_cursor_for_the_next_page`（域）；`filtered_empty_page_keeps_continuation_and_later_page_appends`、`append_deduplicates_overlap_and_failure_preserves_cursor`（数据层）；`feed_end_of_scroll_emits_one_load_more_per_append`、`feed_append_keeps_selection_and_scroll_anchor`、`feed_append_failure_shows_warning_and_retries`（视图）；`feed_end_of_scroll_appends_the_next_page_through_the_real_shell_host`（真实 Shell）。示例预览在 Mixed 场景提供第二页，可真实滚动并追加。

## 验证

- 串行 Cargo helper（`.agents/task/cargo-serialized.ps1`，共享 build mutex、独立 `target/`）：`fmt --all --check`、`check --locked --all-targets`、`clippy --locked --all-targets -- -D warnings`、`test --locked`、`build --locked` 全部通过（退出码 0），在整合当前 `feature/shell-pages` 基线（`1721eec`）之后运行。
- 最终检查使用独立日志与 JSON 保存每个 Cargo 退出码及耗时；此前编译阶段曾超过工具一小时超时，随后恢复，不把超时日志当作测试通过证据。第三轮完整检查：fmt/check/clippy `-D warnings`/test/build/example-build 全部退出 0；测试计数为主程序 857、account 857、home 84、profile 216、reader 187、reader_images 534、repository 608、repository_overview 39。
- 补充边界回归：`membership_uses_stable_ids_across_renames_and_login_reuse`、`refresh_reloads_membership_so_an_unfollowed_subject_disappears`、`switching_accounts_while_following_is_delayed_rejects_the_whole_feed`、`cancelled_delayed_following_does_not_publish_a_snapshot`、`author_budget_withholds_remaining_contributions_and_never_backfills`；视图在过滤后的最后一项按 End/Enter 仍激活真实源事件 `event-7`。
- 新增/更新的回归（详见 [查询服务验证表](../home-query-service.md#72-离线回归)）：
  - 真实形状：`parses_a_live_merged_event_with_a_slim_pull_request_object`、`a_live_merged_event_becomes_a_contribution_once_the_author_is_resolved`。
  - 归因方向：`a_followed_author_qualifies_even_when_the_merger_is_not_followed`（作者被关注、merger 未关注 ⇒ 显示）、`a_followed_merger_does_not_qualify_when_the_author_is_not_followed`（反向 ⇒ 不显示）。
  - 诚实失败：`a_failed_author_read_withholds_the_contribution_and_reports_partial`、`incomplete_following_read_withholds_unconfirmed_subjects`、`a_failed_following_read_is_an_error_not_an_empty_feed`、`an_unresolved_merged_event_is_withheld_and_reported_not_silently_dropped`。
  - 关注语义：`reads_the_authenticated_following_endpoint_with_paging_and_membership`、`a_later_page_failure_is_partial_not_a_negative_answer`、`a_first_page_failure_is_a_hard_error_not_an_empty_list`、`truncated_pagination_budgets_are_partial`、`untrusted_or_cycling_pagination_never_extends_the_set`、`entries_without_a_login_are_not_membership_evidence`、`empty_following_list_is_a_complete_negative_answer`。
  - 类别边界：`starred_and_created_repositories_require_a_followed_actor`、`watching_and_branch_or_tag_creation_never_produce_a_card`、`closed_events_never_qualify_without_merge_evidence`、`opened_and_reopened_pull_requests_are_never_contributions`。
  - 真实适配器 → 可见模型：`real_adapter_shapes_produce_only_followed_contribution_star_and_create_cards`（断言只有 3 张卡、顺序、作者归因，以及请求数恰为 login + events + following + 1 次详情）、`duplicate_merged_events_share_one_author_lookup`。
  - 视图：`the_visible_feed_is_the_policy_output_not_the_raw_event_list`（真实策略输出经真实组件渲染，断言无占位行、合并卡链接作者）、`a_policy_empty_feed_is_a_normal_minimal_state`、`an_incomplete_membership_read_shows_a_partial_notice_not_an_empty_feed`、`a_merged_pull_request_credits_the_author_not_the_merging_actor`。
  - 正式 Shell 端到端：`the_shell_mounts_real_home_and_reads_its_two_columns` 经真实 adapter/data/view 断言 open-only Home、被关注星标、合并贡献有行，Push 与未被关注账号的星标无行，且 `/user/following` 被请求。
- Home open-only 的旧回归（`closed_merged_and_unknown_home_states_never_become_rows_or_targets`、`a_refresh_that_returns_only_closed_items_clears_the_previous_open_snapshot`、`non_open_items_are_dropped_from_the_returned_list`）在合并基线后仍通过。
- 首轮那条“`action=merged` 必须被拒绝”的错误回归已删除，并以真实形状回归替代。

### 真实公开数据核对

无凭据、`X-GitHub-Api-Version: 2022-11-28`，只用于核对字段形状（不是应用认证响应，Feed 会随时间变化）：

| 观测 | 结果 |
| --- | --- |
| `GET /users/torvalds/received_events/public` | 70 条事件，4 条 `action=merged` |
| merged `payload` 键 | `action`、`number`、`pull_request` |
| merged `pull_request` 键 | `base`、`head`、`id`、`number`、`url`（无 `user`、无 `merged`） |
| `GET /repos/subsurface/libdc/pulls/144` | `merged: true`、`merged_at`、`title`、`user.login` |
| `GET /users/octocat/following?per_page=2&page=2` | next 链接为 `/user/583231/following?...` |
| `GET /user/following`（无凭据） | `401`（必须认证，符合数据层用法） |

聚合（脱敏、无凭据）保存在本地 `.agents/task/home-feed-public-verification-2026-10-30.json`，不进入仓库。

### 原生预览证据

本机 Windows 上构建并运行真实组件的独立预览，用 `PrintWindow` 抓取原生帧：

```sh
cargo build --locked --example home
powershell -File .agents/task/capture.ps1 -Scenario 0 -Out .agents/task/home-append-mixed2.png
powershell -File .agents/task/capture.ps1 -Scenario 3 -Out .agents/task/home-rework-empty.png
```

- 预览 fixture 现在按真实形状构造：合并卡片的事件是瘦 `action=merged`（无作者），作者由 `AuthorResolutions`（相当于详情端点结果）提供；主语 `synthetic-contributor` 被关注，merger `synthetic-user` 也被关注但不会被冒充为作者；星标与建库由 `synthetic-user` 完成。
- 2026-10-31 恢复工作后重新构建的两张完整 PrintWindow 帧均为 1792×1256 物理像素。Mixed 帧：Home 六项全部 `#N · open`；Feed 三张卡 —— 合并贡献（作者 `synthetic-contributor`）、星标仓库、创建仓库。没有 Push/issue/评论/Release/Fork/未知卡片。
- Filtered-out 帧：把 Push、issue、评论、Release、分支、未合并 PR、未知事件，以及**未被关注账号**的星标/建库与未被关注作者的合并全部交给真实 `build_feed`；Home 三项正常，Feed 只显示正常最小空态文案（`No activities from accounts you follow in this Feed snapshot.`），没有占位卡。
- `examples/home.rs` 保留仅预览用的可选首场景参数（标签或 0-based 索引），不进入产品代码。第三轮 Mixed 场景暴露第二源页，预览 host 的 `load_more` 在 `LoadMoreFeed` 到达时经 `window.defer` 追加（与真实 Shell host 同构）；抓取的 1792×1256 帧 `home-append-mixed2.png` 显示 5 张卡——前三张为第一页，第四、五张来自第二页追加（`synthetic-contributor contributed to synthetic-owner/older-repo` + `synthetic-user starred`），未被关注主语仍被排除，证明真实组件确实滚动并追加，而非只渲染首帧。

## 未覆盖

- **未用真实 PAT 跑过应用自身的 Feed**：私有仓库事件、SSO 组织、真实关注列表分页与认证形态下的合并事件仍未验证。上表只核对字段形状。原生预览与自动化测试都使用合成 fixture，不代表真实账户数据。
- 首轮原生帧右侧曾被裁切；第二轮约束预览工具栏宽度并启用 DPI 感知截图，新帧完整显示两栏和底部状态行。未做真人鼠标/键盘/IME/多 DPI/触控板交互验收。验证边界遵循 [L002](../../LESSONS.md#l002模拟验证要写清边界)。
- 关系每次刷新重读；作者解析仅在当前调用内按 PR 去重复用（并发 1，最多 30 个详情）。没有跨刷新/跨账户缓存。页面保留快照表示上次读取状态，不承诺实时察觉外部取关；下一次成功刷新会移除对应卡片。正式数据层不发条件验证器，意外 304 报错，不复活旧卡片；尚未实现离线/304 缓存重放。
- Follow 两类事件卡片仍待可靠来源；出现受支持的权威来源时先报告证据再扩大范围。

## 后续

- 审查反馈按 [githux-pr-review](../../.agents/skills/githux-pr-review/SKILL.md) 处理；任何一轮出现待裁决事项后只能由负责人或主 Agent 合并。
