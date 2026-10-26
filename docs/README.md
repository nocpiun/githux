# 共享开发文档

更新：2026-10-21。这里是负责人、主 Agent、基层 Agents 和云端审查 Agent 共同维护、随 Git 分发的项目知识。新任务从根目录 [AGENTS.md](../AGENTS.md) 进入，按需读取主题，避免每次加载所有历史。

## 阅读路径

| 需要了解 | 文档 |
| --- | --- |
| 产品范围、UI 与未定选择 | [产品约定](product.md) |
| 首版开发顺序、依赖与验收讨论稿 | [开发路线图](roadmap.md) |
| 模块职责、状态归属与实现提案 | [架构方案](architecture.md) |
| 各类数据如何存储、刷新、失效 | [缓存策略](cache-strategy.md) |
| PAT 认证、凭据存储、账户代次与可取消的 GitHub 读取服务 | [认证与网络](auth-and-network.md) |
| 首版功能对应的 REST/GraphQL 接口、token 权限与限流边界 | [GitHub API 能力核验](github-api-capabilities.md) |
| 派发、并行、审查循环与合并权限 | [Agent 工作流](agent-workflow.md) |
| 实现和验证的共同要求 | [代码约定](code-standards.md) |
| 实际教训 | [LESSONS.md](../LESSONS.md) |
| 已做工作、证据与后续交接 | [开发日志](devlog/README.md) |
| 审查、历史线程、CI 与超时恢复的完整流程图 | [PR 审查与等待流程](pr-review-flow.md) |
| 等待审查的可执行步骤 | [githux-pr-review skill](../.agents/skills/githux-pr-review/SKILL.md) |
| 建立任务 worktree、启动/恢复可见执行会话 | [githux-agent-dispatch skill](../.agents/skills/githux-agent-dispatch/SKILL.md) |

## 当前状态

- Rust + GPUI + Ely 正式单 package 工程已建立。当前实现 R1 的外壳布局：Windows 自定义标题栏、固定侧边栏与有界仓库列表、island、Home 双列假数据、本地导航搜索浮层；运行及源码入口见 [README](../README.md)，实现与真实窗口验收状态见 [外壳布局开发日志](devlog/2026-10-22-shell-layout.md)。登录、网络、数据库与业务页面仍未实现，模块边界之外没有业务代码。
- R2 的首个服务切片已实现（`auth`/`network`/`github`/`data`/`domain`/`diagnostics`）：Classic PAT 身份验证（`GET /user`）、Windows Credential Manager 存取、账户授权代次与旧响应丢弃、`GET /user/repos` 完整 `Link` 分页与部分结果、可取消的后台 Tokio 执行器。它是真实服务基础，UI 尚未接入，也没有账户元数据落盘、SQLite、缓存或 H1/H7 写操作；接口、验证与未覆盖项见 [认证与网络](auth-and-network.md) 和 [本轮开发日志](devlog/2026-10-23-pat-data-service.md)。
- `githux-pr-review` 的等待脚本有离线测试，跨平台 CI 配置已建立。具体PR的检查、审查和合并状态以GitHub记录为准，不由本地测试或规范文档推断。
- `githux-agent-dispatch` 提供 create/start/status/resume，生成独立 worktree 和任务上下文，在可见 Windows CMD 中启动配置的交互 CLI。离线测试以及本机真实启动、键盘接管、正常退出与同会话恢复已通过，见 [验证记录](devlog/2026-10-21-agent-dispatch.md)。手工 attach 不在单写入者锁保证内；多实现者并行集成仍待验证。
- 模块拆分、认证类型、具体网络/数据库库和缓存参数中的提案仍需确认，不因出现在这里而自动定案。R2 切片已选定 reqwest + Tokio 与 Classic PAT 的类型化错误契约，见 [认证与网络](auth-and-network.md)；数据库、缓存参数与私有内容保留政策仍未定。

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
