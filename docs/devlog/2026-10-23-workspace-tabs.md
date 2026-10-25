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

## 第四轮：负责人真实预览的六项修订（2026-10-25）

负责人查看真实预览后提出六点，均已实现；这同样不是验收通过，`human_review_approved` 仍为 `false`。

**1. 标签与 island 的内边距缩小。** `TAB_STRIP_INSET` 现在直接取 `ISLAND_INSET`（7），上/左/右同一个值，不再是独立写的 8。`TAB_STRIP_HEIGHT` 改为 `inset + TAB_HEIGHT + TAB_SCROLLBAR_BAND`（7+26+5=38），滚动条带随细滚动条收窄（`TAB_SCROLLBAR_THICKNESS + 2`），不再留出 12px 的空条。测试核对：`shell-body` 到 island 右/下的距离等于 `ISLAND_INSET`，标签条内边距也等于它，并且首标签相对 island 内边界（计入 1px 边框）的上/左间距相等。island 圆角/外边距、侧栏与原生标题栏未动。

**2. 滚动条更细、更轻、只在 hover 时淡入。** 新增 `src/ui/tab_scrollbar.rs`：仍用 Ely 的交互模型（覆盖式 thumb 绑同一个 `ScrollHandle`、拖动时按比例改偏移），但视觉是 3px 细圆角、`fg` 20% 透明度（hover 时 38%），不画实心轨道；不再用 Ely `Scrollbar` 的 12px 粗 thumb 和常显行为。`Fade` 是纯状态：进入/离开各 140ms，反向从"当前已绘制的不透明度"继续（不是跳回 0/1），拖动期间 pin 到 1，释放后回到 hover 状态；动画结束后 `run = None`，没有常驻 timer；完全隐藏时不构建元素，所以不存在"透明但仍能点击的轨道"。共享 Ely 主题与依赖未改。

**3. 图标与标题间距。** 标签内容间距改为 `TAB_ICON_GAP = 7`，但左内边距（8）与图标本身宽度保持不变，close 槽位（16）也不放大，所以标题可用宽度没有被挤掉。

**4. 标签拖拽排序。** `Workspace::reorder(id, target)` 按稳定 `TabId` 移动 `Session`，不关闭/重建会话、不改 id、不改活动标签。`Shell` 侧有 `TabDragState { id, target }`：`start_tab_drag` 记录起点并停掉未完成的 reveal，`move_tab_drag` 用 `tabs::landing_slot(viewport_x, offset, count)` 算插入槽（以滚动视口的实际偏移与宽度为基准，因此工具栏不可能是目标），靠近左右边缘（`EDGE_ZONE = 24`）时每帧固定移动 `EDGE_STEP = 12` 做有界自动滚动，`drop_tab_drag` 落地一次，`finish_tab_drag` 用于取消/Esc/失焦/无效投放/被拖标签关闭。渲染时被拖标签降到 45% 不透明度、目标槽画 2px 强调色细线。普通点击仍激活；close 按钮与中键关闭走各自路径，不经过 `on_drag`，所以不会误触发排序。`Ctrl+Tab` 与关闭邻居按新顺序（`Workspace::cycle`/`close` 本来就用集合顺序）。

**5. close 只在 hover 或 active 时出现。** 每个标签的 close 槽位始终保留宽度（`TAB_CLOSE_SIZE`），非活动且未 hover 时用 GPUI 的 `Visibility::Hidden`（不绘制、不参与命中测试，但仍占布局），并通过 `group_hover` 在 hover 时立即 `visible()`；没有渐变动画，也没有第三条件（键盘聚焦不显示）。因此隐藏时点不到、也不会留下不可见的 focus target，显示/隐藏不移动标题或相邻标签。

**6. 右侧固定工具栏。** 标签条改为"内边距盒子 = 滚动视口 + 工具栏"：toolbar 在视口之外（`TAB_TOOLBAR_WIDTH = TAB_HEIGHT - 2`），不参与横向滚动、不被挤出、不遮最后一个标签；视口因此变窄，reveal/overflow/拖拽坐标全部基于新的视口宽度。里面先放一个 Ely `IconButton`（Ghost、Sm、`RotateCw`、tooltip Refresh），处于 `disabled` 状态，不接任何刷新逻辑、请求或导航。

**验证：** 本机 `cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked`、`cargo test --locked`（232 项，含合并 main 后的服务层用例）与 `git diff --check` 全部通过。新增测试覆盖：inset 与 island 间距一致；工具栏固定且缩小视口、其占位按钮不改变状态；close 只在 hover/active 时响应且不移动布局；拖拽前后顺序、状态跟随、取消/被关闭/无效投放、边缘自动滚动不滚动页面、排序后的 cycle 与 close 邻居；滚动条只在 hover 时出现、可拖动、淡入淡出与反向连续、拖动 pin 与隐藏后不可点击。动画用固定时钟值驱动，没有随机 sleep。拖拽的落地槽位另有一条纯函数单元测试（`landing_slots_follow_the_tab_middles_and_the_current_offset`），投放位置指示线是覆盖层，因此显示它不会移动任何标签（`the_drop_marker_does_not_shift_any_tab`）。

**第四轮审查修复（head `f605e08`）。** Codex 该轮三条 P2，均直接修复：

