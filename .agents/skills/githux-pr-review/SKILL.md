---
name: githux-pr-review
description: 使用 gh CLI 等待 Githux PR 最新提交的 Codex 审查和 CI，处理反馈并按全程裁决记录决定合并执行者。在创建 PR 或推送修复并请求重审后使用。
metadata:
  internal: true
---

# Githux PR 审查循环

用于已授权任务的现有分支、worktree 和执行会话。依赖 Node.js 20+ 与已登录的 gh。先读 [共享工作流](../../../docs/agent-workflow.md) 的整批分类、裁决历史和合并门槛；本 skill 不另设逐 PR 人工审批。脚本只读，不发评论、改代码或合并。

## 开始一轮

1. 记录仓库、PR、远端完整 head SHA；恢复 PR 描述、分页评论/review 和本地任务中的全程裁决历史。不完整不默认 false；已知 true/coordinator 永不重置。
2. 首次创建 PR 自动审查，直接等待。后续每次 push（含文档）先确认远端 head，再用 gh 发一次精确的 `@codex review`，记录返回的 comment ID/created_at。发送结果不明时先查询，不能重复触发。
3. 同一 head 无新提交时不因等待超时重复请求审查。不要用短 SHA 或本机猜测时间代替证据。

```sh
gh api --method GET repos/OWNER/REPO/pulls/123 --jq .head.sha
gh api --method POST repos/OWNER/REPO/issues/123/comments -f 'body=@codex review'
```

## 一次前台等待

```sh
node "/ABSOLUTE/SKILL/DIR/scripts/wait-review.mjs" --repo OWNER/REPO --pr 123 --head FULL_40_CHARACTER_SHA --wait-ci true --interval-seconds 30 --timeout-seconds 1800 --out .agents/task/review-result.json
```

后续重审增加 `--since TRIGGER_CREATED_AT --after-comment-id TRIGGER_ID`；需要排除已处理正式 review 时加 `--after-review-id ID`。这些游标不排除原地更新的摘要，摘要按代码审查自身时间与完整 SHA 验证。具体信任、轮次与兼容性边界见 [摘要与 CI 契约](references/review-evidence.md)。

调用后由脚本轮询，状态不变时不重复读取 API 或调用模型。不并行启动第二个监控脚本；使用宿主可恢复工具等待。工具尚未返回时停止生成，工具超时要大于脚本时限或适配宿主等待方式，不宣称已有后台唤醒。主 Agent 派发后结束聊天与执行者前台等待是不同情形。

`--wait-ci true` 等待当前 head 的 GitHub Actions 汇总检查 `CI`；额外门槛可重复传 `--check NAME`（指定后替换默认集合，应包含 CI）。不传 wait-ci 时只等待审查。结果文件留在忽略的任务目录。

| 状态 / 退出码 | 执行者处理 |
| --- | --- |
| review_arrived / 0 | 整批阅读审查正文与行内意见；到达不代表通过 |
| review_completed / 0 | 可信摘要明确完成且无 findings；未等待 CI，继续核验其他门槛 |
| checks_ready / 0 | 可信摘要无 findings，所选 CI 全成功；按共享工作流核验最终 head、权限历史、基线及任务验收后再合并 |
| ci_failed / 6 | CI 失败、取消、跳过或其他非成功结论；定位并修复，不认定通过 |
| review_failed / 7 | 摘要报告审查失败/取消；排查原因，不当作无问题 |
| unverified_bot_message / 5 | 身份、摘要格式、轮次或普通评论无法验证；阅读证据，不放行 |
| timeout / 2 | 等待到期，报告当前缺项；不自动重触发 |
| head_changed / 3；pr_closed / 4 | 停止旧轮次；核对变更或关闭原因 |
| error / 1；interrupted / 130 | 查询错误或取消，不算成功 |

所有结果的 `review_passed` 仍为 null、`verdict` 为 not_evaluated：脚本提供事实，不代替项目合并裁决。空检查列表、无评论、COMMENTED、退出 0 都不是自动批准。

## 反馈、修复与交付

按共享工作流整批处理：全可修则修复、验证、更新文档、提交/push、请求新 head 审查；混合批次先永久记录 true/coordinator，再修独立小项并停下交接（默认保留本地）；全需裁决先记录再停，不改代码。误报、冲突及新取舍交负责人或主 Agent。不得用争议解决、重启、换执行者或另建 PR 清除裁决历史。记录同步失败仍保持 true，保存待同步内容并停止。

普通无争议 PR 由原执行者完成检查和合并，无需主 Agent 重做整套审查。有过裁决的 PR 只能由负责人或主 Agent 合并。两者都须最新 head 的 CI 与对应代码审查通过，并核验完整历史、未决线程、基线/分支保护及验收；不得用旧 head 的结果。

```sh
gh pr merge PR --repo OWNER/REPO --squash --match-head-commit FULL_HEAD_SHA
```

保留 GitHub 默认标题/正文，不传 subject/body/body-file，不使用 admin/auto 绕过门槛。查询实际 MERGED 与 merge commit 后再报告。保留在使用或有改动的 worktree。共享文档随实现提交；最终合并证据留 PR 或私有交接，不为记录结果再补未经检查的提交。

交接只给结论、PR/head、验证链接、未解决问题及是否需裁决；原始日志按需读取。负责人手动提醒主 Agent的约定不变。

## 离线验证

```sh
node --test "/ABSOLUTE/SKILL/DIR/scripts/wait-review.test.mjs"
```

测试模拟 GitHub 和时钟；真实 API/审查行为另记证据，不把模拟结果当实际通过。
