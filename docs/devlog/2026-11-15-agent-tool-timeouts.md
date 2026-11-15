# 目录搜索卡住的恢复与工具超时边界

- 日期：2026-11-15（Asia/Shanghai）；任务：agent-tool-timeouts；协调者整理事实，文档执行者完成 PR 流程。
- 状态：教训与执行约定已写入本分支，待 PR 审查；没有部署强制超时扩展，也没有修改用户全局 Pi 配置。

## 事故与恢复

三个 UI 任务查找 Ely 主题源码时，从根目录递归搜索且未设置 timeout：repository-summary-polish 持续 1 小时 40 分 6 秒，sidebar-quick-actions 持续 1 小时 38 分 8 秒，dark-theme-comparison 持续 1 小时 46 分 35 秒。三者进程存活，但一直在等同一条搜索，尚未修改产品源码。搜索命令末端接了 `head`，但 `head` 只限制输出，不会让上游 `find` 提前结束。

协调者在各自已验证的 Kero pane 使用 Pi 的 Escape 取消当前调用，提供 Cargo.lock 所锁定的 Ely checkout 及 theme 模块位置，再在原会话继续任务。机器上另有一份同名旧依赖 checkout，因此路径存在仍不足以证明版本正确。

Pi 显示取消后，三个 find.exe 子进程仍存在，原父进程已经退出。协调者匹配 PID、创建时间、完整命令和父进程情况后，仅终止这三个孤立搜索，再确认它们退出。没有终止 Pi/Kero/Cargo、改动锁或重建任务。三者随后恢复源码读取和实现活动；不能据此宣称产品任务已经交付。

可复用教训写入 [L028](../../LESSONS.md#l028目录搜索必须同时限定范围和时间取消后核实子进程退出)，执行要求维护在[工作流](../agent-workflow.md#目录搜索与工具时限)。

## 已核验的 Pi 能力

核对本机已安装的 @earendil-works/pi-coding-agent 1.5.2 的源码与文档，并对照当前上游（仓库 `earendil-works/pi`，`packages/coding-agent`）：

- [bash 工具](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/bash.ts)：timeout 是可选秒数；缺省不启动超时计时器，设置后超时触发进程树终止（Windows 走 `taskkill /F /T`，Unix 为进程组信号）。它是单次 bash 调用参数，不是全部工具的统一超时。
- [扩展事件](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)：`tool_call` 可在执行前就地修改 `event.input`，故可补默认 timeout 或钳制上限；修改后不重新校验，扩展自身须验证有限正数、单位和范围。
- [CLI 参数](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/cli/args.ts)：`--no-extensions` 禁用扩展发现和内置扩展，显式 `-e <path>` 仍可加载。本项目派发当前使用该禁用参数，没有自动加入超时扩展。
- [shell 终止实现](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/utils/shell.ts)：Windows 路径调用 `taskkill /F /T` 且失败被忽略；源码注释也说明 shell 的 PID 消失后无法再找到其后代。本次观察到取消后的残留；没有进一步确定该残留的完整底层原因，不能将其推广为所有命令都会残留。
- 模型 HTTP idle timeout 与 shell 工具 timeout 属于不同层，不能相互替代。

以上是源码/文档核验和一次真实恢复的证据；本任务没有构建超时扩展，也没有声称通过完整 Windows 进程树回归。

## 验证与交付

改动仅涉及 LESSONS、工作流和本日志。执行 `git diff --check`，逐个核对新增的相对链接与锚点；不为纯文档变更跑 Rust 编译与测试。最终 PR/head、CI/review 和合并证据记录在本任务 `delivery-state.json` 与 PR，不为记录自身提交号再生成一笔需要重审的提交。

## 后续方案

如实施强制时限，优先仓库维护、显式加载的 Pi 扩展，区分探索、普通命令和有意长任务。普通 read/write/edit 等工具是否支持取消要逐项核验，不能只用 Promise 超时返回就声称底层操作停止。Windows 验证必须包含真正退出的后代进程；不能只断言出现 timeout 字样。保持正在运行的任务会话，按正常退出/恢复边界接入，不能为启用扩展直接批量重启任务。
