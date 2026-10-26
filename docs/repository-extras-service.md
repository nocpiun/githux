# 仓库顶部资料 low-level API

更新：2026-10-25。已实现 R4 顶部信息栏的独立 Releases、Contributors、Languages REST 读取；源码为 `src/github/repository_extras/**` 与 `src/domain/repository_extras.rs`。它们是后续顶部资料的真实数据入口，**不是已接入 data/auth context 或 UI 的业务服务**。本切片没有 Packages、Environments、Release 管理写操作、顶部栏布局、缓存、数据库或高频轮询。

## 方法与返回

调用方在已有后台 Tokio 上下文传入 `GitHubClient`、`Pat`、`RepositoryIdentity`、`RepositoryExtrasLimits` 和独立 `CancellationToken`：

```rust
let releases = fetch_releases(&client, &pat, &repository, &limits, cancel.clone()).await?;
let latest = fetch_latest_release(
    &client, &pat, &repository, &limits, None, cancel.clone(),
).await?;
let contributors = fetch_contributors(
    &client, &pat, &repository, &limits, cancel.clone(),
).await?;
let languages = fetch_languages(
    &client, &pat, &repository, &limits, None, cancel,
).await?;
```

- `fetch_releases`：`GET /repos/{owner}/{repo}/releases`，沿受控 `Link` 读取。`Release` 保留服务端 `draft`、`prerelease`、时间、名称和完整 `raw` JSON；assets、author 及未知字段不因投影丢失。草稿按服务端对当前权限的可见性返回，不在客户端推断或扩大可见范围。只读取元数据，绝不下载 asset 或执行其 URL。
- `fetch_latest_release`：`GET …/releases/latest`，返回 `LatestReleaseOutcome`。`release: None` 对应 404，只表示“没有符合条件的可见 Release，或当前 Release/仓库不可见”，不能宣称仓库一定没有 Release。401/403/SSO/限流/解析错误不会变成 None。GitHub 的 latest 是最新非草稿、非预发布版本，按目标提交的 `created_at` 排序，不是按发布动作时间排序；读取器不再对列表或 latest 自行过滤 draft/prerelease 字段。
- `fetch_contributors`：`GET …/contributors?anon=true`，含关联 GitHub 用户与匿名贡献者。`state` 区分 200 `Content`（包括 `[]`）和 204 `EmptyRepository`。`observed_entries()` 仅报告已取得的条目数，不是权威贡献者总数；短首页、分页 Partial 以及 GitHub 只关联前 500 个作者邮箱都不能被解释为完整人数。403 是权限/策略失败，不是空列表。服务端贡献统计可能延迟数小时。
- `fetch_languages`：`GET …/languages`，把语言名映射到代码**字节数**，不是 LOC。`LanguagesOutcome` 以 `u64::checked_add` 计算 `total_bytes`，溢出明确报错；每项 `share` 为派生 0..=1 比例。`{}` 或全零是成功观察，零总量时 `share: None`，不伪造 0%。负数、小数、字符串等非法计数值是响应错误。输出按字节数降序、名称升序，比例计算不依赖溢出后的总字节。

每个 outcome 携带原 `RepositoryIdentity`。所有返回都是本次观察值，不代表永远最新，也不构成授权、缓存新鲜度或永久可见性的证据。

## 有界分页与失败

`RepositoryExtrasLimits` 默认：

| 限制 | 默认值 | 行为 |
| --- | --- | --- |
| `per_page` | 100，必须 1..=100 | 不在客户端请求超出 GitHub 页大小上限 |
| `max_pages` | 10 | 尚有 next 时返回 `Partial(PageLimit)` |
| `max_entries` | 1,000（每个列表调用） | 截断数据或还有 next 时返回 `Partial(EntryLimit)`；恰达上限且没有 next 可为 Complete |
| `max_response_bytes` | 每响应 2 MiB | 经 `Transport::execute_bounded` 在接收阶段限制；后续页超限保留旧页为 `Partial(ResponseLimit)` |

