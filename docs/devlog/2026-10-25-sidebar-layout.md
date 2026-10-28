# 侧边栏布局、resize 与 collapse

- 日期：2026-10-25；任务：sidebar-layout；分支 feature/sidebar-layout；派发基线 262a013。
- **manual acceptance required / DO NOT MERGE。** 负责人要求完成实现和审查后保持 PR OPEN，等待真实窗口验收；禁止 auto-merge。人工验收未完成。没有已知产品争议，不把人工验收伪记成 arbitration。
- 多标签 PR #16 与本任务独立开发。本分支未合入、未复制其代码；本预览没有多标签不代表删除该功能。最终集成由主 Agent 协调。

## 实现

行为与尺寸的主要契约在 [侧栏布局](../sidebar-layout.md)，参数位于 `src/ui/geometry.rs`。默认 232px，展开 200–360px，收起 56px；窗口预算保留 island 600px，880px 窗口时上限 265px。本次运行记住最后展开宽；折叠不导航、不重置两条滚动句柄。只有仓库 viewport 滚动。

行背景左内缩与 footer 底内缩统一 8px，右侧 island gap 补齐 Profile 对称外留白。repo/footer 独立间隔 12px。账号头像由 24px 增至 32px，账号行 48px；repo 行左内缩由 8px 改 4px。Home、Settings、仓库、Profile 的活动线移至背景之外客户区左缘，仓库行与指示线一起裁剪。Ely NavItem 没有关闭内建 indicator 的接口，因此侧栏内组合等价主题/导航行为的行，没有改 Ely 依赖。

分界 8px 带使用 GPUI drag，圆形 24px 收起按钮单独命中，位于标题栏下 body 中央。处理 clamp、离开命中带、释放、Esc、失焦与窗口变化。收起菜单复用 Ely OverflowMenu，四项含图标、标签和活动态，New 继续禁用。测试发现 Ely 外点关闭仍会激活背后行，侧栏补透明 dismissal shield 阻止穿透；窗口变化让菜单焦点离开而关闭。

没有改原生标题栏、字体、workspace、palette 核心、后端或多标签实现。

## 验证

本机 Windows，Rust 1.98.1。`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（177 passed）、`cargo build --locked`、`git diff --check` 全部通过，最终结果在 PR/交接记录绑定提交。新增 `src/ui/sidebar_tests.rs` 通过 GPUI 测试平台实际绘制 Shell，覆盖：

- 客户区左/下 inset、Profile 至 island 的对称外留白、12px repo/footer 间隔、48px 账号行、活动线在行外并居中；
- 200/360px 越界 clamp、拖出命中带、释放后停止、Esc 取消，拖动不改变 Home 路由；
- 收起 56px、展开恢复 320px、路由及 repo 滚动保持；
- 880×560 预算限制为 265px、island 至少 600px、footer 在窗口内、收起后键盘聚焦第 124 项会 reveal；
- 真实 Ely 菜单四项、New 禁用、鼠标和键盘导航、Esc、外点击关闭且不导航、窗口 resize 关闭与存在焦点返回目标；
- 选中仓库行滚出 viewport 后 indicator 随行移动，footer 与页面滚动保持不动。

测试平台不经过 Windows 原生输入/IME，不能替代最终人工验收，也不通过颜色镜像断言宣称美观。

## 真实预览

独立 worktree debug：`D:\githux-sidebar-layout\target\debug\githux.exe`。在本 worktree 运行 `cargo run --locked` 使用既有 runner 自动准备 Windows App SDK；本轮已准备 app-local runtime。只启动/关闭本 worktree 的预览进程，没有修改全局环境。

本机 150% DPI 下检查了展开默认布局、32px 头像与两行文字、长 namespace/repo 截断、124 条 fixture、Profile 活动态、行外 indicator；实际点击收起/展开、菜单向上展开与 Esc，实际拖动到最大/最小宽并释放，Profile 导航正常。最后一版真实预览还确认外点击菜单关闭后没有导航穿透，随后展开及 Profile 导航可用；最小/矮窗口及全套菜单行为以测试平台为主要证据；原生 caption、多 DPI 跨屏、中文 IME 完整验收仍需负责人复测。

本轮截图诊断曾因 PowerShell 未启用 DPI awareness 导致 GetWindowRect 与 CopyFromScreen 坐标不一致，只截到左上部分，误以为 footer 越界。修正截图工具 DPI awareness 后完整窗口证明 footer 正常；相关临时根容器实验和诊断 I/O 已移除，没有以该误判修改原生 caption。

## 修订：统一收起尺度、动画、More 与贴边 indicator

负责人查看真实预览后提出四项修订，本轮已处理；这不是验收通过，最终观感仍待其真实窗口确认。

收起栏由一个共享 28px 方形控件尺度导出宽度：56px = 28px 控件 + 8px 左内缩（右侧由 island gap 补齐）。Home/Settings、repo、account、More 的命中背景都是同尺寸正方形并水平居中；account avatar 在收起时与 repo 的 Ely Xs 同为 20px，向上展开时连续插值到 32px Md，文字同时淡入。展开仍保留大 avatar + 姓名/ID，Profile 页面内的 Ely avatar 不受影响。共同尺度集中在 `SIDEBAR_RAIL_ITEM_SIZE / SIDEBAR_COLLAPSED_WIDTH`，避免账号内容反向撑宽。

展开/收起用约 200ms 的 Ely `ease_out_cubic` 按呈现时间插值宽度与展开比例；sidebar、island 左边、resize 命中带、圆按钮和行高共享同一帧值。动画由渲染采样驱动，只在过渡未完成时请求下一帧，结束即清空状态，不使用空转 timer 或保留回调。快速再次点击先采样当前呈现值再反向；手动拖动和窗口尺寸变化会终止动画并限制到合法宽度，展开仍恢复本次运行的原 `expanded_width`。遵循 Ely `Theme::reduced_motion` 与 `App::reduce_motion`，不增加新设置。标签点击与路由、两条滚动状态不受影响。

收起底部入口的 tooltip 与可访问名称均为精确的“More”；菜单内容仍为 Notifications/Issues/PRs/New，New 继续禁用。展开模式四个图标保留各自 tooltip。

活动 indicator 从可绘制左边界 x=0 开始，宽 2px，左上/左下圆角为 0，右上/右下 2px；上下各内缩 2px，因此高度随当前行（含动画中间高度）变化。仓库行和 indicator 仍一起被独立 viewport 裁剪，滚动出屏后不侵入 footer；indicator 无鼠标命中，不影响窗口 resize 或行点击。

## 验证（修订）

`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（182 passed）、`cargo build --locked`、`git diff --check` 通过。本轮新增覆盖：收起行/More 实际正方形与共同宽度、account 与 repo avatar 实际尺寸相等、展开恢复 32px、indicator 分角绘制与 1.5 倍缩放/滚出裁剪、动画中间帧与侧栏/island/divider/按钮同宽、连续反向、结束后不再请求帧、拖动/窗口 resize 中断和 reduced-motion。

