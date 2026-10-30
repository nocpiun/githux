# Home / Feed 查询服务（R4 低层适配）

更新：2026-10-30。本文记录 Home 左栏 `involves:` 查询与右栏 Feed 事件流的**低层 GitHub API 适配**：查询参数、返回模型、数据来源限制与验证证据。

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

## 3. Feed：接收事件流与保留活动

入口：`github::home::fetch_received_events(client, token, &Login, if_none_match, &FeedLimits, cancel)`。

请求形状：

```text
GET /users/<login>/received_events?per_page=<n>&page=<n>
If-None-Match: "<etag>"        # 仅第一页，且调用者显式提供时
```

- **保留** `actor{id,login,display_login,avatar_url}`、`repo{id,name,url}`、`created_at`、`id`、`type`、`public`。
- **只发布产品允许的活动**。`domain::home::classify_feed_activity` 是唯一的策略函数，适配器、账户数据 facade、fixture 与视图都复用它，因此不存在“某条路径未过滤”。保留三类：
  - **合并的 PR**：`PullRequestEvent`，官方语义为 `action=closed` 且 `pull_request.merged=true`；不存在 `action=merged`，仅 `closed` 不算合并。标题取 `pull_request.user`（真实 PR 作者）而不是 event actor，`<user> contributed to <owner>/<repo>`；缺少可靠作者证据时整条不发布，不把维护者 actor 冒充作者。作者字段来自同一 payload，不额外请求 PR 详情。
  - **星标仓库**：`WatchEvent` 且 `action=started`；仓库 watch 订阅、`stopped` 均不保留。
  - **创建仓库**：`CreateEvent` 且 `ref_type=repository`；分支/标签创建不保留。
- **解析仍对未知形状宽容**：常见事件仍按官方 schema 抽取少量字段；未知 type 不 panic，`FeedEventType::Unknown(name)` + `FeedPayload::Unknown` 且**不保留整个 payload**，避免未来形状无限增长。解析成功不等于会发布，发布与否只由 `classify_feed_activity` 决定。
- **过滤不是截断也不是错误**：Push、issue、评论、Release、Fork、成员、公开仓库、分支/标签删除、未知类型与非合并 PR 都不生成卡片、占位卡或回退句；整页都是噪声时 `FeedReadiness` 仍为 `Complete`，只是保留列表为空。`FeedLimits::max_events` 统计的是**解析到的事件**，过滤不会触发额外分页或请求来填满页面。
- **保留顺序与身份**：保留的活动维持服务返回顺序、真实 `id` 与仓库身份，供选择/导航继续使用；跨页按字符串 `id` 去重。
- **调度信息**：返回 `etag` 与 `poll_interval_seconds`（来自 `X-Poll-Interval`）。条件请求命中返回 `FeedFetch::NotModified { etag, poll_interval_seconds }`，与 `Events(...)` 区分，不当作错误。`304` 不消耗主限额。
- **部分结果不返回 ETag**：分页未完整时（`FeedReadiness::Partial`）`etag` 置空。否则调用方把部分结果的验证器传回会得到第 1 页 `304`，永远不重试缺失页；`poll_interval_seconds` 仍保留。
- **本层不自己轮询**：没有后台定时器、没有网页 Cookie、没有内部端点；调用方按 `X-Poll-Interval` 自行调度。
- **数据来源限制**：Events 时间线只含**最近 30 天、最多 300 条**事件，且可能延迟 30 秒–6 小时。`Complete` 只表示本次分页未截断，绝不代表完整历史或与网站 Following feed 逐条一致。

### 3.1 Follow 两类暂缓（负责人 2026-10-30 确认）

当前官方 Events API 的事件类型列表没有 `FollowEvent`；followers/following 端点只返回当前关系、没有事件时间戳（2026-10-30 核对官方文档）。负责人回答“先实现三类，关注两类暂缓”：本轮不实现 “Followed someone” / “Followed me” 卡片，不生成合成近期事件、不抓取私有网页端点/Cookie、不加监控后端、不伪造发生时间。保留为待可靠来源的后续需求；若将来找到受支持的权威来源，先报告证据再扩大范围。

`FeedReadiness::Partial` 的 `PageFailed`/`PageLimit`/`EntryLimit`/`ResponseLimit`/`ParseFailed`/`UntrustedPagination`/`InvalidPagination`/`PaginationCycle`/`SsoPartial` 与 Home 同义；跨页按字符串 `id` 去重。

## 4. 限制与错误

| 限制 | 默认 | 说明 |
| --- | --- | --- |
| `HomeSearchLimits::max_pages` / `max_items` | 10 / 1000 | 搜索服务上限对齐 |
| `HomeSearchLimits::per_page` | 100 | 端点上限 |
| `HomeSearchLimits::response_bytes` | 4 MiB | `execute_bounded` 接收期限制 |
| `FeedLimits::max_pages` / `max_events` | 3 / 300 | 对齐 Events 窗口 |
| `FeedLimits::per_page` | 100 | 端点上限 |
| `FeedLimits::response_bytes` | 4 MiB | 接收期限制 |

`per_page=0`、超过端点上限、`max_pages=0`、条目预算为 0 会在发请求前返回 `HomeError::InvalidInput`，不消耗网络。

