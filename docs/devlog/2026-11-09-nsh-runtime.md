# nsh-runtime：Native Shell 账户/会话运行时桥接

- 日期：2026-11-09；任务 ID：nsh-runtime；执行者：本任务基层 Agent（无 PR，本地迁移分支）。
- 目标与状态：**runtime-ready-for-integration**。`src/nsh/runtime/**` 已实现并离线验证可复用的账户/页面会话运行时桥接，等待基础作者接线与组合验证。

## 改动与决定

- 新增 `src/nsh/runtime/`：`RuntimeHandle`/`Snapshot`/`Command`/`Completion`、`Backend`/`ExitGate`、`Services`/`PageContext`/`PageTicket`/`PageSlot<T>`、`Sessions`/`Workspace`/`Route`/`TabId`/`PageIdentity`，全部 plain Rust，无 GPUI/Entity/FocusHandle/ScrollHandle，无 PAT 泄露（`Pat` 无 Debug/Display/Serialize）。
- 复用既有服务：`AccountService`（含 `open_with_cache` 的账户账本）、`RepoService::list_repos_cached`（侧栏在线/历史回退）、`PageReadSession`/`RepositorySession`（页面）、`CacheStore`/`CacheAuthorization`（撤权令牌）、`BackgroundExecutor`。不复制业务树、不改 SQLite schema/凭据目标/缓存路径。
- 决定（已在代码注释中写明理由）：
  - 账户命令只读 future 取消后**不丢弃**，等待服务结算再对账，`CancelledSettled` 明确不声称回滚（保留原控制器语义）。
  - `Services` 用 `Arc<AtomicBool>` 在账户变更**意图**时先失效（commit 前），页面无法把旧结果发进新账户；显示列表同时持有 `CacheAuthorization` 与 `RepositoryReadContext`，drain 空 inbox 也做原子校验，实现单次 drain 撤权清屏（L022）。
  - `PageContext`/`PageSlot` 把“结果发布门槛”外化为可复用类型：账户+workspace incarnation+tab+资源+授权令牌逐项复核；失败与成功同门槛，失败不映射为有效空值。
  - `ExitGate` 保留旧 `app::accounts::runtime` 的退出提交门槛语义，但拆成可克隆、非阻塞、带超时的纯 Rust 门。
  - 会话模型只记导航（branch/file），不记 `RepositoryReadContext`/已读内容/在途代次，返回账户时由页面用新授权重建读取，与 `workspace` 现有语义一致。

## 验证

- 隔离探针 `.agents/task/probe/core/Cargo.toml`（`#[path]` 引用本任务源码 + 必要服务层，不含 GPUI/Ely/UI），全部经 `node scripts/verify.mjs -- cargo ...`：
  - `cargo check` 通过（run `3efab6bf`）。
  - `cargo clippy --all-targets -- -D warnings` 通过（run `58fb88e7`；之前 `--lib` 通过 run `bc72b74b`）。
  - `cargo test --lib`：**556 passed, 0 failed**（含既有 548 + 本任务 8，run `43375ac6`；格式化后复跑 `b5560cc9`）。
  - `cargo fmt --all -- --check` 通过（run `66ad7bc9`）；新增文件另以 `rustfmt --edition 2024 --check` 单独核验。
- 本任务 8 项定向测试覆盖：账户 A/B 独立标签且同仓库/同编号不串号；路由大小写别名与 PR 身份；切换与 A→B→A 后迟到结果被拒；替换/删除推进授权并使页面票据失效；缓存打开失败在线回退且 `cache_open_error` 可见；单次 drain 撤权清屏；退出门槛 drain/超时；关闭时注册退出门。
- 未覆盖（如实记录）：未在正式 NSH 二进制接线（`Cargo.toml`/`src/nsh/mod.rs` 属基础作者）；未真实 GitHub/PAT、真实 Windows Credential Manager、真实原生窗口；页面自身 data 读取算法（Home/Profile/Repository 视图）与 Reader/图片/字体/IME 均属后续任务，本任务不宣称 NSH 应用业务已跑通。

## 后续

- 基础作者接线：`src/nsh/mod.rs` 声明 `pub mod runtime;`，`main.rs` 声明 `mod nsh;`；NSH 显示层在每次 drain 后绑定整份 `Snapshot` 到 signal、事件/轻节拍调用 `drain`、事件循环结束后 `ExitGate::wait`。详细接线点见 `src/nsh/runtime/README.md` 与本任务 `.agents/task/integration-notes.md`。
- 集成作者实际接线/组合测试完成前，交付状态保持 `runtime-ready-for-integration`，不标“已跑通正式 NSH 业务”。
- 本任务无需新增 Cargo 依赖；probe manifest 仅本地验证用，不入共享锁文件。
