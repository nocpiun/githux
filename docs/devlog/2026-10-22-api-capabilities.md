# 首版 GitHub API 能力核验

- 日期：2026-10-22（Asia/Shanghai）；任务 ID：`api-capabilities`；执行者：基层 Agent（本任务独立会话）。
- PR/Issue：任务分支 `docs/api-capabilities`，PR 链接见交接与 GitHub 记录。
- 目标与状态：研究交付，待审查。产出 [GitHub API 能力核验](../github-api-capabilities.md)，不实现登录、网络或业务代码。

## 改动与决定

- 新增 `docs/github-api-capabilities.md`：按产品保留范围给出接口/认证/权限/分页/限流/错误与空数据/缓存标识矩阵，并标注“可实现 / 受限 / 需选择 / 无公开等价”。
- 认证部分把 classic PAT、fine-grained PAT、OAuth App（web 与 device flow）、GitHub App user token 分开，说明各自对保留功能（尤其 H1 Notifications）的覆盖差异与桌面 secret 边界。
- 提出的待裁决问题有三项本轮必须决定（H1 是否进首版及对应登录载体、“全部仓库”的语义、Home 参与与 Feed 的口径），另有第四项可在 Pinned/Popular 实现时再定（gist-only 用户的仓库热门回退）。均为选项陈述，不是选定结论。
- 与 [缓存策略](../cache-strategy.md) 的差异只列依据与建议，未修改该文档或其他主题文档；跨领域更改留给主 Agent 汇总。
- `docs/README.md` 只增加一行能力表索引链接，未改动其他条目。

## 核验方式与边界

- 文档证据：`docs.github.com` 与 `github/docs` 源码、`github/rest-api-description` 的 `api.github.com.2022-11-28` 描述、公开 GraphQL schema、官方 `gh` 源码。
- 实测：用本机已有 `gh` 授权（OAuth app token，只读）验证了仓库、分支、commit、树、内容、README、Releases、Contributors、Languages、Events、Feeds、Notifications、搜索、用户、`/rate_limit`、`/versions` 与若干 GraphQL 查询；确认分页 `Link`、条件请求 `304` 不扣主限额、空数据与 404 的实际形状（含 `releases/latest` 404、Languages `200 {}`）、`X-RateLimit-Resource` 分桶，以及 GraphQL 响应没有缓存验证头。空仓库 409 与 Contributors 204 未构造实测，只引用官方描述。
- 未实测并标为待集成验证：私有仓库内容、空仓库 409 / Contributors 204、组织 SSO 与细粒度 PAT 审批、标记通知已读/完成、任何写操作、真实的 fine-grained PAT 与 GitHub App token。
- 未创建 token、未注册 App、未修改远端资源；未输出或记录凭据值。
- 未运行 Rust 编译与 GUI 验证：本任务只改 Markdown，与该检查无关；PR 仍按仓库最新 head 的 CI 与代码审查门槛执行。

## 修复轮次（审查反馈）

- 第一轮审查指出三处：能力表误把 Notifications 写成“仅 classic PAT”（端点段落说的是“不支持细粒度 token”，OAuth scope 参考与官方对照页都明确 `notifications` scope 可读通知）；Pinned/Popular 回退的数据源写错（应查 `itemShowcase`，只查 `pinnedItems` 无法得到 Popular）；开发日志的相对链接缺 `../`。三处均已核实并修正。
- 第二轮审查指出：不应把“不能保密 `client_secret`”直接推导为“桌面方案应走 device flow”。官方 [OAuth App 最佳实践](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/best-practices-for-creating-an-oauth-app) 把桌面应用归为 public client，建议随包发行 secret 并用 PKCE 加固，且**优先 authorization code + PKCE 而非 device flow**（device flow 无回调 URI，更易被远程钓鱼滥用）。已改为两个流程均保留、默认候选是 web flow + PKCE + loopback，并补充了各自的 secret 与钓鱼风险。
- 第三轮审查指出：`/user/orgs` 对细粒度 token 会返回 `200` + 空列表，不能与 `/user/memberships/orgs` 当作等价。已核实官方端点说明并修正：表格与要点里明确该差异，错误表新增一行，并建议组织枚举统一改用 memberships 端点。
- 第四轮审查指出两处：GraphQL 主限流耗尽返回 HTTP `200` + `errors`（次级为 200 或 403），只看状态码会误判；issue 语义/混合搜索另限 10 次/分钟。两处均已核实（实测 `search_type=semantic|hybrid` 返回 `X-RateLimit-Resource: semantic_search`、`X-RateLimit-Limit: 10`）并在错误表与限流节补充。
- 第五轮审查指出：`itemShowcase.hasPinnedItems` 计的是仓库**或 gist**，`items` 是 `PinnableItem` 联合，gist-only 用户无法从 `itemShowcase` 拿到仓库热门回退。已按 schema 文本修正 Pinned/Popular 条目与要点，并把它作为第四项候选问题（可延后到该功能实现时再定）。
- 第六轮审查指出：交接里仍写“三项”，与能力表已列四项不一致。已把开发日志里两处“三项”更新为四项并注明各自时点。
- 第七轮审查指出：OAuth App 也支持 expiring tokens（8 小时 access + 6 个月可轮换 refresh，`expires_in` 等响应字段有条件出现），不能只在 GitHub App 行里写这个生命周期。已核实授权文档并在两个 OAuth 流程行与桌面 secret 边界里补充。

## 后续

- 需要主 Agent/负责人裁决的问题共四项（见能力表末节，其中三项本轮必须决定，Pinned/Popular 的 gist-only 回退可在实现时再定）；其中 Notifications 的 token 支持差异会直接改变登录方案可行性。
- 待登录方案确定后，按“待集成验证清单”补齐私有资源、SSO、通知状态与写操作的真实验证。
- 若裁决要求调整登录方式或仓库集合语义，需要另开实现任务，不在本文范围内直接实施。
