# Home / Feed 查询服务（R4 低层适配）

更新：2026-10-30（第二轮更正）。本文记录 Home 左栏 `involves:` 查询与右栏 Feed 的**低层 GitHub API 适配**：查询参数、返回模型、关注关系与作者解析、数据来源限制与验证证据。

**范围事实：** 这是 `src/github/home/**` + `src/domain/home.rs` 的可取消、有界读取能力，**不是 Home UI**。低层与现有 `github` 模块一致：接收 `Pat` 与调用者显式提供的登录名，不做活动账户上下文校验——那属于 data 接线，不能把本层说成已经保护了 UI。

## 1. 复用的共享设施

| 设施 | 用途 |
| --- | --- |
| `github::client::GitHubClient` | 固定头、仅受信 host 注入 `Authorization`、`url_is_trusted` |
| `network::transport::Transport::execute_bounded` | 按 `response_bytes` 在接收阶段限制响应体 |
| `network::cancel::CancellationToken` | 全程可取消；取消是硬错误，不是部分结果 |
| `github::headers::{parse_link_next, parse_sso_header}` | RFC 8288 形状的 `Link` 与 `X-GitHub-SSO` |
| `github::error::{classify_status, classify_transport}` | 401/403/404/429/5xx/超时/断网/解析分类 |

没有修改 shared 网络层、`Cargo.toml`/`Cargo.lock`，也没有新增依赖。

## 2. Home：参数化 `involves:` 查询

入口：`github::home::search_involving(client, token, &HomeQuery, &HomeSearchLimits, cancel)`。

请求形状（负责人 2026-10-30 确认的 Home 规则）：

```text
GET /search/issues
    ?q=involves:<login> is:open[ updated:>=<from>][ updated:<=<to>]
    &sort=updated&order=desc
    &per_page=<n>&page=<n>
```

- **登录名由调用者显式给出**（`domain::home::Login`）。校验只允许 1–39 个 ASCII 字母数字与连字符、不以连字符开头/结尾，因此无法注入 `is:pr`、`review-requested:` 等额外限定符，也不会退化成 `@me`。查询串用 `Url::query_pairs_mut` 做百分号编码，限定符不能逃出 `q` 值。
- **`is:open` 固定出现在实际查询中**，而不是先请求无限制页面再在本地隐藏 closed 行。Home 最终可见条目只能是 open 的 Issue/PR；适配器在转换阶段再丢弃任何非 `open` 状态（closed/merged/未知/缺失），作为缓存、mock 或未来非合规数据路径的防御。
- **`updated` 时间窗口是可选参数**（`UpdatedWindow`，`from`/`to` 至少一个，格式为 UTC `YYYY-MM-DD`）。调用者不传就**没有** `updated:` 限定符；本层不定义产品默认窗口。
- **固定参与语义**：查询始终以 `involves:` 开头，`involves:` 覆盖 author/assignee/mentions/commenter 的逻辑或（官方搜索限定符）。本层不扩大到 review-requested，也不把它当作浏览历史或未读状态。
- **Issue/PR 区分**：搜索响应用 `pull_request` 对象是否存在区分，落到 `HomeItemKind::{Issue, PullRequest}`。`id` 是稳定的 issue/PR id，`number` 是仓库内编号。
- **返回字段**：`id`、`kind`、`number`、`title`、`html_url`、`state`、`updated_at`、`repository{owner,name}`（从 `repository_url` 解析，绝不请求该 URL）。只读，不含编辑。`state` 反序列化为可选并在转换时归一化为 `open`；缺失、空串或未知值一律不作为 open 行返回。

### 2.1 分页、上限与完整性

- 保留服务端的 `total_count` 与 `incomplete_results`。
- 跨页按稳定 `id` 去重。
- **`HomeReadiness::Complete` 只表示“未检测到截断”，不表示无损同步。** `sort=updated` 是动态排序，条目在翻页之间会移动，可能重复或漏项；调用方必须按 id 去重，不能宣称完整集合。
- 搜索端点最多返回 **1,000** 条结果，这是服务端硬上限。`total_count > 1000` 时标记 `ResultLimit`。

