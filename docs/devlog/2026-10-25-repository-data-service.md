# 2026-10-25：仓库账户数据服务

任务：`repository-data-service`；初始基线 `262a013`，交付分支 `feature/repository-data-service`。范围仅 src/data 与本任务主题文档；不改 UI、workspace、app、github/network、领域仓库模型、内容层或依赖。PR18 账户持久化和 PR19 REST 已在基线，未重复实现。

## 实现

- `RepoService::session()` / `RepositorySession` 固定账户 AuthContext 和服务实例，提供 repository / branches / revision / directory / readme 五个最小入口。PAT 仅作为单次请求的后台局部值，不出现在会话、普通数据、Debug 或日志中。
- `Scoped<T>` 返回领域值与 `RepositoryReadContext`；`RepoService::is_current` 供接收端在页面发布前再次检查。错误仍为 `RepositoryError`，调用方保留 session context 对错误做相同校验。页面 TabId 和请求代次留给后续 UI。
- 小型通用认证 helper 同时服务既有 list_repos，统一处理凭据加载、共享并发许可、取消与每个 await 后的成功/错误归属复查。旧 Unauthorized、退休 slot 缺失、worker 错误、取消和 scheduler 错误均以过期上下文优先；凭据加载期间取消也可及时返回，已进入 OS 的 load 本身继续结束，不发后续网络。
- PR19 的固定 commit/tree/README descriptor、完整性及 README 404 不确定状态原样保留。没有跨 Tab 去重、共享订阅、业务缓存或额外调度框架。

## 验证

Windows / Rust 1.98.1，本轮已执行：

- `cargo fmt --all -- --check`：通过。
- `cargo check --locked --all-targets`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- `cargo test --locked`：175 项通过，包含 7 组新增 data 测试及既有账户持久化、repo list、github/network、GPUI 测试平台用例。
- `cargo build --locked`：进行中，提交前补记最终结果。
- `git diff --check`：提交前核验。

新测试使用临时 SQLite、合成凭据和 oneshot / blocking load 可控 gate：五个入口在下一步调用前失效不发送、A→B→A、切账户、PAT replace、remove/re-add、重验、close 的成功/401 均丢弃，退休槽清理期间的 missing / CredentialError / worker panic 不误报新凭据；排队失效不发请求，取消凭据等待/排队/在途可结束且不影响独立读取；跨服务实例相同代次不能混用。完整成功链路验证分支移动后 A 的树和 README 不变、子目录句柄、Partial/409/README 不确定状态与 Debug 脱敏。

新链路为可控 mock 验证；已有 localhost HTTP 与自清理 Windows Credential Manager 测试仍通过。本任务没有读取真实 PAT、真实账户联调、HTTPS 集成或真实 UI 验收，不以模拟证据宣称产品已接通。

## 集成与剩余

实现期间 main 已合入 blob 服务 PR21；交付前同步并验证组合，具体 SHA、CI/review 和合并结果留 PR / delivery-state。PR16、PR20 未操作。

后续 app 装配现有服务、UI 保留 browse context 与页面请求代次并在发布成功/错误前复查、接入 blob 正文、阅读器与业务缓存。私有内容保留政策、切账户的标签产品规则仍未在本任务决定。文档契约见 [仓库账户数据服务](../repository-data-service.md)。
