# 首版 GitHub API 能力核验

- 日期：2026-10-22（Asia/Shanghai）；任务 ID：`api-capabilities`；执行者：基层 Agent（本任务独立会话）。
- PR/Issue：任务分支 `docs/api-capabilities`，PR 链接见交接与 GitHub 记录。
- 目标与状态：研究交付，待审查。产出 [GitHub API 能力核验](github-api-capabilities.md)，不实现登录、网络或业务代码。

## 改动与决定

- 新增 `docs/github-api-capabilities.md`：按产品保留范围给出接口/认证/权限/分页/限流/错误与空数据/缓存标识矩阵，并标注“可实现 / 受限 / 需选择 / 无公开等价”。
- 认证部分把 classic PAT、fine-grained PAT、OAuth App（web 与 device flow）、GitHub App user token 分开，说明各自对保留功能（尤其 H1 Notifications）的覆盖差异与桌面 secret 边界。
- 提出的待裁决问题只有三项：H1 是否进首版及对应登录载体、“全部仓库”的语义、Home 参与与 Feed 的口径。均为选项陈述，不是选定结论。
- 与 [缓存策略](cache-strategy.md) 的差异只列依据与建议，未修改该文档或其他主题文档；跨领域更改留给主 Agent 汇总。
- `docs/README.md` 只增加一行能力表索引链接，未改动其他条目。

## 核验方式与边界

- 文档证据：`docs.github.com` 与 `github/docs` 源码、`github/rest-api-description` 的 `api.github.com.2022-11-28` 描述、公开 GraphQL schema、官方 `gh` 源码。
- 实测：用本机已有 `gh` 授权（OAuth app token，只读）验证了仓库、分支、commit、树、内容、README、Releases、Contributors、Languages、Events、Feeds、Notifications、搜索、用户、`/rate_limit`、`/versions` 与若干 GraphQL 查询；确认分页 `Link`、条件请求 `304` 不扣主限额、空数据与 404/409/204 的实际形状、`X-RateLimit-Resource` 分桶，以及 GraphQL 响应没有缓存验证头。
- 未实测并标为待集成验证：私有仓库内容、组织 SSO 与细粒度 PAT 审批、标记通知已读/完成、任何写操作、真实的 fine-grained PAT 与 GitHub App token。
- 未创建 token、未注册 App、未修改远端资源；未输出或记录凭据值。
- 未运行 Rust 编译与 GUI 验证：本任务只改 Markdown，与该检查无关；PR 仍按仓库最新 head 的 CI 与代码审查门槛执行。

## 后续

- 需要主 Agent/负责人裁决的三项问题见能力表末节；其中 Notifications 的 token 支持差异会直接改变登录方案可行性。
- 待登录方案确定后，按“待集成验证清单”补齐私有资源、SSO、通知状态与写操作的真实验证。
- 若裁决要求调整登录方式或仓库集合语义，需要另开实现任务，不在本文范围内直接实施。
