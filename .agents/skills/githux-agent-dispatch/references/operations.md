# 配置、命令与恢复

## 本机配置

默认从协调仓库 `.agents/dispatch.local.json` 读取，受仓库 `.gitignore` 忽略。create 可传 `--repo ROOT`；create/start/resume 均可传 `--config FILE`。status 只读 worktree 的状态，不需配置。示意 JSON（占位符需按本机配置替换）：

```json
{
  "node": "node",
  "cli": "<absolute JavaScript CLI entry path>",
  "provider": "<exact provider>",
  "model": "<exact inventory model ID>",
  "thinking": "medium",
  "worktree_parent": "<existing absolute parent directory>",
  "kero_project_id": "<shared local Kero project ID>"
}
```

`kero_project_id` 是同一仓库共用的 Kero 分组 ID，不是显示名称。通过 `kero projects --json` 核对 `dir` 对应协调仓库根目录，选择一个现有 `githux` 分组并写入本地配置。缺省仅在该目录恰好有一个分组时自动选择；没有分组或已有多个同名/同目录分组时，在创建会话和锁之前报错，要求明确选择。不会自动新建分组，也不自动移动/关闭旧会话；固定 ID 不受侧栏排序改变影响。首次使用可先在 Kero 建立该仓库的项目分组，再记录 ID。

只接受这些字段，不接受 API key、token、env、URL、自定义参数或凭据文件路径。认证由执行工具自己的既有配置提供；脚本不读写或打印其认证配置，也不修改全局设置。`node` 可为字面值 `node`（解析为运行调度器的 Node），或绝对 Node executable 路径；CLI 必须为已有 .js/.mjs/.cjs 文件。启动使用 PATH 中的 `kero` 与系统 Windows PowerShell；Kero 应已运行。PATH 刚更新时重开终端或刷新当前进程的 PATH，不写死本机安装路径，不改全局配置。worktree parent 缺省采用项目约定的盘根目录；测试和其他本地布局显式指定现有目录。

配置文件和相关目录应由本机可信用户维护；任务进程不是沙箱。实际 CLI 的 Node 版本要求另行满足。脚本把交互子进程挂在 supervisor 下，因此 start/resume 命令返回时 CLI 可能仍在启动；随后用 status 确认，而不是把短暂的 starting 当成已完成。路径支持空格、Unicode、`& % !`、单引号和 `$()` 等字符；拒绝控制字符、双引号、`< > |`。model/provider 只接受字母数字开头及字母数字、`_ . / - & % !`，拒绝空格、冒号 thinking 后缀、通配符和参数注入。过长编码命令在创建 launch lock 之前拒绝，缩短脚本或 worktree 路径再试。

## 四条命令

命令入口：`node .agents/skills/githux-agent-dispatch/scripts/dispatch.mjs COMMAND OPTIONS`。`--config/--followup/--worktree/--repo` 传入时必须为绝对路径。create 的相对 `--spec` 从已验证的 `--repo` 根解析，而不是 shell cwd；调用者负责在自己的终端正确引用参数；使用程序调用时优先 `spawn(executable, args, {shell:false})`。stdout 为 JSON，错误和 Git/库存诊断发到 stderr。可见监督进程的输出留在新终端。

| 命令 | 必需参数 | 可选参数 | 效果 |
| --- | --- | --- | --- |
| create | `--task ID --branch TYPE/NAME --base REF --spec FILE --delivery local-only或pr` | `--repo ROOT --config FILE --first-release true或false` | 将 REF 解析为完整 commit SHA，建立新 branch + linked worktree，生成本地上下文，不启动 |
| start | `--worktree ROOT` | `--config FILE` | 仅首次 created 状态，库存及本机 Kero 检查通过后新建 Kero 会话 |
| status | `--worktree ROOT` | 无 | 本地 JSON，包含诊断和进程观察 |
| resume | `--worktree ROOT --followup FILE` | `--config FILE --model EXACT_ID --provider EXACT_PROVIDER` | 确认全部旧进程停止且原会话有效，再从精确文件恢复 |