真实 debug 预览确认展开态 layout 正常，收起后的 account avatar、行背景、贴边 indicator 与 More 行为可用；完全动画流畅度、四项视觉与手感仍由负责人确认，不把测试或构建结果写成已验收。

最终真实窗口视觉和全部手感由负责人验收。重点：外 inset 与 Profile 两侧留白是否舒适、repo/avatar 左移量、圆按钮位置/命中、边缘窗口 resize 不受活动线影响、菜单焦点与中文 palette 组合输入。PR 与 CI/Codex 最新提交结果只记在 PR/交接，避免为后补结果制造额外未经审查提交。

## 第二轮修订：footer 分割、身份区、toggle 对齐、indicator、平滑滚动、入口边框

负责人再次真实查看预览后提出六项反馈，本轮已处理；仍未验收通过，最终观感待其确认。

1. 仓库 viewport 与固定 footer 之间加入 1px、低对比度水平细线（`sidebar-footer-divider`），在 12px 总间距内居中，不随仓库滚动。账号外层不再是整行 button，也不整行 hover/active 点亮；只有 `account-identity`（头像 + 名字 + @ID）打开 Profile，空白右侧不响应。展开时右侧增加独立 emoji 状态按钮（`account-status`，占位 💻 / `Coding`，tooltip `Coding`），点击不导航且停止冒泡；无编辑弹窗/网络/写状态。收起时状态按钮隐藏，状态并入头像 tooltip；账号活动 indicator 仍贴左缘。
2. 收起/展开圆按钮的水平中心改到 island 左边框线（`width + ISLAND_GAP + ISLAND_BORDER_WIDTH/2`），垂直中心对齐 island 自身高度中点（扣底部 inset），两轴都由与 island 相同的常量推导。
3. 仓库标题精确改为 `Repositories (N)`，N 来自当前列表长度。
4. 活动 indicator 缩短并加宽：宽 3px，左直角、右 4px 圆角，高度取行高约 2/3 并 clamp 到 20–24px、垂直居中，随行高（含动画中间值）变化。
5. 新增 `src/ui/scrolling.rs`：离散滚轮累加目标偏移并按真实时间 `ease_out_cubic` 缓动（约 130ms），触控板像素滚动直接应用，clamp 到 `[-max, 0]`，无动画不请求帧，遵循 reduced-motion；捕获阶段只消费一次并抑制 GPUI 内建跳变；键盘 reveal/路由重置/光标移动取消缓动。已接入侧栏仓库、island 页面、palette 结果列表；workspace-tabs 横向滚动未在本分支，未宣称覆盖。
6. 展开态四个底部入口各加一条 Ely 中性低对比度细边框，保持四等分与圆角；收起 More 使用一致轻描边。

### 验证（第二轮）

