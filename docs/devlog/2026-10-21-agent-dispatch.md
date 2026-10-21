# 任务派发 skill 与可见执行会话

- 日期：2026-10-21；任务 ID：agent-dispatch；基层 Agent 实现，主 Agent 复核，负责人验证真实键盘交互。
- 验证环境：Windows，Node.js 24.19.0；脚本声明兼容 Node.js 20+。
- 状态：实现与本地验证完成；最新提交的云端 CI、审查及合并结果以 PR 记录为准。

## 改动与决定

新增 [githux-agent-dispatch skill](../../.agents/skills/githux-agent-dispatch/SKILL.md)，提供 `create/start/status/resume`。create 从显式已提交 base 创建独立分支、linked worktree 和任务上下文，不复制协调仓库的未提交内容或私人记忆；start 在可见 Windows CMD 中启动配置的交互 CLI；status 读取本地状态和进程身份；resume 严格恢复原会话文件，不能以新建会话冒充恢复。

任务进度、PR 和裁决记录保存在基层 Agent 拥有的 `delivery-state.json`；身份、模型配置历史和进程事实保存在监督进程拥有的 `dispatch-state.json`。主 Agent 复核时发现原单文件方案存在并发覆盖风险，已拆开写入所有权并验证退出/恢复不覆盖 `true/coordinator`、PR URL 或进度。每任务锁覆盖启动至子进程退出，PID 联合创建时间核验；重复启动和未知状态拒绝继续，不自动杀进程、解锁或删除 worktree。模型库存精确匹配，失败不替换模型；本地配置不承载凭据。详细契约见 [operations](../../.agents/skills/githux-agent-dispatch/references/operations.md)。

同步更新共享入口、工作流与代码约定。两套 Node 离线测试加入现有 Windows Code check（Node 24，无依赖缓存）；Rust 步骤、四平台 Debug 矩阵和末尾 CI 汇总逻辑不变。未修改应用功能或已有审查脚本，也未添加常驻调度或自动唤醒。

## 离线验证

运行两套 `node --test`：派发 8 项、审查等待 17 项，共 25 项通过。派发测试覆盖临时 Git 仓库的分支/base/worktree、脏工作区与私人内容隔离、路径/配置/库存拒绝、重复启动、进程身份、正常和非零退出、精确恢复、部分创建失败保留证据，以及交付记录在运行/退出/恢复期间保持。Windows 编码桥在隐藏进程中验证空格、Unicode 和 shell 特殊字符往返。

skill-creator 的 `quick_validate.py` 经本机已有 uv 离线缓存提供 PyYAML 后通过；actionlint 1.7.12、Markdown 本地链接和空白检查通过。测试 Git 身份及禁用签名仅用于一次性测试仓库的单次提交，未修改用户全局配置。

## 真实 Windows 验证

使用本机已配置的真实执行工具和可用模型组，在独立 smoke worktree 完成以下流程；任务只读共享内容并在忽略的任务目录写验证标记，没有提交或推送。

1. create 生成新分支、worktree、TASK 和两个状态文件；start 打开可见 CMD，真实模型读取任务并生成初始标记。
2. 活动状态再次 start 被启动锁拒绝，未生成第二个会话。
3. 负责人确认窗口显示、键盘补充指令正常，并使用 `/quit` 退出。监督进程记录退出码 0，status 返回 exited，启动锁释放。
4. resume 使用原会话文件和新的补充文件。原 session ID、cwd 和会话路径保持不变，历史包含负责人补充输入；执行工具读取初始标记并生成恢复标记，返回完成回复。

恢复后的终端保留供负责人使用；模型停止生成不等于 CLI 进程退出。以上结果不证明未知版本 CLI、其他操作系统可见启动、所有特殊字符路径的真实窗口交互，或最终路由后端。特殊字符的验证仍属于离线编码桥；脚本只记录配置的 provider/model。多实现者并行集成尚未验证，当前不解除首轮并发限制。

## 后续

按共享工作流对本次提交运行云端 CI 与 Codex 审查；当前最新 head 的全部门槛通过后 squash merge，保留 GitHub 默认提交信息。未来升级执行工具时复核参数、会话格式与真实终端交互；不以模拟测试替代实际验收。

## 首次云端 CI 修正

首次 PR 检查在 Windows 编码桥测试失败：runner 的临时目录使用 `RUNNER~1` 短路径，而子进程 `cwd` 返回 `runneradmin` 长路径；两者指向同一目录，参数本身完整保留。修正测试以 `realpath` 比较工作目录身份，同时继续逐字核对传入参数，保留特殊字符往返约束。运行脚本无需改变；修正后重新运行派发测试，并对新提交重新触发 CI 和 Codex 审查。
