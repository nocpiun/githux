# 目录搜索卡住的恢复与工具超时边界

- 日期：2026-11-15（Asia/Shanghai）；任务：agent-tool-timeouts；协调者整理事实，文档执行者完成 PR 流程。
- 状态：教训、执行约定与派发扩展的强制缺省超时已在本分支实现，待 PR 审查。扩展随 `githux-agent-dispatch` 显式加载，没有修改用户全局 Pi 配置，也没有重启任何运行中的会话。

## 事故与恢复

三个 UI 任务查找 Ely 主题源码时，从根目录递归搜索且未设置 timeout：repository-summary-polish 持续 1 小时 40 分 6 秒，sidebar-quick-actions 持续 1 小时 38 分 8 秒，dark-theme-comparison 持续 1 小时 46 分 35 秒。三者进程存活，但一直在等同一条搜索，尚未修改产品源码。搜索命令末端接了 `head`，但 `head` 只限制输出，不会让上游 `find` 提前结束。

协调者在各自已验证的 Kero pane 使用 Pi 的 Escape 取消当前调用，提供 Cargo.lock 所锁定的 Ely checkout 及 theme 模块位置，再在原会话继续任务。机器上另有一份同名旧依赖 checkout，因此路径存在仍不足以证明版本正确。

Pi 显示取消后，三个 find.exe 子进程仍存在，原父进程已经退出。协调者匹配 PID、创建时间、完整命令和父进程情况后，仅终止这三个孤立搜索，再确认它们退出。没有终止 Pi/Kero/Cargo、改动锁或重建任务。三者随后恢复源码读取和实现活动；不能据此宣称产品任务已经交付。

可复用教训写入 [L028](../../LESSONS.md#l028目录搜索必须同时限定范围和时间取消后核实子进程退出)，执行要求维护在[工作流](../agent-workflow.md#目录搜索与工具时限)。

## 已核验的 Pi 能力

核对本机已安装的 @earendil-works/pi-coding-agent 1.5.2 的源码与文档，并对照当前上游（仓库 `earendil-works/pi`，`packages/coding-agent`）：

- [bash 工具](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/bash.ts)：timeout 是可选秒数；缺省不启动超时计时器，设置后超时触发进程树终止（Windows 走 `taskkill /F /T`，Unix 为进程组信号）。它是单次 bash 调用参数，不是全部工具的统一超时。
- [扩展事件](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)：`tool_call` 可在执行前就地修改 `event.input`，故可补默认 timeout 或钳制上限；修改后不重新校验，扩展自身须验证有限正数、单位和范围。
- [CLI 参数](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/cli/args.ts)：`--no-extensions` 禁用扩展发现和内置扩展，显式 `-e <path>`/`--extension` 仍可加载。派发保留该禁用参数，只额外加载本 skill 的超时扩展。
- [shell 终止实现](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/utils/shell.ts)：Windows 路径调用 `taskkill /F /T` 且失败被忽略；源码注释也说明 shell 的 PID 消失后无法再找到其后代。本次观察到取消后的残留；没有进一步确定该残留的完整底层原因，不能将其推广为所有命令都会残留。
- 模型 HTTP idle timeout 与 shell 工具 timeout 属于不同层，不能相互替代。

以上是源码/文档核验和一次真实恢复的证据。

## 已实现的派发超时策略

`.agents/skills/githux-agent-dispatch/extensions/bash-timeout.js`（无依赖的 Pi 扩展）监听 `tool_call`，只就地修改内置 `bash` 的输入：缺省 `timeout` 补 `DEFAULT_TIMEOUT_SECONDS=600`；显式有限正数原样保留（20 秒探索搜索、小时级构建/等待都不会被上提或压低）；超过 `MAX_TIMEOUT_SECONDS=10800` 钳到 10800；非数字、`NaN`/`Infinity`、≤0 返回 `block` 并附原因。单位是秒，不做命令文本解析，不改 read/write/edit、MCP、`powershell` 或模型 HTTP 超时。

`dispatch.mjs` 在 start 与 resume 都加一个显式绝对 `--extension <skill>/extensions/bash-timeout.js`，同时保留 `--no-extensions`/`--no-mcp`；路径相对 `dispatch.mjs` 自身用 `realpath` 解析，因此带空格路径可用，恢复旧 worktree 也会用协调侧当前资产。该扩展缺失或不是常规文件时，在创建锁、会话或子进程之前报错，不会静默退化成无时限执行。

生效边界：只对改动之后新启动或 resume 的子进程生效，已在运行的 Pi 会话不被追溯修改；本任务没有重启任何 Agent 会话或 Kero 终端。

## 验证与交付

改动包括 LESSONS、工作流、本日志、派发 skill 的扩展/脚本/测试与其 SKILL.md、reference。执行 `git diff --check` 并逐个核对新增相对链接与锚点；按 CI 的改动分类，本任务只需要 `script-check`（`node --test` 的 review-wait、dispatch、verify 三套），不为技能脚本与文档跑 Rust 编译测试。

本地结果：`node --test .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs` 20/20 通过（新增超时策略语义、显式扩展参数与缺失资产拒绝）；`wait-review.test.mjs` 71/71、`verify.test.mjs` 15/15 通过。共享验证 runner 对这些纯脚本测试仍按重命令加 Cargo 互斥槽，等待中的一次调用未取到槽即被放弃，未观察到新问题；改用直接 `node --test` 跑同一组命令，结论以实际退出码为准。

另做一次有界真实快速核验（已安装 Pi 1.5.2）：探针扩展记录到模型自发的无 timeout `bash` 调用被补为 600；显式 `timeout: 8` 的自建父子进程树被终止，工具返回超时文本，父/子进程随后均不存在。这是单次受控观测，不证明所有命令、其他工具或完整 Windows 进程树清理。

最终 PR/head、CI/review 和合并证据记录在本任务 `delivery-state.json` 与 PR，不为记录自身提交号再生成一笔需要重审的提交。

## 后续方案

剩余未覆盖：其他工具（read/write/edit/MCP）没有统一的强制取消；`powershell` 工具不在策略内；超时只保证触发终止逻辑，不单独证明所有后代都已退出，更多 Windows 进程树形态仍需单独验证；扩展只在新启动的会话生效，旧会话要靠正常 resume 才接入。若后续把 `powershell` 或 MCP 工具也纳入，必须按同一方式在 `tool_call` 修改其实际输入并补验证，不能只用 Promise 超时返回就声称底层操作已停止。
