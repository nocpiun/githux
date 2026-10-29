# 认证、网络与账户上下文（R2 首个切片）

更新：2026-10-23。本文记录 **Classic PAT 身份验证、Windows 安全凭据存取、账户授权代次、可取消的 GitHub 读取服务** 的实现范围、接口、验证证据与未覆盖项。

**范围事实：** 本次交付的是真实服务基础，不是空 trait 占位，也不是完整登录产品。UI 尚未接入（那属于后续 UI 任务），因此 binary 里这些模块暂未被调用，`auth/network/github/data/domain/diagnostics` 各自带一个**模块级** `#![allow(dead_code)]`，并写明了原因；crate 根没有 `#![allow]` 兜底。首个切片不含账户落盘；后续已实现独立 SQLite 账户目录、活动选择和恢复接口，详见 [账户持久化](account-persistence.md)。私有业务内容落盘与重启后多账户 UI 仍未实现。仓库快照 REST 已通过固定账户上下文的读取会话接入 data，接口、跨步骤归属与发布复查见 [仓库账户数据服务](repository-data-service.md)。

## 1. 已实现的内容

| 层 | 位置 | 已实现 | 未实现（明确边界） |
| --- | --- | --- | --- |
| domain | [`src/domain`](../src/domain) | `AccountKey`/`HostId`/`AccountIdentity`/`AccountSummary`/`AuthEpoch`、`GitHubError`、`RepoSummary`/`ReposOutcome`/`ReposReadiness` | 仓库详情、分支、内容模型 |
| auth | [`src/auth`](../src/auth) | `Pat`（Debug 脱敏，无 Display/Serialize）、`CredentialStore` 抽象、Windows Credential Manager 适配、内存 fake、多账户注册表与代次、最小 scope 建议；后续账户恢复见专文 | UI 登录流程 |
| network | [`src/network`](../src/network) | 独立 Tokio 后台执行器、有界并发信号量、可取消令牌、reqwest 传输、仅同源重定向、localhost 测试服务 | 常驻调度服务、通用重试框架 |
| github | [`src/github`](../src/github) | 固定请求头、受信 host 才注入 Authorization、`GET /user`、`GET /user/repos` 完整 `Link` 分页、错误分类、SSO/限流头解析、写权限占位边界 | H7 写操作、GraphQL |
| data | [`src/data`](../src/data) | 登录（先验证后保存）、替换/移除推进代次、过期响应丢弃、仓库读取 | 缓存、在途请求合并 |
| diagnostics | [`src/diagnostics`](../src/diagnostics) | URL 脱敏、诊断事件结构 | 遥测、性能采样、更新机制 |

## 2. 账户身份与授权代次

**稳定身份 = `HostId` + 数字账户 ID**，来自 `GET /user` 的 `id`。`login` 只用于显示，改名不会新建账户；`AccountRegistry::upsert_verified` 用 `AccountKey`（host + id）匹配。

`AuthEpoch` 是每个账户的单调授权代次：

| 操作 | 代次变化 |
| --- | --- |
| 首次验证通过 | 从注册表的单调序列取值 |
| 替换 PAT（`AccountService::replace`，同一账户重新验证） | 重新分配，必然大于旧值 |
| 重复验证同一账户（`login`/`upsert_verified`） | 重新分配，必然大于旧值 |
| 移除账户 | 账户删除，活动指针修正；**不移交也不复用**旧代次 |

代次来自注册表级别的单调分配器（`AccountRegistry::next_epoch`），不是每账户计数器。这样“移除 A → 重新加入 A”不会回到旧值：ABA 场景下旧的 `{account, epoch}` 捕获永远无法再匹配活动上下文。

`AuthContext` 还带一个 `selection` 也即“活动账户选择代次”，在**每次活动账户真正变化**时推进（切换、新增后自动激活、移除后重新选择）。只靠 `{account, epoch}` 不够：A → B → A 会恢复同一对值，切换前发出的响应就会被误接受。回归测试为 `auth::tests::switching_away_and_back_invalidates_in_flight_context`，另有 `selecting_the_already_active_account_does_not_bump_selection` 确认重复选中同一账户不算切换。

