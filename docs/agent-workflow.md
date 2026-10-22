# Agent 开发工作流

更新：2026-10-22。本文是共享流程的主要维护位置，适用于负责人、主 Agent 和基层 Agents。负责人已授权下述条件内的自主推进；不是每个 PR 都要人工批准，也没有启用无人值守调度服务。

## 角色、派发与并行

| 角色 | 职责 |
| --- | --- |
| 负责人 | 把握整体方向、参与产品和架构决策、确认关键体验，并参与需要上交的裁决 |
| 主 Agent | 与负责人讨论方案，拆分和派发任务、维护全局上下文、协调共享接口与例外 |
| 基层 Agents | 接收任务，完成实现、验证、审查反馈处理和交付；单个执行者称“基层 Agent” |

负责人和主 Agent 共同讨论方向、架构、功能和体验。基层 Agents 使用本地任务上下文指定的执行工具，会话在负责人可见、可补充指令或接管的终端中运行。主 Agent 保持既有协调会话，当前不实现自动唤醒。并行任务各自使用独立 worktree、index 与 `target/`，并在派发时明确共享文件的写入归属。

实现任务独立 worktree：`D:\githux-<task>`；独立分支：`<type>/<branch-name>`，例如 `feature/...`、`fix/...`。使用同一任务基层 Agent 会话完成实现、审查修复和交付。

**并发：** 首次流程试跑只用一个实现 Agent 的限制已由负责人在 2026-10-22 解除。当前按就绪依赖、写入冲突、审查积压和机器资源安排并行，不保留固定 Agent 总数上限；编译与真实 GUI 验证可以有独立资源限制。历史事实保留：首轮试跑确实只运行了一个实现者，后续并行集成仍需继续观察。同一任务仍然同一时刻只有一个有效写入者。

任务持续流动，不建立必须整批完成的里程碑。无依赖任务可并行，共享接口先明确最小契约；多人修改同一入口时协调该部分的任务执行者，不把整个项目串行化。一个任务同一时刻只有一个有效写入者，换人先停止或隔离旧执行者。

派发包至少包含：任务 ID、目标与验收场景、范围/不做的内容、依赖和共享接口、相关文档版本、worktree/分支/base SHA、执行会话、所需检查。全体共识从 `docs/` 读取，任务专属上下文放该 worktree 的 `.agents/task/`。模型接入与私人调度配置留在本机，派发时明确提供，不要求云端 Bot 获取。

基层 Agents 的 worktree/上下文生成和可见会话启动使用 [githux-agent-dispatch skill](../.agents/skills/githux-agent-dispatch/SKILL.md)：create 不自动启动，start 在 Windows 可见 CMD 中运行配置的交互 CLI，status 只报告进程事实，resume 严格恢复原会话。负责人可在该终端阅读、补充和接管；本机真实 CLI 启动、键盘输入、正常退出及同会话恢复已验证，见 [开发日志](devlog/2026-10-21-agent-dispatch.md)。脚本没有自动唤醒、调度或自行合并。手工外部 attach 不受脚本锁保护，不能与已有 TUI/RPC 同时使用。后续多实现者并行集成仍待验证。

## 执行成本与交接

普通实现任务默认使用派发 skill 的 `--delivery pr`，由原执行者完成实现、测试、文档、PR、可直接修复的反馈以及符合门槛的合并。`local-only` 用于实验、诊断或明确要求本地验收的任务。主 Agent 亲自实现的例外按负责人指令执行。

主 Agent 确认任务上下文与实际启动成功后，可以结束当前轮次；无需持续读取执行日志、轮询 CI 或重做执行者检查。负责人可在可见终端查看和补充，完成或需裁决时手动提醒主 Agent。仅在有已授权且需要主 Agent 处理的独立工作时继续，不为保持在线制造工作。

等待外部结果交给前台脚本：状态不变时继续等待，只在完整反馈、所选检查齐备、错误、未知状态或超时时返回。普通 PR 不固定增加主 Agent 全量复审；架构和共享接口在设计阶段协调，具体争议按既有规则处理。主 Agent 读取简短交接后按问题定位代码和原始日志，不默认加载完整会话。