| 情形 | `HomeReadiness` |
| --- | --- |
| 所有页成功、无 `next`、`total_count ≤ 1000`、`incomplete_results=false` | `Complete` |
| 第一页失败（401/403/404/限流/传输/解析） | `Err(HomeError)`，不返回空列表 |
| 后续页失败（HTTP/传输/解析） | `Partial { PageFailed / ParseFailed }`，保留已取条目 |
| 本地页预算 `max_pages` 用尽且仍有 `next` | `Partial { PageLimit }` |
| 本地条目预算 `max_items` 截断 | `Partial { EntryLimit }` |
| `total_count > 1000` | `Partial { ResultLimit }` |
| `incomplete_results=true` | `Partial { IncompleteResults }` |
| `Link` 指向不受信 host / 非本端点 / 无 `page` | `Partial { UntrustedPagination / InvalidPagination }` |
| `Link` 页码不前进 | `Partial { PaginationCycle }` |
| 后续响应体超预算 | `Partial { ResponseLimit }` |
| `X-GitHub-SSO: partial-results` | `Partial { SsoPartial }` |
| 任意阶段取消 | `Err(GitHubError::Cancelled)`（硬错误） |

`Link: rel="next"` 只用于确认“还有下一页”并取页码：目标必须同源、路径为本搜索端点、`page` 必须严格大于当前页；请求 URL 由**本层用调用者自己的查询身份重建**，因此恶意或过期的链接不能替换搜索表达式，也不能造成循环。

## 3. Feed：事件流、关注关系与贡献作者

Feed 由三段有界读取组成，**策略只在 `domain::home::build_feed` 里应用一次**：

| 读取 | 入口 | 端点 |
| --- | --- | --- |
| 1. 源事件 | `github::home::fetch_received_events` | `GET /users/<login>/received_events` |
| 2. 关注关系 | `github::home::fetch_following` | `GET /user/following` |
| 3. 贡献作者 | `github::home::fetch_pull_request_detail` | `GET /repos/{owner}/{repo}/pulls/{number}` |

账户作用域的编排与预算在 `data::PageReadSession::feed`；它先读源事件，再读关注关系，然后为**尚无作者的合并 PR**做去重后的详情读取，最后调用 `build_feed`。

### 3.1 源事件（`received_events`）

```text
GET /users/<login>/received_events?per_page=<n>&page=<n>
If-None-Match: "<etag>"        # 仅第一页，且调用者显式提供时
```

- 保留 `actor{id,login,display_login,avatar_url}`、`repo{id,name,url}`、`created_at`、`id`、`type`、`public` 与最小 typed payload；**未知 type 不 panic**，`FeedEventType::Unknown(name)` + `FeedPayload::Unknown`，不保留整个 payload。
- 适配器**发布源列表**：筛选、作者解析与关注关系过滤都在 `build_feed`。首次加载与刷新都重读源事件、关系与作者，再应用策略。正式数据层不发送条件验证器；意外 304 返回错误，不能复活旧卡片。低层适配器仍独立支持 304，但尚无可发布的缓存重放路径。
- **数据源语义**：`received_events` 合并“关注的人”和“watch 的仓库”产生的事件，**不等价于只含关注用户的 Feed**。因此关注关系必须显式过滤，不能假定事件 actor 一定被关注。
- 调度信息：返回 `etag` 与 `poll_interval_seconds`（`X-Poll-Interval`）；`304` 返回 `FeedFetch::NotModified`，不是错误。分页未完整（`Partial`）时不返回可复用 ETag，避免把部分结果 `304` 成“完整”。
- 本层不自己轮询、不使用网页 Cookie、不调用内部端点。
- 数据来源限制：Events 时间线只含**最近 30 天、最多 300 条**，可能延迟 30 秒–6 小时；`Complete` 只表示本次分页未截断。

### 3.2 关注关系（`/user/following`）

- 读**已认证账户本人**的列表（`/user/following`），因此结论与 Feed 判定使用的是同一个关系。
- 完整跟随 `Link: rel="next"`（官方 next 链接写作 `/user/{id}/following`，本层只取页码、用 `/user/following` 重建请求），并有页数/账户数/响应字节预算。
- **完整性是证据的一部分**：
  - `FollowingReadiness::Complete` 且列表不含某登录 ⇒ 明确“未关注”；
  - `Partial`（后续页失败、预算截断、SSO、分页异常）或整体 `Err` ⇒ 缺失的登录属于**未知**，绝不能当作“未关注”，也不能当作“关注”；
  - 只有 `Complete` 的空列表才表示“谁都没关注”。
- 卡片主语优先按稳定数字 ID 匹配，避免用户改名或旧登录被重用时串身份；主语没有 ID 时才按登录名做 ASCII 大小写不敏感回退。缺有效 ID/login 的关系条目标记为 Partial。

### 3.3 贡献作者（pull request 详情）

