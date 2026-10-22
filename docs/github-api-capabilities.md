# GitHub API 能力核验（首版 R2 输入）

核验日期：2026-10-22（Asia/Shanghai）。范围来自 [产品约定](product.md#首版范围) 与 [路线图 R2](roadmap.md#r2认证与最小数据通路)。

**本文区分 API 研究与已确认产品决定。** 2026-10-22 负责人已选 PAT 登录、`/user/repos` 可见集合及 Home/Feed 的标题和方向，权威约定见 [产品文档](product.md)。HTTP 库、运行时、数据库、私有缓存政策及下文标明的剩余细节仍未定；研究矩阵不是已实现或已验证的业务能力。

## 结论速览

| 能力域 | 结论 | 常用入口 | 主要限制 |
| --- | --- | --- | --- |
| 仓库集合（本人/协作者/组织） | 可实现 | `GET /user/repos`、GraphQL `viewer.repositories` | 细粒度 PAT 一次只能覆盖一个 resource owner；组织策略与 SSO 会让结果“少而不报错” |
| namespace 头像 | 可实现 | 仓库响应里的 `owner.avatar_url` | 同一 URL 也可能换图，需按响应缓存头验证 |
| 仓库资料、Stars/Forks | 可实现 | `GET /repos/{owner}/{repo}` | 改名会 301；私有资源无权限时返回 404 而非 403 |
| 分支与具体 commit | 可实现 | `GET …/branches`、`…/commits/{ref}`、GraphQL `defaultBranchRef.target` | 空仓库/不可用仓库返回 409；commit 列表与详情字段语义不同 |
| 树、文件、README、原始内容 | 可实现（有大小上限） | `…/git/trees/{sha}`、`…/git/blobs/{sha}`、`…/contents/{path}`、`…/readme` | 目录列表 1,000 文件上限、递归树 100,000 项/7 MB、blob 100 MB 上限 |
| Releases / Contributors / Languages | 可实现 | `…/releases`、`…/contributors`、`…/languages` | Contributors 结果可能滞后数小时、匿名化上限 500 个邮箱；Languages 可能为空对象 |
| Packages / Environments | 已排除出首版 | 不请求 | 不因接口存在而纳入 |
| Home 最近参与且有更新 | 受限 | `GET /user/issues` + `GET /search/issues`（`involves:`） | 没有与网站“Recent activity”完全等价的接口；参与语义需自行定义 |
| Home 关注动态 Feed | 已选择受限事件流 | `GET /users/{u}/received_events` | 接受 Events 延迟 30 秒–6 小时、仅 300 条/30 天；不承诺网站 Feed 镜像 |
| Profile 资料 | 可实现 | `GET /users/{username}`、GraphQL `user` | 私有字段需要 `read:user`/`user` |
| 个人 README 发现 | 受限（无专用字段） | 仓库名=用户名的公开仓库 + README | 无公开等价字段，必须自行组合发现规则 |
| Pinned / Popular | 可实现（仅 GraphQL） | `user.pinnedItems`、`user.itemShowcase` | REST 无公开等价；`itemShowcase` 会混入“热门回退” |
| 贡献热力图 | 可实现（仅 GraphQL） | `contributionsCollection.contributionCalendar` | REST 无公开等价；私有贡献需要 `read:user` |
| H1 Notifications | 受限 | `GET /notifications` | 只支持 classic PAT 与 OAuth token（`notifications`/`repo` scope）；细粒度 PAT 与 GitHub App token 明确不支持 |
| H2/H3 全局 Issues/PRs | 可实现 | `GET /user/issues`、`/search/issues` | 只是候选数据源，默认查询范围仍需产品决定 |
| H4 New / H7 写操作 | 需选择 | 按动作逐个列权限 | 本文不批准任何创建/编辑/管理动作 |
| Command palette 搜索 | 受限 | `/search/*`、GraphQL `search` | 最多 1,000 条结果、搜索桶独立限流、类别未定 |

## 核验方法与证据边界

- 来源优先级：`docs.github.com` 官方文档源码（`github/docs`）、GitHub 官方 OpenAPI 描述（`github/rest-api-description`，`api.github.com.2022-11-28` 与 `2026-03-10`）、公开 GraphQL schema（`https://docs.github.com/public/fpt/schema.docs.graphql`）、官方 `gh` CLI 源码与手册。
- 端点级“细粒度 token 支持/所需权限”直接取自文档页面渲染出的 *Fine-grained access tokens for "…"* 段落，以及 OpenAPI 里的对应描述，不凭记忆补全。
- **实测范围**：用本机已有的 `gh` 授权（OAuth app token，只读 GET/query）对公开资源验证了 `GET /user/repos`、`/user/issues`、`/repos/{owner}/{repo}`、`/branches`、`/commits/{ref}`、`/git/trees`、`/contents`、`/readme`、`/releases`、`/releases/latest`、`/contributors`、`/languages`、`/feeds`、`/notifications`、`/search/issues`、`/search/repositories`、`/users/{login}`、`/user`、`/rate_limit`、`/versions` 与若干 GraphQL 查询；确认了响应头、分页 `Link`、条件请求 `304`、空数据与 404 的实际形状。
- **未实测**：私有仓库内容、空仓库 409 与 Contributors 204、组织 SSO 与细粒度 PAT 审批流程、标记通知已读/完成、任何写操作、GitHub App 与细粒度 PAT 的真实 token。这些只按官方文档/OpenAPI 描述记录，标记为待集成验证。
- 未创建 token、未注册 GitHub App、未修改任何远端资源，未输出或读取凭据值；日志中不记录 token。
- REST 版本：不带 `X-GitHub-Api-Version` 时实测响应头 `X-GitHub-Api-Version-Selected: 2022-11-28`；`GET /versions` 当前返回 `["2026-03-10","2022-11-28"]`。版本受支持约 24 个月，过期版本返回 `410 Gone`，因此请求应显式固定版本号。
- GraphQL 无 ETag/Last-Modified 可用：实测 GraphQL 响应只有 `X-RateLimit-*`，没有缓存验证头；不要把 REST 条件请求机制默认套到 GraphQL。

## 认证方式矩阵

“Bearer”只是 HTTP 头写法，不是 token 类型。下表按 token 类型区分。

| 方案 | 首版读取覆盖 | H1 Notifications | 权限粒度 | 多组织/账户 | 桌面 secret | SSO 与组织策略 | 主要限制 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Classic PAT | 覆盖，含私有 `repo` | 支持（`notifications` 或 `repo`） | 粗：`repo`、`read:user`、`notifications`、`read:org` | 单 token 覆盖用户可见的全部组织 | 用户手动生成、手动粘贴 | 建后需为该组织单独授权；未授权返回 403（含 `X-GitHub-SSO` URL，1 小时有效）或多组织 `partial-results` | 组织可禁用 classic PAT（`403`）；无法限制具体仓库；一年未用会被自动移除 |
| Fine-grained PAT | 覆盖（`Metadata`/`Contents` 读），公开仓库无需权限 | **不支持**（文档明确） | 细：按仓库/权限 | **单 token 只能一个 resource owner**，多组织要多个 token | 用户手动生成、手动粘贴 | 创建时选定组织即完成 SSO；组织可要求审批，待审批时仅能读公共资源 | 不支持外部协作者/非成员公共仓库写入；组织可禁用；首版未选用 |
| OAuth App（web flow + PKCE） | 覆盖 | 支持（`notifications` 或 `repo` scope，scope 参考与官方对照页均有说明） | 粗 scope | 单 token 覆盖用户全部可见资源 | public client 无法保密 `client_secret`：官方建议随包发行并使用 PKCE 加固，或改由后端代换 token | **token 只对登录时已有活动 SSO 会话且允许该 App 的组织生效**；loopback `127.0.0.1`/`::1` 回调可用 | 组织可限制 OAuth App，成员无法授权；scope 无法细分；带发行 secret 属于已知风险。**若 App 启用 token 过期或请求 `offline_access`，access token 8 小时过期 + 可轮换 refresh token（6 个月）** |
| OAuth App（device flow） | 覆盖 | 支持（同上） | 粗 scope | 同上 | **只需 `client_id`**，不需要 `client_secret` | 同上 | 需在 App 设置里启用 device flow；必须遵守 `interval`，否则 `slow_down`；官方建议只在受限环境（CLI/IoT/无头）启用，因为无回调 URI 更容易被远程钓鱼滥用。**同样可能拿到 8 小时 access + 6 个月 refresh token** |
| GitHub App user token | 覆盖 | **不支持**（Notifications REST 明确排除） | 细：按 App 注册权限 ∩ 用户权限 | 需逐组织安装 + 逐用户授权 | device flow 只需 `client_id`；web flow 换 token 需 `client_secret` | 需组织安装；SAML 场景需先建立活动 SSO 会话再授权 | token 生命周期**取决于 App 是否启用 expiring user tokens**（默认启用，可 opt-out）：启用则 8 小时过期 + 6 个月可轮换 refresh，opt-out 则长期且无 refresh；H1 因此不可用 |

补充事实：

- 细粒度 PAT 的权限/支持矩阵按端点查询官方文档；`X-Accepted-GitHub-Permissions` 头用于诊断缺权限（实测对 classic/OAuth token 为空，这是预期行为，不是缺失信息）。
- OAuth App 每个用户/应用/scope 组合最多 10 个 token、每小时最多创建 10 个；超出会回收旧 token 或触发重新授权。
- 细粒度 PAT 每次只能选一个 resource owner，因此“侧边栏显示全部仓库”在细粒度 PAT 下需要多 token 或退化为单组织范围；这条直接影响仓库集合的可实现程度。
- Notifications 的 token 支持差异（端点级“不支持细粒度 token” vs scope 级“OAuth 可用”）见下文 [H1](#h1-notifications)；它决定登录方案是否可行，必须作为裁决输入。

### 首版认证决定（2026-10-22）

负责人选择 PAT 作为正式产品登录方式，不建设服务器来托管 `client_secret`。结合保留的 Notifications，首版使用 Classic PAT；原研究中的 OAuth 优先建议已被该决定取代。产品边界见 [登录与未来写能力](product.md#登录与未来写能力)，未来写操作的权限与缓存边界见 [架构方案](architecture.md#认证与外部边界)。

Classic PAT 的 scope 和组织授权仍限制实际能力。读取私有仓库所需的 `repo` 同时包含写权限，不宣称 token 是只读，也不因未来要支持写操作就预先索取所有管理权限；创建引导的最小 scope 清单在认证任务前细化。过期或撤销时由用户替换 PAT。PAT 选择不表示 OAuth 必须部署后端：矩阵仍保留公开客户端 PKCE 和无 secret 的 device flow 的研究事实，但首版不接入这些流程。

### 其他认证方案的研究边界（首版未采用）

- device flow 的客户端只需要 `client_id`，因此不需要随包分发 secret；web flow 换 token 需要 `client_secret`，官方对 public client 的结论是“无法保密，只能随包发行并用 PKCE 加固”，所以 web flow 方案必须同时接受这个已知风险，不能把它当作“安全完成”。
- 两种流程都不会把 `client_secret` 变成可保密的对象，但**不想要 secret 时除了后端代换 token 和 GitHub App（device flow 只需 `client_id`），OAuth App 的 device flow 同样不需要 secret**，且仍然覆盖 H1；它的代价是官方更弱的安全评价（更易被远程钓鱼），且必须在 App 设置里启用。刷新 token 时，device flow 签发的 token 也不需要 `client_secret`。
- access token、refresh token 属于凭据，按既有约定只进系统凭据设施，不进普通数据库、缓存表、日志或崩溃报告；账户表只保存平台凭据引用与稳定账户 ID。
- OAuth App 的 token 生命周期不是固定长命：若 App 启用 expiring tokens 或请求了 `offline_access`，access token 8 小时过期、refresh token 6 个月且**每次刷新后旧 token 与旧 refresh 同时失效**（轮换）。响应里的 `expires_in`、`refresh_token`、`refresh_token_expires_in` 是有条件出现的字段，客户端不能假设一定没有 refresh，也不能假设一定存在；不实现刷新会在 8 小时后丢登录态。
- 授权后必须立刻用 `GET /user` 重新确认身份，避免用户在中途切换账号造成“token 属于 A、界面显示 B”。

## 资源能力矩阵

列说明：“接口”给 REST 路径或 GraphQL 字段；“认证/权限”给文档标注的细粒度权限与必需 classic scope；“分页/限流”只写该资源特有部分，通用规则见后文；“空/错误”指关键区分；“缓存标识”指可用于稳定复用的标识。

### 账户与仓库集合

| 能力 | 接口 | 认证/权限 | 分页/限流 | 空/错误 | 缓存标识 |
| --- | --- | --- | --- | --- | --- |
| 当前账户身份 | `GET /user`；GraphQL `viewer` | 细粒度：无需权限（不支持 installation token）；classic：`read:user` 才返回私有字段 | core 桶 | 401 未认证 | 账户 `id`/`node_id` |
| 全部可见仓库 | `GET /user/repos`（`affiliation=owner,collaborator,organization_member`，`visibility`，`sort`，`direction`） | 细粒度：`Metadata` 读；不适用于 installation token；classic：`repo` 才有私有 | `per_page` ≤ 100，`Link` 分页 | 401/403/422；空数组≠无仓库 | 仓库 `id`/`node_id` |
| 某用户公开仓库 | `GET /users/{username}/repos` | 细粒度：`Metadata` 读（公共资源可匿名） | 同上 | — | 仓库 `id` |
| 组织仓库 | `GET /orgs/{org}/repos` | 细粒度：`Metadata` 读 | 同上 | 私有不可见时只返回可见集合 | 仓库 `id` |
| 组织枚举 | `GET /user/orgs`（OAuth/classic，需 `user` 或 `read:org`）；**细粒度 token 请用 `GET /user/memberships/orgs`**（`state` 可选 `active`/`pending`） | 细粒度：`/user/orgs` 无需权限但**总返回 200 + 空列表**；`/user/memberships/orgs` 无需权限；classic：`user`/`read:org` | 同上 | `/user/orgs` 对细粒度 token 返回 **200 空数组**（不是 403）；classic scope 不足为 403 | 组织 `id` |
| 仓库范围差异 | — | — | — | 侧栏“全部仓库”在细粒度 PAT 下按 resource owner 分裂；GitHub App 按安装范围收敛 | — |

要点：

- 三个 REST 列表来源含义不同：`/user/repos` 是“显式可用”集合（本人、协作者、组织成员），`/users/{u}/repos` 只有公共仓库，`/orgs/{o}/repos` 是单组织视图。首版已确定使用 `/user/repos` 当前授权下的可见集合，完整读取分页；不能混用后两者再声明完整。
- GraphQL `viewer.repositories(affiliations:)` 与 REST 等价，可与资料字段合并成一次查询，但受点数与节点上限约束。
- 细粒度 PAT 的“一个 resource owner”不是分页问题，而是集合本身被截断；这是登录方案裁决的关键输入，不能靠翻页修补。
- SSO 缺失时，多组织请求不报错而是缺失结果，并用 `X-GitHub-SSO: partial-results; organizations=…` 提示；单组织请求可能直接 403 并给出 1 小时有效的授权 URL。因此“组织仓库为空”必须区分未授权、无成员身份和真的没有仓库。
- `/user/orgs` 对细粒度 token 的官方描述是“返回 200 与空列表”，这是端点级行为而非权限错误；用它枚举组织会在细粒度 token 下静默得到“没有任何组织”。需要枚举组织时应统一改用 `/user/memberships/orgs`，并把“空列表”当作可疑状态而不是权威结果。

### 仓库资料与统计

| 能力 | 接口 | 认证/权限 | 空/错误 | 缓存标识 |
| --- | --- | --- | --- | --- |
| 仓库资料（描述、Topics、默认分支、可见性、`pushed_at`） | `GET /repos/{owner}/{repo}`；GraphQL `repository` | 细粒度：`Metadata` 读；classic：`repo` 才含私有 | 改名/转移返回 301；无权限的私有仓库返回 404 | 仓库 `id` |
| Stars / Forks / Watchers | 同上的 `stargazers_count`、`forks_count`、`subscribers_count`；GraphQL `stargazerCount`、`forkCount` | 同上 | 计数可能滞后；`watchers_count` 不是订阅数语义 | 随仓库摘要一起存，不单独设高频刷新 |
| Open issues 计数 | REST `open_issues_count`（含 PR）；纯 issue 计数用 GraphQL `repository.issues(states:OPEN).totalCount` | 同上 | REST 的“每个 PR 也是 issue”，所以 `open_issues_count` 是 issue + PR 的合计数；不能直接标成“Open issues” | 需要纯 issue 计数时以 GraphQL 为准或自行减 PR |
| 仓库体积 | 同上 `size`（KB，按小时计算，新建为 0） | 同上 | 0 不代表空仓库 | 仅作提示，不作内容边界 |
| namespace 头像 | `owner.avatar_url`；GraphQL `owner.avatarUrl(size:)` | 同上 | — | URL 含用户 ID 与版本参数；实测头像响应有 `Cache-Control: max-age=300` 与 `ETag` |
| 用户/组织资料 | `GET /users/{username}`、`GET /orgs/{org}`；GraphQL `user`/`organization` | 细粒度：无需权限；私有字段需 `read:user` | 404 表示不可见或不存在 | 用户 `id` |

### 分支、commit、树、文件、README

| 能力 | 接口 | 认证/权限 | 空/错误 | 缓存标识 |
| --- | --- | --- | --- | --- |
| 分支列表 | `GET /repos/{o}/{r}/branches`（`protected`） | 细粒度：`Contents` 读；公共可匿名 | 404 仓库不可见 | 分支名 + 其 `commit.sha` |
| 单个分支（含保护状态） | `GET …/branches/{branch}` | 同上 | 301（改名）、404 | `commit.sha` |
| 浏览版本解析 | `GET …/commits/{ref}`、GraphQL `defaultBranchRef.target.oid` | 细粒度：`Contents` 读 | 409 空仓库/不可用；404 引用不存在 | **commit SHA**（固定版本根标识） |
| commit 列表 | `GET …/commits`（`sha`、`path`、`since`、`until`、`author`） | 同上 | 400/409/422/429/500（`429` 为“服务器负载”，非限流桶耗尽） | commit SHA |
| 固定 commit 原文 | `GET …/commits/{ref}` | 同上 | 文件数 >300 时才分页，最多 3,000 个文件 | commit SHA |
| 递归树 | `GET …/git/trees/{tree_sha}?recursive=1` | 同上 | `truncated: true` 表示超限；上限 100,000 项 / 7 MB | **tree SHA** |
| 单文件/目录 | `GET …/contents/{path}`（`ref`） | 同上 | 目录 >1,000 文件需改用 Trees；302 为下载跳转 | 文件 `sha`（= blob SHA） |
| blob | `GET …/git/blobs/{sha}` | 同上 | blob ≤ 100 MB | **blob SHA** |
| README 发现 | `GET …/readme`（`ref`） | 同上 | 无 README 返回 404；仓库不可见也 404 | 返回体 `sha` = blob SHA |
| 原始内容 / HTML | `contents`/`readme` 的 `application/vnd.github.raw+json`、`html+json` | 同上 | 1–100 MB 只支持 raw/object；>100 MB 不支持 | 同上 |

要点：

- 内容大小分档是硬边界：≤1 MB 全功能；1–100 MB 只有 raw/object，且 object 的 `content` 为空、`encoding` 为 `none`；>100 MB 不支持。首版“过大内容”的具体呈现需要据此定阈值，不能假设任意文件都能读。
- 表中 Git 数据库的 **409**（空仓库/不可用）与 Contributors 的 **204** 来自官方文档/OpenAPI 描述，本轮没有构造空仓库实测；`truncated` 上限与 blob 大小同样来自官方描述。
- 空仓库与不可用仓库都会让 Git 数据库端点返回 409，不要把它渲染成“文件不存在”。目录/文件不存在是 404，两者语义不同。
- README 有专门发现端点，不需要自己拼文件名；`readme` 与 `contents/README.md` 实测返回同一 blob SHA，可以互相校验缓存。**注意：`readme` 文档称 `application/vnd.github.raw+json` 是默认媒体类型，但实测不带 `Accept` 时返回 JSON 的 content-file 对象**（`content` 为 base64），需要原文时必须显式指定媒体类型，不要依赖“默认”。
- 固定版本复用必须以 commit/tree/blob SHA 为键；分支名只是可变指针。仓库改名会 301，稳定标识应使用仓库 `id` 而不是 `full_name`。
- `contents` 的 `download_url` 会过期且“一次性使用”，不能当作内容标识；需要长期复用时下载 blob 或自行保存正文。

### 顶部信息栏：Releases、Contributors、Languages

| 能力 | 接口 | 认证/权限 | 分页/缓存 | 空/错误 |
| --- | --- | --- | --- | --- |
| Releases 列表 | `GET …/releases`；GraphQL `repository.releases` | 细粒度：`Contents` 读；classic：`repo` 才有私有 | `per_page` ≤ 100，`Link` 分页；有 ETag | 无 Release 时列表为空数组；草稿只对有 push 权限者可见 |
| 最新 Release | `GET …/releases/latest`；GraphQL `repository.latestRelease` | 同上 | 单个对象 | 没有已发布 Release 时 **404**（实测），不是空对象 |
| Contributors | `GET …/contributors`（`anon`） | 细粒度：`Metadata` 读；classic：`repo` 才有私有 | `per_page` ≤ 100；有 ETag/Last-Modified | 空仓库按官方描述返回 **204**（不是 200 空数组）；数据可能滞后数小时 |
| Languages | `GET …/languages`；GraphQL `repository.languages` | 细粒度：`Metadata` 读 | 单个对象，无分页 | 无语言统计时返回 `200 {}`（实测），不是 404 |
| Packages / Environments | 不请求 | — | — | 首版排除，不因接口存在而纳入 |

要点：

- `releases/latest` 的“最新”是最新**非草稿、非预发布**版本，按 `created_at` 排序，而 `created_at` 是发布所指向提交的日期，不是发布动作时间。因此“最新 Release”与“最近发布动作”可能不同，界面文案需要避免暗示后者。
- Contributors 的计数按邮箱归并到 GitHub 用户，只有前 500 个作者邮箱会关联用户，其余显示为匿名；因此“贡献者列表”天然可能不完整，不能用来推断权限或活跃度。
- Languages 返回字节数映射而非百分比；百分比由客户端计算，且仓库可能返回空对象。

### Home：最近参与且有更新

| 能力 | 接口 | 认证/权限 | 分页/限流 | 说明 |
| --- | --- | --- | --- | --- |
| 跨仓库 Issues/PRs（含 PR） | `GET /user/issues`（`filter` 可为 `assigned`、`created`、`mentioned`、`subscribed`、`repos`、`all`；`state`；`sort` 可为 `created`、`updated`、`comments`） | 细粒度：无需权限（不支持 installation token）；classic：`repo` 才有私有 | `per_page` ≤ 100；core 桶 | 文档称“每个 PR 也是 issue”，响应中可用 `pull_request` 字段区分；`id` 是 issue id，不是 PR id |
| 按“参与”搜索 | `GET /search/issues?q=involves:@me …` | 细粒度：无需权限；classic：`repo` 才有私有 | `per_page` ≤ 100，**最多 1,000 条**；`search` 桶（30 次/分） | `involves:` 是 author、assignee、mentions、commenter 的逻辑或；可叠加 `updated:>=` 与 `sort=updated` |
| GraphQL 备选 | `user.issues(filterBy:{viewerSubscribed:true}, orderBy:{field:UPDATED_AT})`、`search(type:ISSUE)` | 同上 | 游标分页，`first/last` 1–100 | 可与其他字段合并，但点数与节点上限更高 |

要点与局限：

- **没有与 github.com 首页 “Recent activity” 完全等价、且语义被文档保证的接口。** 网站该区只展示**最近两周、最多 4 条**更新，上榜条件是：本人打开、有人评论、被重新打开、被请求评审、被指派、被 commit 引用、本人评论过。REST 的 `filter` 参数没有“我评审过/我评论过/被 commit 引用”这些单选项，`involves:` 覆盖 author/assignee/mentions/commenter，但不等价于“我的评审”与“commit 引用”，也没有“最多 4 条”的同款裁剪规则。
- 左栏标题已确定为 Home，内容为最近参与且有更新的 Issues/PRs；时间窗口与额外参与关系仍须在实现任务中明确。候选实现为：`involves:@me` 搜索（可解释、可叠加 `updated:` 与 `sort=updated`），或 `filter=all&sort=updated`（覆盖订阅/可见但语义更宽）。两者都不能声称复刻网站首页。
- 排序不是“更新时间稳定流”：`sort=updated` 会让条目在刷新时移动，跨页拼接会重复或漏项；应保存查询、排序与页信息并按 ID 去重。
- 空结果、权限不足与“确实没有更新”必须区分：搜索对无权限的 `repo:`/`org:` 会返回 422 或静默省略，而列表型端点会直接返回更少的条目。

### Home：关注动态 Feed

| 能力 | 接口 | 认证/权限 | 结果形状 | 限制 |
| --- | --- | --- | --- | --- |
| Feed 目录 | `GET /feeds` | 细粒度：无需权限；classic：`repo`/`notifications` 影响私有可见性 | 返回各种 Feed 的 URL（timeline、user、current_user_public 等） | 文档说明私有 Feed 只在 Basic Auth 且使用旧的不可撤销 token 时返回；实测任何认证方式都只返回 `current_user_public_url`，没有 `current_user_url` |
| 事件流 | `GET /users/{u}/received_events`、`/users/{u}/events` | 细粒度：无需权限（文档提到查看私有事件可选 “Events” 用户权限） | 事件数组 | **延迟 30 秒–6 小时**；最多 300 条、只含 30 天内事件；带 `X-Poll-Interval` 与 ETag，支持 304 |
| GraphQL Feed | 无公开等价 | — | — | schema 中没有 feed/notification 查询字段 |

结论：关注 Feed 属于**受限**能力。可行路径是 Events API 的 `received_events`（“关注的人 + 订阅仓库产生的事件”），它与网站首页的 Following feed 不是同一数据源：网站还包含 star、follow、建库、Release 等条目，API 事件流不保证覆盖同一集合，也不保证同样的顺序与新鲜度。首版已接受此事件流及延迟，右栏标题为 Feed；不承诺与网站逐条一致，也不要求每张卡片显示技术性来源标签。

### Profile：资料、README、Pinned/Popular、热力图

| 能力 | 接口 | 认证/权限 | 说明 |
| --- | --- | --- | --- |
| 基本资料 | `GET /users/{username}`；GraphQL `user`（`name`、`bio`、`avatarUrl`、`company`、`location`、`websiteUrl`、`followers.totalCount`） | 细粒度：无需权限 | 单次 GraphQL 查询即可取全，REST 需要一次请求 |
| 本人私有字段 | `GET /user` | classic：`read:user` 或 `user` | 私有字段（私有仓库数、磁盘用量等）仅本人可见 |
| 个人 README | 无专用字段。按公开规则组合：仓库名 = 用户名、公开、根目录 `README.md`、内容非空 | 细粒度：`Metadata`/`Contents` 读 | 规则来自官方文档；API 不会告诉你“该仓库是否被分享到 Profile”。一个用户可能不存在同名仓库（实测 `torvalds/torvalds` 为 404）。**2020-07 前创建的同名仓库不会自动展示，需要用户手动“Share to profile”，且 API 不暴露这个分享标记**，因此这套组合只能作为“大概率是个人 README”的启发式，不能保证与网站 Profile 一致 |
| Pinned | GraphQL `user.pinnedItems(first:, types:[REPOSITORY])`、`pinnedItemsRemaining`、`anyPinnableItems` | 细粒度：无需权限 | **REST 无公开等价**；`types:[REPOSITORY]` 能滤掉 gist 等非仓库 pin |
| Pinned → Popular 回退 | GraphQL `user.itemShowcase { hasPinnedItems items(first:) }` | 同上 | `hasPinnedItems` 计算的是**仓库或 gist**，不是仓库；`items` 是 `PinnableItem` 联合，只有“一个 pin 都没有”时才返回热门仓库 |
| 贡献热力图 | GraphQL `contributionsCollection(from:, to:).contributionCalendar`（`weeks[].contributionDays[]`：`date`、`contributionCount`、`contributionLevel`、`color`） | classic：`read:user` 才含私有/内部仓库贡献 | **REST 无公开等价**（`/users/{u}/contributions` 实测 404）；默认区间为最近一年，`contributionYears` 可枚举可选年份 |
| Contribution Activity 列表 | 不实现（首版排除 D5） | — | 与热力图是不同的数据；不因包含在 `contributionsCollection` 就顺手接入 |

要点：

- `itemShowcase` 的语义是“已 Pin 任意项就返回 Pin，否则返回热门”，且 `hasPinnedItems` 统计的是**仓库或 gist**，`items` 是 `PinnableItem` 联合。这带来两个产品事实：
  1. **gist-only 用户**：`hasPinnedItems=true` 但 `items` 返回的是 gist 而不是仓库，无法用 `itemShowcase` 拿到 Popular 仓库回退。
  2. **混合 pin 用户**：`items` 返回的仓库数量可能少于请求数（被 gist 占位）。
  首版只展示仓库，因此应以 `pinnedItems(types:[REPOSITORY])` 作为真实仓库 pin 列表；Popular 回退取 `itemShowcase`，但当 `itemShowcase` 里出现 gist（或仓库数不足）时，服务端不会提供“排除 gist 的仓库热门选择”，需要产品决定这类少见情况下的呈现（例如只展示仓库 pin、留空或标记部分）。实测 octocat 无 pin 时 `itemShowcase.items` 返回 8 项热门仓库、`pinnedItems` 为空，与“无 pin 则热门回退”一致。
- 热力图等级是相对分位（`FIRST_QUARTILE`…`FOURTH_QUARTILE`），不是绝对阈值；`color` 由服务端给出，包含季节性主题（schema 里有 `isHalloween`），不应硬编码配色。
- 私有贡献被计入时只显示数量，不显示仓库细节；打开私有贡献可见性属于用户设置，客户端无法探测“是否有被隐藏的贡献”，只能显示服务端返回的结果。

### H1 Notifications

| 能力 | 接口 | 认证/权限 | 说明 |
| --- | --- | --- | --- |
| 通知列表 | `GET /notifications`（`all`、`participating`、`since`、`before`） | classic PAT 或 OAuth token：`notifications` 或 `repo` scope；**fine-grained PAT 与 GitHub App token 不支持** | `per_page` 最大 **50**；`Last-Modified` 条件请求；`X-Poll-Interval` 必须遵守 |
| 单条线程 | `GET /notifications/threads/{id}` | 同上 | 返回 `reason`、`unread`、`updated_at`、`last_read_at` |
| 标记已读/完成 | `PATCH /notifications/threads/{id}`、`DELETE /notifications/threads/{id}`、`PUT /notifications` | 同上 | 全部已读可能返回 202 异步处理 |
| 仓库通知 | `GET /repos/{o}/{r}/notifications` | 同上 | 首版是否需要尚未定 |
| GraphQL 通知 | 无公开等价 | — | schema 无 notification 查询字段 |

**这条决定登录方案可行性，证据分两层：**

- 端点级：每个通知端点的 *Fine-grained access tokens* 段落写明**不支持** GitHub App user token、installation token 与 fine-grained PAT；页面顶部另有一则说明：这些端点仅支持 classic PAT。这层说明的是“细粒度 token 不支持”，不是“只有 classic PAT 能用”。
- scope 级：OAuth App 的 scope 参考明确 `notifications` scope 授予“读取用户通知、将 thread 标记为已读、订阅管理”，GitHub Apps 与 OAuth Apps 对照页也直接以“为已登录用户查找全部通知”作为 OAuth App 的示例。实测在 OAuth app token（scope 含 `repo`）下 `GET /notifications` 返回 `200`、空数组、`X-Poll-Interval: 60`、`X-Accepted-OAuth-Scopes: notifications, repo`，与上述 scope 说明一致。

结论：H1 在 **classic PAT 或 OAuth App token**（`notifications` 或 `repo` scope）下可用；**fine-grained PAT 与 GitHub App user token 不可用**。因此：

- 若首版要做 H1，登录载体必须是 classic PAT 或 OAuth App；OAuth App 是两者中唯一不需要用户手工建 token 的方案。“细粒度 token 不支持”这条要从“全部认证方式都不支持”中区分开，不要因为它而把 H1 推向 classic PAT。
- 若负责人选 GitHub App user token（权限面最小），H1 必须延后或改用它处获取通知；这不是文档模糊，而是端点不支持。
- Notifications 的 `per_page` 上限是 50（不是 100），且必须尊重 `X-Poll-Interval`；把“未读数”当成可高频轮询的计数会触发次级限流，也违反约定。

### H2/H3 全局 Issues/PRs、H4 New、H7 写操作（仅列候选与约束）

以下只是“值得选择的最小范围候选”，不构成批准。

| 候选动作 | 接口 | 最小权限 | 约束 |
| --- | --- | --- | --- |
| 全局 Issues 列表（默认查询待定） | `GET /user/issues` 或 `/search/issues` | 细粒度：无需权限；classic：`repo` | 是否含已关闭、是否仅“参与”、分页上限与搜索 1,000 条上限都会改变可实现范围 |
| 全局 PRs 列表 | `GET /search/issues?q=is:pr …` 或 GraphQL `search` | 同上 | PR 的 `id` 与 issue `id` 不同；不能用 issue id 直接调 PR 端点 |
| New：创建 Issue | `POST /repos/{o}/{r}/issues` | `Issues` 写 | 需要仓库可写、组织策略允许；创建过快触发次级限流 |
| New：创建 PR | `POST /repos/{o}/{r}/pulls` | `Pull requests` 写 | 需要 head/base 分支与内容权限；fork 场景更复杂 |
| New：创建仓库 | `POST /user/repos` / `POST /orgs/{o}/repos` | `Administration` 或 `Repository creation` 写 | 组织创建通常需要更高角色；首版建议不纳入 |
| 评论 | `POST /repos/{o}/{r}/issues/{n}/comments` | `Issues` 写或 `Pull requests` 写 | 与 `repo` scope 等价的粗权限在 classic 下无法细分 |
| Star / Unstar | `PUT/DELETE /user/starred/{o}/{r}` | `Starring` 写 + `Metadata` 读 | 成功后需协调按钮状态与仓库摘要计数 |
| 编辑文件 | `PUT /repos/{o}/{r}/contents/{path}` | `Contents` 写（涉及 workflow 文件还需 `Workflows` 写） | 超出“远程写操作最小集合”的范围，需单独裁决 |

### Command palette 搜索

| 类别 | 接口 | 认证/权限 | 限制 |
| --- | --- | --- | --- |
| 仓库 | `GET /search/repositories`；GraphQL `search(type:REPOSITORY)` | 细粒度：无需权限（公共资源可匿名） | `per_page` ≤ 100，最多 1,000 条结果；`search` 桶 |
| Issues/PRs | `GET /search/issues`；GraphQL `search(type:ISSUE)` | 同上 | 同上；`incomplete_results` 表示超时截断 |
| 用户 | `GET /search/users`；GraphQL `search(type:USER)` | 同上 | 同上 |
| 代码 | `GET /search/code` | **必须认证**；细粒度：无需权限 | 10 次/分钟（独立于 30 次/分）；需要能读到目标仓库 |
| 语义/混合 issue 搜索 | `GET /search/issues` 带 `search_type=semantic` 或 `hybrid`；GraphQL `ISSUE_SEMANTIC`、`ISSUE_HYBRID` | **必须认证**；细粒度：无需权限 | 10 次/分钟（独立 `semantic_search` 桶，低于普通搜索的 30 次/分）；能力存在但产品未定类别，不因枚举存在而扩张范围 |

搜索的通用限制：查询去操作符后 >256 字符、或 >5 个 AND/OR/NOT 会返回 422；单次查询最多扫描 4,000 个仓库；认证后仍需对目标资源有权限，否则 422 或静默省略。结果默认按 best match，`sort` 按端点各有白名单：仓库 `stars`、`forks`、`help-wanted-issues`、`updated`；issue/PR `comments`、`reactions`、`reactions-+1`、`reactions--1`、`reactions-smile`、`reactions-thinking_face`、`reactions-heart`、`reactions-tada`、`interactions`、`created`、`updated`（**没有 `reactions*` 通配值**）；用户 `followers`、`repositories`、`joined`；代码仅 `indexed`；commit `author-date`/`committer-date`。**跨类别或未知值的实际行为是静默忽略 sort 并退回 best match（实测 `sort=comments`、`sort=bogus` 对 `/search/repositories` 都返回 200 且顺序与无 sort 相同），不是 422**；因此不能用“请求没报错”证明排序生效，需要按类别验证返回值或干脆不交叉使用。首版类别未定，本文只说明“能搜什么、代价多大”，不预设 palette 内容。

## 错误、空数据与“看不见”的区分

| 现象 | 含义 | 处理要求 |
| --- | --- | --- |
| 404（私有资源无权限） | 可能是无权限，不一定是资源不存在；`readme`、`contents` 等子资源同样适用 | 不能缓存成“永久不存在”；负结果必须绑定“当前授权上下文下已确认可见”，否则授权恢复后仍会显示为空（区分“无权限”只能靠已知上下文或 403/SSO 头） |
| 403 + `X-GitHub-SSO` | 需要为该组织授权 token（URL 1 小时有效） | 提示可操作路径，不显示为普通错误 |
| 200 + `X-GitHub-SSO: partial-results` | 多组织结果被静默截断 | 列表状态要能表达“部分结果” |
| 409（Git 数据库，官方文档说明） | 空仓库或仓库正在创建/不可用 | 与“文件不存在”分开呈现 |
| 204（Contributors） | 仓库没有内容 | 不是错误，但也不同于“200 空列表” |
| 200 `{}`（Languages） | 无语言统计 | 显示空态，不显示 0% |
| 404（`releases/latest`） | 没有已发布 Release | 与“Releases 列表为空”合并为空态，但不要伪造版本号 |
| 422（搜索） | 查询语法或权限问题 | 不重试同一查询；提示修改而不是“无结果” |
| 200 + 空组织列表（`/user/orgs`） | 细粒度 token 下固定行为，不代表无组织 | 改用 `/user/memberships/orgs`，不要缓存为“无组织” |
| 429（commit 端点） | 服务端负载，不一定是限流 | 按 `retry-after`/退避处理，不误报“额度用尽” |
| 301（仓库端点） | 仓库改名/转移 | 跟随并更新本地稳定 ID 映射 |
| `incomplete_results: true` | 搜索超时，结果不完整 | 明确标注不完整，不做“已全部加载” |
| **HTTP 200 + `errors` 字段**（GraphQL 主限流耗尽） | GraphQL 主限流超限仍返回 `200`，错误在响应体 `errors` 里，`x-ratelimit-remaining` 为 0 | **不能只看状态码**；`data` 与 `errors` 同时存在时按错误处理，不把部分数据当成功写入 |
| HTTP 200 或 403 + 次级限流错误（GraphQL） | 次级限流在 GraphQL 下可能是 200 也可能是 403 | 同样要解析错误体；不根据 200 就重试 |

## 分页与限流（区分维度）

**必须先区分三件事：限额所属 resource/bucket、REST 与 GraphQL、主限流与次级限流。**

主限流（primary）：

| 认证方式 | REST 主限额 | GraphQL 主限额 |
| --- | --- | --- |
| 未认证 | 60 次/小时/来源 IP | 不适用：官方需认证；未认证请求 `POST /graphql` 实测返回 `403`（错误体为来源 IP 限流提示），未认证 `/rate_limit` 中的 `graphql` 额度为 0 |
| 用户 token（PAT classic/细粒度、OAuth、GitHub App user token） | 5,000 次/小时/用户，**共享同一预算**；GitHub Enterprise Cloud 组织拥有/批准的 GitHub App 或 OAuth App 可到 15,000，但会同时抵扣 5,000 的基础预算 | 5,000 点/小时/用户；同上的 Enterprise Cloud 应用可到 10,000 点 |
| GitHub App installation token | 5,000 起，按仓库/用户规模增加，上限 12,500（Enterprise Cloud 组织 15,000） | 5,000 点起，上限 12,500（Enterprise Cloud 10,000） |
| `GITHUB_TOKEN`（Actions） | 1,000 次/小时/仓库 | 1,000 点/小时/仓库 |

其他事实：

- 搜索使用独立的 `search` 桶：认证 30 次/分钟、代码搜索 10 次/分钟（且必须认证）、未认证 10 次/分钟。**issue 搜索的 `search_type=semantic` 与 `hybrid` 走另一个 `semantic_search` 桶，10 次/分钟且必须认证**（实测响应头 `X-RateLimit-Resource: semantic_search`、`X-RateLimit-Limit: 10`），不能按 30 次/分调度。GraphQL 有独立的 `graphql` 桶。实测响应头 `X-RateLimit-Resource` 可为 `search`、`semantic_search`、`graphql`、`core`，可据此分桶计数。
- **GraphQL 的限流信号不只靠状态码**：主限流耗尽时官方描述为 HTTP 200 + 错误信息 + `x-ratelimit-remaining: 0`；次级限流可能是 200 或 403。因此 GraphQL 必须同时解析响应体 `errors` 与 `x-ratelimit-*` 头，不能把 200 一律当成功。REST 主限流则是 403/429 + `remaining: 0`。
- 条件请求：正确授权且返回 `304` 的请求不消耗主限额。实测连续 3 次带 `If-None-Match` 的 304 只让 `core` 已用数增加 1（来自那次 200 请求），验证了文档描述。
- `GET /rate_limit` 不计入主限额，但可能计入次级限流；它是总览，响应头才是权威值，两者不一致时以响应头为准。
- GraphQL 点数按连接数估算：把各连接需要的请求数相加、除以 100 取整，最小 1 点；单次查询节点上限 500,000，`first`/`last` 必须在 1–100 之间；超过 10 秒会返回 502/504 并额外扣点。

次级限流（secondary，REST 与 GraphQL 共享）：

- 并发请求不超过 100（REST 与 GraphQL 合计）。
- 单端点每分钟不超过 900 点（REST）/ 2,000 点（GraphQL）；GET/HEAD/OPTIONS 记 1 点，写操作记 5 点，部分端点有未公开点数。
- 每 60 秒实际时间内 CPU 不超过 90 秒（其中 GraphQL 不超过 60 秒）。
- 内容创建 80 次/分钟、500 次/小时（含网页操作）。
- OAuth token 请求 2,000 次/小时。
- 次级限流无法查询状态；触发后返回 403/429，需按 `retry-after`、`x-ratelimit-reset` 或至少 1 分钟退避，并指数退避。

分页：

- REST 使用 `Link` 头（`prev`/`next`/`first`/`last`），不要把分页 URL 当稳定标识，也不要手动拼页；`per_page` 上限通常 100（Notifications 为 50），超过会被静默截断而不报错。
- 搜索最多 1,000 条结果（`Link` 的 `last` 实测为 `page=1000`），超过部分不可得；这不是分页 bug。
- GraphQL 用游标（`after`/`before` + `first`/`last`），不保证偏移稳定。

## 与缓存策略的对照

现有 [缓存策略](cache-strategy.md) 的总体方向与本次核验一致（固定 SHA 复用、latest 需验证、条件请求、按 ID 合并、头像落盘）。以下是需要补充或调整的依据，**不自行修改架构**：

| 缓存策略现状 | 核验结果 | 建议（待主 Agent/负责人采纳） |
| --- | --- | --- |
| 头像“无明确规则时可暂以 24 小时为验证间隔” | 实测头像响应带 `Cache-Control: max-age=300` 与 `ETag` | 优先遵循响应头与验证器；24 小时只能作为无头信息时的兜底，不应覆盖 `max-age` |
| “条件请求返回 304 不消耗主限额”（已正确） | 实测确认 | 保持；但需绑定同一账户/端点/参数/媒体类型，GraphQL 无此机制 |
| Home 列表“约 1–2 分钟后按需刷新”“支持手动刷新” | Feed 数据源本身延迟 30 秒–6 小时；搜索桶只有 30 次/分 | 刷新间隔不能承诺新鲜度；按已确认的 Feed 呈现说明来源限制，并把搜索类刷新纳入限流预算 |
| Notifications “SQLite 保存最近内容、遵守 `X-Poll-Interval`”（已正确） | 接口只对 classic PAT / OAuth token 可用；细粒度 PAT 与 GitHub App token 不支持 | 首版已选 Classic PAT；Notifications 仍按实际 scope/组织访问能力处理，具体动作待定 |
| “搜索 30 秒内复用、不落盘” | 搜索桶 30 次/分、结果上限 1,000、`incomplete_results` 存在 | 复用窗口与限流预算应一起核算；不完整结果不能作为可缓存完整集合 |
| 贡献热力图“30–60 分钟按需刷新” | 仅 GraphQL，按点数计费 | 可保留建议，但需计入点预算；不要把 REST 条件请求当作省点手段 |
| 仓库列表“5 分钟内可复用” | 细粒度 PAT 会截断集合；SSO 会造成部分结果 | 缓存键必须包含账户/授权上下文与“是否部分结果”，不能跨 token 复用 |
| 固定 SHA 内容按 SHA 复用（已正确） | 409/404 语义、1,000 文件与 100 MB 上限已确认 | “没有 README”可随 SHA 缓存，但**只在已确认当前授权上下文能看到该仓库时**；403/SSO 缺失不可缓存为不存在，未授权导致的 404 与“真的无 README”共享同一状态码，不能直接写成 SHA 绑定的空结果 |

## 裁决状态与剩余问题

负责人已确认的选择统一维护在 [产品约定](product.md)：PAT 产品登录（首版 Classic PAT）、`/user/repos` 可见集合、左栏 Home 与右栏 Feed，以及接受 Events 数据延迟。当前以阅读为先，但架构为后续写能力保留边界。

仍需在相关任务前细化：

- PAT 创建引导的最小权限清单；Notifications/全局 Issues/PRs/New 和 H7 写操作的具体内容与动作，不由本次认证选择自动扩张。
- Home 的时间窗口、额外参与关系和筛选；`involves:` + 更新时间排序是候选，不代表“未读”或与网站首页等价。Feed 卡片的事件类型和过滤仍待实现规格确认。
- 账户切换时标签处理、凭据失效和移除账户后的私有内容策略。
- 已明确延后的 Profile 边界：gist-only 的 Pinned→Popular 仓库回退；2020-07 前同名仓库个人 README 的遗留分享规则。执行者交接将这两项合并为第 4 项，待对应任务开始时再定。

GraphQL 合并查询可能减少同页往返，但点数与节点上限仍需实测，不能把候选优化写成已采用的依赖或架构。

## 待集成验证清单

以下随已选 PAT 登录和相关功能补齐，不能由本文的文档证据替代：

- Classic PAT 的身份、scope、过期/撤销/替换、真实组织（含 SAML 与禁止 classic PAT 策略）下的实际可见集合与错误形状；其他 token 类型不列为首版接入验收。
- 私有仓库内容的 404/403/SSO 分支，以及“无权限”与“不存在”在 UI 上的区分方式。
- 标记通知已读/完成的实际返回（202/205/204）、`X-Poll-Interval` 变化与未读数同步。
- 1–100 MB 文件的 raw 读取、>100 MB 的失败呈现、目录 >1,000 文件与递归树 `truncated` 的真实体验。
- GraphQL 合并查询的实际点数（`rateLimit.cost`）与在低配网络下的超时行为。
- 条件请求在真实 token 与真实端点上的 `ETag` 稳定性（尤其 `contents` 与 `readme` 是否随 `ref` 变化）。
- 搜索在“用户有权限但结果被截断”时的 `incomplete_results` 呈现。

## 官方来源

REST 端点（每项的 “Fine-grained access tokens” 段与参数/响应说明）：

- [Repositories](https://docs.github.com/en/rest/repos/repos)、[Contents](https://docs.github.com/en/rest/repos/contents)、[Branches](https://docs.github.com/en/rest/branches/branches)、[Commits](https://docs.github.com/en/rest/commits/commits)、[Git trees](https://docs.github.com/en/rest/git/trees)、[Git blobs](https://docs.github.com/en/rest/git/blobs)、[Releases](https://docs.github.com/en/rest/releases/releases)
- [Events](https://docs.github.com/en/rest/activity/events)、[Feeds](https://docs.github.com/en/rest/activity/feeds)、[Notifications](https://docs.github.com/en/rest/activity/notifications)、[Starring](https://docs.github.com/en/rest/activity/starring)
- [Issues](https://docs.github.com/en/rest/issues/issues)、[Pull requests](https://docs.github.com/en/rest/pulls/pulls)、[Search](https://docs.github.com/en/rest/search/search)、[Users](https://docs.github.com/en/rest/users/users)、[Organizations](https://docs.github.com/en/rest/orgs/orgs)、[Organization memberships](https://docs.github.com/en/rest/orgs/members#list-organization-memberships-for-the-authenticated-user)
- [Rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)、[Best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api)、[Pagination](https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api)、[Troubleshooting](https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api)、[API versions](https://docs.github.com/en/rest/about-the-rest-api/api-versions)
- [Git database guide（409 语义）](https://docs.github.com/en/rest/guides/using-the-rest-api-to-interact-with-your-git-database)
- [Authenticating to the REST API（含 `X-GitHub-SSO`）](https://docs.github.com/en/rest/authentication/authenticating-to-the-rest-api)、[Fine-grained PAT permissions](https://docs.github.com/en/rest/authentication/permissions-required-for-fine-grained-personal-access-tokens)

GraphQL 与账户/Profile：

- [GraphQL rate and node limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api)、[Forming calls](https://docs.github.com/en/graphql/guides/forming-calls-with-graphql)、[Public schema](https://docs.github.com/public/fpt/schema.docs.graphql)、[Users reference](https://docs.github.com/en/graphql/reference/objects#user)、[Repositories reference](https://docs.github.com/en/graphql/reference/objects#repository)
- [Personal dashboard（Recent activity 只保留两周内最多 4 条）](https://docs.github.com/en/account-and-profile/reference/personal-dashboard)、[Profile contributions reference](https://docs.github.com/en/account-and-profile/reference/profile-contributions-reference)、[Profile README](https://docs.github.com/en/account-and-profile/how-tos/profile-customization/managing-your-profile-readme)
- [Searching issues and pull requests（`involves`、`@me`）](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests)

认证与授权：

- [Managing your personal access tokens](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)、[Authorizing OAuth apps（web/device flow、PKCE、loopback 回调）](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps)、[Best practices for OAuth apps（public client、PKCE 优于 device flow）](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/best-practices-for-creating-an-oauth-app)、[Scopes for OAuth apps](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps)、[GitHub Apps vs OAuth apps](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/differences-between-github-apps-and-oauth-apps)
- [Generating a user access token for a GitHub App](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app)、[Refreshing user access tokens](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/refreshing-user-access-tokens)、[Authenticating on behalf of a user](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-with-a-github-app-on-behalf-of-a-user)
- [Authorizing a PAT for SAML SSO](https://docs.github.com/en/authentication/authenticating-with-single-sign-on/authorizing-a-personal-access-token-for-use-with-single-sign-on)

机器可读描述（用于核对字段与状态码）：

- [github/rest-api-description](https://github.com/github/rest-api-description)（`descriptions/api.github.com/api.github.com.2022-11-28.yaml`）
- [github/docs](https://github.com/github/docs)（`content/rest/*`、`data/reusables/*`）
