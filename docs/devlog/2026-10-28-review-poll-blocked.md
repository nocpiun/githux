# blocked 标签不再中断审查等待

- 日期：2026-10-28（Asia/Shanghai）；任务：review-poll-blocked；执行者：主协调 Agent。
- 状态：已实现并通过本地验证，待当前提交的远端审查/CI；负责人要求修复并推送，本轮不合入 main。

## 问题与改动

旧实现将负责人 `blocked` 标签放在 `inspectHead()` 的提前返回路径：首次读取 PR 即返回 `merge_blocked`，没有读取 review/线程/CI，也阻止一小时无结果后的单次重审。截图中的执行者不得不另写轮询，根因是 waiter 的行为契约把禁止合并扩大成停止收集反馈。

现在标签仍在开始、最终复核和超时重审前校验，但只作为 `integration.blockers` 中的 `blocked_label`。pending/running 正常轮询；完整反馈、review/CI 失败、历史线程、证据异常按原有优先级交回并保留标签限制。没有审查结果时仍适用同一 head 的两轮期限与一次重审额度。等待进度同时显示 blocked 合并限制。

clean review、线程和所选 CI 已就绪后，最终仍带标签则返回 `merge_blocked`/15，不能返回 `checks_ready`；不等待 CI 的模式同样不能用 `review_completed` 放行。加入/移除标签以最终实时读取为准；无效标签仍 fail closed。独立 `PR / Check` 工作流、标签所有权、required checks、合并权限和保护设置均未改动。

主要契约见 [审查流程](../pr-review-flow.md) 和 [Agent 工作流](../agent-workflow.md#pr-标签与规模门槛)。

## 验证

- 先改回归用例，旧代码在 blocked 相关场景中有 9 项失败：未读取审查、未发生轮询、遮蔽失败/线程等。修复后 waiter 全部 71 项通过。
- `node --test .agents/skills/githux-pr-review/scripts/wait-review.test.mjs .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs .github/scripts/ci-scope.test.mjs`：85 项通过。
- `node --test .github/scripts/pr-policy.test.mjs`：8 项通过，blocked 仍使服务器门槛失败，20000 行边界未改变。
- 回归覆盖 pending/running 到 clean/反馈、等待中增删标签、最终移除标签、CI 单独等待、review/CI 失败和历史线程、只读超时、有界单次重审、CLI 退出 15，以及缺失/无效标签。
- 真实 PR #38、head `0f2aeafe20898ae67fb50f1c448389d8286401d3` 只读对比：旧版直接退出 15，review/CI 均未读取；修复版已读取 `review.state=running` 并保留 blocked，单次诊断在 CI 查询达到时限后返回 timeout，未宣称 CI 通过。没有移除标签或对该 PR 发送重审评论。
- `git diff --check` 通过。没有 Rust 产品改动，不重复运行 Rust 构建；模拟时钟的两轮超时测试不冒充真实一小时等待。

## 后续

创建首版配套 PR，按 skill 等待当前 head 的 review/CI，未获本轮 main 合并授权。其他 worktree 的旧脚本需在后续同步该修复后才改变行为；不直接覆盖运行中的 agent 文件。
