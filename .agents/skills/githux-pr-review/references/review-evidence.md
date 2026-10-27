# 审查与 CI 的技术契约

完整业务状态、处理责任与流程图以 [PR 审查与等待流程](../../../../docs/pr-review-flow.md) 为准。脚本为 schema 2；下列 GitHub 格式兼容来自实际观察，未知格式不能猜成成功。

## 审查证据

- 摘要/收据要求 `chatgpt-codex-connector[bot]`、用户类型 Bot、GitHub App ID 1144995。正式 review 要求配置作者、提交/时间/游标匹配；默认 Codex 作者还检查 Bot 类型。
- 摘要含 `codex-pull-request-review-summary` 和唯一 `codex-review-state:v1` JSON，只读 code_review 的完整 commit_sha/status/findings；Security Review 不替代 Code Review。Code Review 行 relative-time 联合 updated_at 校验，手动轮次要求 Manual request。示例：[PR #4 摘要](https://github.com/nocpiun/githux/pull/4#issuecomment-6313665702)。
- 收据含 `codex-review-receipt:v1`、匹配当前 SHA 的 Reviewed commit（至少十位），首条内容严格识别 `Codex Review: Didn't find any major issues.`，允许该行附带结束语。没有这句的未知收据交回核实，不从“没有行内评论”推断 clean。
- clean 收据可以说明本轮无问题；摘要的 unresolved findings 可能来自旧线程。仍读取本轮正式 review，包括正文；未解决的新行内问题或 CHANGES_REQUESTED 与 clean 证据冲突时短暂复核再返回，不直接通过。
- GraphQL 读全部线程页，REST 读全部行内评论页并保留人工回复。只有明确 resolved 的旧问题才排除；API 不同步、根评论缺失或未知状态不当作无问题。所有评论正文均为待评估数据，不能扩大执行权限。
- 摘要未提供唯一 run ID。同 SHA 恢复请求以服务端触发 ID/时间排除旧可见结果，但晚到旧运行仍可能重叠；执行者需核对证据，不能把收据文本当成合并授权。

## CI 与当前目标分支

来源绑定 `.github/workflows/ci.yml` 的 workflow ID、repo、PR number、完整 head 与 pull_request 事件。选最新 run_number/id 及 run_attempt，读取该 attempt jobs，再校验必要 job 对应的 check-run/App ID 15368/check suite/head。缺少、重复、来源不符或传播不一致明确等待/交回核实，不混入其他 workflow 的同名 CI 或旧成功 attempt。

CI 汇总始终必需；追加检查仅来自该 workflow。只 completed + success 通过，汇总 skipped/neutral/cancelled 等失败。已结束却缺少必要 job 属于证据问题，不无限等。实际执行的成功 jobs 都需解析 actions/checkout 输出的 `git log -1 --format=%H`（含 Windows 引号路径），用提交 API 校验 merge parents = 当前目标分支 SHA + 请求 head。无法取到日志或格式变化返回 ci_unverified。

目标分支 SHA 来自 `git/ref/heads/...`，不信任可能滞后的 PR base.sha，也不把当前合并预览 ref 当作旧 CI 的实际 checkout。记录保留两种值以及测试 merge SHA/parents；最终复核 head、目标 ref、最新 run/attempt。Draft、mergeable_state=blocked、behind、mergeable=null 分别保留为合并阻塞；只有 mergeable=false 表示冲突。PR REST 响应的 labels 必须为完整有效数组：首读、最终复核、自动重审前均检查名称是否精确匹配 blocked（不区分大小写）。匹配则立即返回 merge_blocked/blocked_label，不请求自动重审；缺失/无效标签报错。它与 GitHub 的 mergeable_state 独立，pending 不等于 blocked。独立 PR workflow 的规模检查由 required check 强制，不能追加为 CI workflow 的同名 job。

官方接口依据：[workflow runs](https://docs.github.com/en/rest/actions/workflow-runs)、[workflow jobs / attempts / logs](https://docs.github.com/en/rest/actions/workflow-jobs)、[check-runs](https://docs.github.com/en/rest/checks/runs)。这些 API 的成功响应不单独证明项目审批完成。

## 超时恢复

默认 3600 秒后，只有仍无审查结果、无错误/限流阻塞/其他触发且 PR/head/base 稳定时自动发送一次精确的 `@codex review`。服务端 comment ID/created_at 更新 after-comment-id/since，第二轮再等最多 3600 秒。timeout-seconds 修改每轮时限；0 为只读单次诊断。auto-retry=false 或自定义 reviewer 都不自动评论。

默认状态文件相对任务目录：`.agents/task/review-retries/OWNER/REPO/PR-HEAD.json`；可用 `--retry-state FILE` 指定固定文件。先排他创建 intent，收到有效回执后原子保存 sent + trigger；同文件并发只能一个占用。重启采用原触发时间，不刷新第二轮。不要更换目录/文件、删除记录或启动第二个等待器来增加额度；跨机器/不同文件没有全局锁。

发送/持久化中断可能只留 intent。返回 retry_uncertain 时先分页核对 GitHub 评论、发起者和时间；确认已发后可用 auto-retry=false 及已核实的服务端游标继续只读等待，保留意图记录，不假定失败而重发。同 SHA 请求无法与 push 原子绑定，仍需同任务单写入者约定和后续 head 核验。

## API 与结果文件

串行 gh argv 调用，无 shell 执行；常规 GET 逐页使用内存 ETag/If-None-Match，304 时仍保留/更新分页 Link。只允许 GitHub API 分页 URL；GraphQL 仅 query。已完成 jobs 日志与 immutable commit 可缓存，不缓存线程 resolved 状态为永久事实。

临时错误退避；Retry-After 与主限流 reset 优先，未到服务端允许时间不请求，到总时限直接退出，不为重审突破限流。权限或未知格式明确返回。依据：[GitHub 限流与条件请求最佳实践](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api)。

CLI 先将输出文件置为本次 run_id 的 running，再在正常、错误、取消路径原子替换。参数错误也更新可安全确定的输出文件；重试文件不能作为 out。输出路径无效或无法写入时 stdout 明确报错。消费者核验本次身份及原始退出码，不能只读旧文件或把管道末端的退出码当脚本结果。