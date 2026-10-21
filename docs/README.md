# 共享开发文档

更新：2026-10-21。这里是负责人、主 Agent、基层 Agents 和云端审查 Agent 共同维护、随 Git 分发的项目知识。新任务从根目录 [AGENTS.md](../AGENTS.md) 进入，按需读取主题，避免每次加载所有历史。

## 阅读路径

| 需要了解 | 文档 |
| --- | --- |
| 产品范围、UI 与未定选择 | [产品约定](product.md) |
| 模块职责、状态归属与实现提案 | [架构方案](architecture.md) |
| 各类数据如何存储、刷新、失效 | [缓存策略](cache-strategy.md) |
| 派发、并行、审查循环与合并权限 | [Agent 工作流](agent-workflow.md) |
| 实现和验证的共同要求 | [代码约定](code-standards.md) |
| 实际教训 | [LESSONS.md](../LESSONS.md) |
| 已做工作、证据与后续交接 | [开发日志](devlog/README.md) |
| 等待审查的可执行步骤 | [githux-pr-review skill](../.agents/skills/githux-pr-review/SKILL.md) |

## 当前状态

- 已选择 Rust + GPUI + Ely，已有原型体验与产品设计讨论；本仓库尚未建立正式应用源码和构建流水线。
- `githux-pr-review` 的等待脚本有离线测试；新的合并权限已写入规范。尚未用真实 PR 验证 Bot 的完成信号、CI 配置和合并全流程。
- 基层 Agents 可见会话的任务派发 skill、worktree/context 自动生成仍待落地；本目录不代表这些能力已经实现。
- 模块拆分、认证类型、具体网络/数据库库和缓存参数中的提案仍需确认，不因出现在这里而自动定案。

## 文档怎样维护

一个事实只设一个主要维护位置，其他文件通过链接引用。当前约定在主题文档中，发生过什么在开发日志中，可复用教训在 LESSONS 中。AGENTS 保持简短，不成为不断追加的聊天记录。

改变用户可观察行为、接口、状态归属、失败处理或兼容性时，在同一 PR 更新所属主题；纯内部整理且语义不变时，不机械增加设计文字。文档区分“已确认 / 提案 / 已实现 / 已验证”，实际代码存在后补上源码路径和关键类型，不用伪造的代码链接填位置。

每个交付任务维护一个 `docs/devlog/YYYY-MM-DD-<task-id>.md`；同一任务的修复轮次更新原文件。相关教训有实际证据才进入 LESSONS；成为稳定约束后转入代码约定或所属设计，原条目保留证据与链接，不复制两份规则。多个 Agent 要改同一条规则时协调负责人；普通任务不必更新本索引或统一总日志。

发现共享文档冲突时，在授权范围内修正；涉及未决产品/架构选择交负责人或主 Agent。不要擅自通过修改文档取消审查门槛。

## 本地资料与共享资料

本目录发布早期本地设计中的共同基线，并标注尚未确定的内容。被忽略的 `.agents/design/` 保存详细讨论稿、结构化草案和视觉资产，`.agents/output/` 保存辅助产物；这些本地资料不是云端或新 worktree 的隐含前置。

后续供全体执行者遵循的新决定，应先更新这里；本地草案若讨论不同方向，需明确标为提案。涉及细节的任务由主 Agent 提供对应版本的精简上下文包或将必要规格发布到本目录。若某项批准只存在于本地，应补充共享规格，不能要求其他 Agent 猜测。主 Agent 私人记忆、模型接入配置、凭据和原始会话日志不整体复制到 Git。

## 借鉴来源

参考 Magpie 的 [Agent 入口](https://github.com/yetone/magpie/blob/main/AGENTS.md)、[子系统文档维护方式](https://github.com/yetone/magpie/blob/main/docs/subsystems/README.md)、[教训记录](https://github.com/yetone/magpie/blob/main/LESSONS.md) 与 [代码标准](https://github.com/yetone/magpie/blob/main/docs/code-standards.md)：入口指向主题，变更同步维护知识，真实教训形成规则。未照搬其 Go 命令、项目专属规则或持续发布节奏。按任务分文件的开发日志是 Githux 为并行协作采用的组织方式，并非声称 Magpie 使用同名目录。
