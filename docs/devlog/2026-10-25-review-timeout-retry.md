# Codex 审查等待超时后单次恢复

- 日期：2026-10-25；任务：review-timeout-retry；执行者：主 Agent 亲自实现。
- 状态：实现与本地验证完成，准备 PR；实时 CI/review 以 GitHub 为准。

## 改动与决定

负责人要求将总等待时限先调整为一小时，并在未收到审查时自动补发 `@codex review`。现默认每轮 3600 秒，首次到期后重新核对审查与 PR；仍无结果才补发一次并再等一轮，不无限循环。保留 `--timeout-seconds` 覆盖，增加 `--auto-retry false` 只读开关。

重试使用 GitHub 返回的评论 ID/created_at 作为新轮次边界。任务目录中按 repo/PR/head 保存排他创建的发送意图与确认回执；发送结果不明不重发，重启不刷新第二轮期限。已有完成证据、CI 单独等待、历史问题/证据不齐、未知消息、新触发、head 改变、关闭或冲突均不会通过重复评论来处理。

共享契约见 [Agent 工作流](../agent-workflow.md) 与 [超时恢复](../../.agents/skills/githux-pr-review/references/review-evidence.md#超时恢复)。此次未改变审批/合并权限，不自动解决历史线程；此前审计中的其他建议另行处理。

## 验证

- `node --test .agents/skills/githux-pr-review/scripts/wait-review.test.mjs`：58 项全部通过，包含原有 42 项回归。
- 新测试使用假时钟和临时目录覆盖两轮时限、准确 POST、服务端新游标、重启恢复、发送不明/取消、竞争记录、只读模式、已完成/CI 等待、临界时刻关闭/推送/冲突及结果文件隔离。
- skill frontmatter 校验和 `git diff --check` 通过。
- 未实际等待两小时或向其他任务 PR 发送测试评论；模拟测试不证明云端 Codex 收到重试后必定恢复。未改应用代码，未重复 Rust 构建。

## 后续

Codex 摘要没有唯一 run ID；同一 SHA 重试后仍需由执行者核验重叠/矛盾证据，脚本不自动批准。已在运行的 Node 等待进程不会热更新；现有任务完成当前等待后，需同步新版脚本与 skill 再使用新默认值。