交接包含结论、PR/head、验证证据链接、剩余问题、是否需要裁决。当前状态覆盖更新，历史证据保存在按任务组织的记录中；稳定约定只维护一份并链接引用。主 Agent 自主管理其私有记忆，不上传私有模型配置或日志。

## 实现、文档和提交

所有参与者，包括主 Agent、实现者和诊断/审查执行者，都负责把自己确认的共享知识写回相应文档；多人涉及同一主题时协调写入者。实现者完成相关检查，更新改变了契约的主题文档和本任务开发日志；有可复用实际教训时更新 LESSONS。把这些与实现一起提交，再 push 分支并创建目标 `main` 的 PR。PR 描述包含目标、行为变化、验证、已知限制和任务标识。

首次创建 PR 由本仓库设置自动触发 Codex review。每次后续 push 新提交，无论是代码、文档、修复还是合并基线，都必须通过 `gh` 评论精确的 `@codex review`。先确认远端 head，记录触发评论 ID/时间；不要每次轮询重复评论。

基层 Agent 使用 [githux-pr-review](../.agents/skills/githux-pr-review/SKILL.md) 以前台工具调用等待普通程序轮询；工具返回后原会话继续。等待期间无需反复调用模型。超时、Bot 额度失败、无回复或无法绑定提交均不能认定通过。等待器发现合并冲突会返回 `merge_conflict`（退出码 8）；执行者先同步目标分支并解决冲突，推送后对新 head 触发审查再等待，不原样重启等待器。冲突中的语义取舍按裁决流程处理，已有人工 review / Draft 限制不变。

### 首版 Milestone