空结果（`total_count=0` / 空数组）是 `Complete` 空列表，不是错误；权限不足（401/403）、限流（403+`X-RateLimit-Remaining: 0` 或 429）、SSO、404、5xx、超时、断网、解析失败分别由 `HomeError::GitHub(GitHubError)` 表达，不会伪装成“没有活动”。

## 5. 安全边界

- 只接收 `Pat`，不读取 `gh`/环境/日志里的凭据；`Pat` 无 `Display`/`Serialize`，`Debug` 恒为脱敏。
- 事件/搜索 payload 里的 URL 只作为字符串保存，**绝不请求**。
- 分页链接必须同源且重建查询身份；跨源重定向由共享 transport 拒绝，凭据不会转发。
- 自动测试只用合成登录名（如 `test-user`）与 mock/localhost，不含真实用户。

## 6. 仍待决定（产品，不在本层）

- Home 是否设时间窗口、窗口多长；是否额外包含“仅被请求评审”等关系。左栏刷新间隔仍属 host 调度，本层不定义。
- Feed 的后续自动刷新间隔（初始只有手动刷新）、离线旧列表呈现；Follow 两类的可靠来源（当前暂缓，见 3.1）。
- 低层如何接入 data 层：活动账户上下文、登录名来源、`StaleContext` 校验、在途合并与缓存；这些属于 data/UI 接线，`PageReadSession` 只提供账户作用域 facade。

## 7. 验证

全部本地测试，不访问 GitHub、不使用真实用户凭据。`src/github/home/search/tests.rs` 与 `src/github/home/events/tests.rs` 覆盖：

| 场景 | 测试 |
| --- | --- |
| 查询转义、固定 `involves:` + `is:open` 语义、无默认窗口 | `query_is_encoded_and_fixed_to_involves`、`updated_window_is_explicit_and_optional`、`the_open_filter_is_not_an_unrestricted_limit_page_hidden_afterwards` |
| 非 open 状态不返回 | `non_open_items_are_dropped_from_the_returned_list` |
| issue/PR 区分 | `distinguishes_issue_and_pull_request` |
| 跨页去重 | `deduplicates_items_across_pages`、`deduplicates_events_across_pages` |
| `incomplete_results` / 服务 1000 上限 / 本地页预算 | `incomplete_results_marks_partial`、`service_result_limit_marks_partial`、`local_page_limit_marks_partial` |
| 后续页失败保留已取数据 | `later_page_failure_keeps_earlier_items` |
| 不受信链接 / 循环 / 不改查询身份 | `untrusted_next_link_is_partial`、`pagination_cycle_is_partial`、`next_link_cannot_change_the_query_identity` |
| 空结果不是错误 | `empty_result_is_complete_not_error`、`empty_feed_is_complete_not_error` |
| 权限 / 限流 | `unauthorized_is_a_hard_error`、`rate_limited_is_a_hard_error`、`forbidden_is_distinct_from_unauthorized` |
| 未知事件类型、缺 payload 不 panic | `unknown_event_type_is_compatible_and_keeps_no_payload`、`missing_payload_does_not_panic` |
| 保留活动顺序/身份、PR 作者归因 | `retained_activities_keep_source_order_and_identity` |
| 错误 action/ref_type、缺合并证据、全噪声页仍 Complete | `wrong_actions_ref_types_and_missing_merge_evidence_are_excluded`、`an_all_filtered_feed_stays_complete_and_empty` |
| 过滤不触发额外请求 | `filtered_activities_do_not_trigger_extra_fetching_to_fill_the_page` |
| 304 / `X-Poll-Interval` / `If-None-Match` | `not_modified_returns_etag_and_poll_interval`、`conditional_request_sends_if_none_match`、`localhost_304_round_trip_returns_not_modified`（真实 localhost socket） |
| 取消 | `cancellation_is_a_hard_error`（两个适配器） |
| 输入校验 | `invalid_logins_are_rejected`、`invalid_dates_are_rejected`、`invalid_limits_are_rejected_without_network` |

执行：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`cargo build --locked`，全部通过。

**验证边界：** 没有对 `api.github.com` 发过真实请求，没有真实 PAT 联调；真实服务端在私有仓库、SSO、限流、事件延迟下的实际形状仍未验证。`action=merged` 与 `closed+merged=true` 的官方语义依据 [GitHub event types](https://docs.github.com/en/rest/using-the-rest-api/github-event-types) 静态核对（该文档的 `PullRequestEvent.action` 不含 `merged`，合并以 `closed` + `pull_request.merged` 表达），没有对真实合并事件回放。真实 GUI、账户上下文与离线缓存不在本任务范围。

## 8. 官方来源

- [Search issues and pull requests](https://docs.github.com/en/rest/search/search#search-issues-and-pull-requests)（`q`/`sort`/`order`/`per_page`/`page`、`total_count`、`incomplete_results`、1,000 条上限、4,000 仓库扫描上限）
- [Searching issues and pull requests](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests)（`involves:` 语义、`updated:` 日期语法）
- [Events](https://docs.github.com/en/rest/activity/events#list-events-received-by-the-authenticated-user)（`GET /users/{username}/received_events`、300 条/30 天、`ETag`、`X-Poll-Interval`、`304`）
- [GitHub event types](https://docs.github.com/en/rest/using-the-rest-api/github-event-types)（payload schema）
- 字段与状态码同时对照 `github/rest-api-description` 的 `api.github.com.2022-11-28` 描述。
