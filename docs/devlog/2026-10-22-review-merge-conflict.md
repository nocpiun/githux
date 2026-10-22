# 2026-10-22：等待审查时识别合并冲突

## 问题与行为

PR #8 的 e66ffccead0dbf2fe7b1166aeffdd2ea9a6b3975 已收到完成且无 findings 的 Code Review，但与目标分支冲突，最新提交没有 CI。原等待器只查 head/关闭状态，把缺失 CI 继续当成 pending，直到超时；上一轮历史 findings 的修复已经同步，不能解决本次问题。

等待脚本现在在每轮首次查询及交回证据前读取的 PR 响应中检查 `mergeable === false`，立即返回 `merge_conflict` / 退出码 8，提供目标分支 ref/SHA 和处理提示。无需额外 API 调用。只等待 review 时也报告冲突；PR 关闭、head 改变仍优先，null 不误判为冲突，Draft 或保护状态也不误判。

执行者先同步目标分支、处理冲突并推送，再针对新 head 请求审查；不能原样重启等待器。语义取舍按裁决流程处理。完整契约见 [skill reference](../../.agents/skills/githux-pr-review/references/review-evidence.md#合并冲突)，交付规则见 [工作流](../agent-workflow.md)。PR #8 的人工审查限制保持有效，本修复没有操作其工作区、进程或合并权限。

## 验证

- `node --test .agents/skills/githux-pr-review/scripts/wait-review.test.mjs`：42 项通过。新增 5 项回归覆盖立即停止、null 后转为冲突、非冲突状态、返回前出现冲突及 head/关闭优先级；其中 3 项在修改实现前失败。
- 新版 CLI 只读查询真实 PR #8 上述 head：返回 `merge_conflict`、`mergeable=false`、`mergeable_state=dirty`，进程退出码 8，成功保存结果 JSON，没有进入轮询休眠。证据是当时快照，不代表 PR 后续状态。
- 未修改 Rust 或 CI 配置，本地无需重新构建应用。实际修复 PR 的 CI 和 Code Review 以 GitHub 最新 head 的结果为准。
