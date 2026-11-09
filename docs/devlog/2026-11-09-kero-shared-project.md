# 2026-11-09：派发会话复用同一 Kero 分组

## 问题与行为

多个独立 worktree 启动时只传 `kero new --dir`，Kero 按目录归类后出现多个显示名称均为 githux 的项目分组。现在启动和恢复先解析协调仓库对应的共享 project ID，显式传 `--project ID`，各 agent 的工作目录和会话仍独立。

本地配置新增可选 `kero_project_id`。未配置时只自动选择唯一匹配目录的分组；多个匹配、无匹配、失效 ID 或其他仓库的 ID 均在创建锁与会话之前返回错误。既有重复分组和会话不自动合并或关闭。ID 记录在终端状态与启动回执，方便验证。

## 验证

离线测试覆盖重复名称与目录、固定 ID 在排序变化后仍保持、未知/异仓库 ID、解析失败不会启动或留下锁、配置校验，以及启动命令保留独立 worktree 参数。原会话恢复与进程锁回归一起运行。`node --test .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs` 共 11 项通过。随后使用修改后的脚本在本机 Kero 恢复 1 个原 Pi 会话、启动 5 个新 Pi 会话；dispatch 核验全部 active，Kero 实际会话清单核验六者 project ID 完全相同，项目列表仅一个 githux 分组。各任务的独立 worktree、session/pane 与进程身份仍保留。此验证读取 Kero API 元数据，未做 GUI 截图，也不代表各业务任务已经完成。