`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（187 passed）、`cargo build --locked`、`git diff --check` 通过。新增覆盖：footer 分割线位置且不随仓库滚动、身份区导航而空白/状态不导航、状态按钮展开存在/收起隐藏、indicator 宽度/高度/贴左、平滑滚动纯状态（缓动/反向/边界/精确/内容变短/停止）、overlay 覆盖 viewport、单次滚轮不被内建跳变加倍；并更新了因新几何/行为而过时的既有断言。

真实 Windows debug 预览（150% DPI，本 worktree）确认展开态：`Repositories (124)`、footer 分割线、身份区 + 💻 状态按钮、四个带边框底部入口、贴左且左直右圆的 indicator，以及 toggle 落在 island 边框上。**收起态的合成点击未能送达**：该窗口在本会话不是前台且无法抢占前台，`PostMessage`/真实鼠标事件均未被处理，所以收起/展开的逐帧观感仍只由 GPUI 测试平台覆盖，不写成已真实验收；平滑滚动的实际手感（触控板/连续滚轮）也需负责人真机确认。

### 审查修复：滚轮捕获尊重遮挡

Codex 首轮审查（针对 `4b92c0b`，P2）指出：palette 打开并覆盖在可滚动的 Home 页面之上时，页面 overlay 只按矩形范围判断，会在捕获阶段抢先消费属于 palette 的滚轮事件，导致滚动穿透到背景。已修复：`wheel_overlay` 在 prepaint 插入一个普通 hitbox，并在监听器里用 `hitbox.should_handle_scroll(window)`（GPUI 内建的同款检查）代替矩形判断。palette 的 `Backdrop` 与其子层已有 `occlude`，被遮挡的背景区域因此不再消费滚轮。新增回归 `wheel_over_the_palette_scrolls_the_palette_not_the_page`：短窗口下同时让 Home 页面与 palette 列表可滚动，验证滚轮只滚 palette、页面偏移不变。该检查同样覆盖 Ely 菜单等前置浮层，不针对单个 palette 特判。

### 审查修复：滚动范围收缩时同步 clamp 存储目标

Codex 第二轮审查（针对 `b3c9a3d`，P2）指出：滚动范围在缓动中收缩时，`tick` 只 clamp 呈现值而未 clamp 存储目标，后续反向滚轮仍会累加到不可达目标上，看起来不响应。已修复：每 tick 先把存储 `target`/`from` 一并 clamp 到当前 `[-max, 0]` 再计算，含 reduced-motion 分支；新增 `shrinking_range_clamps_target_so_reverse_scroll_still_moves`（覆盖中途收缩后离散反向与精确反向）和 `reduced_motion_range_shrink_is_clamped_immediately`。全量 190 tests 与 clippy 通过。

## 第三轮修订：分割线居中、身份 hover、indicator 变细、sash 反馈（2026-10-26）

负责人第三次真实查看预览后提出五项反馈；本轮已处理，仍未验收。

1. 分割线改为自带上下对称留白（`FOOTER_DIVIDER_SPACE`）；此前列 gap 只加在下方造成上 8px/下 12px，现在渲染实测上下各 8px（1px 线），两态一致，仍固定在 footer 内、不随仓库滚动。旧 `REPO_ACCOUNT_GAP` 常量随职责转移删除，间距由 `FOOTER_DIVIDER_SPACE/HEIGHT` 表达。
2. 收起态 More 触发按钮去掉可见边框（保留焦点环、tooltip "More"、菜单与键盘）。展开态四入口浅边框保留。
3. 身份区去掉整块 hover 背景（此前 `account-identity` 有 `.hover(bg)`），改为 pointer 光标 + `@ID` 文本下划线；账号行左侧内缩减少，身份簇与上方仓库 avatar 轴对齐，头像与两行文字作为整体在行内垂直居中。空白区域仍不可点，状态 emoji 仍是独立操作。
4. indicator 宽度 3px → 2px（`SIDEBAR_INDICATOR_WIDTH`），保持贴可绘制左缘、左直角/右 4px 圆角、约 2/3 行高并垂直居中。
5. resize 热区新增可见 sash：2px 前景色竖线（`SIDEBAR_SASH_WIDTH`），仅在 hover 或拖动时以 140ms ease-out 淡入、离开淡出，从中断时的呈现值可逆续接；纵向从 island 顶部圆角附近到底部圆角/inset 之内，横向居中于 8px 命中带，不铺满侧栏，也不遮挡中部 toggle（toggle 占据带中点，其自身命中优先）。遵循 reduced-motion，动画结束不再请求帧。

合并 `origin/main`（含已验收的 PR16 会话标签）时保留双方：标签条、页面会话滚动/焦点、Refresh 等上/右间距不变；`scrolling.rs` 的页面平滑滚动改为每会话状态（`PageState::smooth`），只覆盖标签条以下的页面区域，不再抢标签条滚轮；侧栏收起菜单改为侧栏自有的浮层状态（原因：Ely `OverflowMenu` 的私有 focus takeover 在收起动画重绘中会被重新武装，导致 Esc/窗口变化无法关闭），视觉仍用 Ely 主题 token（overlay/border/elevation/menu width）与同样的四项/禁用 New。

验证：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（373 passed）、`cargo build --locked`、`git diff --check` 通过。新增/更新测试：分割线两态上下等距、身份登录行存在、indicator 宽度与几何、sash 淡入淡出可逆与 reduced-motion、sash 线宽与 island 纵向范围、外点关闭不触发下层导航（按真实 press/release 语义覆盖）、标签条滚轮与页面/侧栏互不影响。真实视觉、sash 实际手感与动画观感仍待负责人验收。

### 审查修复：菜单焦点约束与其他浮层协调

Codex 针对 `6827ce4`（P2）指出：More 菜单打开时 Tab 可把焦点移出面板，菜单与全窗遮罩仍可见但 Up/Down/Enter 失去作用；Ctrl+K 也会在菜单之上再叠开 palette。已修复：菜单打开期间在 shell 按键拦截器消费 Tab/Shift-Tab，面板与触发按钮是唯一可达焦点；`open_palette` 先关闭 More 菜单再打开 palette；收起/展开同样关闭菜单。新增回归 `menu_contains_tab_and_closes_for_other_overlays_or_a_mode_change`。全量 374 tests 与 clippy 通过。

### 审查修复：关闭菜单后焦点回到可操作按钮

Codex 针对 `9adf9b0`（P2）指出：Esc 关闭 More 后焦点落在布局 wrapper，而 tab stop 与点击在子按钮上，Enter/Space 无法重开、也不显示按钮焦点样式。已将 `sidebar_more_focus` 挂到实际按钮（`sidebar-more-button`）上，关闭时回到可操作的 More；新增 `escape_returns_focus_to_the_more_button_and_enter_reopen` 覆盖 Esc 后立刻 Enter 重开。全量 375 tests 与 clippy 通过。

### 审查修复：键盘激活后保留 toggle 焦点

Codex 针对 `bb28fb6`（P2）指出：键盘 Tab 到收起/展开按钮并按 Enter/Space 后，代码无条件把焦点转给 shell root，再按 Enter/Space 无法反向切换。已修复：toggle 使用自己的 `sidebar_toggle_focus` 并 `track_focus`，激活后焦点留在同一个可操作按钮上。新增 `keyboard_toggle_keeps_focus_and_reverses_immediately`（Enter 折叠→焦点检查→Enter 展开→Space 再折叠）。全量 376 tests、clippy、build 通过。

### 审查修复：拖动结束后按真实 hover 重置 sash 目标

Codex 针对 `c2fa641`（P3）指出：拖出 resize 带后松开，`dragging` 被清掉但 sash 目标仍为 1，导致高亮常驻。已修复：`SidebarState::end_resize` 统一结束/取消拖动，并根据指针是否仍在热区（新增的 `geometry::sidebar_resize_band`，与绘制共用几何）重新计算目标；释放、out-of-band 释放、Esc 与失焦都走该路径。新增 `releasing_the_drag_outside_the_band_clears_the_sash_target` 验证带外释放后淡出归零。全量 377 tests 与 clippy 通过。

### 第四轮（新会话）：五项逐条复核与修正

负责人以新会话重派同一五项要求，并明确「分割线是左右、不是上下」。以 rollback `628ae27` 为基线重新集成已验收 main 的 tabs（`84b029e`），再逐条按实测几何复核：

1. **分割线**：此前只把上下留白做对了，水平方向仍在列宽内用 `px(8)` 双侧内缩。但侧栏的可见范围是「列宽 + island gap」，行背景是 8px 左内缩 + 右侧留 0，所以线段相对可见范围左偏约 4px。改为与行背景同一 lane（`pl(SIDEBAR_PADDING)`、`pr(0)`），两态都用 `visible_left = sidebar.left()`、`visible_right = island.left()` 的中点断言，误差 ≤0.5px；divider band 高度、账号行与 More 纵向位置未动。
2. **收起 More**：`6827ce4` 去掉可见边框时连 `border_1` 一起删了，`.focus(border_color)` 因此没有可绘制的边框，键盘 focus 环消失。改为保留 `border_1` + `transparent_black()`：可见边框仍无，盒子尺寸/中心/命中/28px 方形与 focus 反馈都回到基线语义。
3. **账户身份**：左移、无整块 hover 背景、pointer、仅 `@ID` 下划线、收起头像与 repo 同为 20px 已具备。补充按真实布局测量的断言：身份块左缘距行左 ≤4px，头像与两行文字组成的块上下留白差 ≤1px（不靠 `items_center` 声明推断）。
4. **indicator**：`SIDEBAR_INDICATOR_WIDTH` 已是 2px；贴可绘制左缘、左直角/右圆角、约 2/3 行高、长度与垂直位置均未改，原测试仍断言宽度、高度与 left。
5. **resizer sash**（本轮后续两轮负责人反馈已取代这里的做法，见下方「第五、六轮」）：先把 `sash_hovered()` 从「等于 target」改为独立记录指针是否在热区，并把两端从手写常量改为共享几何函数。当时仍用 `on_mouse_move` / `on_mouse_exit` 驱动，纵向仍是「圆角到圆角」的直线段——这两点后来都被负责人真实窗口复现证伪，已在本轮末尾重写。

验证：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（378 passed）、`git diff --check` 通过。测试平台不经过 Windows 原生输入分发，五项最终视觉、sash 真实手感、折叠动画观感与多 DPI 仍待负责人真实窗口验收。

## 第五轮（原会话）：collapsed More 图标、sash 离开残留、sash 与 island 等高

负责人真实窗口发现三项：收起态 More 的三个点在按钮内部偏左上；sash 在鼠标移开后不消失（展开与收起都复现）；sash 高度要与主页面 island 完整外框一致，覆盖此前「圆角到圆角」的范围描述。

三项都先在旧实现上写出会失败的真实事件/布局测试，再改实现：

1. **More 图标**：`sidebar-more-button` 是固定 28px 的 `div`，没有为子元素建立布局；外层 trigger 的 `justify_center` 只把 28px 盒子居中，`Icon` 内部的 `svg()` 是 `flex_none`，于是落在槽位左上角（实测图标中心比槽位中心偏 6px）。给按钮自身加 `flex / items_center / justify_center`，图标落到同一盒子的正中。外框尺寸、位置、占位、命中、无可见边框与 focus 环都未变，也没有移动 footer。
2. **sash 离开残留**：根因是 `on_mouse_exit` 绑定的是 `MouseExitEvent`，只在指针离开窗口时触发，不是元素 hover 结束；指针从热区移到侧栏或 island（仍在窗口内）永远不会通知。改用该版本真正提供 true/false 生命周期的 `on_hover`。它带来两个必须处理的边界：`on_hover` 在 active drag 期间被抑制，所以拖动时改由拖拽事件按指针位置自行判定；判定基准必须与指针事件同坐标空间，因此不再用 `body` 局部原点手算 band（会被标题栏/Windows content_top 偏移），而是在热区内用 `canvas` prepaint 记录真实的 window 坐标 bounds。同时删除 `geometry::sidebar_resize_band` 这个易误用的局部坐标辅助函数。
3. **sash 与 island 等高**：原实现用 `(ISLAND_RADIUS, ISLAND_INSET + ISLAND_RADIUS)` 作为直线段内缩，父命中带又自带一层底部内缩，等于把底端扣了两次，实测底端比 island 少 8px；顶端也多缩了一个圆角。改为竖线直接铺满命中带的纵向范围，而命中带的顶部就是 island 顶边、底部内缩改为与 island 相同的 `ISLAND_INSET`（此前误用 `SIDEBAR_PADDING`，相差 1px）。测试用 `island` 与 `sidebar-sash` 的实际 bounds 断言两端各在 0.5px 内对齐，覆盖展开/收起与 560/900 两种窗口高度。`sidebar_sash_insets` 随之删除。

**行为约定澄清**：sash 现在只由真实 hover 决定。此前 `retarget_sash` 用 `pointer_inside || dragging`，会让按住拖动期间持续点亮；这与负责人「只有 hover 时出现、不要因为 dragging 长期点亮」冲突，已改为只看指针是否在热区内。拖动仍可用，拖出热区淡出、拖回再淡入，释放到热区外保持熄灭。

验证：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（382 passed）、`cargo build --locked`、`git diff --check` 通过。

**未验证边界**：GPUI 测试平台不经过 Windows 原生输入分发，也不驱动真实非客户区命中测试。三项的最终视觉（More 图标与 rail 视觉中线、sash 两端与 island 的实际观感、快速进出与拖动手感）仍需负责人在真实窗口确认；多 DPI 跨屏、最大/还原过程与矮窗口的真实观感同样留待人工验收。

## 第七轮（原会话）：同步 main 后的 Ely / GPUI 0.3.10 迁移

负责人授权同步最新 main（本地合并提交 `99bc2a8`，main `7ed70cd` 为其祖先）后，由本任务完成剩余迁移。主线共享 Shell（`mod.rs`、`chrome`、`palette`、`tabs`、`tab_scrollbar`、`app`）已由 PR #30 迁移完成，本分支只补侧栏与滚动相关的遗留：

1. **crate 入口**：侧栏新增模块里的 `ely_gpui_component::*` 全部改为 `ely_core::*`，包括 `theme::ControlSize` / `theme::Elevation`、`motion::ease_out_cubic`、`init_for_tests` 与 `theme::Theme::update`。依赖仍为官方 Git `ely-core 0.2.0`，固定 revision `30c66fe1d5d23db769a6c834b771d054d7f288cf`，`default-features = false` 保留，没有 vendor、patch 或本地缓存修改。
2. **事件类型**：`scrolling.rs` 的 `ScrollWheelEvent` 改为 `ScrollEvent`；`sidebar_tests.rs` 的两处滚轮模拟补上显式 `source: ScrollSource::Pointer`，不把普通鼠标与触屏混淆。
3. **触屏直通**：新增 `ScrollSource::Touch` 语义。`precise` 现在为 `event.delta.precise() || source == Touch`，让触屏平移（含 GPUI 在抬手后合成的动量）直接落到 offset，不叠加第二层惯性。新增 `touch_pan_applies_directly_without_a_second_inertia` 覆盖。
4. **按键别名**：`sidebar_tests.rs` 的 `press` helper 跟随 `Keystroke::parse` 的 `&'static str` 签名；菜单测试里旧的 `secondary-k` 别名在新解析器中会返回 `InvalidKeystrokeError`，改用可解析的 `ctrl-k`（主线的 `KeyBinding::load` 仍负责按 OS 解析 Ctrl/Cmd）。
5. **保留项**：最新三项反馈（collapsed More 图标在按钮槽内居中、sash 只在真实 hover/合法 drag 时出现且移开消失、sash 与 island 外框等高）与其测试未改动；此前的分割线左右居中、身份区 hover、indicator 2px 等也保持。开关 toggle 的样式、大小、位置与独立命中未改。

