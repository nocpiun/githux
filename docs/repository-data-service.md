# 仓库账户数据服务

更新：2026-10-25。R2→R3 的 data 接入已实现于 [`src/data/repository.rs`](../src/data/repository.rs)。复用已有 AccountService、注册表、凭据引用、GitHubClient、BackgroundExecutor 与 CancellationToken；没有新增数据库或 HTTP client。仓库快照与目录句柄的版本关系仍遵守 [仓库快照 REST 契约](repository-snapshot-service.md)。本服务尚未接入真实 UI 或业务缓存。固定 SHA blob 正文与准备后内容已由本 session 接线，见下方“正文读取”。

## 接口与归属

`AccountService::repo_service().session()` 捕获当前活动账户，返回 `RepositorySession`。它只固定授权上下文，不保存 PAT。每步重新从该上下文对应的凭据引用加载 PAT，方法为：

| 方法 | 结果 |
| --- | --- |
| `repository(locator, limits, cancel)` | `Scoped<RepositoryDetails>` |
| `branches(identity, limits, cancel)` | `Scoped<BranchesOutcome>` |
| `revision(identity, spec, limits, cancel)` | `Scoped<RepoSnapshot>` |
| `directory(directory_handle, limits, cancel)` | `Scoped<DirectoryTree>` |
| `readme(snapshot, limits, cancel)` | `Scoped<ReadmeDiscovery>` |
| `blob(identity, limits, cancel)` | `Scoped<BlobRead>`（原始字节或 TooLarge） |
| `content(identity, limits, cancel)` | `Scoped<PreparedContent>`（后台准备后内容） |

所有方法返回原有 `RepositoryError`。GitHub 错误、分页/树完整性、README 不确定状态原样保留；不把失败当作空集合。`RepositoryLimits` 继续来自现有 github 服务。`list_repos` / `list_repos_cancellable` 的签名和 `GitHubError` 契约保持兼容，并复用同一个小型认证 helper。

`RepositoryReadContext` 包含只读 `AuthContext`（host + 稳定账户 ID、授权 epoch、selection generation），还带不可构造的服务实例归属。它不含 PAT。`Scoped<T>` 返回该 context 和领域数据；领域数据自身携带仓库身份、commit/tree/blob 等 PR19 标识。上下文仅在本服务实例内有效，不能序列化后当作重启后的授权。注册表实例的 weak identity 避免两个新服务恰好分配相同 epoch/selection 时误认归属。

多步浏览必须保持同一 session。旧浏览的下一步开始前若已切到新账户，旧 session 在发请求前就返回 `RepositoryError::GitHub(StaleContext)`；不能每步新建 session 再串接旧页面。A→B→A、PAT 替换、移除重加、重验推进代次后旧 session 也不会恢复。调用方可以显式开始新的浏览；固定 SHA 只是内容版本，不提供授权证据，也不允许据此跳过上下文校验。

## await 边界与发布

每次调用按以下顺序执行，同步锁只短暂访问内存，不跨 await：

1. 检查服务未关闭、实例归属及原 AuthContext；同一个注册表锁下复查上下文并取凭据引用。
2. `spawn_blocking` 加载系统凭据，同时允许调用取消及时返回。加载完成先复查上下文，再处理 token、缺失、CredentialError 或 worker JoinError；PR18 已退休并清理的 slot 不会误报为新凭据损坏。
3. 等待 executor 共享 semaphore，与取消竞争。取得许可后再次复查上下文；排队期间失效的读取不发网络请求。持有许可至本次适配器完成/取消，覆盖分支分页和 README 404 的同快照树验证。
4. GitHub 适配器执行，与调用取消竞争。各内部 HTTP await 继续使用同一 token、固定 snapshot 和 cancel；data 不在分页之间重新取活动账户。
5. 所有异步出口统一再次检查上下文，包括成功、Partial、Unauthorized、缺凭据、worker/scheduler 失败和取消。若已过期则 StaleContext 优先，旧结果不发布。服务关闭也使所有旧捕获失效。
6. 仅 `content`：把已取得的 `BlobRead` 移到 `spawn_blocking` 调用 `prepare_content`，准备预算取同一个 `BlobLimits.decoded_bytes`。等待准备与调用取消竞争（取消优先）；准备结束后先收集成功/取消/worker 失败结果，再做一次 `is_current` 并优先返回 StaleContext，通过才传播原结果，与 `read_authenticated` 的过期优先一致。准备 worker 失败为 `GitHub(CredentialStore { message: "content preparation worker closed" })`，沿用现有 worker 失败分类，不新增错误类型。

账户变化不会主动终止所有旧 socket；已经开始的操作可在原凭据下完成，但其成功和错误都会被丢弃。排队等待也可由调用方取消。`close().await` 继续承担账户服务关闭，网络 executor 生命周期沿用已有契约，调用方应取消/等待自己的读取任务再关闭 executor。系统凭据 API 本身不可中断：取消等待后已提交的 blocking load 仍会结束并释放 PAT，不会继续发网络；本层不承诺取消 OS 调用。

检查通过到 UI 实际处理仍可能再次切账户。因此调用方在捕获请求时保留 `session.context().clone()`，收到成功或错误后，用同一个 RepoService 的 `is_current(&context)` 在发布前再验证。成功也可取 `Scoped.context`。未来 UI 还必须验证自己的 TabId、请求代次、仓库/分支/文件选择；本任务不引入 PR16 类型，也不确定切账户时标签如何处理。

