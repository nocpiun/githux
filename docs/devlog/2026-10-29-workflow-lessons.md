# 工作流：任务开工应用 LESSONS 与近期教训回填

- 日期：2026-10-29；任务 ID：workflow-lessons；分支 `docs/workflow-lessons`，worktree `D:\githux-workflow-lessons`，base `3c9a1ae`。PR 与最终 head 见 GitHub 与本任务交接。
- 目标与状态：待审查。落实“所有后续任务开工都要读并应用相关 LESSONS，交付时评估是否值得记录”的明确要求，并回填近期已发生、可复用、有证据的教训。

## 改动与决定

用户要求：所有任务（不只 UI）开工都要读并应用相关教训；交付时评估是否有可复用实际教训，有证据就写、没有不凑数量。本任务只改 `LESSONS.md`、`docs/agent-workflow.md` 与本日志，不改业务源码、其他 agent 状态，也不改 AGENTS 里的权限/合并门槛。

### 1. 工作流中的开工应用与交付评估

`docs/agent-workflow.md`：

- “角色、派发与并行”新增：所有任务开工时，主 Agent、基层 Agents 和诊断/审查执行者都须阅读 [LESSONS](../../LESSONS.md)，查找相关教训并用于任务拆分、实现及验证；文档、诊断和 `local-only` 任务同样适用。
- 角色表头范围补上“诊断/审查执行者”，文档标题的适用范围同步更新。
- “实现、文档和提交”把“有可复用实际教训时更新 LESSONS”扩展为交付评估：有新证据时补充 LESSONS 或完善已有条目，写清规则、原因、可定位证据与适用范围；没有新增无需凑条目；多人同一主题协调唯一写入者，其他执行者把候选证据留在本任务开发日志与交接中；稳定规范归代码约定或所属主题文档。
- 并行说明补一句：同一用户页面拆为多个组件任务时，明确页面组合的唯一执行者、联合预览和整页验收场景（链接到新增 L012）。这是对既有“共享接口先明确最小契约”的补充，不新增审批步骤。

### 2. LESSONS 维护说明

`LESSONS.md` 开头说明开工时阅读并应用、交付时评估，新证据才新增或完善，不凑条目；未发生的问题不伪造复盘。原“维护方式”链接保留。

### 3. 回填的三条教训

按“规则 / 原因 / 证据 / 范围”写，证据全部指向可定位的提交后源码、测试、主题文档与 10-26 之后的开发日志：