**未合并依赖**：shared-scroll / PR #29 未接入，本分支继续用局部 `SmoothScroll` 与 `tab_scrollbar.rs` 的细滚动条，未从其未合并实现复制代码，也没有新增另一套公共 scroll API。PR #29 就绪后的收敛点是把 `wheel_overlay` 与各区域 `SmoothScroll` 接到公共接口。

验证：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（**383 passed**）、`cargo build --locked`、`git diff --check` 通过，均使用 `CARGO_BUILD_JOBS=1` 与本 worktree 的 `target/`。

**未验证边界**：测试平台不经过 Windows 原生输入分发，触屏 `ScrollSource::Touch` 只有事件级回归，没有真实触屏设备证据；真实窗口的滚轮/触控板/触屏手感、IME、多 DPI 与原生窗口按钮仍待负责人验收。迁移不等于原侧栏视觉或交互已验收。

## 第八轮：同步统一滚动条与 PR 门槛，接入仓库 viewport

起点为协调者已完成的 main 合并 `f0fbcb0`；核对 main `0014d6c` 为祖先、工作树干净。main 已包含 PR #29 的公共 `src/ui/scroll/**` 与 PR #31 的标签/规模门槛。本轮不沿用第七轮“PR29 未接入”的历史状态，标签条采用 main 的 `Bars`，不恢复已删除的 `tab_scrollbar.rs`。