`max_pages * max_response_bytes` 是有界请求次数及读取预算，而不是无限响应聚合。分页逐页完整解析后才提交该页；第一页错误/超限/解析失败为 `Err`。后续页传输、权限、SSO、限流或解析失败保留先前页，并在 `ExtrasCompleteness::Partial(Failed(error))` 携带类型化错误。SSO `partial-results` 同样标 Partial，绝不吞掉。取消在请求前后、后续页和解析后均为硬 `Cancelled`，不返回部分成功。

200 空数组与真正的失败分离。`RepositoryExtrasError` 保留 `GitHubError`、仓库输入/上限错误、非法响应/限制和语言字节溢出。仅分页后续已取数据时使用 Partial；Languages/latest 是单响应，失败不返回有效空值。

`Link` 只允许同源且留在本资源 path 的绝对 URL；userinfo、fragment、跨源、端口/scheme 变化、循环或畸形链接停止并标 Partial。仓库 locator 沿用已有独立 segment 编码与校验。`build_get` 只对受信源注入 PAT，既有传输只跟随同源重定向；没有跟随响应体里的 URL。

## 刷新验证器与后续 data/UI 整合

`ResponseMetadata` 保存每响应的 `ETag`、`Last-Modified`、`Cache-Control` 与可选 `X-Poll-Interval`。分页结果用 `page_metadata` 保持页与验证器的对应关系，不能拿一页 ETag 验证整个聚合。latest/languages 可传相同请求表示的验证器生成 `If-None-Match` / `If-Modified-Since`；测试验证了真实 localhost 请求头。条件请求仍需相同账户/授权、仓库、端点、参数和媒体类型，并尊重 Cache-Control 与服务端限流。没有实现自动重试、调度或 poll。

当前 low-level 对 304 明确返回 `InvalidResponse`，因为它没有缓存值可以发布，绝不把 304 当作空数据。后续 data 任务应：

1. 参照 `RepositorySession` 固定账户、授权 epoch/selection 和服务实例归属，按 session 读取凭据，复用 executor 共享并发许可；成功与失败后都复查上下文，旧响应转为 StaleContext。UI 另行核对页面会话和资源请求代次。
2. 在有明确私有缓存政策后实现存储/刷新；维护每个具体表示的验证器和缓存值，处理 304 与 Last-Modified/ETag，保持观察/Complete/Partial/失败的区别。不得跨账户或跨参数复用验证器。
3. 将独立调用组合到**顶部信息栏**（不建立右栏），单项失败不清空其他资料或阻断文件阅读；观察数量、匿名贡献者、空仓库与 404 可见性歧义使用准确文案。不需要自动轮询。

本任务没有修改 data、network、Cargo、app、workspace 或 UI，也没有依赖 PR16/PR20。

## 官方依据与验证边界

实现前以无认证读取核对了 GitHub 官方 `rest-api-description` 的 `api.github.com.2022-11-28` 描述及对应官方 REST 文档：[Releases](https://docs.github.com/en/rest/releases/releases)、[List repository contributors](https://docs.github.com/en/rest/repos/repos#list-repository-contributors)、[List repository languages](https://docs.github.com/en/rest/repos/repos#list-repository-languages)。这些来源确认 draft 可见性、latest 语义、Contributors 204/匿名/500 邮箱上限与 Languages 字节数；本仓库未将它们当作真实账户联调证据。

测试使用合成 PAT、可控 MockTransport 和 localhost HTTP server；没有读取用户 PAT、gh token 或 Cookie，也没有访问真实 GitHub 业务资源。fixture 覆盖非空/空、204、404、403、预发布/草稿及原始字段、匿名贡献者、多页与截断、后续页错误/超限、SSO/限流、取消、零语言/比例/溢出、条件头和同源 URL。完整运行命令和结果见[开发日志](devlog/2026-10-25-repository-extras.md)。本地模拟不等同于真实 GitHub 的 TLS、组织 SSO、私有可见性或 GUI 验收；这些及四平台 CI 需分别核实。
