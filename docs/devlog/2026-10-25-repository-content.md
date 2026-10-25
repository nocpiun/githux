# R3 正文数据层接线（RepositorySession blob/content）

- 日期：2026-10-25；任务 ID：repository-content；执行者：基层 Agent。
- PR：https://github.com/nocpiun/githux/pull/23 ；目标 main，milestone First Release (#1) 已核验为 1。
- 目标与状态：实现完成、本地检查通过，待 PR 审查。基于已合并 PR21（blob/prepare_content）与 PR22（RepositorySession），base 42291f79bbe73188852bb375fd296623885ce9b8。

## 改动与决定

- `src/data/repository.rs` 新增两个最小接口：`RepositorySession::blob(&BlobIdentity, &BlobLimits, cancel) -> Scoped<BlobRead>` 与 `content(...) -> Scoped<PreparedContent>`。不修改 github/network/content/domain 类型，不引入新 auth helper、缓存或 UI。
- `blob` 直接复用 `RepositorySession::read` → `RepoService::read_authenticated` → `github::blob::fetch_blob`，因此 semaphore、取消、固定原授权、成功/错误的最终上下文复查与其他快照步骤一致。
- `content` 先 `blob`，再把 `BlobRead` 移到 `spawn_blocking` 执行 `prepare_content`，预算取同一 `BlobLimits.decoded_bytes`；等待与取消竞争（biased 取消优先），准备完成后再次 `is_current` 才返回。准备 worker 失败沿用现有 `GitHub(CredentialStore { message: "content preparation worker closed" })` 分类，未新增错误类型。
- 已知 size 的本地 TooLarge 不是授权成功证据：仍走全部上下文门槛，准备阶段也不例外；过期/取消优先于 TooLarge 发布。
- 调用方仍持 `session.context().clone()`，在 UI 应用成功或错误前用同一 RepoService 的 `is_current` 二次检查，另核对自己的页面代次和选中资源。契约更新在 [仓库账户数据服务](../repository-data-service.md)。
- 仅 `#[cfg(test)]` 的 session 专属 PreparationProbe 在分类完成后暂停 worker 并返回其线程 ID，确定性验证“准备后、发布前”的交错与 worker 失败；非测试构建不含该字段或代码。没有全局门控或全局线程记录。

## 审查修复（第一轮）

- Codex P1：准备阶段取消/worker 失败原先提前 return，绕过最终 `is_current`。改为先收集 outcome、统一复查上下文后再传播，StaleContext 优先；新增“切换后再取消”用例。
- Codex P2：全局单槽准备门控可能被并行测试抢走。最终采用 session 专属探针；即使不同 session 读取完全相同的 BlobIdentity，也不会抢占门控。移除全局线程记录，在线程不迁移的 current-thread runtime 上直接核验目标准备 worker 的线程 ID。
- 交付前补充确定性验证：准备成功后 replace/remove-readd；当前 worker 失败保持类型化错误、切换后 worker 失败为 StaleContext；两个读取同时在途时取消一个；凭据加载线程与调用线程不同。修正凭据 fixture 在阻塞等待时持有 gate mutex 的交错风险。

## 验证

在 `D:\githux-repository-content`、Rust 1.98.1、每命令 `CARGO_BUILD_JOBS=1`、本 worktree 独立 target：

- `cargo fmt --all -- --check`：通过。
- `cargo check --locked --all-targets`：通过。
- `cargo test --locked`：200 passed / 0 failed（原 189 + 新增 11）。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- `cargo build --locked`：通过。

新增 `src/data/repository_content_tests.rs`（`src/data/mod.rs` 仅加 cfg(test) mod 声明），覆盖：repository→revision→tree→BlobIdentity→content/blob 与 README 描述符链路、分支移动后固定 SHA 不漂移、Accept 强制 JSON、context 相同且 is_current；空/非 UTF-8/二进制控制字符/准备预算；已知 TooLarge 不发请求但在调用前过期、准备中切换时仍被丢弃；blob/content 调用前过期无 I/O；在途 200/401 × switch、A→B→A、replace、remove-readd、close；准备门控下 switch/A→B→A/close/cancel（含本地 TooLarge）；排队取消与排队失效不发送；在途取消不影响另一个独立读取；凭据阻塞加载取消、加载期间替换、当前缺凭据仍为类型化错误；准备线程与调用线程不同；成功与 Unauthorized 到达后再切账户，接收方通过保留 context 的 `is_current` 二次检查拒绝发布。

边界：全部使用合成 PAT、临时 SQLite 与可控 mock transport，没有读取真实 PAT、gh token 或访问真实 GitHub；未做 UI/GUI 验证。fixture 的有界读取只检查已给出字节数，真实 socket 接收时限流仍由 PR21 测试证明。无真实账户联调结论。

## 后续

- 等待 PR 最新 head 的 CI 与 Codex 审查；全程无裁决才可由本执行者 squash 合并（保留 GitHub 默认提交信息）。
- 本任务不含 reader UI、Markdown/图片渲染、业务正文缓存、跨 Tab 去重或写操作。
