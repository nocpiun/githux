# Settings / Accounts 原生组件

任务 `settings-accounts`，基线 `997f6e947fced45ca00b728fcea4756957f36061`，分支 `feature/settings-accounts`。负责人批准宽态分类栏/行式账户列表、按需表单和窄态单行横向 Tabs；本任务不接 Shell，不批准候选设置分类。

## 实现与接线

新增 `src/ui/settings/**`：SettingsView 负责响应布局和公共滚动，SettingsBinding 连接既有 Accounts 控制器，账户视图展示权威目录。公开分类只有 Accounts；原有 `src/ui/accounts/**`、`src/app/accounts/**` 与 Shell 注册文件保持原样。宿主接口与临时状态归属见 [Settings 契约](../settings.md)。

Add/Replace 默认收起；非当前账户替换固定点击目标，identity mismatch 仍由现有 AccountService 拒绝；提交后重建输入实体。Enter/Esc 避让 IME marked range。进行中重复提交和冲突操作禁用；取消调用现有控制器，保留已投递提交的收尾/对账，不宣称回滚。成功根据权威目录 epoch 与控制器结果判定，返回原入口焦点。当前账户省略 Switch。头像直接复用公共圆形渲染器。

新增 `examples/settings.rs` 及任务专属 `settings_example/settings_fixture`，预览与测试共用可重复传输。临时 SQLite + 现有内存凭据，无真实凭据/账户、无实网。场景为正常、过期、缺凭据、重启待验证、网络失败和慢请求；fixture 分类显式标注，用于窄态溢出验证。场景加载 generation 防止旧后台初始化替换新场景。

## 验证记录

首轮 `node scripts/verify.mjs -- cargo check --locked --example settings` 暴露并已修正传输签名、PAT 导入路径和滚动时钟类型问题。第一次聚焦测试（run `a7544002…`，exit 101）又暴露三处局部编译问题：带交互的 DOM 元素不能 clone、测试作用域缺 `AppContext`、`debug_bounds` 要求静态 selector；均已按日志修正。后续运行结果在本节补齐，不把编译失败当作行为失败。

聚焦测试使用 `node scripts/verify.mjs -- cargo test --locked --example settings settings::`；已通过的 run `5d3edeb5…` 为 7 passed、0 failed，覆盖：

- 表单默认收起；Add 空提交显示字段错误；Replace 进入时输入获焦、掩码、可显隐；Esc 清空秘密并回到原 Replace 焦点；再次 Add 输入为空。
- 非当前 Sam 的 Replace：用 Alex token 被真实 `AccountService` 拒绝，两个账户均不变；用 Sam token 只推进 Sam 的 epoch，Alex 的 epoch 与活动选择不变，成功后表单收起并返回原按钮。
- 当前账户不绘制 Switch，非当前账户有 Switch；Add Jordan、Switch Sam、Revalidate Sam 后权威服务状态与 UI 收敛，epoch 推进且状态 Verified。
- Slow 场景重复提交只产生一次请求且 busy；取消关闭表单，不宣称回滚。
- IME marked range 下 Enter 不提交，表单不关闭，账户数不变。
- 648px 内容宽度、五个 fixture 分类：水平 Tabs 不宽于根、Accounts 内容与两行不越界，End 键 reveal 最后一个分类（滚动偏移为负、末项进入可视区）。

同时通过 `cargo fmt --all`、`cargo clippy --locked --all-targets -- -D warnings`（run `cdb86db0…`）、`cargo build --locked --example settings`（run `1c5281fc…`）与 `cargo check --locked --all-targets`（run `4e0c40a3…`）。首次 `cargo build` 的 LNK1104 是仍在运行的 `settings.exe` 占用输出文件，关闭本任务进程后重建成功；不是源码失败。

