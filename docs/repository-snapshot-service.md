# 仓库快照 REST 服务

更新：2026-10-25。R3 第一段独立适配服务，复用 [认证与网络](auth-and-network.md) 的 `GitHubClient`、PAT 参数、Transport、取消令牌、同源信任、固定请求头、错误分类与 Link 解析。此服务没有接入 UI/data 缓存，不替代侧栏 fixture，也不表示 R3 完成。

## 已实现接口

领域类型位于 [`src/domain/repository.rs`](../src/domain/repository.rs)，REST 适配位于 `src/github/{repository,branches,revision,tree,readme}.rs`。

| 接口 | 请求及结果 |
| --- | --- |
| `fetch_repository` | `GET /repos/{owner}/{repo}` → `RepositoryDetails`，包含数字稳定仓库 ID、规范 owner/name、描述、可见性、默认分支、fork/archived、Stars/Forks、含 PR 的 open issues 计数、订阅数（可选）、KB 体积、网页/clone 链接与 pushed_at |
| `fetch_branches` | `GET …/branches?per_page=100` 并沿 Link 分页 → `BranchesOutcome`，按名字保留第一次观察值，携带完整性和类型化失败原因 |
| `resolve_revision` | `GET …/commits/{ref}` → `RepoSnapshot`，包含仓库身份、请求 ref/commit、完整 commit SHA、根 tree SHA、消息、Git 原始作者和提交者的名字/email/时间（允许 null） |
| `fetch_directory` | `GET …/git/trees/{tree_sha}` → `DirectoryTree`，按目录 SHA 单次读取，保留文件/可执行文件/目录/符号链接/submodule/未知类型、size、truncated 和完整性 |
| `discover_readme` | `GET …/readme?ref={完整commit SHA}` → descriptor 或明确的不确定结果；404 时另读一次同快照根 tree，只有完整空树才确认无 README |

Stars、Forks、默认分支和分支 HEAD 都只是本次请求的观察值；服务没有 TTL、磁盘缓存、隐式轮询或自动重试。GitHub 统计本身也可能滞后。仓库大小为 0 不证明空仓库。REST `open_issues_count` 包括 PR，不可标为纯 Issue 数。

## 固定版本不变量

`DynamicRef` 表示可移动名称，`ObjectSha` 只接受 40 位十六进制 SHA-1 并规范为小写；`RevisionSpec` 明确区分二者。解析完整 SHA 时会校验响应仍是该 SHA。

`RepoSnapshot` 的字段不提供公开构造器。后续 README 只接受 snapshot；树只接受 `SnapshotDirectory`，其根由 snapshot 生成，子目录由返回树的目录条目生成。目录 handle 同时携带仓库 ID/locator、commit、tree 和相对路径。响应 tree SHA 必须与请求对象一致。树请求从不使用分支名，也完全省略 `recursive` 参数，展开目录不递归下载其他树。符号链接不解引用，submodule 不连接其仓库。

分支移动到 B 后，已解析 A 的树、子树、README 仍使用 A 的对象标识。需要刷新时由调用方显式解析新快照，再协调页面切换。分支列表跨页也不是原子快照，去重只消除重复，不声称能消除期间的增删；需要选中列表中确切观察值时用 `RevisionSpec::Commit(branch.observed_commit)`。

最小调用顺序（在已有后台 Tokio 上下文执行，PAT 由 auth/data 传入）：

```rust
let repo = fetch_repository(client, token, &locator, &limits, cancel.clone()).await?;
let branches = fetch_branches(client, token, &repo.identity, &limits, cancel.clone()).await?;
// 调用方选择分支，或使用 repo.default_branch；缺少默认分支不能自动当空仓库。
let revision = RevisionSpec::Ref(selected_branch);
let snapshot = resolve_revision(client, token, &repo.identity, revision, &limits, cancel.clone()).await?;
let root = fetch_directory(client, token, &SnapshotDirectory::root(&snapshot), &limits, cancel.clone()).await?;
let child = root.child_directory(directory_index)?;
let subtree = fetch_directory(client, token, &child, &limits, cancel.clone()).await?;
let readme = discover_readme(client, token, &snapshot, &limits, cancel).await?;
```

API 与模型没有 GPUI、TabId 或账户选择状态。**调用方仍必须经 data 层捕获账户/授权/选择代次，并对成功和错误都复查代次后才发布结果。** 服务的稳定仓库 ID 不是账户隔离键，固定 SHA 也不是读取权限证据。后续缓存仍须按账户和授权上下文隔离；页面回写另校验目标会话与当前资源。

## 错误、完整性与保护上限

