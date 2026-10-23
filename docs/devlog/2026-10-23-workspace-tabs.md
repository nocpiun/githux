# 应用级标签：会话、打开/复用/关闭与最小窗口边界

- 日期：2026-10-23；任务 ID：workspace-tabs；执行者：基层 Agent。
- 状态：实现、自动检查与 GPUI 测试平台回归完成；**真实 Windows 窗口的多标签交互与中文候选确认仍待负责人验收**，`human_review_required=true`、`human_review_approved=false`，本轮不自行合并。
- 交付范围：[路线图](../roadmap.md) R1 剩余的基础页面会话/多标签外壳。账户/网络/缓存仍为 fixture，不含真实读取；H6 进阶标签交互不在范围。
- 基线：`b00a9e382137f90c5d2364fb04372b0cacf86d74`（Tall 原生标题栏已验收合并）。

## 确认规则（负责人 2026-10-23）

- 点击侧栏仓库/Profile、底部入口或通过 command palette 选择页面，统一执行“打开或激活”：同一资源已打开则切换，否则新建。
- Home、Settings 及当前全局占位页（Notifications、Issues、Pull requests）按路由单例；仓库按 `owner/repo` 单例。GitHub 名称大小写不敏感，别名不生成重复会话。不支持同资源重复标签。
- 启动仍只有 Home，不恢复上次会话。
- 关闭非活动标签不切换当前标签；关闭活动标签切到左邻，无左邻则右邻；关闭最后一个标签后新建 Home 标签。
- 标签可鼠标打开/切换/关闭，键盘可到达；`Ctrl+W` 关闭当前标签，`Ctrl+Tab` / `Shift+Ctrl+Tab` 切换。启动/上限/最小窗口等边界见 [产品约定](../product.md#外壳与导航)。

## 实现与决定

**会话模型（`src/workspace/mod.rs`）。** `Workspace` 持有有序的 `Session` 集合与唯一活动标签。`TabId(u64)` 单调递增，关闭后不复用。`Route::key()` 给出可比较的资源身份，仓库部分做 ASCII 小写折叠，因此 `Githux-Dev/GitHux` 与 `githux-dev/githux` 落到同一会话（保留首次打开的显示大小写）。`Workspace::open` 返回 `Activated`/`Opened`/`LimitReached`；`close` 返回新的活动标签；`cycle(±1)` 环绕切换。`MAX_OPEN_TABS = 8` 是集中的预览上限，不是永久产品承诺。

**每标签页面状态。** 每个 `Session` 持有真实的 `ScrollHandle`、页面 `FocusHandle` 与标签条 `FocusHandle`，以及仓库会话选择（`branch` / `file`，R3 阅读器填充；本轮只由测试驱动）。关闭会话即 drop 这些句柄与状态；island 页容器按 `TabId` 取元素 id，元素状态不会在页面之间串用。

**island 布局。** island 顶部新增可操作标签条（`src/ui/tabs.rs`），下面是活动页自身的滚动区。标签显示图标 + 省略标题，tooltip 给出完整名称；活动标签用 Ely 的 `active` 背景与中等字重，不新增分隔线或改动已验收配色、圆角、标题栏高度、字体与搜索框尺寸。标签条 `min_w_0 + overflow_hidden`，每个标签在 `TAB_MIN_WIDTH`（56）与 `TAB_MAX_WIDTH`（180）之间伸缩，因此标签数量不会把 island 撑宽。

**打开/关闭入口统一。** 侧栏仓库行、Home/Settings、账户行、底部四入口与 command palette 全部走 `Shell::navigate`，复用规则只此一处。达到上限时当前页不变、不丢会话，标签条右侧显示简短提示（`tab-limit-hint`）；下一次成功的打开/切换/关闭会清除它。提示采用“直到下一次标签操作”的确定性清除，不引入计时器，便于测试。

**焦点归属。** 每个会话的页面句柄互不相同；切换或关闭时，只有当焦点原本位于“正在离开的页面”内时才移动到新页面的句柄，否则保持用户当前焦点（点侧栏行、点标签、搜索入口都不被抢走）。关闭活动标签后焦点落到新的活动页。command palette 关闭仍把焦点还给搜索入口，始终是有效目标。搜索入口与标签条位于 `tab_index(-1)`，根焦点为 `-2`，因此 Tab 顺序是“搜索入口 → 标签条 → 侧栏列表”，不会先穿过 124 条仓库行。

**键盘与输入法边界。** `secondary-w`、`ctrl-tab`、`ctrl-shift-tab` 在 `Shell::bind_keys` 集中绑定；这些组合键不使用 Enter/Escape/方向键，因此不与输入法候选确认或文本输入争用。palette 打开时由它独占键盘，标签快捷键此时不做任何事。

**未做（H6 与后续任务）。** 不做重排、专门的溢出菜单、固定标签、前进/后退历史、重启恢复或重复资源标签；不接入真实分支/文件读取；账户切换时如何处理标签仍未定，未自行决定。

## 验证

本机 Windows x86_64-pc-windows-msvc，工具链 1.98.1。以下命令全部退出 0：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
cargo test --locked
git diff --check
```

`cargo test --locked` 共 50 项通过（基线 34 项 + 本任务新增 16 项，含 8 项 workspace 单元测试）。新增的 GPUI 测试平台回归：

- `opening_the_same_page_reuses_its_tab`：侧栏点击与 palette 都复用同一标签，不重复。
- `repositories_and_profile_keep_separate_scroll_focus_and_selection`：同时打开仓库 A/B 与 Profile；Home 真实滚轮滚动后切走再切回保留自身偏移，A/Profile 偏移保持 0；`Ctrl+Tab`/`Shift+Ctrl+Tab` 后焦点分别落到 B/A 各自的页面句柄；A/B 的仓库会话选择互不串用。
- `closing_tabs_keeps_the_active_page_and_picks_a_neighbour`：关闭非活动标签不切换；`Ctrl+W` 关活动标签切左邻；关最后一个后新建 Home 且 id 不复用。
- `the_tab_limit_keeps_existing_sessions_and_shows_a_hint`：8 个标签后打开第 9 个被拒，活动标签、id 与路由不变，显示提示；切到已打开页仍可用并清除提示。
- `the_tab_strip_stays_bounded_at_the_minimum_window`：880×560、8 个标签时标签条不超出 island，每个标签都落在条内且宽度 ≥ 55px。
- `keyboard_closes_and_cycles_tabs_and_the_palette_keeps_its_keys`、`closing_a_tab_releases_its_state_and_focus`、`tab_reaches_the_session_strip_and_enter_activates`：快捷键、关闭释放状态/焦点、Tab 到达标签条并用 Enter 激活。
- workspace 单元测试：启动只有 Home、复用、大小写别名、上限拒绝、关闭邻居顺序、id 不复用、环绕切换、会话状态隔离。

**最小窗口证据：** 在 880×560、8 个标签时，island 内容宽约 631px，标签条内每个标签实测约 73.5px（测试断言 ≥ 55px），标签标题以省略号截断、tooltip 给完整名称。8 个标签在该窗口仍可用，因此本轮保留 `MAX_OPEN_TABS = 8`；若后续真实窗口认为过窄，可下调该常量或改为更窄的标签样式，不需要新增进阶交互。

**未覆盖 / 待人工验收（不据此宣布通过）：**

- 真实 Windows 窗口的多标签鼠标交互（点击/关闭/切换）、`Ctrl+W`/`Ctrl+Tab`/`Shift+Ctrl+Tab` 在真实键盘分发下的行为。
- 中文输入法候选确认与标签快捷键/文本输入的共存，仍需真实 IME 验证（本轮只保证组合键不与 Enter/Esc/方向键重叠，并在 palette 打开时不动作）。
- 关闭标签后真实窗口的焦点环与滚动位置；多 DPI/跨屏下标签条的截断与 tooltip。
- macOS/Linux 仍用系统标题栏，未做标签条原生验收。

**预览：** 已在本 worktree 执行 `scripts/prepare-windows-runtime.ps1`（x64，写入 `target/debug`）。负责人可在该终端运行 `cargo run --locked` 后按上述检查项验收。

## 首轮审查（head `1f76b37`）与待裁决事项

首轮 Codex review `5627818142` 提出两条：

1. **P2（`src/ui/mod.rs`）：`close_tab` 只检查页面焦点，忽略标签条焦点。** 用户 Tab 到活动标签后按 `Ctrl+W` 时，被关闭的标签条句柄仍持有焦点，焦点可能悬空。属于既有验收（“关闭标签释放专属滚动/焦点/状态”）内的正确性修复：现在同时检查被关闭会话的页面焦点与标签条焦点，焦点在两者任一处时都把焦点移到新的活动页。新增回归测试 `closing_a_focused_tab_moves_focus_to_a_live_page`。该修复已在本地完成并通过 fmt/clippy/test（51 项），按混合批次默认暂不 push。
2. **P1（`docs/devlog/...` 行内意见 `4359204869`）：要求本 PR 更新 `docs/architecture.md`。** 该文件“本轮状态归属”确实已过时，但本任务派发明确禁止修改 `docs/architecture.md`，属于任务边界与审查意见的取舍，已按流程永久记录为待裁决（PR 评论 `6364717050`），`merge_owner=coordinator`。

在裁决前不再 push 新 head，也不进入下一轮审查。

## 交接与后续

1. 负责人/主 Agent 在真实窗口验收多标签交互与中文候选确认后，再按门槛合并；`human_review_required=true`。
2. `docs/architecture.md` 的“本轮状态归属”仍写着“窗口只有一个活动页，`Workspace` 只保存当前 `Route`”，该文件不在本任务写入范围，已变为过时描述，需由有权修改者同步。
3. R3 接入真实分支/文件读取时填充 `RepositorySession` 并替换 fixture 默认值；账户切换如何影响标签仍待负责人决定。