task ID 为 1–64 位小写字母数字/连字符，不接受路径片段和 Windows 设备名。worktree 总是现有 parent 的直接子目录 `githux-ID`。分支为小写 `type/name`，支持数字、连字符和子路径，经 Git 再核验。已有路径、分支、任务文件、session 均不覆盖；重复 create/start 会返回具体拒绝信息。无自动删除或隐式重试。

create 使用显式 committed base，保留协调工作区的未提交修改；共享文档来自 base，不从当前脏工作区复制。规格只从 caller 指定的 UTF-8 文件读取，要求 Goal/Scope/Acceptance/Relevant docs 标题。生成的 `.agents/task/TASK.md` 添加 worktree/branch/base/session、交付模式、milestone 和共享流程链接。规格正文是调用者提供的授权上下文，应由主 Agent检查必要性和私密信息。base 必须忽略 `.agents/task/`，不得跟踪任务文件；部分创建失败保留目录和分支，报告恢复线索。

库存命令为配置 Node + CLI 参数数组：`--offline --no-extensions --no-mcp --no-approve --list-models EXACT_ID`。从表格各列精确比较 provider 与 model（至少六列），不接受模糊包含。CLI 报错、超时或缺少匹配均停止。启动前在协调侧和监督进程各核验一次。

首次交互启动：`node CLI --offline --approve --no-extensions --no-mcp --provider P --model M --thinking LEVEL --session-dir DIR --session-id ID @TASK_FILE`。resume 改用 `--session EXISTING_FILE @FOLLOWUP_FILE`，因为 `--session-id` 在缺失时可能创建新会话。`--offline` 仅禁止启动时自动联网，不禁用推理网络；`--approve` 仅信任项目文件。没有 `--print`、JSON/RPC 或管道输入。使用 `kero new --project PROJECT_ID --name githux-TASK-TOKEN --dir WORKTREE -- powershell.exe -NoLogo -NoProfile -NoExit -EncodedCommand ...`；不让 Kero 再建 worktree。编码 PowerShell 桥通过参数数组启动 Node，继承 Kero 的 TTY，CLI 退出后保留终端。任务名附加每次启动的 token 前缀，避免恢复时与旧终端重名。CLI 和错误都在该 Kero 会话显示；不再创建独立 CMD 或回退到其他终端。

resume 要求有效 session header（`type=session`、精确 `id`、原 `cwd`），已有记录路径也必须一致；缺失、重复、损坏或工作目录不同都拒绝。每份 follow-up 以独立唯一文件保留。原 TASK.md、session ID、branch、worktree 不变。配置模型变化不会被默默采用：resume 明示 `--model`，需要换 provider 时一并给 `--provider`；库存仍必须通过。requested/configured model 和每次启动历史保存；provider/model 或 group 名称不证明最终实际使用的后端模型。

## 在 Kero 中定位、交接和恢复

`start/resume` 先通过 `kero ls --json` 检查本机 Kero；不可用或返回无效时，在创建锁前报错。非空 `KERO_HOST` 被拒绝：本脚本的 worktree 与 PID 校验只覆盖本机，不提供远程派发。调用者无需本身运行在 Kero 内。

启动前通过 `kero projects --json` 解析共享项目 ID；配置 ID 不存在或目录不属于协调仓库时拒绝启动，不回退到按 worktree 分组。终端元数据和启动回执记录 `project_id`，实际工作目录仍是任务自己的 worktree。

启动返回时可能仍为 starting，先通过 dispatch status 确认 `active` 和 `terminal`。监督进程从实际 Kero 环境记录 session/pane ID；启动者将 Kero 返回的 session ID 写到独立 `kero-TOKEN.json` 回执，避免覆盖并行更新的 runtime。status 合并回执，身份不一致时拒绝继续。ID 尚未出现时，可按已记录的唯一 `terminal.name` 在 `kero ls --json` 中定位，不猜测或复用其他人的 pane。

