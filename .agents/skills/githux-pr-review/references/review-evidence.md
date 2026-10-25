# 审查摘要与 CI 证据

脚本兼容正式 review，以及本仓库已观测的 Codex Connector 摘要格式。格式证据来自 [PR #4 摘要](https://github.com/nocpiun/githux/pull/4#issuecomment-6313665702)；这不是 GitHub 的稳定 API 契约，未知格式返回 unverified_bot_message。

- 摘要必须来自 `chatgpt-codex-connector[bot]`、用户类型 Bot、GitHub App ID 1144995，同时包含 summary 标记和唯一的 `codex-review-state:v1` JSON。
- 只使用 `code_review` 的完整 commit_sha、status、findings 布尔值；Security Review 的完成不能替代 Code Review。
- 从 Code Review 表格行读取独立 datetime，联合 updated_at 校验时间边界；手动触发轮次须显示 Manual request。摘要创建日期和 comment ID 可以早于触发评论，因为 Bot 会原地更新它。
- 单独更新安全审查、摘要属于旧 SHA、代码审查时间早于触发均不释放新一轮结果。缺少时间、未知状态、身份不匹配或多份可用摘要返回待核实，不猜测。
- 每个新 head 正常触发一次审查；超时恢复允许一次同 SHA 重试（见下文）。摘要未提供唯一 run ID；重试用新触发评论的服务端时间与游标排除旧证据，但无法证明延迟完成的旧运行与新运行的身份。结果保留 retry 记录；遇到同 SHA 多轮重叠或矛盾证据，执行者须核验，不凭最后更新时间认定特定运行通过。
- 已知 running 摘要优先于提前可见的正式 review：等待整批完成。完成且有未解决 findings（has_unresolved_findings=true）时，须等到本轮对应 head 的正式 review 及行内意见到达；仅摘要、仅收据或旧线程不足以完成本轮等待。收集本轮正式 review 及其评论，另用只读 GraphQL 分页核验线程状态，仅携带明确未解决的历史 Bot 行内意见（含回复），已解决旧意见不再混入本批。线程状态缺失或查询失败明确报错，不猜测；这些意见仍是待分类数据。
- 完成且 has_unresolved_findings=false 时返回 review_completed；has_any_findings=true 可以只是已解决历史，不再要求本轮存在正式 review 或行内意见。真实例子见 [PR #6 摘要](https://github.com/nocpiun/githux/pull/6#issuecomment-6331406055) 与[本轮收据](https://github.com/nocpiun/githux/pull/6#issuecomment-6335038750)。摘要仍须通过身份、完整 SHA 和轮次校验；独立 receipt 或仅线程被标记 resolved 都不能替代有效的完成摘要。启用 wait-ci 后继续核验 CI，返回状态不是合并授权。
- 可信且绑定当前 SHA 的 review receipt 不单独唤醒执行者；普通错误/额度消息仍返回待核实。没有可用摘要时保留原正式 review 到达行为，不擅自把 APPROVED 或无行内意见升级为检查通过。
- 结果返回前重新读取 PR，head 改变、PR 关闭或确认合并冲突就废弃本批结果。合并前执行者仍需再次核验。

## 合并冲突

每轮首先读取 PR，并在交回审查/CI 证据前再次核验；无论是否启用 `--wait-ci`，当前 head 的 `mergeable === false` 都返回 `merge_conflict`、退出码 8，附目标分支 ref/SHA、GitHub 状态和处理提示。此结果优先于审查或 CI 结果；PR 关闭及 head 改变仍优先，不能把其他提交的冲突归到当前轮次。

[REST API](https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request) 的 `mergeable` 是 true/false/null：null 表示仍在计算，不当作冲突；不凭 `mergeable_state` 的 blocked、behind、unstable、draft 等状态判断冲突。未知合并状态不额外阻止审查证据交回，但不能用脚本结果替代合并前核验。

[GitHub 不运行有合并冲突的 pull_request 工作流](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request)。收到冲突结果后应同步目标分支、解决冲突并推送，随后对新 head 触发审查并重新等待，不重复启动同一 head 的等待器。冲突分支只报告事实，不自动改代码、触发审查或解除人工审查/Draft；有语义取舍按共享裁决流程处理。

## CI

`--wait-ci true` 查询当前完整 head 的 check-runs，分页读取 `check_runs` 集合。默认检查名 CI，来源必须是 GitHub Actions App ID 15368、head_sha 一致。同名多个可信检查是歧义错误；缺失、pending/in_progress 持续等待。

仅 completed + success 成功；failure/cancelled/skipped/neutral/timed_out 等均返回 ci_failed。CI 可在代码审查仍运行时报告失败，让执行者处理；有完整审查意见时先交回整批反馈。只有明确无未解决 findings 的完整摘要加全部所选检查成功才返回 checks_ready。

本仓库 CI 是汇总 job，要求所有必需上游成功；脚本不绕过汇总、不推断未知 workflow 的门槛，也不自动改变 required checks。部署或额外检查须显式加入 `--check`，最终核对仓库保护和与 main 的组合仍由执行者承担。

## 等待和输出

默认 30 秒轮询，状态不变仅首次输出等待提示；临时网络错误退避，超时/权限/取消明确退出。正常摘要与 CI 输出精简证据（SHA、状态、ID、链接、代码审查时间），有意见或未知消息时保留必要原文。不存在常驻服务、自动唤醒或自动合并。


## 超时恢复

负责人于 2026-10-25 授权：首次等待默认 3600 秒；若尚无本轮审查结果，自动再评论一次精确的 `@codex review`。成功后用服务端返回的 comment ID/created_at 更新 after-comment-id/since，再等待最多 3600 秒；之后返回 timeout，不无限重触发。显式 timeout-seconds 覆盖每轮时限；timeout=0、auto-retry=false、自定义 reviewer 都是只读模式。

到期先用有界查询重新读取审查，再复核正式 review 与 PR：已有完成摘要、可信当前 SHA 收据、正式 review、CI 失败、未知 Bot 消息、其他新触发、head 改变、关闭或合并冲突均不会发送。已完成但旧线程/新评论未齐时返回 timeout + review_evidence 及证据；只有 CI 未齐时 waiting_on=ci，不以重审解决这些问题。查询失败或无法确认状态也不写入。正常轮询的审查通过条件未改变。

默认记录路径（相对当前任务工作目录）：`.agents/task/review-retries/OWNER/REPO/PR-HEAD.json`；可用 `--retry-state FILE` 指定固定文件。发送前以排他创建写入 intent，收到有效回执后原子保存 sent + trigger。并发等待器共享同一文件时只有一个能占用；记录损坏或身份不符明确报错。不要更换目录/状态路径、删除记录或并行启动第二个等待器来重复触发；跨机器/不同状态文件不提供全局锁。`--out` 不得覆盖状态文件。

重启读取已发送记录，从原触发时间计算第二轮剩余时限，不重新发评论。若发送超时、取消或进程在发送前后退出，可能只剩 intent；返回 retry_uncertain（9），既不假定发送失败，也不盲目重发。执行者应分页读取 GitHub 评论、确认发送者与时间并核对本地记录；可用 auto-retry=false + 已核实的 since/after-comment-id 继续只读等待，保留原 intent 供调查，不删除它来重发。未确认收到的评论不能作为新轮次起点。

同一 SHA 的重试评论无法与 push 原子绑定；发送前复核及后续轮询可发现 head 变化，但仍需遵守任务单写入者约定。脚本只报告 review_passed=null / verdict=not_evaluated，自动重试不放宽最新提交、CI、历史线程与裁决权限门槛。
