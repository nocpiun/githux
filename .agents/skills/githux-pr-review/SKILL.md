---
name: githux-pr-review
description: 使用 gh CLI 等待 Githux PR 最新提交的 Codex 审查和 CI，独立处理历史线程、证据冲突及基线变化，超时单次重试，并按全程裁决记录决定合并执行者。在创建 PR 或推送修复并请求重审后使用。
metadata:
  internal: true
---

# Githux PR 审查循环

依赖 Node.js 20+、已登录 gh；在已有任务 worktree 中执行。先读 [共享工作流](../../../docs/agent-workflow.md) 与 [完整流程图/返回状态](../../../docs/pr-review-flow.md)。不增加逐 PR 人工批准；原有人工验收及永久裁决记录继续适用。脚本只读收集事实，唯一自动写操作是一小时无结果后的单次 `@codex review`，不改代码、resolve 或合并。

## 开始与等待

1. 核对 repo、PR、远端完整 head 和实时 labels；`blocked` 是负责人管理的合并禁令，即使旧 CI/review 已通过也不得放行，Agent 不得自行添加或移除。需要负责人验收时添加 `pending`（`gh pr edit PR --add-label pending`），负责人明确验收通过后由协调者移除；不能用检查变绿代替验收。完整规则见 [PR 标签与规模门槛](../../../docs/agent-workflow.md#pr-标签与规模门槛)。恢复 PR 描述、分页评论/review 及本地任务的全程裁决历史。已知 true/coordinator 永不重置，历史不全不默认 false。
2. 首次创建 PR 自动审查。后续每次 push（含文档/同步基线）核对远端 head 后，用 gh 发送一次精确的 `@codex review`，记录返回的 comment ID/created_at。发送结果不明先查评论，不重复发送。
3. 当前 PR 出现另一新触发时，核对其发起者、时间与 head，改用该轮服务端游标；不要继续把之前结果当作新轮次结果。

```sh
gh api --method GET repos/OWNER/REPO/pulls/123 --jq .head.sha
gh api --method POST repos/OWNER/REPO/issues/123/comments -f 'body=@codex review'
node "/ABSOLUTE/SKILL/DIR/scripts/wait-review.mjs" --repo OWNER/REPO --pr 123 --head FULL_40_CHARACTER_SHA --wait-ci true --out .agents/task/review-result.json
```

手动重审加 `--since TRIGGER_CREATED_AT --after-comment-id TRIGGER_ID`；排除已处理正式 review 可加 `--after-review-id ID`。摘要原地更新，用 Code Review 自身时间和 SHA 校验，不用摘要创建 ID 或本机猜测时间。

默认每轮一小时、30 秒轮询；到期仍无审查结果且状态适用时自动补发一次，使用服务器回执开始第二轮，最多再等一小时。`--timeout-seconds 0` 是一次只读查询；`--auto-retry false` 禁止自动补发；自定义 reviewer 也不自动触发 Codex。证据缺口/矛盾默认最多缓冲两分钟，随后交回，受本轮总期限限制。

状态不变由脚本等待，不调用模型、不并行启动第二个等待器。宿主工具允许约两小时及有界 API 收尾，或使用可恢复前台等待；不要套更短的 timeout、用 `tail` 截断结果或覆盖退出码。无需持续读取过程日志。工具完成后，读取完整 JSON 并核对本次 run_id、head、status 与原始退出码。

`--wait-ci true` 包含 CI 汇总。`blocked` 只禁止合并，不阻断 review/线程/所选 CI 的读取、轮询或原有的一小时后单次重审。pending/running 继续等待；整批反馈、失败与证据问题按正常优先级交回，并保留 `integration.blockers` 中的 `blocked_label`。本轮 clean 且所选 CI 就绪后，若最终实时标签仍含 `blocked`，返回 `merge_blocked`，不得返回 `checks_ready` 或 `review_completed`。`--check NAME` 只能追加同一 CI workflow 内检查，不能移除 CI。没有 wait-ci 不核验 CI；review_completed 不是合并许可。

## 返回后的处理

完整状态表见 [返回约定](../../../docs/pr-review-flow.md#返回与调用约定)。重点：

- `review_arrived`：整批阅读正文、行内意见及人工回复，同时检查返回的 CI 状态；到达不等于通过。
- `history_pending`：本轮可能已 clean，仍须核实旧线程/人工线程。逐条对照当前代码和修复证据，已解决且无争议才能通过 gh resolve。不盲目批量关闭；争议永久记录 true/coordinator 后交回。只收尾线程、没有新 commit 时，用原触发游标重新核验，不重复触发审查。
- `evidence_incomplete`、`evidence_conflict`、`ci_unverified`：缓冲期后仍有缺口/矛盾，读取各维度证据，不原样重启一小时等待。不要把工作流格式变化猜成通过。
- `base_changed`、`merge_conflict`：核对目标分支；解决冲突或补齐组合验证，有新 head 则重新 CI/review。Draft、BLOCKED、未知 mergeability 不冒充冲突；已有人工验收限制不因同步基线解除。
- `timeout`：读取 waiting_on/last_error/retry。`retry_uncertain`：发送可能已发生，保留意图，核对实际评论后用已确认游标只读恢复，不能删除记录来补发。
- `merge_blocked`：审查及所选 CI 已收集，检查 `integration.blockers`；`blocked_label` 必须等负责人手动移除后重查，不因标签所指问题已修复而自行撤掉。其他结果（如 `review_arrived`、`ci_failed`、`timeout`）也可能携带 `blocked_label`：照常处理反馈，仍禁止合并；不要改用手写轮询来规避标签。缺少/无效 labels 会报错，不能当无标签。
- `checks_ready`：重新读取实时标签（不得有 `blocked`）、完整裁决/验收历史、milestone、`PR` 工作流的 `Check` required check、最新 head/保护和当前基线后才能合并；review_passed=null、verdict=not_evaluated 始终保留。
- `error`、`interrupted`、未知 Bot 消息、失败/取消：不算成功。错误输出文件若无法写入，依据 stdout 与退出码处理，不能退回读取旧成功文件。

## 反馈与交付

按共享工作流：全可修则修复、验证、同步文档、commit/push、请求新 head 审查；混合批次先永久记录 true/coordinator，再修独立小项并交回（默认保留本地）；全需裁决先记录并停下，不改代码。不用新提交、换 Agent 或另建 PR 清除历史。

无争议任务由原执行者完成闭环；任何一轮有过裁决，只能由负责人/主 Agent 合并。最终双方都满足完整门槛，用 squash 和 GitHub 默认消息：

```sh
gh pr merge PR --repo OWNER/REPO --squash --match-head-commit FULL_HEAD_SHA
```

不传 subject/body/body-file，不使用 admin/auto 绕过门槛。查询实际 MERGED 后再报告，保留使用中的 worktree。共享文档随代码提交；最终合并证据留 PR 或私有交接，不为记录结果再造未审查提交。

## 技术与验证

Bot 格式、CI 证据、重试记录边界见 [技术契约](references/review-evidence.md)。离线测试：

```sh
node --test "/ABSOLUTE/SKILL/DIR/scripts/wait-review.test.mjs"
```

模拟 GitHub/时钟不证明云端审查一定成功；真实验证只陈述实际查询/触发的结果。