仓库列表新增独立 `repo_bars`，通过纵向 `BarId`、`Style::DEFAULT`、`Placement::overlay(4, 8)` 绘制公共条。hover 只绑定真实仓库 viewport，账号/footer/More 不参与。公共 `Bar` 保留唯一几何/拖动/Fade，侧栏仅接线，原行高与布局不变；resizer sash 仍独立。

内容平滑与条显隐分别维护。`SmoothScroll` 适配 `ScrollTarget`，读写原 `repo_scroll`；thumb 的 start/write/end 钩子取消旧 wheel 目标，拖动期间禁止建立竞争目标，松手后不弹回。捕获阶段 wheel overlay 仍只消费一次，条最后绘制以保留 thumb 命中；触控板 precise Pointer 和 Touch 直通、约 130ms 离散 wheel、页面与 palette 平滑都保留。外部释放只 settle 本区域的已知 thumb drag；Esc、窗口失活与尺寸变化清理仓库条，main 的 tabs 失活订阅与 sidebar resize/menu 订阅并存。失活时也清除非拖动中的 sash hover。

新增真实 GPUI 事件/布局测试：拖动公共 thumb 改变仓库内容并取消待完成 wheel；释放后不弹回；footer、tabs、page 偏移不变；3px 可见线；快速进出可逆 fade、收起/矮窗口范围；Esc、窗口尺寸变化、窗口失活清理仓库拖动且不误记为 tab/resizer drag。既有“折叠动画终点停帧”测试现在先让新增的独立 hover fade 结束，继续断言零常驻帧请求；Touch 直通测试关闭 reduced-motion，防止 reduced-motion 掩盖误加惯性。