`AccountService::replace(expected, pat)` 是更换已有账户 PAT 的**唯一**入口：先 `GET /user` 验证，再比对 `expected`；token 属于别的账户时返回 `IdentityMismatch`，两个账户都不会被保存或改动。`login()` 是“添加账户”路径，不承担替换语义。

读取请求前捕获 `AuthContext { account, epoch, selection }`，返回后再次比对；不一致返回 `GitHubError::StaleContext`，数据被丢弃，不进入新账户。仓库多步读取使用 `RepositorySession` 固定原上下文，并以 `RepositoryReadContext` 标记服务实例，不能每步重新捕获活动账户继续旧浏览；详见 [data 接入契约](repository-data-service.md#await-边界与发布)。

`AccountRegistry` 只保存 `AccountIdentity` + `CredentialRef`，**从不持有 token**。

## 3. 凭据存储

```rust
pub trait CredentialStore: Send + Sync {
    fn save(&self, reference: &CredentialRef, token: &Pat) -> Result<(), CredentialError>;
    fn load(&self, reference: &CredentialRef) -> Result<Option<Pat>, CredentialError>;
    fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError>;
    fn describe(&self) -> &'static str;
}
```

- **Windows**：`WindowsCredentialStore`（`src/auth/windows.rs`）使用 `CredWriteW`/`CredReadW`/`CredDeleteW`，`CRED_TYPE_GENERIC` + `CRED_PERSIST_LOCAL_MACHINE`，基础 target 名 `githux:<host>:<account_id>`；持久化替换用 `githux:<host>:<account_id>:<uuid revision>` 独立槽（均不含密钥）。`ERROR_NOT_FOUND` 视为 `Ok(None)`/幂等删除。
- **其他平台**：`UnsupportedStore` 一律返回 `CredentialError::Unsupported`。**没有明文回退**，不会把 token 写进配置或数据库。
- **测试**：`InMemoryCredentialStore` 只存在于进程内，测试实例互不共享；`failing_saves()` 注入保存失败，用来验证“保存失败不宣称登录成功”。
- **真实 Windows 项**：唯一一个真实 `Credential Manager` 测试用自己的 account id `0xDEADBEEFCAFE`，测试开始先删、结束删除，失败也删（cleanup guard），不读取或修改任何真实凭据。

`Pat` 的设计约束：

- 无 `Debug`/`Display`/`Serialize`；`Debug` 恒为 `Pat(<redacted>)`。
- `expose()` 是 `pub(crate)`，只有 GitHub 层构造 Authorization 时使用。
- `Drop` 覆写自持缓冲区。

## 4. 网络与执行模型

**决策（2026-10-23）：`reqwest 0.13.5`（`native-tls` + `http2` + `charset` + `system-proxy`）+ `tokio 1`（`rt`/`sync`/`time`/`macros`/`net`）。** 依据：GPUI 自带的执行器基于 smol，不驱动 Tokio future；reqwest 提供 HTTP/2、显式重定向控制与稳定 async 接口。我们把整个网络生命周期放进独立 runtime，用通道把结果送回调用方，避免在 GPUI 任务里轮询 reqwest。传递依赖里出现过的 hyper/tokio 不作为“已采用”的依据，实际选型以本文件与 `Cargo.toml` 为准。

```rust
let executor = Arc::new(BackgroundExecutor::new(4)); // 4 = 同时进行中的请求上限
let handle = executor.handle();
let task = handle.spawn(async move { /* 读取逻辑 */ });
let result = task.await;
executor.shutdown(); // 停止接收、宽限期内排空、否则取消并 join
```

- `BackgroundExecutor` 在专用线程上建 current-thread runtime；`handle()` 返回标准 `tokio::runtime::Handle`。
- `semaphore()` 暴露共享信号量，`AccountService`/`RepoService` 在发起请求前 `acquire_owned()`，把并发限制在构造时给的额度。
- `CancellationToken` 基于 `watch`，自带 keepalive，**丢弃 source 不等于取消**；只有显式 `cancel()` 才触发。已取消的 token 在传输层直接返回 `Cancelled`，不建连接。
- 关闭：`shutdown()` 先拒绝新工作，然后**继续驱动 runtime** 直到被跟踪的任务全部完成或宽限期（默认 5s，可用 `set_grace` 覆盖）到期，之后才开始 runtime 关闭。这一步很关键：直接调用 `Runtime::shutdown_timeout` 并不是宽限期——Tokio 文档明确说明 shutdown 开始后，spawned task 只在让出之前继续运行、一旦 yield 就被丢弃，所以先 `shutdown_timeout` 会立刻杀掉在途请求而不是给它收尾机会。
- 只有经 `BackgroundExecutorHandle::spawn` 派发的任务会被计数并纳入排空；`raw_handle()` / `handle.raw()` 提供未经跟踪的原始 `Handle` 给不得不自行 spawn 的代码，这类任务不享受宽限。
- `spawn` 在关闭开始后返回 `SpawnError`。接受标志与 owner 共享，因此存活到关闭之后的 handle 不会再把任务投递给正在销毁的 runtime（`spawn_is_rejected_after_shutdown_begins`）。
- 计数守卫在 `spawn` **之前**构造并移入 future。若任务在 runtime 首次轮询前就被 abort，future 被未轮询地丢弃，守卫随之析构、计数回落；把守卫写在 async block 内部会在这种情况泄漏计数，使关闭为一个从未运行的任务白等整个宽限期（`aborting_an_unpolled_task_releases_its_in_flight_slot`）。
- **自线程 drop 安全**：最后一个 `Arc<BackgroundExecutor>` 可能由网络线程上刚结束的任务释放。若此时 `Drop` 直接 `join()` 自己会死锁。网络线程用 thread-local 标记自己的身份，`Drop` 检测到“在当前线程上”时只发关闭信号并分离 join handle，不 join 自身。
- **自线程 drop 安全**：最后一个 `Arc<BackgroundExecutor>` 可能由网络线程上刚结束的任务释放。若此时 `Drop` 直接 `join()` 自己会死锁。网络线程用 thread-local 标记自己的身份，`Drop` 检测到“在当前线程上”时只发关闭信号并分离 join handle，不 join 自身。

**重定向与凭据保护**（`ReqwestTransport`）：reqwest 配置 `Policy::none()`，由我们自己跟随；只用同样的 scheme+host+port 才继续，跨源、降级到 HTTP 或超过 5 跳都**不跟随**，直接把 3xx 交给上层判定。`GitHubClient::url_is_trusted` 再检查分页 `Link` 的 host，不受信的 next 链接返回部分结果而不是继续请求。

host 判定使用**与 reqwest 相同的标准 URL 解析器**（`reqwest::Url`），不再自己写拆分逻辑。原因是自写的解析器必然在某处与真正的连接方不一致：WHATWG 对特殊 scheme 把 `\` 当作路径分隔符，所以 `https://evil.example\@api.github.com/user` 实际连到 `evil.example`，而按最后一个 `@` 手拆会得出 `api.github.com`。用同一个解析器做“是否受信”的判断和真正发起连接，才能消除这类不一致。

信任要求 scheme、host **和 port** 都一致（端口缺失时用该 scheme 的默认端口比较）：`https://api.github.com:8443/user`、`HTTP` 降级、`//api.github.com/user`、`ftp://…` 都不受信。回归测试为 `github::client::tests::path_content_does_not_make_a_host_trusted` 与 `localhost_mock_is_trusted_only_on_the_configured_origin`（含反斜杠、查询串、`@`、不同端口等用例）。

`ReqwestTransport` 的取消与超时覆盖**整个请求生命周期**：

- header 阶段和 body 读取阶段都在 `tokio::select!` 里与取消竞争，因此大响应或停滞响应在取消后不会继续读到请求超时（`cancellation_stops_a_stalled_body_read`）。
- **body 阶段的超时同样是 `Timeout` 而不是网络错误**，并且两个阶段都报告**实际配置的**时长（向上取整，所以 600ms 报 1s 而不是 0s），避免 UI/重试文案出现“0 秒超时”（`body_phase_timeout_is_typed_and_reports_the_configured_duration`、`timeout_is_classified`）。
- 等待并发信号量许可时也参与取消竞争：所有许可被占时，已取消的读取立即返回 `Cancelled`，不会排队等到别的请求走完整个超时与重定向链（`data::tests::cancelling_a_queued_read_returns_without_waiting_for_a_permit`）。

`HttpRequest` 的 `Debug` 是手写的，会脱敏 `Authorization`/`Proxy-Authorization`/`Cookie`（`request_debug_redacts_credentials`）。Authorization 在请求里是普通头字符串，派生 `Debug` 会绕过 `Pat` 的脱敏把 token 打进日志或 panic 信息。

**重定向的 origin 判定同样使用 `reqwest::Url`**，且 `Location` 用标准的 URL join 解析：`https://evil.example\@api.github.com/steal` 在 WHATWG 规则下宿主是 `evil.example`（反斜杠是特殊 scheme 的路径分隔符），手写解析器会误判为 `api.github.com` 并把 Authorization 重新发给外域。回归测试为 `network::transport::tests::origin_comparison_uses_reqwest_url_semantics` 与 `redirect_with_backslash_does_not_forward_credentials`（后者用真实 socket 确认攻击目标从未被连接）。

**只读 GraphQL POST（R4 新增）**：`HttpRequest` 增加 `Post` 方法与可选 body，`GitHubClient::build_post` 生成 `POST {base}graphql`（`Accept`/`Content-Type: application/json`，仅可信 origin 注入 Authorization），供 Profile 的 Pinned/Popular 与贡献热力图使用。它只发送 query，没有 mutation 入口；走 `execute_bounded` 的接收上限，复用取消与超时。**POST 一律不跟随重定向**（同源也不跟随）：避免 302/303 把方法降级为 GET，或 307/308 把含变量的 query body 重发到别处；3xx 由 GraphQL 层判为错误。`HttpRequest::Debug` 对手写 body 也脱敏。GET 的同源跳转与既有 `execute`/`execute_bounded`/blob 接口保持兼容。详见 [Profile 数据适配服务](profile-api-service.md)。

## 5. GitHub 读取

**固定请求头**：`User-Agent: githux/0.1 (+https://github.com/nocpiun/githux)`、`Accept: application/vnd.github+json`、`X-GitHub-Api-Version: 2022-11-28`。

**`GET /user`**（`fetch_authenticated_user`）→ `AuthenticatedUser { identity }`，是保存凭据前的唯一权威身份核验。

**`GET /user/repos`**（`fetch_user_repos`）：

- 参数 `affiliation=owner,collaborator,organization_member&per_page=100&sort=pushed`。
- 沿 `Link: rel="next"` 读完全部分页；上限 50 页（防御性，不是正常路径）。
- `Link` 按 RFC 8288 形状解析：只在 `<...>` 目标**之外**、且不在带引号参数值内部的逗号处分段。GitHub 的分页链接本身就可能包含逗号（本客户端首请求就带 `affiliation=owner,collaborator,organization_member`），按逗号无条件切分会把 URL 劈成两半、取不到 next，于是停在第一页并错误地报告 `Complete`。回归测试为 `github::headers::tests::next_link_with_commas_inside_the_target` 与 `github::repos::tests::pagination_follows_a_next_link_with_commas_in_its_query`。
- 结果类型同时携带数据与完整性：

| 情形 | 结果 |
| --- | --- |
| 所有页成功、无 next | `ReposReadiness::Complete` |
| 第一页就失败 | `Err(GitHubError)`，不返回空列表 |
| 后续页失败（已有数据） | `Partial { reason: PageFailed }` |
| `X-GitHub-SSO: partial-results` | `Partial { reason: SsoPartial { organizations } }` |
| next 指向不受信 host | `Partial { reason: PageFailed }` |
| 第 2 页起传输失败（超时/断网） | `Partial { reason: PageFailed }`，保留已解析页 |
| 第 2 页起 JSON 解析失败 | `Partial { reason: PageFailed }`，保留已解析页 |

因此“中途失败”“授权不全”和“解析失败”都不会被伪装成完整空列表；只有第一页失败才是硬错误。

**过期错误也按代次处理**：`RepoService` 在请求结束后对**成功和失败**都重新核对 `AuthContext`。旧 PAT 返回的 `Unauthorized` 会被转换成 `StaleContext`，不会被当成“新凭据失效”，从而不会误触发替换提示。

**分页中途取消仍是硬取消**：即使在获取第 2 页及以后时取消，也返回 `Cancelled` 而不是 `Partial`——调用方已经要求停止，就不应拿到那次请求的数据。

**后续持久化变更**：SQLite、系统凭据均已移到 blocking worker；持久化选择必须 await AccountService，替换采用独立凭据槽与可恢复 cleanup journal。以下描述为首个内存切片背景，现行跨存储行为与失败契约以 [账户持久化](account-persistence.md) 为准。

**替换、登录与移除都序列化提交**：`AccountService` 用一把 `credential_commit` 互斥锁把“写/删凭据 + 提交注册表记录”包成一个临界区。移除也必须参与，否则并发的 `remove` 与 `replace` 可能在 replace 通过存在性复查后：remove 删掉旧凭据并移除账户，replace 再把新 PAT 写进存储并返回 `NotFound`，留下没有任何账户引用、也没有清理路径的孤儿秘密。回归测试为 `data::tests::removal_is_serialized_with_replacement`。否则两个并发 `replace` 可能 A 写 token A、B 写 token B 并提交、A 再提交，最终存储在 B、注册表却报 A 的 epoch，后续读取会在 A 的 epoch 下使用 B 的 token。回归测试用真正的多线程 runtime 与一个会在凭据操作重叠时报警的 store 验证（去掉锁会稳定失败）。

**移除账户的凭据删除失败会上报**：`AccountService::remove` 仍会移除注册表条目（不让陈旧引用保持活动），但把删除失败作为 `CredentialStore` 错误返回，避免声称“已成功移除”而实际仍留有持久化秘密。

**错误分类**（`github::error::classify_status`）：

| 状态 | 类型 | 说明 |
| --- | --- | --- |
| 401 | `Unauthorized` | token 失效/撤销，走替换流程，**不自动删除账户** |
| 403 + `X-RateLimit-Remaining: 0` | `RateLimited { Primary }` | 主限流 |
| 403 + `Retry-After` | `RateLimited { Secondary }` | 次级限流 |
| 403 + `X-GitHub-SSO: required` | `SsoRequired { authorization_url }` | 组织 SSO 授权 |
| 403（其他） | `Forbidden` | scope/组织策略不足，**不删除 token** |
| 404 | `NotFound` | 不可见，不等于不存在 |
| 429 | `RateLimited { Secondary }` | 服务端负载/限流 |
| 5xx/其他 | `UnexpectedStatus` | |
| 传输超时 | `Timeout { seconds }` | |
| 断网/连接失败 | `Network { message }` | |
| 主动取消 | `Cancelled` | |
| JSON 解析失败 | `Parse { message }` | |

`Retry-After` 与 `X-RateLimit-*` 解析成 `RateLimitHeaders`/`RetryAfter` 供上层决定退避；本切片**不实现自动重试循环**，避免在服务层留下无限重试。

## 6. 最小权限建议

`auth::capability` 返回能力与最小 scope，**不作为登录硬门槛**，也不索取管理权限：

| 能力 | 最小 scope | 说明 |
| --- | --- | --- |
| 公开仓库读取 | 无（登录即可） | |
| 私有仓库读取 | `repo` | `repo` **同时含写权限**，因此不把 token 描述成只读 |
| 私有 profile 字段 | `read:user` | |
| Notifications | `notifications` 或 `repo` | 细粒度 PAT 不支持该端点 |
| 组织信息 | `read:org` | `user` 只覆盖 profile，**不能**替代 `read:org` |

`report_after_identity_check()` 只把“公开读取”标为 `Available`，其余为 `Unknown`：`/user` 成功不能证明具体 scope 存在，也不能证明组织策略允许。UI 不应把 `Unknown` 渲染成“不可用”。创建引导 UI 与最终 scope 选择仍留待 UI 接入讨论。

## 7. 写能力边界（未实现）

`github::permission` 只有占位语义：任何 `WriteAction` 当前都返回 `WritePermission::Unknown`，`is_write_confirmed_allowed` 只对 `Allowed` 为真。读取成功或存在 `repo` scope **不**构成写权限证据。H7 动作清单与写队列不在本 PR。

## 8. 调用示例（给后续 UI 任务）

```rust
// 组装（在 UI 任务里由 app 层提供，且必须放在后台）
let store: Arc<dyn CredentialStore> = Arc::new(WindowsCredentialStore::new());
let transport = Arc::new(ReqwestTransport::new(Duration::from_secs(30))?);
let client = Arc::new(GitHubClient::new(GitHubConfig::default(), transport));
let executor = Arc::new(BackgroundExecutor::new(4));
let accounts = AccountService::open(
    AccountDatabaseConfig::application_data()?, store, client, executor.clone(),
).await?; // 恢复账户为 PendingVerification，不能当作已验证授权

// 登录：先 GET /user 验证，再保存；保存失败返回 CredentialSave，不会假装已登录
match accounts.login(Pat::from_str(user_input)).await {
    Ok(LoginOutcome::SignedIn(summary)) | Ok(LoginOutcome::Replaced(summary)) => {
        let account: AccountIdLike = summary.identity.key; // host + id，稳定身份
        let epoch = summary.epoch;                        // 授权代次
        // 展示用：summary.identity.login / display_name / avatar_url
    }
    Err(LoginError::CredentialSave(e)) => { /* 明确提示"未保存"，不要提示登录成功 */ }
    Err(LoginError::GitHub(GitHubError::Unauthorized)) => { /* token 无效 */ }
    Err(err) => { /* 其他类型化错误 */ }
}

// 更换已有账户的 PAT：显式给出期望账户；token 属于别的账户时返回 IdentityMismatch
match accounts.replace(existing_account_key, Pat::from_str(new_input)).await {
    Ok(LoginOutcome::Replaced(summary)) => { /* 新 epoch 生效，旧响应会被判定为过期 */ }
    Err(LoginError::IdentityMismatch { expected_account, actual_account }) => {
        // 拒绝把账户 A 静默换成账户 B
    }
    Err(err) => { /* 类型化错误 */ }
}

// 读取仓库
let repos = accounts.repo_service();
match repos.list_repos().await {
    Ok(outcome) => {
        // outcome.repositories: Vec<RepoSummary>（只读展示数据）
        // outcome.readiness: Complete | Partial { reason } —— 不要把 Partial 显示成完整列表
    }
    Err(GitHubError::StaleContext) => { /* 账户/代次已变，丢弃 */ }
    Err(GitHubError::RateLimited { retry }) => { /* 按 retry.seconds 提示退避 */ }
    Err(err) => { /* 类型化错误 */ }
}
```

**GPUI 桥接方式**：UI 动作把请求交给 `BackgroundExecutor`（或一个持有它的服务），通过 `tokio::sync::mpsc` / oneshot 把结果送回；GPUI 侧用现有的 `cx.spawn` + 通道接收后再更新实体，**不要** `block_on`，也不要在 GPUI 任务里直接 `await` reqwest future。`AccountService`/`RepoService` 的方法本身是 async，调用方负责提供 Tokio 上下文（例如在 `executor.handle().spawn(...)` 内执行），这样 UI 线程只做状态更新。

**UI 只接触**：`AccountKey`/`AuthEpoch`（或 `AccountSummary`）、展示元数据、`RepoSummary` 列表、`ReposReadiness`、`GitHubError`。UI 不应持有 `Pat`，也不应自己读凭据。

## 9. 验证证据

全部为本地测试，不访问 GitHub、不使用真实用户凭据：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
git diff --check
```

覆盖内容：

| 场景 | 测试位置 | 方式 |
| --- | --- | --- |
| Authorization 构造与固定头 | `github::client`、`network::transport::tests::real_localhost_request_carries_authorization` | 真实 localhost socket，检查收到的头 |
| `GET /user` 身份核验、401 | `github::user` | mock 传输 |
| 两页以上 `/user/repos` 合并 | `github::repos::merges_two_pages`、`data::tests::localhost_end_to_end_login_and_paginated_repos` | mock + 真实 localhost 两页 |
| 部分分页失败 | `github::repos::partial_on_mid_page_failure`、`data::tests::localhost_partial_page_failure_is_partial` | 503 后保留已取数据 |
| SSO 部分结果 | `github::repos::sso_partial_results`、`network::transport::tests::sso_header_parsed_from_real_response` | 200 + `X-GitHub-SSO` |
| 限流 / Retry-After / 不同 403 | `github::error::tests`、`network::transport::tests::rate_limited_response_is_classified` | 真实响应头解析 |
| 超时 | `network::transport::tests::timeout_is_classified` | 真实 socket 不响应 |
| 取消 | `network::cancel::tests`、`network::transport::tests::cancelled_request_does_not_affect_independent_request` | 已取消 token 不建连接 |
| 换账户/PAT 后丢弃旧响应 | `data::tests::stale_response_after_account_replacement_is_discarded`、`stale_response_after_removal_is_discarded` | 请求进行中推进代次 |
| 旧 PAT 的过期错误不误判新凭据 | `data::tests::stale_error_after_replacement_becomes_stale_context` | 传输层在返回 401 前替换 PAT |
| 替换入口拒绝别的账户 token | `data::tests::replace_rejects_a_token_for_a_different_account`、`replace_unknown_account_is_not_found` | 两账户都不被改动 |
| 代次不复用（ABA） | `auth::tests::epochs_are_registry_wide_monotonic_across_removal_and_readd`、`repeated_verification_never_reuses_an_epoch` | 移除后重加 |
| 凭据删除失败上报 | `data::tests::remove_reports_credential_deletion_failure` | 注入 delete 失败 |
| URL authority 结构化解析 | `github::client::tests::path_content_does_not_make_a_host_trusted` | 路径/查询中的 host 无法冒充受信 |
| 第 2 页起传输/解析失败保留已取数据 | `github::repos::tests::transport_failure_on_later_page_keeps_earlier_pages`、`parse_failure_on_later_page_keeps_earlier_pages` | |
| 带逗号的 next 链接仍翻页 | `github::repos::tests::pagination_follows_a_next_link_with_commas_in_its_query`、`github::headers::tests::next_link_with_commas_inside_the_target` | affiliation 含逗号 |
| 取消覆盖 body 读取阶段 | `network::transport::tests::cancellation_stops_a_stalled_body_read` | header 已到、body 停滞时取消 |
| 取消一个请求不影响另一个 | `data::tests::cancelling_one_read_does_not_affect_another`、`network::executor::tests` | |
| 后台执行器收敛 | `network::executor::tests::shutdown_converges_without_orphaned_work`、`concurrency_is_bounded_by_the_semaphore` | 长任务被取消、短任务完成、峰值并发受限 |
| 凭据保存失败不宣称持久化 | `data::tests::credential_save_failure_does_not_persist` | 注入失败 |
| 令牌不泄露 | `auth::token::tests`、`diagnostics::tests` | Debug/URL 脱敏 |
| 真实 Credential Manager | `auth::windows::tests::windows_credential_roundtrip` | 本机真实 Windows 项，自清理 |
| 跨源重定向不转发凭据 | `network::transport::tests::cross_origin_redirect_is_not_followed` | 真实 socket |
| 移除活动账户 / 活动前账户 | `auth::tests::remove_active_account_activates_previous`、`remove_before_active_keeps_the_same_active_account`、`removing_every_account_leaves_no_active_context` | |
| 反斜杠/端口/相对形式不走信 | `github::client::tests::path_content_does_not_make_a_host_trusted`、`localhost_mock_is_trusted_only_on_the_configured_origin` | 与 reqwest 同一解析器 |
| 分页中途取消仍是 `Cancelled` | `github::repos::tests::cancellation_on_later_page_is_not_a_partial_result` | 取消先于第 2 页 |
| 并发替换被序列化 | `data::tests::overlapping_replacements_are_serialized` | 多线程 runtime + 重叠检测 store |
| 执行器自线程 drop 不死锁 | `network::executor::tests::dropping_from_its_own_network_thread_does_not_deadlock` | 弱引用确认最终释放 |
| 关闭真的排空（不是立刻杀任务） | `network::executor::tests::shutdown_drains_a_task_that_finishes_within_the_grace_window`、`shutdown_does_not_wait_forever_for_a_task_beyond_the_grace_window` | 多次 yield 的任务能跑完 |
| 重定向 origin 用标准 URL 语义 | `network::transport::tests::origin_comparison_uses_reqwest_url_semantics`、`redirect_with_backslash_does_not_forward_credentials` | 反斜杠、默认端口、真实 socket |
| A→B→A 切换使旧响应过期 | `auth::tests::switching_away_and_back_invalidates_in_flight_context` | selection 代次 |
| 移除与替换互斥 | `data::tests::removal_is_serialized_with_replacement` | 多线程 + 重叠检测 store |
| 组织 scope 不接受 profile-only | `auth::capability::tests::organization_read_does_not_accept_profile_only_scope` | 仅 `read:org` |
| body 阶段超时保持类型化并报告配置时长 | `network::transport::tests::body_phase_timeout_is_typed_and_reports_the_configured_duration` | 1s 配置 → `Timeout{seconds:1}` |
| 排队等许可时可被取消 | `data::tests::cancelling_a_queued_read_returns_without_waiting_for_a_permit` | 单许可 executor |
| 关闭后拒绝 spawn | `network::executor::tests::spawn_is_rejected_after_shutdown_begins` | 存活 handle |
| 未轮询即 abort 也释放计数 | `network::executor::tests::aborting_an_unpolled_task_releases_its_in_flight_slot` | 去掉修复会在 32s 后失败 |
| 请求 Debug 脱敏凭据 | `network::transport::tests::request_debug_redacts_credentials` | Authorization/Cookie |
| 多账户与代次 | `auth::tests` | |
| 最小 scope 建议 | `auth::capability::tests` | 不含管理 scope |

### 验证边界（重要）

- **真实 GitHub 认证尚未验证。** 本 PR 没有读取 `gh auth token`、浏览器 Cookie 或任何用户文件，也没有对 `api.github.com` 发请求。真实 PAT 联调要等用户在产品 UI 里显式提供 token。
- **真实 Windows Credential Manager**：只有“用自己的唯一测试项读写删”这一条在本机实际执行通过；产品路径（登录→保存→重启→读取）现已接入正式 Shell，但真实 PAT 联调仍未验证，须由负责人在原生窗口输入 token。
- **local mock 端到端**：真实 socket、真实 HTTP/1.1 解析、真实 reqwest 客户端，但服务端是仓库内的最小 mock。
- **HTTPS/TLS**：本地 mock 走明文 HTTP（仅限 localhost 测试 base URL）。生产 `api.github.com` 的 TLS 路径由 reqwest + native-tls 承担，未在 CI 中真实连通验证。
- **依赖选择**：`reqwest 0.13.5` + `tokio 1.54` 在固定工具链 1.98.1 上本地编译、测试通过；四平台 CI 编译以 PR 检查为准，本地无法代替。

## 10. 未覆盖 / 后续

- 账户目录与引用已由 [独立 SQLite 持久化服务](account-persistence.md) 实现，包括 v1 迁移、活动选择、重启恢复和跨存储清理。UI 尚未接入，私有内容缓存及其保留政策仍待确认。
- UI 接入：登录表单、账户切换、仓库列表渲染、错误呈现；本 PR 不修改 `src/ui/**`、`src/app/**`、`src/main.rs`。
- 在途请求合并与缓存：本切片每次读取直接请求，没有缓存层。
- 自动重试/退避调度：只返回 `RetryAfter`，不自行重试。
- H7 写操作、H1 Notifications、GraphQL：未实现，`github::permission` 仅占位。
- 代理支持：使用 reqwest `system-proxy` 默认系统代理，未做显式配置或测试。

## 相关文档

- [架构方案](architecture.md) · [缓存策略](cache-strategy.md) · [GitHub API 能力核验](github-api-capabilities.md)
- [产品约定：登录与未来写能力](product.md#登录与未来写能力)
- [本轮开发日志](devlog/2026-10-23-pat-data-service.md)
- [仓库账户数据服务](repository-data-service.md) · [仓库快照 REST](repository-snapshot-service.md)