1. **右向投放到中间缝隙落错位。** 插入槽是"当前集合中两个标签之间的位置"，`reorder` 先移除被拖标签再插入，因此落点大于原索引时要减一。此前只有首/尾测试掩盖了这个偏差，现在 `a_rightward_drop_lands_between_the_tabs_the_marker_names` 覆盖中间缝隙往返。纯函数 `landing_slot` 保持"插入槽"语义，索引换算集中在 `Shell::advance_tab_drag`。
2. **指针停在边缘不再继续滚动。** 保留拖动中的指针位置，由 `advance_tab_drag` 同时在 `move_tab_drag`（鼠标移动）与渲染循环里推进有界自动滚动；拖动结束后停止。新增 `edge_scrolling_continues_while_the_pointer_is_stationary`，不依赖鼠标重复移动。
3. **滚动条 thumb 拖动未接入淡入淡出状态。** 拖动 thumb 现在显式 `set_dragging(true)`，渲染循环在平台拖动结束（包括在 strip 外释放）时解除，因此拖到标签条外 140ms 后也不会把正在使用的 thumb 淡出掉。

随后同步 `origin/main`（`42291f7`，含 PR #18/#19/#21/#22），无冲突。该 head 的本地 fmt / check / clippy / build / test（232 项，含合并 main 后的服务层用例）与 `git diff --check` 通过。

**第五轮审查修复（head `ee6ce18`）。** 该轮两条 P2，都是我上一轮修复引入/遗留的问题：

1. **标签拖拽根本无法用鼠标启动。** 我把 `on_drag`/`on_drop` 注册条件写成了"已经在拖动中"（`reorder: tab_drag.is_some()`），而 `on_drag` 正是启动拖动的入口，形成死循环；只有测试用的 `drag_tab` 绕过它。现在 drag/drop 处理器在每次渲染都注册，`reorder: bool` 字段随之删除；新增 `dragging_with_the_mouse_starts_from_a_real_press_and_moves` 用真实鼠标按下/移动/抬起驱动排序，并有 `close_and_middle_click_do_not_start_a_drag` 确认 close 按钮与中键不会误触发。
2. **投放标记与最终位置不一致。** 上一轮我把插入槽就地转换成了"移除被拖项后的索引"，但标记是按仍含被拖项的顺序绘制的，右向拖动时线会画在前一个缝。现在 `TabDragState` 同时保存 `slot`（标记用的插入槽，未转换）与 `target`（交给 `reorder` 的索引），转换只在投递时使用；`a_rightward_drop_lands_between_the_tabs_the_marker_names` 同时断言两者（B/C 缝是 slot 3、目标索引 2）。

合并 `origin/main`（`42291f7`）后本地 fmt / check / clippy / build / test（234 项）与 `git diff --check` 通过。

**第六轮审查修复（head `dd09151`）。** 一条 P2：拖动标签时每个 drag-move 回调都把 strip 的 hover 强制设回 `true`，指针移出标签条后释放仍会因 `hovered = true` 让滚动条常显。现在 drag-move 只更新拖动位置与槽位，不再改 hover；可见性由拖动标志单独维持，释放后立即回到真实指针位置，因此移出后释放会正常淡出。新增 `the_scrollbar_fades_after_a_drag_that_ended_outside_the_strip`（拖动中移出保持 1.0，释放后从 1.0 连续降到 0）。同轮核对并 resolve 了上两轮已修好的五条历史线程。

该 head 本地 fmt / check / clippy / build / test（235 项）与 `git diff --check` 通过。

**第七轮审查修复（head `c1ebd5a`）。** 一条 P2：投放处理器原先只挂在每个标签上，而在标签之间的 4px 缝隙或末尾空白处释放时没有目标，标记显示的落点无法生效。现在 `on_drop` 挂在滚动视口本身（工具栏在其外部，因此仍不可能是目标），缝隙与末尾空白都能落地。新增 `dropping_in_a_gap_applies_the_marked_reorder`，用真实鼠标在 B 之后的缝隙释放并断言顺序改变。该 head 本地 fmt / check / clippy / build / test（236 项）与 `git diff --check` 通过。

**第八轮：审查循环收敛（head `0d491c1`）。** 上一提交 `e403006` 的 Codex 审查已为 clean（无未解决线程）；随后 `main` 又前进两次（PR #23/#25），按要求同步并重跑，`0d491c1` 上审查再次 clean、无线程、CI 全绿。四轮审查共 9 条 P2 全部修复（含我上一轮引入的两条回归），历史线程逐条核对代码后 resolve。

**仍需负责人真实窗口验收：** 缩小后的内边距观感、细滚动条的可见度与淡入淡出是否顺滑、图标与标题间距、拖拽手感与自动滚动速度、close 出现时机、右侧工具栏位置；性能与动画流畅度只能由实际窗口确认。

## 交接与后续

1. 负责人/主 Agent 在真实窗口验收多标签交互与中文候选确认；`human_review_required=true`、`human_review_approved=false`，PR 保持 open，等待验收后才由 coordinator 合并。
2. R3 接入真实分支/文件读取时填充 `RepositorySession` 并替换 fixture 默认值；账户切换如何影响标签仍待负责人决定。
