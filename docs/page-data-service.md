# Home / Profile / 仓库附加资料的账户数据服务

更新：2026-10-30。已实现于 `src/data/page_reads.rs`；它是现有低层 GitHub 适配器与后续页面集成之间的**账户作用域在线读取 facade**，不是 UI view model，也不表示三个页面已经接入真实数据。低层接口、字段和错误契约继续分别见 [Home/Feed](home-query-service.md)、[Profile](profile-api-service.md)、[仓库附加资料](repository-extras-service.md)，本文不替代这些文档。

## 一个 session，一个授权归属

通过 `AccountService::page_session()` 或 `AccountService::repo_service().page_session()` 创建 `PageReadSession`。它一次性捕获与 `RepositorySession` 相同的 `RepositoryReadContext`：host、稳定账户 ID、授权 epoch、selection generation 与服务实例归属；同时从该次活动账户快照得到权威 login。没有 PAT、可序列化凭据或 UI 字符串登录入口。

所有方法复用 `RepoService::read_authenticated`，所以凭据 blocking load、共享 `BackgroundExecutor` semaphore、调用级 `CancellationToken`、开始/加载后/取得许可后/完成后的上下文检查只有一套实现。没有 endpoint 私有 semaphore。A→B→A、替换 PAT、移除重加、关闭服务不会复活旧 session；相同数值的 auth context 在另一个服务实例中也不能通过 `is_current`。

每次调用返回原有 `Scoped<T>`，其中 `value` 是原领域结果，`context` 是发布上下文。错误保留各领域原类型。接收方在发布**成功和错误**前，都必须用创建该 session 的同一 `RepoService::is_current(&context)` 再检查；UI 还要核对自己的标签、目标资源和请求代次。检查与真正更新 UI 之间仍由接收方控制，不把一次成功 await 当成永久授权。

```rust
let repos = accounts.repo_service();
let pages = repos.page_session()?;
let publication = pages.context().clone();
let result = pages.profile("another-user", cancel.clone()).await;
// 在后台 Tokio 任务中取得 result，经通道返回 GPUI。
if repos.is_current(&publication) {
    // 接收端再核对页面/请求代次，然后发布 result 的成功或错误。
}
```

## 给三个页面所有者的接口

### Home / Feed

```rust
let home = pages.home(updated_window, &HomeSearchLimits::default(), cancel.clone()).await?;
let feed = pages.feed(&FeedLimits::default(), cancel).await?;
```

- `home` 固定使用捕获活动账户的 login，查询为 `involves:<login> is:open`、`updated` 降序，最终只发布 open 条目。`Option<UpdatedWindow>` 是显式调用参数；传 `None` 不加时间窗口，不提供隐藏默认日期，不增加 review-requested 关系。
- `feed` 同样派生捕获 login，是显式在线 received-events 读取；不自动轮询。低层适配器已应用 `classify_feed_activity`，所以 `FeedOutcome.events` 只含合并 PR、星标仓库、创建仓库三类活动，保持服务顺序；`FeedOutcome` 原样保留 ETag、`X-Poll-Interval` 和分页 readiness。
- 策略过滤不是分页失败：整页都是技术噪声时仍是 `Complete` 空 Feed，真实上游分页失败仍为对应 Partial。Follow 两类暂缓，不在此层合成事件。
- 本切片不接受、保存或发送 Feed 条件验证器。无验证器却收到 304 时，facade 返回 `HomeError::InvalidResponse`，绝不发布 `NotModified` 或空事件。未来只有具备相同账户/参数/表示的已验证缓存值后，才能另接条件读取。
- `HomeReadiness` / `FeedReadiness` 的 Partial、空集合、SSO、分页、限流和错误语义不变。

### Profile

```rust
let basic = pages.profile(target_login, cancel.clone()).await?;
let pinned = pages.profile_pinned(target_login, &ProfileLimits::default(), cancel.clone()).await?;
let calendar = pages.contribution_calendar(target_login, &range, &ProfileLimits::default(), cancel).await?;
```

`target_login` 只决定查看谁，不改变授权账户。查看他人 Profile 不会令 Home/Feed 改用该 login。GraphQL 的 `data + errors`、Pin/Popular/gist/截断分类、贡献区间与错误模型原样返回，不把局部可用数据包装成全成功。