```sh
kero status PANE_ID --json
kero read PANE_ID -n 40
kero send PANE_ID "补充任务内容" --enter
kero send PANE_ID "/quit" --enter
node .agents/skills/githux-agent-dispatch/scripts/dispatch.mjs status --worktree ABSOLUTE_WORKTREE
```

发送前核对该 pane 属于本任务的 session 和 worktree；使用明确的 pane ID，不依赖 focused pane 或默认当前终端。任务仍在生成时，补充内容遵循 CLI 的排队语义。只有 dispatch status 确认旧 CLI 已停止，才能用本 skill 的 `resume --followup` 恢复原 Pi session；`kero resume` 不替代这条带进程锁和精确会话文件核验的路径。Kero 的 idle/exited 标签不代表任务完成，也不能替代进程身份检查。

确认本次测试或任务 CLI 已退出、无需保留终端后，才可 `kero close SESSION_ID` 关闭自己创建的会话；不使用 force，不删除 worktree，不关闭负责人正在使用的终端。若继承的是旧 CMD 会话，先在旧终端正常退出 CLI；既有状态文件仍可读取，下一次 dispatch resume 自动进入 Kero。启动失败可能已创建 Kero 会话：保留锁和回执，按下方恢复步骤核验，禁止盲目重复 new/start。

## 状态所有权与单写入者

状态刻意拆成两个文件，避免监督进程退出/启动的写入覆盖基层 Agent 的并发交付更新：

- `.agents/task/dispatch-state.json`（schema_version=1）：调度器拥有任务身份、会话、模型历史和 runtime。监督进程只原子替换这个文件。
- `.agents/task/delivery-state.json`（schema_version=1）：基层 Agent 拥有 `progress`、`pr_url`、`arbitration_ever_required`、`merge_owner`、`updated_at`。调度器只读取并在 status 合并输出，从不改写它；恢复或换模型也不能把它重置为 false/worker。

| 字段 | 所在文件 | 含义 |
| --- | --- | --- |
| task_id/repo/worktree/branch/base_sha | dispatch | 固定任务和 Git 身份 |
| session_id/session_dir/session_path | dispatch | 固定 ID、独立目录、已观测精确会话文件 |
| provider/requested_model/configured_model/thinking/model_history | dispatch | 配置请求与启动历史；backend_model=unverified |
| delivery_scope/milestone | dispatch | local-only 或 pr；显式首版任务为 1，否则 null |
| runtime.phase/attempts | dispatch | launcher/supervisor/child 的 PID + 创建时间、时间戳、exit code/signal；与任务是否完成无关 |
| runtime.attempts[].terminal | dispatch | Kero kind/name；监督进程观测到的 session_id/pane_id，每轮保留 |
| terminal | status 派生 | 最近一次启动的 Kero 身份，合并独立启动回执；旧 CMD 任务为 null |
| arbitration_ever_required/merge_owner/pr_url/progress | delivery | 交付状态；出现过裁决就保持 true/coordinator，PR 历史仍需按共享流程核验 |
| process_state/observed/diagnostic | status 派生 | 即时进程观察，不落盘、不写假 heartbeat |

基层 Agent 用“同目录临时文件 + rename”原子替换 delivery-state.json，保留固定 `task_id`；不要编辑 dispatch-state.json。若 delivery 文件缺失或格式无效，status 会明确失败，而不是默认 false。两个文件之间的交叉字段没有自动同步，status 将 dispatch 与 delivery 合并输出，同名字段不应重复设置。

`created` 表示尚未启动；`starting` 表示持锁的启动者或监督进程仍在启动阶段；`active` 仅表示监督进程与 child 的创建身份均存活；`exited` 表示已观测 CLI 正常退出；`failed` 表示已观测 spawn 错误或非零退出；`unknown` 表示无法安全确定。退出 0 不能将 progress 改成完成；active 不能证明模型正在推理，也不能证明任务完成。脚本没有 heartbeat，也不从进程存活推断思考状态。状态/进程读取权限不足也不能默认进程已死。

