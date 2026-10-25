# 2026-10-25：仓库快照服务

任务 `repo-snapshot-service`，基线 `32ec5acbbe970bbba7f8dc766ef914e5299bea27`，独立分支 `feature/repo-snapshot-service`。本轮为 R3 第一段 REST 适配与领域模型；行为契约、官方依据、调用示例与缺项统一在 [仓库快照服务](../repository-snapshot-service.md)。

实现仓库资料观察、按名字去重的分支完整 Link 分页、动态 ref/固定 SHA 的显式区分、commit 与根 tree 一次解析、单目录按需 tree 展开，以及固定 commit 下 README descriptor 定位。沿用 PR14 客户端、传输、PAT 参数、固定头、同源信任、取消与类型化错误，不读取凭据，不另建生产 HTTP 客户端。未改 Cargo、auth/data/network/storage/content/UI/workspace/app 等并行任务所有文件。

`RepoSnapshot` 携带数字仓库身份、完整 commit/root tree SHA、消息和 nullable Git 原始签名。树 handle 从 snapshot 根或其目录条目生成，保留 commit/tree/path 绑定。返回的 tree URL、submodule URL、download_url 不被请求。未知类型、服务端截断和本地保护上限明确为 Partial；409 明确空仓库或不可用，不能当空目录。README 裸 404 不足以确认缺失；只有随后成功读取同版本完整空树才返回确定缺失，其余保留不确定或传播错误。

实现前无认证读取 GitHub 官方文档，核对 repo、branches、commit、tree、README 的实际路径、ref 和 JSON 字段。确认 `recursive=0/false` 也会开启递归，因此实现完全省略该参数。commit 详情的文件 diff 分页不影响所用静态 commit 元数据；本轮不取 diff 后续页。JSON README 会携带内联 content，适配层忽略而不解码；不提供正文能力。

本地验证（Windows、固定 Rust 1.98.1、独立 `target/`）：

- `cargo fmt --all -- --check`
- `cargo check --locked --all-targets`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked`：157 通过、0 失败，其中本轮新增 13 项测试。
- `cargo build --locked`
- `git diff --check`

新增测试集中在 `src/github/repository/tests.rs`。真实 localhost HTTP + ReqwestTransport 完整串联 repo → 两页分支去重 → 默认分支解析 A → 观察 main 移到 B → A 的根 tree/子树/README；逐条断言真实请求路径和 Authorization/Accept/API-Version/User-Agent。另一个 socket 用例验证斜杠、空格、Unicode、百分号、查询/片段和反斜杠作为 ref 数据进行标准编码，不改变 API 路径或 host。

mock 回归覆盖所有读取入口的 401/403/SSO/主次限流/404/409、预取消与分页中途取消、第一页和后续页传输/解析失败（包括第一页合法空数组）、恶意/循环/损坏分页、SSO partial、页数/条目/响应上限、tree SHA 不符、符号链接/submodule/未知模式、truncated=true/缺失、非法 SHA/路径和 README 404 的证据边界。补充检查防止“损坏 next + 合法 prev”被误判为完整结束，合法 unquoted prev 仍可完成。

验证边界：全部测试凭据均为合成字符串，无真实 GitHub API/PAT/gh token/Cookie 联调。官方文档 GET 与 GitHub PR 交付操作不是产品认证联调。Transport 的 body 大小保护发生在整段字符串接收后，尚不限制传输峰值内存。四平台构建和 CI 以对应 PR 最新提交结果为准。服务未接 UI 或 data，调用方必须对成功/错误都执行账户授权代次检查后再发布；无缓存、正文、渲染、自动重试、隐式轮询或 R3 完成声明。

PR 的最终 head、review/CI 链接、裁决记录与合并结果留在 PR 和任务交接中，避免为追加合并状态产生未经审查的文档提交。