#### 个人 README helper

```rust
let readme = pages.profile_readme(
    &basic.value,
    &RepositoryLimits::default(),
    &BlobLimits::default(),
    cancel,
).await?;
```

helper 使用 `basic.value.login`（服务端 canonical login）定位 `/{login}/{login}` 对应的公开仓库，然后**在同一个 `PageReadSession` / `RepositorySession`** 中：读取仓库资料 → 要求 public、canonical owner/name 同名且 default branch 存在 → 把 default branch 解析为固定 commit → 读取完整根 tree、只选择根目录普通 `README.md` 文件（不使用会优先 `.github/` 或 `docs/` 的 repository preferred-README endpoint）→ 读取固定 blob 并生成已有 `PreparedContent`。步骤之间不重新捕获活动账户，不接收任意 download URL，不 checkout、不发现 gist、不加载图片。

`Scoped<ProfileReadme>` 的数据状态：

| 状态 | 含义 |
| --- | --- |
| `Prepared { repository, commit, content }` | 已有固定 commit/blob/path 身份的准备后正文 |
| `Missing { repository, commit }` | 完整、未截断的根 tree 证明根目录没有普通 `README.md`；只有 `.github/README.md`、`docs/README.md` 或其他文件也不是 Profile README |
| `Unavailable { repository, commit }` | 根 tree 不完整、截断或发现不确定，不能证明根 README 缺失 |
| `NotEligible { repository, reason }` | 非 public、canonical owner/name 不同名，或没有 default branch |

仓库/commit/README/blob 的网络、权限、解析等失败为 `Err(RepositoryError)`，不伪装成 `Missing` 或 `NotEligible`。即使授权账户能读 private/internal 同名仓库，也不会把它选作 Profile README。旧版 “Share to profile” 标记无法由当前 API 观察，因此本 helper 不承诺识别该历史状态。

### Repository extras

```rust
let releases = pages.releases(&repository, &extras_limits, cancel.clone()).await?;
let latest = pages.latest_release(&repository, &extras_limits, cancel.clone()).await?;
let contributors = pages.contributors(&repository, &extras_limits, cancel.clone()).await?;
let languages = pages.languages(&repository, &extras_limits, cancel).await?;
```

这些方法接收已有 `RepositoryIdentity`；不会用其他账户的 context 为同一仓库发布结果。四个领域调用独立，没有 all-or-nothing 聚合。Releases/Contributors 的页级 `ResponseMetadata`、Latest/Languages 的单响应 metadata 保留供未来集成。Latest 404 歧义、Contributors `observed_entries()` 与 204、Languages 字节数/比例和 Partial/SSO/rate-limit 分类不变。Latest/Languages 无条件读取不发送验证器，意外 304 保持 typed error，不会变成成功的空表示。

## 模型映射与未实现边界

本层不建立第二套 DTO：`HomeSearchOutcome`、`FeedFetch`、`ProfileBasic`、`ProfilePinned`、`ContributionCalendar`、`ReleasesOutcome`、`LatestReleaseOutcome`、`ContributorsOutcome`、`LanguagesOutcome` 和 `PreparedContent` 直接供页面消费。UI 自己决定展示策略和本地 request identity，不接触 PAT。

本轮没有缓存或存储、请求合并、轮询、自动刷新、离线读取、条件重验证或图片下载。fixture preview 与真实数据接入是不同事项；即使后续页面接受这些模型，也不表示已完成生产接线。离线缓存、验证器与表示的对应关系、账户移除后的私有内容政策仍待相应集成任务明确实现。

## 验证

`src/data/page_read_tests.rs` 通过 `AccountService::open`、临时 SQLite、合成 PAT 和可控 mock transport 验证：每个 endpoint 的 URL/query/login/Authorization/limits；GraphQL partial；extras 模型与 metadata；无验证器 304；public/private/internal/no-default-branch/非 canonical 同名仓库；README 缺失/不确定/失败与固定身份；A→B→A、PAT replacement、remove/re-add、close、service-instance mismatch；慢凭据、共享许可等待、取消、worker failure，以及旧成功/错误/GraphQL partial 的 stale 优先。没有真实 PAT、真实 GitHub 请求或 GUI 验证。