本机 fmt、locked all-target check、clippy `-D warnings`、400 项测试、preview build 与 diff check 通过，重型 Cargo 使用 `CARGO_BUILD_JOBS=1`。最终 CI/review 证据随 PR 交付记录绑定当前 head。一次全量运行中既有网络 mock 的 malformed framing 用例返回传输错误，单独复核通过；没有改网络代码或降低它的断言。真实滚动条外观、动画流畅度、原生输入/DPI 与此前全部侧栏视觉仍需负责人验收，测试平台不代替 Windows GUI。PR 保留 pending / OPEN / coordinator，不合并或启用 auto-merge。

## 第九轮：平滑滚动归入公共 scroll，并实际接入横向 tabs

负责人明确扩展范围：平滑不能继续另放 `ui::scrolling` 或只服务 repo，必须成为统一 scroll 的公共能力。以 `30e9d50` 为起点，PR 上负责人管理的 blocked 保留；旧 CI/Check/review 为历史证据，不作为本轮放行。

实现位于 `src/ui/scroll/motion.rs`，由 scroll 入口导出 `Motion<T>`、`wheel_overlay`、`cancel_region`。旧 `src/ui/scrolling.rs` 与旧 mod/调用路径删除，无兼容壳。Motion 基于真实 `ScrollTarget + Clone`，每区域独立状态、横纵轴各自目标/呈现/时钟；约 130ms 离散 wheel 累加与反向缓动，precise Pointer 与 Touch（含系统动量）直通，reduced-motion 即时。保留 `Bars` / `Bar` 的 140ms fade、样式和几何，不复制公共条，也不把 resizer sash 纳入内容 motion。

所有现有消费者已迁移：仓库 viewport、每个 workspace Session 的页面、palette 列表，以及横向 workspace tabs。tabs 不再依赖 GPUI 即时 wheel 跳变：公共 overlay 选横轴，原生 x（含 Windows Shift+wheel）优先，否则以 y 为 fallback；每事件捕获消费一次，另一轴 offset 保留。Bar 与 wheel 传同一个 motion clone；thumb start/write/end 取消本轴目标并转调原生 target 生命周期，拖动期间不累加竞争目标，松手不弹回。

程序化 reveal、切换/关闭标签、键盘导航、palette 查询重置、排序边缘自动滚动即时接管旧目标；会话切换冻结离开与进入页面的当前呈现位置，失活、窗口变化、palette 关闭和 session 释放不留旧动画。无后台 timer；owner 仅在 `tick` 表示未完成时请求后续帧。公共接口不依赖 Shell/sidebar/固定名字，也不固定为纵向 ScrollHandle。

测试先后发现并修复了两个真实边界：新标签 reveal 发生在下一帧范围布局之前，过早按旧 max-offset clamp 会让末尾新标签被裁掉，故程序写入取消目标后交原生新布局 clamp；原生 ListState 初始 reach 只有已测部分，测试应验证真实 clamp，不能通过 measure_all 伪造全量范围。ListState 纵向保留原生 drag hook，不支持的横轴安全 no-op。

公共测试使用可控 Instant 与真实窗口布局，覆盖 ScrollHandle 横纵轴/多区域、累积反向、range shrink、reduced-motion、thumb 接管与即时程序跳转；真实 UniformList base_handle 与 ListState、原生拖动开始/结束；嵌套区域单次消费且子控件点击可达。tabs 真实 ScrollEvent 回归验证离散中间帧/最终九个 line delta、横轴优先、Pointer pixels/Touch 直通、reveal/切换/失活取消；旧立即 wheel 的静止断言改为等待公共 motion 完成后继续验证原来的交互。

account-ui / content-reader 的独立 worktree 未修改，文档给出真实 ScrollHandle / UniformList / ListState 最小接线与输入所有权要求；它们尚待协调者同步，不宣称未合并 UI 已迁移。正式应用无新增调试入口、examples、vendor、fork、依赖升级或 Cargo 缓存补丁。

本机 fmt、locked all-target check、clippy `-D warnings`、404 tests、preview build 与 diff 检查的最终结果在 PR/任务状态绑定本轮 head；GPUI 事件测试不替代真实 Windows 滚轮/触控板/触屏手感、IME、多 DPI 及原侧栏视觉验收。PR 保持 OPEN / coordinator，人工验收未通过，不合并或启用 auto-merge。

### 审查修复：tabs 的 wheel overlay 不能随滚动内容移动

Codex 针对公共 motion 首版（`d629d0e`，P2）指出：tabs 的公共 overlay 被放在 `view.scroll` 所属的横向滚动行内，滚动后 overlay 随内容左移，右侧可见区域失去命中，滚轮退回 GPUI 立即滚动，导致平滑依赖指针位置且可能打断在途动画。已把 overlay 移入不滚动的 `tab-wheel-viewport` 包裹层，行内容仍在内层滚动；新增 `tab_wheel_stays_smooth_after_the_strip_scrolls_a_full_viewport`，先滚满一个视口再在可见右缘发送 wheel，确认仍由公共 motion 接管并可反向移动。全量 **404 tests**、clippy、check 通过。

## 第十轮：sash 宽度/拖动可见性、toggle 去边框、阈值折叠与展开

负责人 2026-10-27 的真实窗口反馈四项。基线 `1c127f9`，保留公共 `scroll::Motion` 与全部既有侧栏约束。

