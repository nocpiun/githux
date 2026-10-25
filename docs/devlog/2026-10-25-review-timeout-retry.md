# PR 审查完整流程与有界超时恢复

- 日期：2026-10-25；任务：review-timeout-retry；执行者：主 Agent 亲自实现。
- PR：[#17](https://github.com/nocpiun/githux/pull/17)，First Release；当前重构待最新提交 CI/review。

## 改动与决定

首轮只实现一小时后的单次恢复。负责人随后明确要求同一 PR 完整包含前面讨论的流程图，因此补齐 [完整状态与流程](../pr-review-flow.md)，并更新 skill/共享工作流。首轮通过的审查不沿用到新代码。

- 独立读取本轮 review、所有未解决线程、CI、当前目标分支及合并阻塞。正文反馈可以直接交回；历史线程不再被当成本轮仍在运行。保留人工回复与全程裁决处理。
- clean 摘要/收据必须与本轮正式 review 一致；完成但缺失/矛盾的证据最多缓冲两分钟，随后明确交回，不无限等新行内评论。CI 失败可与反馈同时输出。
- CI 必须包含汇总，绑定可信 workflow、PR/head、最新 run/attempt、job/check suite。用各已执行成功 job 的实际 checkout SHA 及 merge parents 核验测试基线；目标分支从 ref 读取，不信任可能滞后的 PR base.sha。
- 正常轮询尊重配置间隔，GET 使用 ETag 与分页，限流遵守服务端等待时间；阶段变化才输出简短状态。结果以新 run_id 先写 running，包含错误/取消的结束路径均写最终结果，避免沿用旧成功。
- 保留每轮默认一小时、最多一次自动 `@codex review`、排他 intent/sent 与服务端游标。只读诊断、明确错误、已完成/CI/线程待处理、新触发及 PR 变化不自动重审。

脚本按职责拆为等待/持久化、审查状态、CI 证据、GitHub 传输四个模块。没有派发其他 Agent，没有修改其他活跃任务状态，没有自动 resolve 或绕过合并权限。

## 验证

- `node --test .agents/skills/githux-pr-review/scripts/wait-review.test.mjs`：重组后的 56 项测试通过（多项含参数化场景），覆盖完整流程及首轮重试边界。
- PR #14 的 clean 收据 + 旧 unresolved 线程情形在离线回归中立即返回 history_pending，同时保持 review=clean；纯正文和矛盾 review 有单独回归。
- 只读真实验证：对 PR #17 的 f807c56，传入负责人手动触发的 ID 6392942472 / 2026-10-25T03:26:41Z。解析到了可信 clean 摘要/收据、全部线程、CI run 40158048254 attempt 1，以及各执行 job 的 checkout c0cb1f9。
- 该 checkout 的 parents 是 a57e56c + f807c56。真实 PR base.sha 仍为 a57e56c，但目标 main ref 已为 9125427；补强 ref 核验后正确返回 base_changed / ci=stale_base，而非 checks_ready。结果仅存忽略的任务目录。
- 本分支已同步 main 的 PAT 服务提交；没有改应用代码。等待脚本不执行 Rust 构建；云端按变更范围执行 Agent script checks。
- 未实际等待两小时或向其他任务 PR 发测试评论；离线测试不保证云端审查服务一定恢复。

## 边界

同 SHA 的 Connector 摘要没有唯一 run ID；重叠运行或未知文本仍需核实。checkout 日志格式变化/过期、跨 workflow 的额外门槛或 CI 证据不足会明确返回，不能猜测成功。正在运行的旧 Node 进程不会热更新；执行者完成旧调用后同步新脚本与 skill 再运行，保留任务单写入者约定。