- **L010：交互预览的 fixture 必须支持反复操作**。repository example 原用一次性 `pop_front()` 队列，仅首屏有数据，耗尽后统一 404，正常后续点击被误报为资源/权限问题；预览与测试各用一套 transport，测试通过没暴露预览缺口。规则为按请求/资源身份应答、预览与行为测试共用 fixture、未建模请求报开发配置错误。证据：[返工背景](2026-10-28-repository-browser.md#本轮返工背景)，[共享 fixture](../../src/bin_support/repository_fixture.rs) 的 `the_same_resource_can_be_read_repeatedly_and_in_any_order`、`scripted_failures_fire_once_so_a_single_retry_recovers`、`an_unmodelled_request_is_a_development_error_not_a_permission_result`，以及 [页面行为测试](../../src/ui/repository/tests.rs)。范围含返回/重复读取/重试类交互预览；实网边界引用 L002。
- **L011：裁切要核对实际绘制节点与设备像素边界**。第一处：头像最初只在外层 `rounded_full().overflow_hidden()`，图片节点仍露方角，修复为图片节点自身 `.rounded_full()`。第二处：GPUI 0.3.10 把不相交祖先 mask 钳成零面积，设备坐标取整可能把小数零高度扩为一像素，离屏头像及 tinted frame 在 viewport 下方露出细条；修复为 `VisibleAvatar` 只在真实交集宽或高为零时跳过绘制，并包住整个 frame。证据：[头像形状修正](2026-10-28-account-shell.md#负责人反馈轮二头像形状修正)、[裁切防护与失败复现](2026-10-28-account-shell.md#裁切防护与失败复现)（关闭防护回归失败；150% 原生探针修复前 46 个亮色图片像素、修复后 0），[生产渲染器](../../src/ui/avatar.rs) 与 `offscreen_images_never_reach_paint_with_an_empty_fractional_mask`、[离线 avatar example](../../examples/avatar.rs)。范围限定为 GPUI/Ely 图片与祖先 mask 排查，空 mask 根因限定于日志核验的 GPUI 0.3.10；100% 缩放与手动拖动仍按 L002 未完成原生验收。
- **L012：同一页面的组件拆分要保留联合验收**。repository-overview 独立于 browser 交付时，独立示例下方用模拟阅读区，真实 browser 未挂载该组件；分别验证无法证明同页的空间分配、分支切换与局部失败。后续两任务合并，由 browser 单一执行者统一组合验收。规则为拆分时指定页面组合的唯一执行者、联合预览与整页验收场景，交付结论说明实际组合验证范围。证据：[overview 未接入边界](2026-10-28-repository-overview.md#未执行--后续)、[任务合并与统一页面集成](2026-10-28-repository-overview.md#合并任务与统一页面集成由-browser-执行)、[browser 组合记录](2026-10-28-repository-browser.md#owner-合并-browser--compact-overview统一页面)；组合后的 [联合回归](../../src/ui/repository/extras_tests.rs) 中 `scoped_retry_keeps_source_and_sibling_groups` 与 `overview_reads_once_and_branch_changes_keep_the_embedded_entity` 分别覆盖 extras 失败/重试保留 reader、分支切换不倍增 extras 请求。范围含同页多组件的并行实现与交接；组件内部仍可独立开发，联合 example 不等于正式 Shell 已接线。

### 4. 去重与不写的部分

- 模拟/原生边界已由 L002 覆盖，新条目只链接引用，不复制规则。
- 不把负责人对 overview 密度与布局的新选择写成技术故障，只记录“独立交付导致联合验收缺口”这一有共享证据的部分。
- 不引用私人原始会话；证据只取受 Git 跟踪的 devlog、源码与测试。
- 不机械新增编号：只补三条有实际证据的教训；没有证据的候选不写。

## 验证

本任务仅文档，按任务要求不运行 native 预览或重型 Rust 构建。实际执行：

- `git diff --check` 通过。
- 逐条核对新增链接目标存在：`src/bin_support/repository_fixture.rs`、`src/ui/repository/tests.rs`、`src/ui/repository/extras_tests.rs`、`src/ui/avatar.rs`、`examples/avatar.rs`、`examples/repository.rs`、`tests/repository.rs` 与三份 devlog 文件均在当前提交中；测试函数名用 `rg` 在对应文件核对（如 `the_same_resource_can_be_read_repeatedly_and_in_any_order`、`overview_reads_once_and_branch_changes_keep_the_embedded_entity`）。
- 文档变更会被 CI 范围策略判为纯文档，跳过 Rust 与脚本检查，`CI` 汇总仍须成功；本任务不运行 `cargo`，也不声称未跑的检查通过。

未覆盖：未做原生 UI 验收（纯工作流文档，无用户可见行为变更）；未修改、未重跑 L010/L011/L012 证据所涉的源码与测试，只引用既有记录；L012 引用的原生同页检查、L011 的 100% 缩放与手动拖动等仍以原日志的未覆盖声明为准。

## 后续

- 推分支、创建关联 First Release milestone #1 的 PR，触发 Codex 审查，按 [githux-pr-review](../../.agents/skills/githux-pr-review/SKILL.md) 核验当前 head 的 CI 与审查。
- 本任务为普通文档 PR，全程无待裁决事项，按既有门槛完成审查与合并即可，不需要额外许可；`blocked` 由负责人管理。
- 其他任务的后续新教训由各自 devlog/交接汇入；本任务不冒充尚未复现或修复问题的结论。
