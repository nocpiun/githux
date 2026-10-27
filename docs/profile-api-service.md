# Profile 数据适配服务（R4）

更新：2026-10-25。R4 Profile 的数据适配基础，位于 `src/domain/profile.rs` 和 `src/github/profile/**`。它提供用户资料、Pinned/Popular 来源与贡献热力图的只读契约，并为此补齐了只读 GraphQL POST 传输（`src/network/transport.rs`、`src/github/client.rs`）。尚未接入 data 层、账户上下文或 UI；页面与产品 fallback 不在本任务。

## 范围与边界

- 只做读取。没有任何 mutation 入口，也没有写队列；`HttpRequest` 只有 `Get`/`Post`，`Post` 仅用于 GraphQL query。
- 不修改 `src/data/**`、`src/auth/**`、`src/storage/**`、`src/content/**`，也不改其他 github 领域。后续接线需要按现有约定捕获授权代次并在发布前复查。
- 不实现个人 README 启发式、2020 旧仓库“分享到 Profile”标记、gist-only 的最终呈现或 Popular 排序策略；也不拉取 README 正文。这些仍待产品裁决。
- Contribution Activity 列表和 Profile 其他 Tabs 属首版排除项，未实现。

## 接口

| 函数 | 接口 | 返回 |
| --- | --- | --- |
| `fetch_profile(client, token, login, cancel)` | REST `GET /users/{login}` | `ProfileBasic` |
| `fetch_profile_pinned(client, token, login, limits, cancel)` | GraphQL `user.pinnedItems` + `user.itemShowcase` | `ProfilePinned` |
| `fetch_contribution_calendar(client, token, login, range, limits, cancel)` | GraphQL `user.contributionsCollection.contributionCalendar` | `ContributionCalendar` |

- `ProfileBasic`：稳定 `id`、`login`（仅显示，改名不改变身份）、显示名、bio、avatar、profileURL 及 company/location/website/followers/following/public_repos/public_gists/created_at 等公开字段。私有字段（需要 `read:user`）不在本适配器范围。
- `ContributionRange::new(from, to)`：`YYYY-MM-DD` 形式，校验真实日历日期（含闰年）、起止顺序，以及 `MAX_DAYS = 366` 的防御性跨度上限。非法输入返回 `ProfileError::InvalidInput`，不发请求。
- `ProfileLimits`：`response_bytes`（GraphQL JSON 接收上限，默认 8 MiB）、`pinned_items` / `showcase_items`（`first:` 预算，默认 6）、`max_weeks`（热力图周数上限，默认 60）。这些是工程上限，不是完整性保证。
- `login` 在进入 URL 路径或 GraphQL 变量前由 `validate_login` 校验（非空、无控制字符、无 `/`、`\`、`?`、`#`、空格）；REST 路径用与信任判定相同的 `reqwest::Url` 解析器逐段编码。

## Pinned / Popular 语义

| 来源 | GraphQL 字段 | 保留的事实 |
| --- | --- | --- |
| 仓库 Pinned | `user.pinnedItems(first:, types: [REPOSITORY])` | 只含仓库、保持用户顺序；`totalCount` 保留截断可能 |
| Popular 展示 | `user.itemShowcase { hasPinnedItems items(first:) }` | `hasPinnedItems` 统计**仓库或 gist**；`items` 是 `PinnableItem` 联合 |

`ProfilePinned.showcase` 用 `Showcase` 枚举保留服务端语义，而不是替 UI 决定：

- `PopularRepositories`：`hasPinnedItems=false`，返回的是 GitHub 的 Popular 回退仓库。
- `PinnedRepositories`：有 pin，且完整观察列表都是仓库。
- `PinnedWithGists { repositories, gists }`：完整观察列表下混合 pin；两侧都保留。
- `PinnedGistsOnly { gists }`：完整观察列表只有 gist pin。**不得据此宣布用户没有仓库**。
- `Truncated { repositories, gists }`：`itemShowcase.items` 是分页连接，`pageInfo.hasNextPage=true` 时只观察到前缀；**不用前缀决定 gist-only/混合策略**，保留已观察项供展示。
- `Unclassified`：未知 union 成员、解析漂移、字段或连接缺失，保持显式不确定，不当作空列表。

`ProfilePinned` 同时携带 `pinned_truncated` / `showcase_truncated`：前者在 `totalCount > repos.len()` 或 `pinnedItems.pageInfo.hasNextPage` 时为真，后者取 `itemShowcase.items.pageInfo.hasNextPage`。query 显式请求两处 `pageInfo`，否则截断不可见。

不按 stars 重排，不用“按 stars 排序”替代 GitHub Popular。`pinnedItems` 即使带 `types: [REPOSITORY]` 也按 `__typename` 校验，非 `Repository` 的节点不会被误当仓库。`ProfileRepo` 保留 `databaseId`、name/nameWithOwner、description、url、isPrivate、isFork、stargazer/fork 计数、primaryLanguage、owner login。

## GraphQL 错误与部分结果

GraphQL 的 `200` **不等于**全成功。`errors` 与 `data` 可并存，`errors` 会被还原为安全分类加查询路径；原始服务端 `message` 不保留（它可能回显调用方输入）。`ProfilePinned.errors` / `ContributionCalendar.errors` 承载随可用数据一起返回的错误，`ProfileError::Graphql { kind, path }` 用于没有可用数据的失败。

| 情形 | 分类 |
| --- | --- |
| `x-ratelimit-remaining: 0`（HTTP 200） | `RateLimitPrimary` |
| `errors[].type = RATE_LIMITED`（非 primary） | `RateLimitSecondary` |
| `errors[].type = NOT_FOUND` | `NotFound` |
| `errors[].type = UNAUTHORIZED` | `Unauthorized` |
| `errors[].type = FORBIDDEN` | `Forbidden` |
| `X-GitHub-SSO: required` | `SsoRequired { authorization_url }` |
| 其他未知 `type` | `Unknown`（丢弃 message） |

