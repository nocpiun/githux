# 共同代码与验证约定

更新：2026-10-21。只维护当前项目有用的稳定规则；具体业务要求链接到 [产品](product.md)、[架构](architecture.md)、[缓存](cache-strategy.md) 和 [工作流](agent-workflow.md)。

## 改动与并行

- 一次任务围绕明确行为与验收场景，必要的相邻修复可以一起做；无关重构、依赖升级和产品扩展另开任务。
- 每个实现任务使用独立 worktree/index。共享接口、入口文件或依赖锁文件的冲突由主 Agent 协调，不覆盖其他人的改动。
- 用户选项不能被默认值无声覆盖；失败、不可读取和未知不应当作有效空值写回。
- 账户上下文、会话身份与资源版本明确归属；异步回写同时保护数据层和 UI，凭据不进入普通持久化或日志。
- UI 线程不做阻塞 I/O 或大内容解析；长列表与图片资源按实际规模验证，不用多层缓存掩盖重复请求。

## 验证

- 检查应能说明改动的实际行为，围绕触发条件、错误路径和受影响的边界；修复逻辑缺陷时优先提供能复现原问题的回归验证。纯文案等低影响改动不堆镜像实现的测试。
- 单元测试使用明确 fixture 或模拟外部服务，不依赖实时 GitHub；真正的集成验证单独说明账户、环境与影响。测试用临时目录，结束时清理所启动的任务，不改真实用户配置。
- 运行与改动相称的检查，记录实际命令、结果和未覆盖部分。源码推断、模拟结果与真实 GUI/PR 验证分别说明。
- UI 改动在真实 Ely 窗口核验，涉及字体、IME、滚动和窗口状态时包含对应交互；跨平台支持不能仅由 Windows 编译成功推断。
- CI 包含 Windows 代码检查（Node 脚本离线测试、fmt、check、clippy、test）、四平台 Debug 构建，以及末尾显示名称为 `CI` 的汇总 job（ID 为 `ci`）。负责人将在分支规则中把 `CI` 设为 required check；配置存在不代表规则已经启用。汇总 job 要求所有上游均为 `success`，失败、取消、跳过或缺失均不通过。新增上游 job 时，必须同时加入 `ci.needs` 和结果判断，不能遗漏必需检查。Codex 审查仍按 [工作流](agent-workflow.md#合并前的门槛) 单独核验。

## 当前可运行检查

本仓库有审查等待和任务派发脚本的 Node 离线测试；Windows `code-check` 会在 Rust 步骤前实际运行前两条（Node 24，无依赖缓存）：

```sh
node --test .agents/skills/githux-pr-review/scripts/wait-review.test.mjs
node --test .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs
git diff --check
git diff --cached --check
```

Node.js 20+；审查脚本使用本机 `gh`，其测试不访问 GitHub。派发测试在系统临时目录建立一次性 Git 仓库和假 CLI，不访问模型、不打开可见窗口；Windows 进程身份与编码桥有专项覆盖。两项测试已接入 `CI` 的 Windows `code-check`（setup-node 24，无依赖缓存），Rust 步骤和矩阵/汇总逻辑不变。涉及真实 CLI、可见 CMD/TTY 接管或会话恢复的改动仍需相应真实验证，模拟测试不能替代；当前实现的本机实测证据见 [派发开发日志](devlog/2026-10-21-agent-dispatch.md)。

正式应用是单 Cargo package，固定 Rust 1.98.1。Windows 应用检查如下，CI 配置见 [ci.yml](../.github/workflows/ci.yml)：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
cargo test --locked
```

`cargo test` 使用 GPUI 测试平台覆盖实际 Hello 视图的焦点导航、重绘后焦点保留和按键激活，没有为预留模块编写占位测试。该测试不经过 Windows 原生输入分发，不替代真实窗口复测；具体证据和限制见 [开发日志](devlog/2026-10-21-bootstrap-gpui-ely.md)。依赖锁文件必须入 Git；每个 worktree 使用自己的 `target/`。运行步骤和 GUI 验收要求见 [README](../README.md)；CI 首次实际执行与 macOS/Linux 验证不能由本地 Windows 结果推断。

## 交付与文档

PR 描述说明触发条件和结果行为、必要设计取舍与验证证据。行为契约变更与主题文档同 PR 更新；开发日志记录结果与剩余问题，不复制整段终端输出。检查和审查绑定实际最新提交，规则详见 [合并门槛](agent-workflow.md#合并前的门槛)。