每任务 `launch.lock/` 由原子 mkdir 排他占用，从终端启动前覆盖至 child 真正退出。launch.json 记录启动 token；supervisor.claim 排除同 token 被重放。Windows 以 Win32_Process 的 CreationDate 联合 PID 验证，Linux 离线测试支持 /proc starttime；不会只按 PID 判断。监督进程处理 Ctrl-C 时继续等待 child 退出，不自动杀任何进程；终端关闭可能同时终止两者，此时 lock 保留。

手动外部 `--session`/`--session-id` attach 不遵守此锁，不能保证不会发生竞争。脚本尽力在本机命令行中检测 ID/精确 session path；权限限制、相对路径和 attach 瞬时竞态均有边界。负责人应只在现有终端接管，不在别处同时 attach，不同时使用 RPC/TUI。无法核验的状态保持 unknown。

## 明确恢复，不自动清理

1. `status --worktree ROOT`，查看诊断、attempts、lock 中元数据和新终端错误。若 active，直接在该终端使用或正常退出 CLI。不要再 start。
2. 若 unknown，核对每轮 supervisor/child 的 PID **及创建时间**、可能外部 session 进程、终端是否还存在。supervisor 消失不证明 child 已停；没有 child 元数据可能是在 spawn 前后崩溃。无足够证据就停止恢复，不按 PID 杀进程。
3. 确认全部相关进程停止后，由负责人/主 Agent将锁目录重命名为唯一的归档名，保留 launch/claim 证据；同时仅更新 dispatch-state.json 的 runtime 并追加 recovery 记录（时间、归档名、检查结果），不触碰 delivery-state.json。若曾 claim/spawn 或元数据不完整，先人工查清 CLI 是否存在，保存证据并明确改为可审计状态；不得按 PID 推断“已停止”。
4. resume 仍要求同一原会话文件。若首次启动从未持久化 session，任务不能用 resume 猜测新会话；保留失败证据，明确决定另建任务。只有在全部进程已停、首次 start 确未持久化 session 且已保留审计记录时，人工恢复 created 才能重新首次启动。
5. create 部分失败先检查 `git worktree list`、分支指向、目标目录与上下文。不要把普通重试变成覆盖现有状态；修复由主 Agent明确处理。此 skill 不递归清理 worktree、session 或锁归档。

## 验证边界

`node --test .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs` 在 disposable Git 仓库使用假 CLI，覆盖真实 worktree、命令数组、进程创建时间、生命周期、原会话恢复、模型库存和冲突拒绝。Windows 专项隐藏执行内层 PowerShell recorder，验证路径/参数字符往返，**不创建真实 Kero 会话**。临时 Git 身份只传给测试 commit，签名只对该测试 commit 禁用，不改全局配置。

这些测试不证明真实 CLI 的 TTY、Kero 会话、人工输入/接管、真实模型接入或路由后端。负责人/主 Agent用本地配置实测 create → start → 读写/接管 → 退出 → status → resume，并验证最新安装 CLI 的会话 header/参数与本脚本契约一致。离线脚本测试不替代 PR 的最新 head CI/review 门槛。

2026-10-21 已用本机配置完成真实 create/start/status/resume：负责人确认可见窗口显示和键盘输入正常，并用 `/quit` 正常退出；监督进程记录退出码 0、释放锁，resume 保留原会话 ID/文件与历史，并完成补充任务。证据见 [开发日志](../../../../docs/devlog/2026-10-21-agent-dispatch.md)。这仅验证当时的本机 Windows/CLI 配置，不证明其他 CLI 版本、路由后端或并行集成已验证；特殊字符路径的证据仍限于离线编码桥测试。

Kero 启动与恢复的本机验证记录见 [2026-11-03 开发日志](../../../../docs/devlog/2026-11-03-agent-dispatch-kero.md)。历史 CMD 验证只代表当时的旧启动载体。