- 官方 `PullRequestEvent.action` **包含 `merged`**（[GitHub event types](https://docs.github.com/en/rest/using-the-rest-api/github-event-types)）。线上 `action=merged` 的 `payload.pull_request` 是瘦对象，只有 `url`、`id`、`number`、`head`、`base`，**没有 `merged` 标记也没有 `user`**（2026-10-30 公开响应核对，见下）。
- 因此合并证据为 `action == "merged"`，或旧形状的 `action == "closed"` 且 `pull_request.merged == true`；`closed` 单独不算合并。
- 作者必须来自 `GET /repos/{owner}/{repo}/pulls/{number}` 的 `user` 字段。事件里若内嵌了 `user`（另一种来源），可直接使用，不必再读详情。
- **绝不把点了 merge 的 actor 当作者**。作者缺失、详情读取失败或详情未确认 `merged: true` 时，该贡献不显示，并计入 `FeedPartialReason::AuthorUnresolved`。
- 安全：仓库 owner/name 与编号取自解析后的事件，URL 由本层用受信 base 重建（`.`/`..`、路径分隔符、`?`、`#`、`@` 一律拒绝）；**从不请求 payload 里的 `url`**，因此不可能把凭据发往任意主机（见 [L007](../LESSONS.md#l007安全判定与真正的连接方必须用同一个-url-解析器)）。编号不匹配请求值时视为不可用证据。

### 3.4 产品策略（`build_feed`）

只保留三类，且**每类的主语必须是当前账户关注的人**：

| 活动 | 判定 | 主语 |
| --- | --- | --- |
| 合并的 PR 贡献 | 3.3 的合并证据 + 可靠作者 | PR 作者（不是 merge actor） |
| 星标仓库 | `WatchEvent` 且 `action=started` | 事件 actor |
| 创建仓库 | `CreateEvent` 且 `ref_type=repository` | 事件 actor |

- Push、issue、评论、Release、Fork、成员、公开仓库、分支/标签创建删除、watch 订阅（`stopped`）、未知类型、非合并 PR 都**不生成卡片、占位卡或回退句**。
- 顺序、真实事件 `id`、时间戳与仓库身份原样保留，供选择与导航继续使用。
- **过滤不是截断**：整页都是噪声或都不满足关注关系时，`FeedReadiness` 保持源读取的 readiness；源本身完整就是 `Complete` 空 Feed。关注关系不完整或作者未解析时追加对应 `FeedPartialReason`，让 UI 明确“不是完整的空结果”，而不是伪装成“没有活动”。
- 预算：关注列表默认最多 1,000 个账户/10 页；单次刷新最多 30 次作者详情读取（`src/app/pages/home.rs` 的 `FEED_AUTHOR_LOOKUP_LIMIT`），按 PR 身份去重，作者读取并发为 1，解析结果仅在本次调用内复用。每次刷新重新读关注关系，不缓存长期成员资格。超出预算的目标记为未解析并在 Partial 原因里计数，**不会为了填满页面无限追加请求**。

### 3.5 Follow 事件两类暂缓（负责人 2026-10-30 确认）

当前官方 Events API 的事件类型列表没有 `FollowEvent`；followers/following 端点只返回当前关系、没有事件时间戳。负责人回答“先实现三类，关注两类暂缓”：本轮不实现 “Followed someone” / “Followed me” 卡片，不生成合成近期事件、不抓取私有网页端点/Cookie、不加监控后端、不伪造发生时间。

**暂缓的是那两类事件卡片，不包括读取关注关系本身**：上面 3.2 的关注列表读取是实现“主语必须被关注”约束的必要条件，与本条无关。

`FeedReadiness::Partial` 的 `PageFailed`/`PageLimit`/`EntryLimit`/`ResponseLimit`/`ParseFailed`/`UntrustedPagination`/`InvalidPagination`/`PaginationCycle`/`SsoPartial` 与 Home 同义，另有 `FollowingIncomplete { details }` 与 `AuthorUnresolved { count }`。

## 4. 限制与错误

| 限制 | 默认 | 说明 |
| --- | --- | --- |
| `HomeSearchLimits::max_pages` / `max_items` | 10 / 1000 | 搜索服务上限对齐 |
| `HomeSearchLimits::per_page` | 100 | 端点上限 |
| `HomeSearchLimits::response_bytes` | 4 MiB | `execute_bounded` 接收期限制 |
| `FeedLimits::max_pages` / `max_events` | 3 / 300 | 对齐 Events 窗口 |
| `FeedLimits::per_page` | 100 | 端点上限 |
| `FeedLimits::response_bytes` | 4 MiB | 接收期限制 |
| `FollowingLimits::max_pages` / `max_accounts` | 10 / 1000 | 关注关系的防御预算 |
| `FollowingLimits::per_page` / `response_bytes` | 100 / 4 MiB | 端点上限与接收期限制 |
| `PullRequestDetailLimits::response_bytes` | 1 MiB | 单条 PR 详情的接收期限制 |
| 单次刷新作者详情读取数 | 30 | `FEED_AUTHOR_LOOKUP_LIMIT`，按 PR 去重 |

`per_page=0`、超过端点上限、`max_pages=0`、条目预算为 0 会在发请求前返回 `HomeError::InvalidInput`，不消耗网络。

空结果（`total_count=0` / 空数组）是 `Complete` 空列表，不是错误；权限不足（401/403）、限流（403+`X-RateLimit-Remaining: 0` 或 429）、SSO、404、5xx、超时、断网、解析失败分别由 `HomeError::GitHub(GitHubError)` 表达，不会伪装成“没有活动”。

## 5. 安全边界

- 只接收 `Pat`，不读取 `gh`/环境/日志里的凭据；`Pat` 无 `Display`/`Serialize`，`Debug` 恒为脱敏。
- 事件/搜索 payload 里的 URL 只作为字符串保存，**绝不请求**。
- 分页链接必须同源且重建查询身份；跨源重定向由共享 transport 拒绝，凭据不会转发。
- 自动测试只用合成登录名（如 `test-user`）与 mock/localhost，不含真实用户。

## 6. 仍待决定（产品，不在本层）

- Home 是否设时间窗口、窗口多长；是否额外包含“仅被请求评审”等关系。左栏刷新间隔仍属 host 调度，本层不定义。
- Feed 的后续自动刷新间隔（初始只有手动刷新）、离线旧列表呈现；Follow 两类事件卡片的可靠来源（当前暂缓，见 3.5）。关注关系读取本身不是暂缓项。
- 低层如何接入 data 层：活动账户上下文、登录名来源、`StaleContext` 校验、在途合并与缓存；这些属于 data/UI 接线，`PageReadSession` 只提供账户作用域 facade。

## 7. 验证

全部离线测试使用合成 login（如 `test-user`）、mock transport 与本地 socket，不访问 GitHub、不使用真实凭据。真实公开响应只用于核对字段形状，不进入测试依赖。

### 7.1 真实公开数据核对（2026-10-30，无凭据）

用 `X-GitHub-Api-Version: 2022-11-28`（与应用声明的版本一致）请求公开端点；这是**公开响应形状**，不是应用认证/私有响应，Feed 也会随时间变化：

| 观测项 | 结果 |
| --- | --- |
| `GET /users/torvalds/received_events/public` | 70 条事件，其中 4 条 `PullRequestEvent` 且 `action=merged` |
| 合并事件的 `payload` 键 | `action`、`number`、`pull_request` |
| 合并事件的 `pull_request` 键 | `base`、`head`、`id`、`number`、`url` —— **没有 `user`，也没有 `merged` 标记** |
| `GET /repos/subsurface/libdc/pulls/144` | `merged: true`、`merged_at`、`title`、`user.login` 齐备 |
| `GET /users/octocat/following?per_page=2&page=2` | 官方 next 链接为 `/user/583231/following?...` |
| `GET /user/following`（无凭据） | `401` —— 该端点必须认证，符合数据层捕获账户凭据的用法 |

原始聚合（已脱敏、无凭据）保存在本地 `.agents/task/home-feed-public-verification-2026-10-30.json`，不进入仓库。

### 7.2 离线回归

| 场景 | 测试 |
| --- | --- |
| 查询转义、固定 `involves:` + `is:open` 语义、无默认窗口 | `query_is_encoded_and_fixed_to_involves`、`updated_window_is_explicit_and_optional`、`the_open_filter_is_not_an_unrestricted_limit_page_hidden_afterwards` |
| 非 open 状态不返回 | `non_open_items_are_dropped_from_the_returned_list` |
| issue/PR 区分、跨页去重 | `distinguishes_issue_and_pull_request`、`deduplicates_items_across_pages`、`deduplicates_events_across_pages` |
| 源事件适配器发布源列表（不自行过滤） | `parses_common_event_types`、`a_source_page_keeps_every_event_in_server_order_and_identity` |
| **真实瘦 `action=merged` 形状** | `parses_a_live_merged_event_with_a_slim_pull_request_object`（适配器）、`a_live_merged_event_becomes_a_contribution_once_the_author_is_resolved`（策略） |
| 合并证据：`closed` 单独不算、详情未确认不算 | `closed_events_never_qualify_without_merge_evidence`、`a_detail_response_that_does_not_confirm_the_merge_is_not_merge_evidence` |
| **作者归因**：作者被关注且 merge actor 未被关注 ⇒ 显示；反向 ⇒ 不显示 | `a_followed_author_qualifies_even_when_the_merger_is_not_followed`、`a_followed_merger_does_not_qualify_when_the_author_is_not_followed` |
| 作者无法解析 ⇒ 不显示且报 Partial | `an_unresolved_merged_event_is_withheld_and_reported_not_silently_dropped`、`a_failed_author_read_withholds_the_contribution_and_reports_partial` |
| 星标/建库主语必须被关注 | `starred_and_created_repositories_require_a_followed_actor` |
| watch 订阅、分支/标签创建、删除不生成卡片 | `watching_and_branch_or_tag_creation_never_produce_a_card` |
| 关注关系分页/预算/失败/循环 | `reads_the_authenticated_following_endpoint_with_paging_and_membership`、`a_later_page_failure_is_partial_not_a_negative_answer`、`a_first_page_failure_is_a_hard_error_not_an_empty_list`、`truncated_pagination_budgets_are_partial`、`untrusted_or_cycling_pagination_never_extends_the_set`、`entries_without_a_login_are_not_membership_evidence` |
| 关注关系不完整 ⇒ 未知主语被扣留 | `an_incomplete_following_list_withholds_unread_subjects_and_reports_partial`、`incomplete_following_read_withholds_unconfirmed_subjects` |
| 空关注列表才是“谁都没关注” | `empty_following_list_is_a_complete_negative_answer` |
| 真实详情端点解析/编号不匹配/不可归因 | `resolves_the_author_from_the_documented_detail_endpoint`、`a_mismatched_number_is_not_usable_evidence`、`a_detail_without_a_user_is_unattributable_not_a_merger_fallback` |
| 从不请求 payload URL | `the_payload_url_is_never_used_for_the_request` |
| 端到端真实形状 → 可见模型 | `real_adapter_shapes_produce_only_followed_contribution_star_and_create_cards`（断言请求数 = login + events + following + 1 详情） |
| 重复合并事件只读一次详情 | `duplicate_merged_events_share_one_author_lookup` |
| 304 / `X-Poll-Interval` / `If-None-Match` | `not_modified_returns_etag_and_poll_interval`、`conditional_request_sends_if_none_match`、`localhost_304_round_trip_returns_not_modified` |
| 取消、输入校验 | `cancellation_is_a_hard_error`（各适配器）、`invalid_logins_are_rejected`、`invalid_dates_are_rejected`、`invalid_limits_are_rejected_without_network` |

执行：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked`，全部通过（退出码 0）。

**验证边界：** 没有用真实 PAT 跑过应用自身的 Feed，因此私有仓库事件、SSO 组织、真实关注列表与真实合并事件在认证形态下的表现仍未验证。7.1 只核对了字段形状；`action` 取值与合并/作者语义依据官方文档与公开响应，没有回放真实用户时间线。

## 8. 官方来源

- [Search issues and pull requests](https://docs.github.com/en/rest/search/search#search-issues-and-pull-requests)（`q`/`sort`/`order`/`per_page`/`page`、`total_count`、`incomplete_results`、1,000 条上限、4,000 仓库扫描上限）
- [Searching issues and pull requests](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests)（`involves:` 语义、`updated:` 日期语法）
- [Events](https://docs.github.com/en/rest/activity/events#list-events-received-by-the-authenticated-user)（`GET /users/{username}/received_events`、300 条/30 天、`ETag`、`X-Poll-Interval`、`304`）
- [GitHub event types](https://docs.github.com/en/rest/using-the-rest-api/github-event-types)（payload schema；`PullRequestEvent.action` 明确列出 `opened`、`closed`、`merged`、`reopened`、`assigned`、`unassigned`、`labeled`、`unlabeled`）
- [List the people the authenticated user follows](https://docs.github.com/en/rest/users/followers#list-the-people-the-authenticated-user-follows)（`GET /user/following`，需要认证）
- [Get a pull request](https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request)（`user`、`merged`、`merged_at`、`title`、`html_url`）
- 字段与状态码同时对照 `github/rest-api-description` 的 `api.github.com.2022-11-28` 描述。