实现 [已确认首版功能](product.md#首版范围) 及其直接配套工作的 PR，**创建时必须关联 [First Release（milestone #1）](https://github.com/nocpiun/githux/milestone/1)**。应设置 GitHub PR 的 milestone 字段，只在标题或正文提到它不算关联。已排除的后续功能和无关工作不自动归入首版；仍待明确的功能先确认范围，再按任务归属关联。

创建 PR 时在原有命令中加入 `--milestone "First Release"`；遗漏时补关联，并查询实际字段核验（将 `PR_NUMBER` 替换为编号）：

```sh
gh pr edit PR_NUMBER --repo nocpiun/githux --milestone "First Release"
gh api repos/nocpiun/githux/issues/PR_NUMBER --jq .milestone.number
```

核验应返回 `1`。GitHub 的 PR 同时具有 issue 元数据，以上查询用于读取 milestone；关联对象以本仓库编号 `1` 为准，若标题改变，先查询其当前标题再传给 CLI。

该 milestone 用于追踪首版范围与进度，不要求攒齐全部功能才能合并，不替代任务授权、最新提交的 CI 与审查门槛。[路线图](roadmap.md) 继续按依赖拆成小 PR 交付。

## 收到一批审查意见

先读完正文和全部行内意见。“可以直接解决”意味着既有要求足以决定正确行为；能轻易写出代码不等于有权决定产品。可以采用不同于 Bot suggestion 的修法，但必须解决成立的问题。

| 本批情况 | 基层 Agent 行为 | 此 PR 合并权限 |
| --- | --- | --- |
| 全部可直接解决 | 修复、验证、提交、push、评论 `@codex review`，等待下一轮 | 若此前从未需裁决，仍可由基层 Agent 合并 |
| 部分可修、部分需裁决 | 先登记待裁决记录；修独立可修项并验证；说明已修/待裁决事项，停下等指令 | 从此仅负责人或主 Agent |
| 全部需裁决 | 登记待裁决记录，不改代码；说明理由与选项，停下等指令 | 从此仅负责人或主 Agent |
| 明确没有待处理问题 | 核对最新提交的全部合并门槛和全程裁决记录 | 从未需裁决则基层 Agent；否则负责人或主 Agent |

混合批次默认将独立小修留在本地，不自行 push/进入新一轮；这是对负责人未指定提交动作的执行默认，不是新增人工审批步骤。任务若明确要求也推送，仍须触发 review，然后停下等裁决。收到裁决指令后，核对远端 head 与已有本地小修，按指令完成剩余处理，再统一验证、提交、push并触发新一轮；不覆盖或重复应用已做的小修。可修部分若依赖未定选择，也等待。不为消除 Bot 意见擅自改需求或扩大任务。

误报争议、意见相互冲突、新产品/架构取舍、无法确认结果时交负责人或主 Agent。裁决记录说明依据、选定处理及后续执行者；基层 Agent 可按指令继续修复，但不恢复该 PR 的自行合并权限。仅增加独立诊断 Agent 不等于负责人或主 Agent 已裁决。

同一问题反复失败且没有有证据的新方案时报告阻塞，不无限尝试。裁决认为某条意见不成立，也不自动把一份仍未通过的 review 改写为通过；仍需满足下面的当前提交门槛，不能静默豁免。

## 跨轮次保存合并权限

**是否出现过待裁决事项是 PR 全程的事实，不是当前 head 的瞬时状态。** 当前 SHA 变化、争议解决、会话重启或 Agent 更换都不能清除它。基层 Agent 不能通过另建 PR 隐藏同一交付中已经出现的裁决，替代 PR 的历史与合并责任一并交接。

创建 PR 时在描述中记录下面的流程区块，同时在 `.agents/task/` 保存副本；字段名是协作约定，目前没有自动强制执行这些字段的服务：

```text
<!-- githux:merge-policy:v1 -->
task_id: <任务标识>
arbitration_ever_required: false
merge_owner: worker
```

一旦发现待裁决项，**先**将本地状态改为 `true / coordinator`，并用 `gh` 给 PR 添加持久评论，然后再处理独立小修：

```text
<!-- githux:arbitration-required:v1 -->
task_id: <任务标识>
head: <当时的完整 SHA>
arbitration_ever_required: true
merge_owner: coordinator
问题与 review 链接：...
需要裁决的原因/选项：...
```

同步 PR 描述中的流程区块以方便查看，保留其他人的正文；不能删除历史裁决评论或在后续轮次改回 false。`worker` 指执行该任务的基层 Agent；`coordinator` 指负责人或主 Agent，原基层 Agent 仍可被安排继续修复。评论写入失败时仍然保留本地 true，将未同步的问题、失败原因及 `arbitration_sync_pending: true` 写入任务状态与交接，明确汇报失败并停下，不将失败当作允许自行合并。恢复时先补传记录；换 worktree必须移交这份任务状态，不能仅凭远端描述的 false接收。多行评论/描述先写文件，用 `--body-file`，避免 shell 转义改变文本。

恢复任务时读取 PR 描述、完整分页的相关评论/review 和任务交接；已知存在一次真实待裁决就按 true 处理，不能用最新描述的 false 覆盖历史。仅信任授权执行者/负责人的流程记录，Bot 引用样例里的标记不构成授权，也不单独改变状态。没有已知 true，但历史不完整或记录真实性无法确认时是 unknown，交负责人或主 Agent 核实，不能默认 false。裁决后补充裁决者、结论及依据、后续执行者，并链接原问题评论；保留 true/coordinator。

## 合并前的门槛

无论由谁合并，都核验以下事实：

1. PR 当前远端 head 为完整 SHA `H`，目标为 `main`，任务验收满足且没有遗漏的本地待提交修复；属于首版范围时，确认已关联 milestone #1。
2. **`H` 的 CI/CD 通过，且以 `H` 触发、针对 `H` 的 Codex 审查通过。** 检查缺失、失败、取消、尚未完成或未经约定的跳过不等于成功。`pull_request` CI 的临时 merge SHA 必须可对应当前 head 和测试基线。
3. Review 身份、commit、触发轮次和意见处理有证据。脚本 `review_arrived`/退出 0 仅代表到达，`COMMENTED` 或没有行内问题不等于通过。无 commit 字段的普通 Bot 评论要核实关联，无法确认则停下。
4. 没有未决裁决、冲突或仓库保护要求。并行任务先合入 main 后，后一个 PR 必须验证与新 main 的组合；需要更新分支时，新 head 重新满足 CI 与审查，不能沿用旧 head 的放行。
5. 合并前再次读取 head、检查与合并权限历史。按下节执行 squash merge，使用 GitHub 默认生成的提交信息并以 `--match-head-commit H` 锁定已核验的提交；不使用 `--admin`，不靠 `--auto` 提前放行。不擅自修改仓库保护或 merge queue 配置。

`--match-head-commit` 只防止合并了另一 head，不替代 CI、review、基线或权限核验。仓库原生 merge queue 若已配置，则遵循其规则并核对最终结果；提交成功、排队成功不等于已合并。实际保护设置和 Bot 信号以当前仓库状态及本轮证据核实，不能仅沿用旧 PR 的结果。

若没有配置任何 CI，或预期任务根本没有运行，基层 Agent 报告具体缺项并暂停合并，交主 Agent 安排补齐流水线/必需任务后重新核验；不能以空检查列表或本地测试代替负责人要求的 CI/CD通过。

**基层 Agent 可自行合并的条件**：上述门槛全部满足，并且已确认该 PR 从未出现待裁决事项。满足时无需再找负责人批准。**出现过待裁决的 PR**：基层 Agent 交回证据，负责人或主 Agent 按同一门槛执行合并，不得转授原基层 Agent 来绕过这条规则。

合并后查询实际 `MERGED` 状态和 merge commit，再报告结果。分支/worktree 清理是另一步，不在仍有本地改动或负责人使用该终端时直接删除；GitHub 合并也不自动代表产品发布。

## 提交历史与合并方式

**保持 Git 提交历史线性整洁。所有 PR 合入 `main` 时必须使用 squash merge，并保留 GitHub 默认生成的 commit message，包括标题与正文。** 此规则同时适用于负责人、主 Agent 和基层 Agent；每个 PR 在 `main` 上形成一个提交，分支中的实现、审查修复等中间提交由合并时统一压缩。

在上述门槛全部满足后执行（替换 PR 编号、仓库与完整 head SHA）：

```sh
gh pr merge PR --repo OWNER/REPO --squash --match-head-commit H
```

不要传 `--subject`、`--body` 或 `--body-file`，也不要手动编辑合并提交信息。GitHub 按仓库当前配置生成默认标题与正文，Agent 不自行拼接 PR 标题、编号、提交列表或摘要来替代它。此命令显式选择 squash；已核对 gh 2.101.0 的实现，在省略这些消息参数时不会传入自定义标题/正文。依据：[GitHub 默认 squash 提交信息](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/configuring-commit-squashing-for-pull-requests)、[gh 合并请求实现](https://github.com/cli/cli/blob/v2.101.0/pkg/cmd/pr/merge/http.go)。

不得改用 `--merge`、`--rebase` 或本地 squash 后直接推送 `main`。若仓库要求 merge queue，先确认队列最终使用 squash 且保留 GitHub 默认信息；CLI 的 `--squash` 不能覆盖队列配置，不满足时交主 Agent 处理配置冲突，不绕过保护。该约定用于后续合并，不要求重写既有 `main` 历史或为压缩中间提交而强推任务分支；任何新 head 仍须重新满足 CI 与 review 门槛。

## 交接与日志时机

交接包含 PR URL、最终 head、CI 及对应 review 链接、裁决历史与最终合并执行者、剩余问题、本地改动和实际合并状态。负责人仍手动提醒主 Agent，当前不开发自动唤醒。

开发日志随实现和修复提交，记录截至该轮已知的事实。审查/CI通过后的最终 SHA、链接、合并结果可留在 PR 评论或交接信息中，不为补写“已合并”再制造一笔未经检查的提交。若仍须改文档，正常提交并走新的 CI/review；不授予文档提交例外。

可见会话派发脚本已通过离线测试和本机真实 CLI/终端接管、会话恢复验证；多实现者并行集成与 CI 必需任务和保护配置的持续校准仍需后续处理。合并方式已确定为 squash merge + GitHub 默认提交信息。无需先建设常驻调度器、自动唤醒或大型 Agent 平台。

参考：[gh pr merge](https://cli.github.com/manual/gh_pr_merge)、[GitHub review API](https://docs.github.com/en/rest/pulls/reviews)。首次/后续触发规则来自负责人对本仓库配置的说明。