## 正文读取

`BlobIdentity` 由调用方从本 session 得到的 `DirectoryTree`（`BlobIdentity::from_file`）或 `ReadmeDescriptor`（`BlobIdentity::from_readme`）构造；`content` 内部就是 `blob` + 后台 `prepare_content`，需要原始字节的调用方用 `blob`。底层固定 SHA 请求、限额、分类规则见 [blob 正文服务](blob-content-service.md)，本层不另设大小上限，也不增加渲染、资源解析或缓存层。

正文读取与快照步骤共用同一个 `read_authenticated`：同一个 semaphore、同一个 CancellationToken、同一份原授权上下文。凭据只在后台 blocking load 中出现，返回值只含 `Scoped.context` 与 `BlobRead`/`PreparedContent`。因此已知 size 超限的**本地** TooLarge 也不是本次授权成功：它仍先经过调用前/凭据后/排队后门槛，并在准备后复查；此时过期或取消按原有优先级返回，不发布 TooLarge。`PreparedContent.identity` 保留构造时的 commit/blob/path，分支移动不会让已派生句柄漂移。

接收方仍须在发布前用同一 RepoService 的 `is_current(&context)` 二次检查成功与错误，再核对页面请求代次与选中文件。`content` 只在当前 Tokio runtime 的 blocking pool 上解码/分类，GUI 线程只做 await；取消等待后已提交的准备工作可能结束，但结果不会绕过上下文门槛。

## 调用示例

在既有后台 Tokio 上下文执行，通过通道回到 GPUI；不在 UI render 内直接调用 reqwest 或读取凭据：

```rust
let repos = accounts.repo_service();
let browse = repos.session()?;
let publication_context = browse.context().clone();
let limits = RepositoryLimits::default();
let metadata = browse.repository(locator, &limits, cancel.clone()).await?;
let branches = browse.branches(&metadata.value.identity, &limits, cancel.clone()).await?;
// 先处理 Partial 与空列表，再让用户选择确切观察值；也可用 RevisionSpec::Ref。
let selected = branches.value.branches.first().ok_or("no branch selected")?;
let revision = RevisionSpec::Commit(selected.observed_commit.clone());
let snapshot = browse.revision(&metadata.value.identity, revision, &limits, cancel.clone()).await?;
let root = browse.directory(&SnapshotDirectory::root(&snapshot.value), &limits, cancel.clone()).await?;
let readme = browse.readme(&snapshot.value, &limits, cancel.clone()).await;
// 由目录条目或 README 描述符派生固定身份，再读准备后正文。
if let ReadmeDiscovery::Found(descriptor) = &readme.as_ref().unwrap().value {
    let identity = BlobIdentity::from_readme(descriptor);
    let body = browse.content(&identity, &BlobLimits::default(), cancel).await?;
    if repos.is_current(&body.context) {
        // body.value.body: Empty / Utf8 / Unsupported / TooLarge
    }
}
// 接收端：成功/错误均检查 publication_context，再核对页面请求代次。
if repos.is_current(&publication_context) {
    // 交由目标页面处理 readme；该页面另行检查 TabId / 当前资源。
}
```

示例的分支选择由调用方处理空列表与 Partial，不把默认分支缺失或仓库大小 0 自动当作空仓库。

每个独立调用使用独立 CancellationToken，可同时调用同一 session 或不同 session。取消其中一个不取消另一个。当前每次直接请求；没有跨 Tab 去重、共享订阅、通用调度框架、自动重试、业务内存/磁盘缓存、私有内容保留政策、Markdown/源码渲染或写操作。

## 验证范围

[`src/data/repository_tests.rs`](../src/data/repository_tests.rs) 使用合成 PAT、临时 SQLite 和可控 transport / blocking load gate，覆盖完整资料→分支→revision→根/子目录→README 链路、分支移动后固定 A、Partial/409/README 不确定状态、每个接口调用前过期、在途成功与错误的 A→B/A→B→A/replace/remove-readd/revalidate/close、退休 slot 缺失/读取错误/worker 失败、凭据等待取消、排队取消与排队失效不发送、在途取消不影响独立调用、跨实例归属和 Debug 不泄露 token。

本轮没有读取真实 PAT 或访问真实账户 GitHub。数据层新链路为可控 mock 证据；已有 github/network 的 localhost socket 和 Credential Manager 自清理测试仍保留。完整检查与实际结果见 [开发日志](devlog/2026-10-25-repository-data-service.md)。正文接线另有 [`src/data/repository_content_tests.rs`](../src/data/repository_content_tests.rs)：repository/revision/tree 与 README 描述符到 PreparedContent 的固定身份链路、分支移动不漂移、原始 blob、已知 TooLarge 无网络但仍过门槛、每个正文接口调用前过期、在途成功/Unauthorized 的 A→B/A→B→A/replace/remove-readd/close、按 BlobIdentity 值键控的准备门控下的切换与取消（含切换后再取消仍 StaleContext 优先）、空/非 UTF-8/控制字符/准备预算、排队取消与失效不发送、独立读取取消隔离、凭据加载取消与替换、准备线程非调用线程。fixture transport 的 `execute_bounded` 只检查已给定字节数，真实接收时限流仍由 PR21 的 socket 测试证明。

这些证据不代表 UI 或真实 GitHub 联调已完成。正文检查与实际结果见 [开发日志](devlog/2026-10-25-repository-content.md)。