`RepositoryError::GitHub` 保留既有 401、403、SSO required、主/次限流、RetryAfter、超时、断网、取消与解析错误。409 单独为 `EmptyOrUnavailable`，不当作空目录。一般 404 保留 `NotFound` 的“不存在或当前不可见”语义。所有请求使用同一取消令牌；分页中途取消仍是硬错误，不返回已取消操作的部分数据。

分支第一页失败返回错误；第二页以后失败即使第一页合法返回空数组，也返回 `Partial`，携带原类型化错误和已取得条目。SSO partial、分页循环、恶意/异常 Link、页数或条目超限都明确 Partial。所有 next 必须通过既有 client 同源校验，且仍指向同一 branches 路径；拒绝 userinfo、fragment 和其他端点。初始路径使用 `reqwest::Url::path_segments_mut`，查询用 query_pairs 编码，owner/name、ref 和 SHA 不拼成外来 URL。响应里的 API URL、tree URL、download_url 都不会被请求。

树保留 `truncated: Option<bool>`：true 或缺失均不完整；未知 type/mode 原样保留并标 Partial。未知和超限不伪装成完整空目录。仓库路径拒绝绝对路径、反斜杠、空段、`.`/`..` 和控制字符；URL 查询/片段字符作为合法文件名数据，不在本轮转为文件请求。

集中配置在 `RepositoryLimits`，默认值如下：

| 参数 | 默认 | 超限行为 |
| --- | --- | --- |
| branch_pages | 50 | Partial/PageLimit；0 表示不发请求，返回明确 Partial |
| branch_entries | 50,000 | 保留上限内去重结果，Partial/EntryLimit |
| tree_entries | 100,000 | 保留上限内条目，Partial/EntryLimit |
| response_bytes | 8 MiB | 仓库/commit/README/首个分支页返回 LimitExceeded；后续分支页保留 Partial；树返回无条目的 Partial/ResponseLimit，truncated 为 unknown |

现有 Transport 先把 body 缓冲为 String，再由本服务检查字节上限，因此上限限制解析和服务结果，不是网络接收/峰值内存硬上限。需要流式大小保护时应协调 network 所有者，不复制 HTTP 客户端。

## README 定位边界

README 返回 `ReadmeDescriptor { repository, commit, path, blob, size }`，无 blob 正文请求、base64 解码、Markdown 渲染或临时下载 URL 标识。使用已有固定 `Accept: application/vnd.github+json`；官方 JSON 响应含内联 content，本服务接收响应但忽略该字段，不把它暴露为正文能力。

404 后查询同快照根 tree：完整空树给出 `AbsentInEmptyTree`；可读非空或不完整树给出 `NotFoundOrInaccessible`。不自行枚举 GitHub 所支持的 README 扩展名/优先级，非空仓库即使看起来没有 README 仍保留不确定语义。根 tree 的 401/403/404/409/取消/网络错误向上传递，不被降级为空或确认缺失；解析时可见不保证之后仍可见，不能缓存裸 404 为永久缺失。

## 官方核验与验证

本轮实现前以无认证 HTTP GET 读取 GitHub 官方 REST 文档，核对路径、参数、JSON 字段、nullable Git 作者、模式、分页、README ref 和截断：

- [仓库资料](https://docs.github.com/en/rest/repos/repos#get-a-repository)
- [分支列表](https://docs.github.com/en/rest/branches/branches#list-branches)：每页最大 100。
- [单 commit](https://docs.github.com/en/rest/commits/commits#get-a-commit)：ref 可为 SHA/heads/branch/tags；文件 diff 分页不影响本服务使用的静态 commit 元数据，不请求剩余 diff。
- [Git tree](https://docs.github.com/en/rest/git/trees#get-a-tree)：recursive 参数任何值都会开启递归，必须省略；递归上限 100,000 项/7 MB，truncated 要保留。
- [README](https://docs.github.com/en/rest/repos/contents#get-a-repository-readme)：ref 支持 commit/branch/tag，推荐 JSON accept，blob SHA/size/path 来自 descriptor。

[`src/github/repository/tests.rs`](../src/github/repository/tests.rs) 包含 mock 回归与真实 localhost ReqwestTransport 串联：两页分支去重、默认分支解析、观察移动到 B 后 A 的根/子树/README 请求不变、固定头与安全编码、取消、首/后续页失败、错误分类、未知/截断/超限树、symlink/submodule、非法输入及 README 404 的证据边界。全部凭据为合成测试字符串；不请求真实 GitHub API、不读取用户 PAT、gh token 或 Cookie。官方文档读取不等于真实 GitHub 认证联调。

本轮缺项：data 接入与代次封装、UI、blob/正文、渲染、磁盘缓存/离线、在途合并、递归树、README 非空树缺失的进一步证据、真实私有/SSO 联调，以及流式传输限额。后续阅读器应消费本服务的 commit/tree/blob 标识，不能回到每步重读 main。