这与 REST 的语义一致：REST 主限流是 `403/429` + `x-ratelimit-remaining: 0`（由既有 `classify_status` 处理），GraphQL 主限流是 `200` + 同一响应头 + `errors`。非 2xx 仍走既有 `classify_status`（401/403/404/429/5xx），401 等不会变成空数据。`data.user` 为 `null` 且带 `NOT_FOUND` 时返回 `ProfileError::Graphql { NotFound }`，不是空 Pinned 列表。未知 union 成员被保留为 `Showcase::Unclassified`，未知 `contributionLevel` 字符串原样保留，未知 error `type` 归入 `Unknown`。

## 只读 GraphQL POST 传输

`HttpRequest` 增加 `Post` 方法与可选 `body`；`GitHubClient::build_post` 生成 `POST {base}graphql`，固定 `User-Agent`、`Accept: application/json`、`Content-Type: application/json`，并只在可信 origin 注入 `Authorization`。`Debug` 对手写实现增加 body 脱敏，只显示 `<redacted>`，避免变量中的资料进入日志。

安全规则：

- **有界**：GraphQL 走 `execute_bounded`，接收阶段按 `response_bytes` 限制；`Content-Length` 只作提前拒绝提示，实际 chunk 仍累计计数。超限是 `ProfileError::LimitExceeded`。
- **可取消、可超时**：复用既有 `CancellationToken` 与请求/body 阶段超时；已取消的请求不建连接。
- **仅可信 origin**：`build_post` 与 `build_get` 共用信任判定（scheme+host+port，`reqwest::Url` 语义）。
- **POST 不跟随重定向**：保守规则是任何 3xx 都不跟随（即使同源），直接把 3xx 交给上层；GraphQL 层返回 `ProfileError::RedirectNotFollowed`。这样不会把 query body 重新发往别处，也不会因 302/303 被降级成 GET。GET 的同源跳转行为保持不变。
- **不自动重放 mutation**：本层根本不提供 mutation 能力，传输也不重试。

`execute` / `execute_bounded` / blob 的既有签名与行为保持兼容；`MockTransport` 新增 `recorded_requests()` 以便测试断言 POST 的方法与 body，原 `recorded()` 不变。

## 官方依据

- REST 用户资料：[Get a user](https://docs.github.com/en/rest/users/users#get-a-user)（字段、公开/私有可见性）。
- GraphQL 对象：[User](https://docs.github.com/en/graphql/reference/objects#user)、[PinnableItem](https://docs.github.com/en/graphql/reference/unions#pinnableitem)、[PinnableItemShowcase](https://docs.github.com/en/graphql/reference/objects#pinnableitemshowcase)、[ContributionsCollection](https://docs.github.com/en/graphql/reference/objects#contributionscollection)、[ContributionCalendar](https://docs.github.com/en/graphql/reference/objects#contributioncalendar)。
- GraphQL 限流与查询上限：[Rate limits and query limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api)（`first` 1–100、点数、200 + errors 的主限流语义）。
- Profile 贡献与 README：[Profile contributions reference](https://docs.github.com/en/account-and-profile/reference/profile-contributions-reference)、[Managing your profile README](https://docs.github.com/en/account-and-profile/how-tos/profile-customization/managing-your-profile-readme)。
- 能力矩阵与 SSO/限流汇总见 [GitHub API 能力核验](github-api-capabilities.md)。

## 产品未决（本任务不决定）

- 个人 README 的发现启发式（同名公开仓库 + 根 README），以及 2020-07 前同名仓库需要手动“Share to profile”、API 不暴露该标记的遗留规则。
- gist-only 或混合 pin 时的最终呈现，以及 Popular 回退是否/如何展示。
- 热力图默认区间与刷新节奏（缓存策略仍为建议，未实现）。

## 验证范围

本地测试不访问 GitHub、不使用真实凭据。新增用例覆盖：

| 场景 | 位置 |
| --- | --- |
| REST 基本资料字段、404 分类、login 校验与路径编码 | `github::profile::basic::tests` |
| 固定 query+variables 的 JSON 编码、Authorization/Content-Type | `github::profile::graphql::tests::fixed_query_and_variables_are_encoded_as_json`、`github::client::tests::post_authorization_only_for_trusted_host` |
| 200 仅 errors、200 data+errors 部分结果、主限流头、POST redirect 拒绝、非 2xx、超限、取消 | `github::profile::graphql::tests` |
| 有界 POST、取消 POST 不建连接、同源/跨源 POST redirect 不跟随、POST 方法+body | `network::transport::tests` |
| pin 顺序、REPOSITORY 过滤、gist-only、混合、Popular 回退、未知 union、部分结果、未知登录 | `github::profile::pinned::tests` |
| Pinned/showcase 截断未被当成完整列表（`Truncated`）、`pinned_truncated`、`pageInfo` 请求 | `github::profile::pinned::tests::truncated_showcase_is_not_classified_as_gist_only`、`complete_showcase_without_next_page_is_classified`、`higher_total_count_marks_pins_truncated`、`pinned_query_requests_page_info` |
| 热力图字段、零贡献、未知 level、权限不足、限流、区间变量 | `github::profile::contributions::tests` |
| 区间格式/真实性/顺序/跨度校验 | `domain::profile::tests` |

本地全量 `cargo test` 通过（298 passed）。测试使用合成凭据与本地 mock/真实 localhost socket；未做真实 GitHub GraphQL 联调、真实私有贡献/SSO/TLS 验证。运行证据见[开发日志](devlog/2026-10-25-profile-api.md)。
