# 应用级标签：会话、打开/复用/关闭与最小窗口边界

- 日期：2026-10-23，2026-10-25 同步更新；任务 ID：workspace-tabs；执行者：基层 Agent。
- 状态：实现、自动检查与 GPUI 测试平台回归完成；首轮审查的焦点修复与架构文档同步已完成，正在新一轮审查。**真实 Windows 窗口的多标签交互与中文候选确认仍待负责人验收**，`human_review_required=true`、`human_review_approved=false`，PR 保持 open，不自行合并。
- 交付范围：[路线图](../roadmap.md) R1 剩余的基础页面会话/多标签外壳。账户/网络/缓存仍为 fixture，不含真实读取；H6 进阶标签交互不在范围。
- 基线：`b00a9e382137f90c5d2364fb04372b0cacf86d74`（Tall 原生标题栏已验收合并），已合并 `origin/main`（含 PR #14 认证服务、#15 运行时 runner、#17 审查流程）。

## 确认规则（负责人 2026-10-23）

- 点击侧栏仓库/Profile、底部入口或通过 command palette 选择页面，统一执行“打开或激活”：同一资源已打开则切换，否则新建。
- Home、Settings 及当前全局占位页（Notifications、Issues、Pull requests）按路由单例；仓库按 `owner/repo` 单例。GitHub 名称大小写不敏感，别名不生成重复会话。不支持同资源重复标签。
- 启动仍只有 Home，不恢复上次会话。
- 关闭非活动标签不切换当前标签；关闭活动标签切到左邻，无左邻则右邻；关闭最后一个标签后新建 Home 标签。
- 标签可鼠标打开/切换/关闭，键盘可到达；`Ctrl+W` 关闭当前标签，`Ctrl+Tab` / `Shift+Ctrl+Tab` 切换。启动/上限/最小窗口等边界见 [产品约定](../product.md#外壳与导航)。

## 实现与决定

**会话模型（`src/workspace/mod.rs`）。** `Workspace` 持有有序的 `Session` 集合与唯一活动标签。`TabId(u64)` 单调递增，关闭后不复用。`Route::key()` 给出可比较的资源身份，仓库部分做 ASCII 小写折叠，因此 `Githux-Dev/GitHux` 与 `githux-dev/githux` 落到同一会话（保留首次打开的显示大小写）。`Workspace::open` 返回 `Activated`/`Opened`；`close` 返回新的活动标签；`cycle(±1)` 环绕切换。标签数量不设上限（2026-10-25 负责人修订，原 `MAX_OPEN_TABS = 8` 已删除）。

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

`cargo test --locked` 在同步 `origin/main` 后共 161 项通过（本任务的 51 项外壳/工作区用例 + 服务层用例）。本任务新增的 GPUI 测试平台回归：

- `opening_the_same_page_reuses_its_tab`：侧栏点击与 palette 都复用同一标签，不重复。
- `repositories_and_profile_keep_separate_scroll_focus_and_selection`：同时打开仓库 A/B 与 Profile；Home 真实滚轮滚动后切走再切回保留自身偏移，A/Profile 偏移保持 0；`Ctrl+Tab`/`Shift+Ctrl+Tab` 后焦点分别落到 B/A 各自的页面句柄；A/B 的仓库会话选择互不串用。
- `closing_tabs_keeps_the_active_page_and_picks_a_neighbour`：关闭非活动标签不切换；`Ctrl+W` 关活动标签切左邻；关最后一个后新建 Home 且 id 不复用。
- `more_tabs_than_fit_scroll_horizontally_instead_of_compressing`：880×560 下打开 12 个标签全部成功；每个标签保持固定可读宽度；island 不被撑宽；平移回行首后最后一个标签确实超出可视区；标签纵向位置与只有 1 个标签时一致。
- `wheel_and_manual_scroll_move_only_the_strip`：垂直滚轮把标签条横向移动，`Shift`+滚轮（Windows 已转成横向 delta）只移动一次，页面与侧栏偏移不变；重绘不抢回手动位置；关闭尾部标签后偏移被 clamp。
- `the_strip_scrollbar_can_be_dragged`：Ely 的覆盖式水平滚动条拇指可拖动，拖动只改变标签条横向偏移。
- `opening_and_switching_reveal_the_target_tab`、`tabbing_to_an_offscreen_tab_reveals_it`：新建/激活/Tab 聚焦可视区外的标签后它被滚入视野，且后续重绘不重置手动滚动；键盘焦点不会激活标签。
- `middle_click_closes_a_tab_once_without_activating_it`、`left_click_and_enter_still_activate_a_tab`：中键在图标、关闭按钮、空白/末个标签上各关闭一次且不激活、不重复关闭；左键与右键语义不变。
- `the_first_tab_is_inset_equally_on_top_and_left`：首标签相对 island 内边界（计入 1px 边框）的上/左间距相等，聚焦不改变标签几何。
- `keyboard_closes_and_cycles_tabs_and_the_palette_keeps_its_keys`、`closing_a_tab_releases_its_state_and_focus`、`tab_reaches_the_session_strip_and_enter_activates`：快捷键、关闭释放状态/焦点、Tab 到达标签条并用 Enter 激活。
- workspace 单元测试：启动只有 Home、复用、大小写别名、任意数量会话、关闭邻居顺序、id 不复用、环绕切换、会话状态隔离。

**最小窗口证据：** 880×560 下 island 约 633×505，标签条 631×46（8px inset + 26px 标签 + 12px 滚动条带），首标签相对 island 内边界的上/左间距都是 8px；12 个标签时每个保持 168px 宽、行内容宽约 2064px，可视宽 615px，滚动余量约 1445px。

**未覆盖 / 待人工验收（不据此宣布通过）：**

- 真实 Windows 窗口的多标签鼠标交互（点击/关闭/切换）、`Ctrl+W`/`Ctrl+Tab`/`Shift+Ctrl+Tab` 在真实键盘分发下的行为。
- 中文输入法候选确认与标签快捷键/文本输入的共存，仍需真实 IME 验证（本轮只保证组合键不与 Enter/Esc/方向键重叠，并在 palette 打开时不动作）。
- 关闭标签后真实窗口的焦点环与滚动位置；多 DPI/跨屏下标签条的截断与 tooltip。
- macOS/Linux 仍用系统标题栏，未做标签条原生验收。

**预览：** `.cargo/config.toml` 已由 PR #15 合入，Windows 下 `cargo run` 会自动准备 app-local Windows App SDK 运行时（见 [Windows 运行时](windows-runtime.md)），不再需要手工执行 `scripts/prepare-windows-runtime.ps1`。负责人可直接在该终端运行 `cargo run --locked` 并按上述检查项验收。

## 首轮审查（head `1f76b37`）与裁决记录

首轮 Codex review `5627818142` 提出两条：

1. **P2（`src/ui/mod.rs`）：`close_tab` 只检查页面焦点，忽略标签条焦点。** 用户 Tab 到活动标签后按 `Ctrl+W` 时，被关闭的标签条句柄仍持有焦点，焦点可能悬空。属于既有验收（“关闭标签释放专属滚动/焦点/状态”）内的正确性修复：现在同时检查被关闭会话的页面焦点与标签条焦点，焦点在两者任一处时都把焦点移到新的活动页。新增回归测试 `closing_a_focused_tab_moves_focus_to_a_live_page`（提交 `2afffde`）。
2. **P1（`docs/devlog/...` 行内意见 `4359204869`）：要求本 PR 更新 `docs/architecture.md`。** 该文件“本轮状态归属”确实已过时，但原派发禁止写入该文件，属于任务边界与审查意见的取舍。

按流程已永久记录待裁决（PR 评论 `6364717050`），`arbitration_ever_required=true`、`merge_owner=coordinator`。负责人/主 Agent 随后裁决**采纳选项 A**，在本 PR 内更新 `docs/architecture.md` 的相关段落（公开裁决记录：[comment 6393565586](https://github.com/nocpiun/githux/pull/16#issuecomment-6393565586)）。

## 裁决后的文档同步

- `docs/architecture.md`：修正过时的“窗口只有一个活动页，`Workspace` 只保存当前 `Route`”描述，改为会话集合、活动标签、每会话滚动/焦点/仓库会话状态与不可复用 `TabId`；同时修正 `TabId`/`ResourceKey` 段落（当前规则不允许重复资源标签）、工程边界中的 `ui::Shell`/`workspace::Workspace` 职责与 `src/ui/tabs.rs`，并保留 R2 服务已实现但 UI 未接入的说明。账户切换时的标签处理仍写明未决定。
- `docs/README.md`、`docs/roadmap.md`、`README.md`：只调整与本任务直接相关的入口描述（应用级基础标签已实现、服务层已实现但 UI 未接入），不重写无关章节。
- 与 `origin/main` 同步：只有 `README.md` 出现冲突（测试证据段落），合并两侧内容为“外壳 + 服务层两部分证据”；其余文件自动合并，未重写无关章节。

再次验证（合并 `origin/main` 之后，本机 Windows x86_64-pc-windows-msvc，工具链 1.98.1）：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked`、`cargo test --locked`（161 项）、`git diff --check` 全部通过。

## 第二轮审查（head `91b3ada`）

新 head 的 Codex review `5641707950` 提出一条 P2：焦点落在**非活动**标签的标签条条目上，用鼠标关闭按钮关闭该标签时，`focused_in_closed` 成立但 `was_active` 为假，原先的 `was_active && focused_in_closed` 守卫不会移动焦点，被关闭的句柄随之离开树。属于既有验收（“关闭标签释放专属滚动/焦点/状态”）内的正确性修复，直接修复：

- `close_tab` 现在区分焦点来源：焦点来自被关闭会话的**标签条**时，转到当前活动会话的标签条条目；来自**页面**时，转到当前活动会话的页面。活动会话本身不变（关闭非活动标签仍是“不切换当前页”）。
- 新增 `closing_a_focused_inactive_tab_keeps_the_page_and_restores_focus`；同时修正上一轮用例的期望——`Ctrl+W` 关闭活动标签时若焦点在标签条上，落点是新活动标签的条目而不是页面。
- 本地 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo build --locked`、`cargo test --locked`（162 项）与 `git diff --check` 全部通过。

## 第三轮：负责人真实窗口反馈后的四项修订（2026-10-25）

负责人查看真实窗口后提出四点，均已实现；这不是验收通过，`human_review_approved` 仍为 `false`。

**1. 上/左间距不一致。** 原先 `TAB_STRIP_HEIGHT = 38` 用 `items_center` 隐含 6px 上间距、`px_2` 给 8px 左间距。现在标签条只有一个集中值 `TAB_STRIP_INSET = 8`，同时用于 `pt`/`pl`/`pr`；高度改为 `inset + TAB_HEIGHT + TAB_SCROLLBAR_BAND`（8+26+12=46），滚动条带始终保留，所以滚动条出现/消失不移动标签。测试在 880×560 下核对首标签相对 island 内边界（计入 1px 边框）的上/左间距相等。island 圆角、Ely 配色、原生 Tall 标题栏与搜索框尺寸未动。

**2. 去掉 8 标签上限，改为横向滚动。** `MAX_OPEN_TABS`、`OpenResult::LimitReached`、`tab-limit-hint` 与其测试全部删除；`Workspace::open` 永远新建或激活。标签宽度改为一个固定可读值 `TAB_WIDTH = 168`（不再在 min/max 之间压缩），行使用 GPUI 的 `overflow_x_scroll` + `track_scroll`，因此只有横向一个滚动轴：垂直滚轮、Windows 已转换的 `Shift`+滚轮横向 delta、设备原生水平 delta 各移动一次，没有二次转换。滚动条复用 Ely `layout::Scrollbar`（覆盖式、可拖动），绑同一个句柄；未溢出时 Ely 自身按 `reach <= 0` 不绘制。滚动条带固定保留，避免出现/消失推移标签。

**3. reveal 行为。** `Shell` 保存一个 `tab_scroll` 句柄和一个待 reveal 的 `TabId`；新建、显式激活、`Ctrl+Tab`/`Shift+Ctrl+Tab`、以及标签条条目获得键盘焦点时登记 reveal，渲染时按 `index × (TAB_WIDTH + TAB_GAP)` 计算位置并做最小位移滚动，成功后清空标记，所以普通重绘不会反复抢回位置，手动滚动仍可浏览。关闭后 `clamp_tab_scroll` 把偏移收进新内容范围，避免末尾空白。测试覆盖“重绘不重置手动滚动”“失去焦点不回滚”。

**4. 中键关闭与蓝色边框。** 标签用 GPUI 的 `on_aux_click`（只接收非主键）在标签主体上处理中键，`event.is_middle_click()` 时关闭一次；嵌套的关闭按钮只有左键 `on_click`，所以中键按在关闭按钮上也只有标签这一层执行一次关闭，不会先激活（`on_click` 只服务左键/键盘激活），也不会重复关闭。左键、Enter/Space、`Ctrl+W` 不变。标签的 `focus(...)` 样式从 `colors.focus`（蓝色描边）改为中性文字色 `colors.fg`，`border_color` 在所有状态保持透明；非活动标签因此仍不会被看成活动态。`track_focus`、ARIA 角色与 `aria_selected` 保留。

**同步基线：** 合入前 `origin/main` 已前进到 `262a013`（PR #18 账户持久化、PR #19 仓库快照）。核对 `git merge-tree` 无冲突，且当前 head 的 CI 已与 `262a013` 组合测试，因此本轮**没有**为形式制造合并提交，也未触碰 `auth/data/storage/github/Cargo`。

**验证：** 本机 `cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked`、`cargo test --locked`（168 项）与 `git diff --check` 全部通过。新增/替换的用例见上文“验证”列表。

**仍需负责人真实窗口验收（自动测试不能替代）：** 上下左右间距在实际 DPI 下的观感、滚轮与 `Shift`+滚轮以及触控板横向手势、拖动滚动条、中键关闭、中键在关闭按钮上、标签颜色与焦点提示、900/1200 宽窗口下的标签可读性。

## 交接与后续

1. 负责人/主 Agent 在真实窗口验收多标签交互与中文候选确认；`human_review_required=true`、`human_review_approved=false`，PR 保持 open，等待验收后才由 coordinator 合并。
2. R3 接入真实分支/文件读取时填充 `RepositorySession` 并替换 fixture 默认值；账户切换如何影响标签仍待负责人决定。
