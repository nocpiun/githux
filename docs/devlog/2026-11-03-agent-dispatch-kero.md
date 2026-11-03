# Pi 派发迁移到 Kero

- 日期：2026-11-03；任务：dispatch-kero；执行者：主 Agent。
- 状态：实现及本机验证完成，准备 PR；CI/review 与合并结果以 PR 为准。

## 改动与决定

按负责人要求，所有 Pi 的首次启动和原会话恢复都通过 PATH 中的 `kero` 创建独立本机会话，保留现有 worktree、模型库存、supervisor、进程身份与单写入者锁。Kero 不可用时在持锁前报错，不回退到独立 CMD；不支持远程 `KERO_HOST`，因为本机 PID/worktree 检查不能证明远端状态。

每轮保存 Kero 名称、session/pane ID。启动回执和 supervisor 状态分开写，避免 CLI 已启动或退出时被启动者覆盖；status 不调用 Kero。补充任务使用已核对的明确 pane，正常退出后仍用 dispatch resume 恢复精确 Pi 会话文件。Kero 会话 ID 与 Pi 会话 ID 分开记录。

同一 PR 纳入负责人要求的 dispatch skill `metadata.internal: true`。本机烟测还发现生成 TASK 的验证命令代码块缺少一个闭合反引号，一并修复并增加回归断言。使用与恢复约定维护在 [派发 reference](../../.agents/skills/githux-agent-dispatch/references/operations.md) 和 [工作流](../agent-workflow.md)。

## 验证

- `node --test .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs`：Windows 下 10 项通过。涵盖真实假 CLI 子进程生命周期、原会话恢复、重复启动拦截、裁决记录保留、Kero 不可用/远程选择拒绝、参数边界及独立回执；不请求模型、不创建真实 Kero 会话。
- skill-creator `quick_validate.py`：通过；`git diff --check`：通过。
- 本机真实烟测：通过修改后的脚本 create/start 建立独立测试 worktree 和 Kero 会话，Pi 返回 `KERO_START_OK`；发送中文“你好”后收到 `KERO_GREETING_OK`。通过 `/quit` 正常退出，status 确认 supervisor/child 均已停止、退出码 0、锁释放。
- 使用 follow-up 文件和 dispatch resume，在新 Kero 会话恢复相同 Pi session ID/文件；Pi 返回 `KERO_RESUME_OK` 并正确复述上一轮口令。再次 `/quit` 后确认退出码 0、锁释放，再关闭自己创建的两个测试终端。没有触碰原有用户终端。
- 原始会话与本机配置不入库。烟测证明当前安装版本的交互输入、TTY 和精确恢复；未声称人工验收、多主机派发或模型路由后端已验证，特殊字符路径证据仍限于离线测试。

本任务应用 LESSONS L002 的证据边界：假 CLI 测试和真实 Kero/Pi 烟测分别记录。没有新增独立可复用教训，不重复添加 LESSONS 条目。