1. **sash 向右加宽**：原实现用 `(hit_width - sash_width) / 2` 的居中式定位，直接加宽会让左缘左移。现在把左侧锚点与绘制宽度分开：`SIDEBAR_SASH_LEFT` 固定为旧 2px 线的中心位置，`SIDEBAR_SASH_WIDTH` 扩展到命中带末端；右缘在布局上等于 island 外框左缘，上下端仍与 island 等高。collapsed/expanded、动画与 resize 各帧都由同一组几何常量推导；toggle 仍在上层且命中独立。测试按实际 bounds 断言左缘不动、右缘贴 island、上下等高。
2. **拖动全程可见**：`retarget_sash` 现在由“有效 resize 手势进行中或真实 hover”决定，不再只看 hover。拖拽事件仍记录真实指针是否在带内，供释放瞬间交还 fade 使用。因此快速拖动时指针多次跑出移动中的旧带，线也不闪；释放在外侧渐隐、在内侧保持、移开后消失；Esc、失焦、窗口变化仍走同一 `end_resize` 清理路径，reduced-motion 保持即时。
3. **toggle 去边框**：移除 `.border_1` + `.focus(border_color)`；尺寸、位置、图标、圆形背景与点击范围不变，键盘焦点保留在同一 focus handle 上，仅不再绘制边框环。覆盖旧的“不要改 toggle 样式”，本轮授权范围仅去边框。
4. **阈值折叠/展开**：拖动基线改为“按下点 + 本次手势宽度”，模式切换时重设位移基准。展开态连续缩到 200px 停靠，再越过 `SIDEBAR_DRAG_COLLAPSE_THRESHOLD = 32` 才折叠；收起态 rail 保持到同一阈值后直接展开到记住的展开宽度（窗口预算 clamp）。阈值 32 是本轮可调实现起点，供负责人验收，不是永久产品硬规定。修复了三个真实缺陷：折叠动画中途开始拖动时会误走 rail 分支、连续拖动可能暴露窄于 200px 的宽度、切换后位移未重设导致来回抖动。

验证：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（**407 passed**）、`cargo build --locked`、`git diff --check` 通过，均 `CARGO_BUILD_JOBS=1`、本 worktree target。旧测试中“拖出命中带即灭线”“收起态禁止 resize”等与新契约冲突的断言已按本轮明确要求更新，其余侧栏与公共 scroll 回归保留。

未验证边界：GPUI 测试平台不经过 Windows 原生输入分发。真实拖动跨越阈值的手感、sash 新宽度观感、toggle 无边框后的焦点可辨度、最大化/还原与多 DPI 下的表现仍需负责人真实窗口验收；本轮不据此宣称验收通过。

## 第十一轮：切换动画、160px 临界宽度、展开后 sash 跟随指针

负责人 2026-10-27 三项反馈。基线 `942a315`，保留 sash 右贴 island、拖动全程高亮、toggle 无边框与公共 `scroll::Motion`。

1. **模式切换走动画**：`resize` 不再在阈值处 `settle()`。折叠/展开触发时用既有 `Transition`（约 200ms ease-out、共享时钟与 reduced-motion）从当前呈现帧过渡到目标；拖动继续收到的 move 通过 `retarget_transition` 从当前帧重新衔接，静止指针也能完成动画。`sample()` 只在非拖动状态下因外部目标变化丢弃 transition，避免下一个鼠标事件抹掉动画。普通跟手缩放仍是即时的，只有模式切换需要过渡。
2. **更窄临界宽度**：`SIDEBAR_MIN_WIDTH` 从 200 调到 **160** logical px。折叠仍需要越过额外 32px 超程；从 rail 展开另用 12px 的 `SIDEBAR_DRAG_EXPAND_THRESHOLD`，避免“只动一点就跳到远处记忆宽度”。两者都保留为可调常量。
3. **展开后按指针对齐**：删除“恢复 `expanded_width` 再重建相对基准”的做法。宽度统一由 `pointer - grab_offset` 的绝对位置决定，`grab_offset` 只是按下点在命中带内的真实偏移（clamp 到 8px），模式切换不再重设位移基准。因此同一指针位置不因旧记忆宽度或事件序列不同而得到不同宽度；展开过渡完成后 sash 落在指针处，之后左右移动直接跟手。

修复的真实缺陷：rail 展开在 reduced-motion 下未即时落到目标；展开目标被 clamp 到最小值导致 sash 停在 160 不跟手。新增测试覆盖切换中间帧/完成帧、reduced-motion、静止指针收敛、持续 move 不抹动画、不同记忆宽度同一指针结果相同、命中带内不同抓取点只差该偏移、同一手势重复切换不抖动，以及真实 GPUI 事件下从 rail 展开后 sash 右缘跟随指针。

验证：fmt、locked all-target check、clippy `-D warnings`、**411 tests**、build、diff check 通过（`CARGO_BUILD_JOBS=1`、本 worktree target）。阈值 160/32/12 是验收起点，不是永久产品硬规定；真实拖动观感与动画流畅度待负责人确认。

## 第十二轮：临界区 footer 稳定性

负责人反馈 resize 到 collapse/expand 临界状态时底部四按钮与 More 反复闪。主 Agent 只读定位到两个叠加原因：Footer 直接按 `collapsed` 选择入口；`resize_pointer_at` 的 rail 分支按 12px 位移展开，而展开分支按“低于最小宽度 32px”折叠，二者可在同一指针位置同时成立。

修复：
- 删除 12px 的独立展开判定，改为在**同一指针派生宽度**上定义互斥滞回：`requested >= 160` 才从 rail 展开，`requested <= 128` 才折叠，128–160 为停靠带并保持当前模式。重复坐标与小幅抖动幂等。
- 新增派生 `footer_compact()`：收起、停靠/临界带、切换动画期间统一 More；只有明确宽于 `160 + 24` 才切回四按钮。拖动中该决定按明确边界锁存，瞬时宽度不会翻转呈现。
- Footer 渲染从 `collapsed` 改为 `footer_compact`，expanded-but-compact 时 More 菜单正常打开、键盘导航、关闭并返回焦点；同一帧只渲染一套入口。

