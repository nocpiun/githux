# 审查摘要与 CI 证据

脚本兼容正式 review，以及本仓库已观测的 Codex Connector 摘要格式。格式证据来自 [PR #4 摘要](https://github.com/nocpiun/githux/pull/4#issuecomment-6313665702)；这不是 GitHub 的稳定 API 契约，未知格式返回 unverified_bot_message。

- 摘要必须来自 `chatgpt-codex-connector[bot]`、用户类型 Bot、GitHub App ID 1144995，同时包含 summary 标记和唯一的 `codex-review-state:v1` JSON。
- 只使用 `code_review` 的完整 commit_sha、status、findings 布尔值；Security Review 的完成不能替代 Code Review。
- 从 Code Review 表格行读取独立 datetime，联合 updated_at 校验时间边界；手动触发轮次须显示 Manual request。摘要创建日期和 comment ID 可以早于触发评论，因为 Bot 会原地更新它。
- 单独更新安全审查、摘要属于旧 SHA、代码审查时间早于触发均不释放新一轮结果。缺少时间、未知状态、身份不匹配或多份可用摘要返回待核实，不猜测。
- 本工作流每个新 head 触发一次审查，不支持靠重复请求相同 head 来区分并发审查轮次。摘要未提供唯一 run ID；遇到同 SHA 多轮重叠须人工核验，不凭最后更新时间认定特定轮次通过。
- 已知 running 摘要优先于提前可见的正式 review：等待整批完成。完成且有 findings 时收集正式 review 及其评论，并保留可能从旧 review 延续到当前提交的 Bot 行内意见；它们是待分类数据。
- 可信且绑定当前 SHA 的 review receipt 不单独唤醒执行者；普通错误/额度消息仍返回待核实。没有可用摘要时保留原正式 review 到达行为，不擅自把 APPROVED 或无行内意见升级为检查通过。
- 结果返回前重新读取 PR，head 改变或 PR 关闭就废弃本批结果。合并前执行者仍需再次核验。

## CI

`--wait-ci true` 查询当前完整 head 的 check-runs，分页读取 `check_runs` 集合。默认检查名 CI，来源必须是 GitHub Actions App ID 15368、head_sha 一致。同名多个可信检查是歧义错误；缺失、pending/in_progress 持续等待。

仅 completed + success 成功；failure/cancelled/skipped/neutral/timed_out 等均返回 ci_failed。CI 可在代码审查仍运行时报告失败，让执行者处理；有完整审查意见时先交回整批反馈。只有明确无 findings 的完整摘要加全部所选检查成功才返回 checks_ready。

本仓库 CI 是汇总 job，要求所有必需上游成功；脚本不绕过汇总、不推断未知 workflow 的门槛，也不自动改变 required checks。部署或额外检查须显式加入 `--check`，最终核对仓库保护和与 main 的组合仍由执行者承担。

## 等待和输出

默认 30 秒轮询，状态不变仅首次输出等待提示；临时网络错误退避，超时/权限/取消明确退出。正常摘要与 CI 输出精简证据（SHA、状态、ID、链接、代码审查时间），有意见或未知消息时保留必要原文。不存在常驻服务、自动唤醒或自动合并。
