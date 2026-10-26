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