原生窗口（Windows，本机，2560×1440 桌面/150% 缩放）：`target/debug/examples/settings.exe` 标题为 `Githux · Settings Accounts preview`、Responding。宽态用整屏截图核对以下内容：200px 预留侧栏、Accounts 分类列、720px 上限内容列、两个圆头像行、当前行只有 Revalidate/Replace PAT、非当前行有 Switch，以及没有启动提示残留。通过 `GITHUX_SETTINGS_PREVIEW_FORM=replace` 让同一窗口在自动读取 settle 后打开 Replace 表单，截图确认目标 Sam（非当前）、掩码输入、帮助入口、Cancel replacement 与空输入禁用的 Verify and replace。880 物理窗口（本机 DPI 约 895px 外框）确认 Tabs 同行且 Accounts 被选中。更窄的 648px 内容溢出边界只有 GPUI 布局测试证据；没有声称手工压到了该逻辑宽度。

**未执行 / 不能由此推断**：真实 PAT 登录或 GitHub 联调；真实 Windows 输入法消息顺序、键盘接管和手写拖动手感；多 DPI 全覆盖；正式 Shell 挂载；macOS/Linux 产品验收。截图为真实 Ely 原生窗口，但未经负责人视觉验收，PR 保持 `pending`。

测试写在组件旁，使用 example 既有 target 的过滤测试，不新增 `tests/settings.rs` 复制整应用测试目标。静态设计 PNG/HTML 和 GPUI 测试平台均不算原生证明。

## 审查修复

Codex 首轮 finding（[discussion](https://github.com/nocpiun/githux/pull/56#discussion_r4434714882)）指出了真实契约缺陷：当前账户成功提交后，控制器会立即自动读取仓库；若该读取失败，其错误状态覆盖提交状态，而旧 `settle` 以“控制器不 busy 且最新状态为成功”判定完成，会使已提交的表单保持打开，并把后续读取失败呈现为 PAT 保存失败。

修复：`settle` 只以提交前记录的账户身份与权威目录中的 epoch 差确认提交，不再依赖最新状态或 busy。`Scenario::ReposFail` 让 `/user` 成功、`/user/repos` 返回 503；新增 `committed_form_closes_even_when_the_follow_up_repository_read_fails` 验证表单关闭、提交不被误报为保存失败、仓库失败留在独立状态。聚焦测试现为 8 passed，clippy 通过。

## 第二轮审查修复

第二轮 finding（[discussion](https://github.com/nocpiun/githux/pull/56#discussion_r4434744048)）指出：提交失败后表单保持打开，但其行的 Revalidate 可用；无关重验证推进 epoch 会被上一轮实现误当成 PAT 已提交。修复为：提交失败且控制器不再 busy 时清除本绑定的待完成状态，之后 epoch 变化不再能关闭表单；现行表单保留，已提交的秘密仍按既有规则立即清空，失败后新输入不受影响。新增 `a_failed_submission_is_not_completed_by_a_later_unrelated_revalidation`，并再次覆盖第一轮“自动读取失败不能让已提交表单重开”的场景。聚焦测试 9 passed。

## 第三轮审查修复

第三轮 finding（[discussion](https://github.com/nocpiun/githux/pull/56#discussion_r4434773534)）指出 `activity` 门控把启动错误/警告也隐藏了。修复为只抑制尚未操作时的 `StatusTone::Info`，Error/Warning/Success 照常显示；新增 `a_startup_repository_failure_is_visible_before_any_user_action` 直接以 `Scenario::ReposFail` 验证自动读取失败在无用户操作时可见。同时为状态行补 debug selector。10 项聚焦测试、clippy 与全量测试通过。

## 交付与剩余边界

待完成最终 required checks 与 PR。UI PR 添加 `pending`，负责人原生验收通过前不合并。真实 PAT、Windows IME 消息顺序、多 DPI、正式 Shell 组合和非 Windows 产品验收不能由该离线预览推断。

LESSONS 本任务基线仅 L001–L012；任务提及的 L014 未在该基线存在，按任务明确的紧凑可操作错误要求实现。未直接修改共享 LESSONS；若验证发现可复用问题，在本日志保留证据交协调者整合。
