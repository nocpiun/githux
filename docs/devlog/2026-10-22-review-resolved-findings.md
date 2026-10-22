# 修复已解决历史审查意见导致的等待超时

## 问题与结果

PR #6 的最新提交 `39c7f0696f09527c121bf1d7d888cdf8b18cf849` 已完成审查，摘要同时报告 `has_any_findings=true`、`has_unresolved_findings=false`；本轮只有完成收据，没有新的正式 review 或行内意见。等待脚本按前一个字段选择收集问题的分支，持续等待不存在的新行内意见，直到超时。

修复后按 `has_unresolved_findings` 区分完成后的行为：仍有未解决问题则收集审查批次；没有未解决问题则返回 `review_completed`，或在启用 `--wait-ci true` 且所选 CI 成功后返回 `checks_ready`。有已解决历史不再阻塞。只对完成且已验证的摘要应用此分支，保留 Bot 身份、完整 SHA、触发轮次、未知消息、CI 和返回前 head 复查；结果仍不代表自动批准合并。

实现和回归测试位于 [wait-review.mjs](../../.agents/skills/githux-pr-review/scripts/wait-review.mjs) 与 [wait-review.test.mjs](../../.agents/skills/githux-pr-review/scripts/wait-review.test.mjs)，字段语义和边界维护在 [审查证据契约](../../.agents/skills/githux-pr-review/references/review-evidence.md)。不改变轮询间隔和超时，也不改动其他 Agent 的 worktree 或运行进程。

## 验证

- 先添加 4 项回归测试：旧实现共 37 项中 3 项失败，真实故障场景错误返回 `timeout`；修复后 `node --test .agents/skills/githux-pr-review/scripts/wait-review.test.mjs` 全部 37 项通过。
- 新用例覆盖：历史问题已解决且无本轮正式 review（有/无 receipt）；CI 缺失、进行中、旧 head、来源错误、失败和成功；审查未完成、错误轮次/身份/字段；收集证据期间 push 或关闭 PR。原有未解决问题的整批等待测试继续通过。
- 对真实 PR #6 执行单次只读查询（`--timeout-seconds 0 --wait-ci true`），保留原触发时间 `2026-10-22T07:11:19Z` 和评论 ID `6334983339`，正确返回 `checks_ready`。证据：[审查摘要](https://github.com/nocpiun/githux/pull/6#issuecomment-6331406055)、[无重大问题收据](https://github.com/nocpiun/githux/pull/6#issuecomment-6335038750)、[该 head 的 CI](https://github.com/nocpiun/githux/actions/runs/39738769744/job/119690699099)。原始结果仅保留在本任务被忽略的目录。
- 本次只改审查 skill 和文档，无 Rust 应用变更。最终云端检查和审查状态以本修复 PR 的最新 head 为准。
