# R4 Profile 数据适配与只读 GraphQL POST

任务：`profile-api`；基线 `42291f79bbe73188852bb375fd296623885ce9b8`；分支 `feature/profile-api`；交付 `pr`，首版 milestone #1。

## 目标与状态

实现 R4 Profile 的数据适配基础：REST 用户资料、Pinned/Popular 来源、贡献热力图，并补齐必要的只读 GraphQL POST 传输。状态：实现完成、本地检查通过、待 PR 审查。不接入 data/UI，不定义产品 fallback。

## 改动与决定

- 新增 `src/domain/profile.rs`：`ProfileBasic`、`ProfileRepo`、`ProfilePinned` + `Showcase`、`ContributionCalendar`/`Week`/`Day`/`Month`、`ContributionRange`、`ProfileError`、`GraphqlErrorKind`/`ProfileGraphqlError`。`Showcase` 保留 gist-only/混合/Popular 回退/未知 union 的不确定事实，不替 UI 选择 fallback；`ContributionRange` 校验真实日期、顺序与 366 天跨度。
- 新增 `src/github/profile/**`：`basic.rs`（`GET /users/{login}`）、`graphql.rs`（有界只读 POST、data+errors 解析与安全分类、`require_user`）、`pinned.rs`（`pinnedItems(types:[REPOSITORY])` + `itemShowcase` 联合）、`contributions.rs`（`contributionsCollection.contributionCalendar`）。
- 修改 `src/network/transport.rs`：`HttpMethod::Post`、`HttpRequest.body`、`HttpRequest::post`、`MockTransport::recorded_requests`。`Debug` 脱敏 body。执行时按请求方法发送并携带 body；**POST 一律不跟随重定向**（同源也不跟随），返回 3xx 由上层判定；GET 同源跳转不变。`execute`/`execute_bounded`/blob 契约保持兼容。
- 修改 `src/github/client.rs`：抽出共用 `build`，新增 `build_post`（`Accept`/`Content-Type: application/json`，仅可信 origin 注入 Authorization），`build_get` 行为不变。
- 修改 `src/network/test_server.rs`：按 `Content-Length` 读取并记录 POST body（`RecordedRequest.body`），GET 行为不变。
- `src/github/mod.rs`、`src/domain/mod.rs` 只新增本任务的模块声明与再导出，放在独立命名处。

安全取舍：GraphQL 只发送 query，没有 mutation 入口，传输不重试；主限流按响应头 `x-ratelimit-remaining: 0` 判定（HTTP 200 + errors），与 REST 的 403/429 语义区分；GraphQL error 只保留类型分类与路径，不保留原始 message；POST body 不进日志。

接口、语义与官方依据见 [Profile 数据适配服务](../profile-api-service.md)。`auth-and-network` 只补充 POST 运输说明，未改该文其余归属。

## 验证

Windows 本地全部通过：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（228 passed，0 failed）、`cargo build --locked`、`git diff --check`。重型 Cargo 命令使用 `CARGO_BUILD_JOBS=1`。

新增用例覆盖：REST 资料/404/login 编码；固定 query+variables 的 JSON 编码与 Authorization/Content-Type；真实 localhost POST 方法+body、有界 POST 超限、取消不建连接、同源/跨源 POST redirect 不跟随；GraphQL 200 仅 errors、200 data+errors 部分结果、主限流头、非 2xx、超限、取消；pin 顺序/REPOSITORY 过滤/gist-only/混合/Popular/未知 union/部分结果/未知登录；热力图字段/零贡献/未知 level/权限不足/限流/区间变量；区间格式/真实性/顺序/跨度。既有 GET/blob/network 用例随全量测试执行，无回归。

验证边界：全部为合成凭据与本地 mock/真实 localhost socket，不访问 GitHub，不读取用户 PAT/gh token/Cookie，未做真实 GraphQL 私有贡献、SSO、TLS 联调。未接入 data/UI，未做真实窗口验收。

## 后续

- data 层接线：捕获账户与授权代次，成功/失败后复查，不把本适配器当作授权证据。
- 待产品裁决：个人 README 启发式、2020 旧仓库分享标记、gist-only/混合 pin 的最终呈现、Popular 回退展示、热力图默认区间与刷新。
- 首版 milestone #1；当前无待裁决项。后续若出现争议，按永久 `arbitration_ever_required` 规则处理并交负责人/主 Agent 合并。
