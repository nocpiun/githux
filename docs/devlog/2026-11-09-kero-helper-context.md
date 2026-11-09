# 2026-11-09：辅助会话也必须复用 Kero 项目

## 实际复现

PR #76 已让派发器创建的六个 Pi 主会话共享一个 project ID，但执行者直接运行 `kero new -n nsh-inplace-check -d D:/githux-native-shell -- ...` 时没有传 `--project`。同一任务又创建了三次 check、一次依赖树、一次 build 和一次 run，出现六个新同名分组。会话清单确认主会话仍在共享项目；执行日志确认六个辅助命令绕开派发器。旧任务 worktree 的 skill 基线也早于 #76。

## 修改

把分组要求明确覆盖所有辅助会话，默认复用当前终端。每次 start/resume 将真实解析的 project ID、worktree 和辅助命令约束写入独立 KERO-TOKEN.md，与原 TASK/FOLLOWUP 一起作为文件参数传给 Pi。恢复时生成新的上下文，不改原 TASK、会话 ID、旧 follow-up 或旧上下文。主会话仍使用 #76 的显式 project 参数。

这是补齐执行者上下文，不是拦截任意 kero 命令的全局机制。当前六个执行者已在身份核验后通过 Kero send 收到相同纠正，无需重启；不删除用户会话、历史分组或终端证据。

## 验证

离线子进程回归检查真实收到的 argv：首次启动与恢复都附带 Kero 上下文；恢复采用新项目 ID，保留旧文件和原 TASK；其他锁/身份/裁决测试继续运行。`node --test .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs` 共 11 项通过（0 失败）。现有 Pi 的补充消息发送有回执，但不将发送成功等同于每个 agent 都已执行后续命令。