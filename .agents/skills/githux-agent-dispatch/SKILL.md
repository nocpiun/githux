---
name: githux-agent-dispatch
description: Create isolated Githux task worktrees and start, inspect or resume the configured coding executor in a visible interactive Windows CMD. Use when dispatching a scoped task or recovering its existing session.
---

# Githux Agent Dispatch

主 Agent 使用本 skill 准备任务并启动配置的执行工具。普通 create/start/resume 在已有任务授权内直接执行，不重复索取批准。执行工具的 `--approve` 代表信任项目文件，不是权限沙箱，也不扩大任务范围。

先读 [配置、命令、状态和恢复](references/operations.md)。脚本入口为本 skill 的 `scripts/dispatch.mjs`，需要 Node.js 20+；实际执行工具可能要求更高版本。可见启动仅支持 Windows，status 不联网。

1. 检查协调仓库分支、worktree 和已有改动；明确任务范围、验收、相关文档及交付模式。首次试跑人数由主 Agent 管理，脚本不设置全局上限。
2. 在忽略的 `.agents/dispatch.local.json` 配置执行工具；凭据只由执行工具既有配置管理。可用 `--config` 指定另一份本地 JSON，不改用户全局配置。
3. 写 UTF-8 任务规格文件，至少包含 `# Goal`、`# Scope`、`# Acceptance`、`# Relevant docs`（也接受目标、范围、验收、相关文档）。写清依赖、共享接口、不做内容和检查。只传必要任务上下文，不复制私人记忆。
4. 普通实现任务使用 `--delivery pr`，让原执行者完成 PR 闭环；实验、诊断或明确本地验收才用 `local-only`。`create` 显式指定已提交 base、新分支、task ID、规格文件、`local-only` 或 `pr`。首版配套任务显式加 `--first-release true`；其他任务不继承 milestone。create 不启动执行工具。
5. 核对输出的 worktree、branch、base SHA 和 `.agents/task/TASK.md`，再 `start`。负责人在可见 CMD 中直接阅读、输入和接管配置的 CLI；脚本用继承的终端运行交互会话，没有 print/RPC 模式。本机真实启动、键盘接管、正常退出和同一会话恢复已验证，证据与边界见 reference。

   确认实际启动成功后，主 Agent 可以结束当前轮次，交代任务/终端位置；不持续读取日志或重复执行检查。后续由负责人手动提醒完成或裁决事项。继续阅读完整过程和交互在可见终端进行。

6. `status` 获取 JSON；`active` 仅来自进程身份存活，不能推断模型正在推理、任务完成或可以合并。任务进度、PR URL 和裁决历史写入 `.agents/task/delivery-state.json`，与监督进程维护的 `dispatch-state.json` 分开；status 合并读取。出现裁决后保持 `true/coordinator`，恢复或换模型不得重置。
7. 要补充任务可在当前终端输入。要通过 `resume` 恢复，先正常退出旧 CLI，确认进程停止，再提供 UTF-8 follow-up 文件。resume 严格恢复原会话，不覆盖 TASK.md 或既有补充。

```sh
node .agents/skills/githux-agent-dispatch/scripts/dispatch.mjs create --task example --branch chore/example --base origin/main --spec task-spec.md --delivery pr
node .agents/skills/githux-agent-dispatch/scripts/dispatch.mjs start --worktree ABSOLUTE_WORKTREE
node .agents/skills/githux-agent-dispatch/scripts/dispatch.mjs status --worktree ABSOLUTE_WORKTREE
node .agents/skills/githux-agent-dispatch/scripts/dispatch.mjs resume --worktree ABSOLUTE_WORKTREE --followup ABSOLUTE_FOLLOWUP
```

以上命令从协调仓库执行；`ABSOLUTE_WORKTREE`/`ABSOLUTE_FOLLOWUP` 替换为绝对路径。create 的相对 `--spec` 从已验证的 `--repo` 根解析；start/resume 的 `--config`/`--followup`/`--worktree` 必须绝对。路径含空格或 shell 字符时，在调用终端按其语法引用整个参数，或通过参数数组调用。脚本内部不把这些参数插入 shell 文本。

模型库存必须精确包含 provider + model；失败就报告，不改用其他模型。已授权换模型时在 resume 显式指定 `--model EXACT_ID`，换 provider 同时指定 `--provider EXACT_PROVIDER`。历史记录中的 configured model 只是传给 CLI 的 ID；provider/model 或 group 名称不证明最终实际使用的后端模型。

独占锁保留到子进程结束。锁缺损、监督进程崩溃、孤儿 CLI、身份不可查或不完整启动均按 unknown 处理；按 reference 人工核验和保留证据，禁止自动杀进程、解锁或删除 worktree。手工外部 attach 不在锁保证范围内，不得与 TUI/RPC 同时写入。

交付与合并遵循 [共享工作流](../../../docs/agent-workflow.md)，PR 审查使用 [githux-pr-review](../githux-pr-review/SKILL.md)。派发脚本不操作 GitHub、不判断审查通过、不合并 PR，也没有自动唤醒、调度器或自动清理。

离线验证（临时仓库、假 CLI，不启动可见窗口或模型请求）：

```sh
node --test .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs
```

可见 CMD、真实 TTY 接管、真实 CLI 会话持久化和真实模型接入由负责人/主 Agent 单独实测；子进程模拟不能证明这些体验已通过。