新增测试：rail 右拖逐像素（每点两次）到明显展开，模式不回落、footer 只切换一次；expanded 左拖对称；过渡中间帧、完成后、临界处释放都保持 More；真实 GPUI 事件序列中每步只有一套入口，且 expanded-but-compact 的 More 可操作。旧测试中与该滞回冲突的“折叠过渡中继续向左仍保持中间帧”断言已按新语义更新。

验证：fmt、locked all-target check、clippy `-D warnings`、**415 tests**、build、diff check 通过（`CARGO_BUILD_JOBS=1`）。真实拖动与临界停靠观感仍待负责人验收。

### 审查修复：离开 compact footer 时丢弃隐藏的 More 菜单

Codex 针对 `638b702`（P2）指出：展开过渡期间 More 仍可用，若在过渡结束前打开，`footer_compact()` 随后变假会直接卸载 trigger 和面板，却没有清 `sidebar_menu` 或归还焦点；shell 的 Tab 拦截器仍认为菜单打开，键盘导航被卡住。已在 footer 从 compact 切到四按钮的同一渲染路径中关闭菜单并把焦点移到已挂载的根焦点；新增 `leaving_compact_presentation_drops_an_open_more_menu`，验证隐藏面板撤销、菜单状态清理且 Tab 恢复移动焦点。全量 **416 tests**、clippy、fmt 通过。


## 第十三轮：持续 resize 不再截断模式切换动画

负责人确认其余侧栏行为基本符合要求，但通过 sash 触发折叠/展开时动画再次消失，本轮由主 Agent 直接修复。根因是模式切换事件创建了 transition，随后的普通 move 在 rail、展开或停靠分支又清掉 transition 并直接写入目标 frame。前轮防抖修复没有覆盖“触发后连续输入”的动画行为；只发送一次阈值事件或开启 reduced-motion 的测试无法发现它。

同一手势的每个 move 先采样当前动画：模式没变时保留 transition，目标改变则从当前呈现帧衔接、仅使用原动画的剩余时间。因此连续 move 不会让侧栏瞬移，也不会无限延后完成时间；模式反向切换才重新开始约200ms过渡。普通 resize 仍立即跟手；重新按下 sash 的新手势仍接管先前按钮动画。切换动画期间 More 优先于 footer 的宽度锁存结果，结束后按原临界区规则恢复四按钮，阈值/几何/命中和公共滚动均未改动。

新增三条受控时间回归，旧代码上全部失败，分别覆盖 collapse 后连续 move、expand 后持续改变目标（仍在原200ms期限内完成）、反向后继续 move 并释放。新增启用动画的 GPUI 真实事件与布局测试，检查连续鼠标事件前后没有跳帧、中间 expansion、sidebar/island/sash 同步、只有 More、完成后指针与 sash 命中带对齐。原有重新抓取中断按钮动画的测试保留，全量420 tests通过。`cargo fmt --all -- --check`、`cargo check --locked --all-targets -j 1`、`cargo clippy --locked --all-targets -j 1 -- -D warnings`、`cargo build --locked -j 1` 和 `git diff --check` 均通过。本机桌面自动验证通道连接失败，备用通道选择应用时授权等待超时，未完成 Windows 原生拖动视觉复测；上述 GPUI 事件/布局证据不替代负责人真机验收。旧预览占用 exe 时已核对其 worktree 路径和 PID+系统创建时间后关闭，再成功构建。


## 合并前同步：与已验收阅读器组合（2026-10-28）

负责人已验收侧栏并授权合并。同步 main `dd2b1f2`（已合并 content-reader PR #27）时，唯一冲突是 `src/ui/mod.rs` 的模块声明：同时保留 `pub mod reader` 与 `pub(crate) mod scroll`，其余 reader 代码直接采用 main；没有调整侧栏状态机、阈值、动画或布局。共享滚动文档同步为已实现的消费者状态，区分 reader 已复用的 Bar/Bars 与尚待接入的公共 wheel Motion。

组合验证：fmt、locked all-target check、clippy `-D warnings`、cargo test（517 bin + 123 reader，均通过）、cargo build 与 diff check 通过，所有 Cargo 编译均使用本 worktree target、`-j 1`。第一次测试在构建产品 binary 时因旧预览占用 `githux.exe` 被系统拒绝，尚未执行用例；核对预览路径及 PID40368/系统创建时间后只关闭该旧窗口，再执行上述通过的测试。负责人验收针对此前的 `f1ccf31` 侧栏行为，组合提交仍须重新通过 CI 和审查后由主 Agent squash 合并。


### 同步后审查：动画中重新抓取 sash 的交接帧

Codex 在 `c95397f` 指出：collapse 中重新开始 resize 会取消 transition 并把模式改回 expanded，紧接着 `resize_pointer_at` 的 `sample` 将当前呈现覆盖为旧展开目标。新增回归在旧实现上复现：80ms 时约78.336px，第一次小幅移动跳到232px。

修复 `sample` 的无 transition 分支：非 reduced-motion 的手动拖动期间保留当前帧，由指针输入更新；不再在重绘或第一次 move 的前置采样中恢复旧目标。普通非拖动采样、reduced-motion、窗口 settle、已有模式切换曲线继续保持原语义。两条新增回归覆盖折叠/展开动画被抓取后到首个 move 前的帧冻结，以及折叠中抓取、小幅移动、后续中间帧和释放后的正确终点。

旧实现复现失败后，修复版519 bin + 123 reader tests全部通过；fmt、全目标check/clippy、普通build和diff check通过，`-j 1`。四个旧线程逐项核实收尾：阈值重叠、持续move截断动画和隐藏More菜单已有修复；旧“拖动展开恢复记忆宽度”意见按负责人后续“展开后sash对齐鼠标绝对位置”的明确要求裁决为被取代，保持true/coordinator。
