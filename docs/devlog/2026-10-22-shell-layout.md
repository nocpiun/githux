# 外壳布局：自定义标题栏、island 与固定侧边栏

- 日期：2026-10-22；任务 ID：shell-layout；执行者：基层 Agent。
- 状态：实现与本地检查完成，真实 Windows Debug 窗口的布局、拖动、搜索入口与窗口状态已在本机核验；[PR #8](https://github.com/nocpiun/githux/pull/8) 已创建并收到首轮审查，修复后待最新 head 的 CI 与审查；IME/多 DPI 等剩余人工验收仍待完成。
- 交付范围：[路线图](../roadmap.md) R1 的第一份可运行交付（外壳、固定导航、有界仓库列表、搜索入口、Home 双栏 fixture）。R1 整体未完成：多页面会话与标签仍未实现。

## 改动与决定

**外壳结构。** 窗口为自定义标题栏 + 固定侧边栏 + 一个 island。标题栏左侧 Githux，随后是可拖拽空白，右侧是固定搜索入口和 Windows 最小化/最大化还原/关闭。品牌只在标题栏出现，侧边栏顶部直接是 Home、Settings。

拖动、双击最大化与边缘缩放没有引入平台 API：标题栏空白用 GPUI 的 `WindowControlArea::Drag` 与 `start_window_move`，按钮用 `WindowControlArea::{Min,Max,Close}`，都由平台上已有的非客户区命中测试驱动。品牌、搜索入口和按钮各自 `occlude()`，因此按在它们上面不会落到拖动区。

**侧边栏有界滚动。** Home、Settings、仓库组标题、账户行与四个快捷入口都是 `flex_none` 子项，只有仓库组是 `flex_1 + min_h_0 + overflow_y_scroll`。窗口变矮时只有仓库组让出高度。仓库行显示 namespace 头像 + `namespace/repo`，长名字在列内截断并带 tooltip，列宽固定，不会被撑开。

**底部入口。** 头像/名称进入 Profile；下方四个等宽图标为 Notifications、Issues、PRs、New。四者都有可访问名称。New 的清单尚未批准，因此仅保留禁用的图标入口：不获得焦点、不执行任何动作，避免出现假创建。侧边栏没有主题开关。

**island 与会话条。** island 与窗口边缘、侧边栏之间留小间距，圆角取 `ISLAND_RADIUS`，内缩量见 `src/ui/geometry.rs`，这些数值只是实现起点，不是视觉签字。island 顶部有独立组合边界 `session-strip`，目前只显示当前页标题并预留右侧空间；没有加号或关闭按钮，因为打开/复用/关闭会话属于下一份 R1 任务，也不把仓库内容 Tabs 当作会话 Tabs。

**路由。** `src/workspace/mod.rs` 的 `Route` 是唯一地址模型，`Workspace::new` 固定启动 Home（不恢复上次页面，首版约定）。本轮一个窗口只有一个活动页，`TabId` 与页面会话集合留给下一任务，但 `Route` 就是它将来指向的稳定边界。

**搜索与 command palette。** 标题栏搜索入口打开一个 Ely 风格浮层，结果只有本地导航命令：Home、Settings、Profile、Notifications、Issues、Pull requests 与当前 fixture 仓库。不调用 GitHub，也不决定未来全局搜索范围。Esc、上下移动、Enter 打开、点击行、点击遮罩关闭；关闭后焦点回到搜索入口。

没有直接用 Ely 的 `CommandPalette`，原因只有一个：它在任何 Enter 上运行高亮行，包括输入法候选确认的 Enter。这里用 `intercept_keystrokes` 在按键绑定之前接管 Up/Down/Enter/Escape，并在 Enter 时先查询查询框的 marked（composing）区间；组合中存在候选时放行该 Enter，不执行导航。测试通过 `EntityInputHandler::replace_and_mark_text_in_range` 构造组合状态验证这条分支，但**这不等于验证了 Windows 原生 IME 消息顺序**，见“后续”。

**页面内容。** 只有 Home 画真实内容：左侧最近参与（类型、标题、仓库/编号、更新时间），右侧关注动态。Repository 与 Profile 只显示最小资源标识（全名/描述、头像/名称/ID）以核验路由。Settings、Notifications、Issues、Pull requests 内容区按产品约定留空，不加“未实现”卡片；没有仓库阅读器、设置选项、Issue 详情。

**测试数据。** `src/ui/fixtures.rs` 提供 124 条稳定仓库（含中文名、超长名、长 namespace）与两组 Home fixture。没有网络、凭据或缓存路径读取它；生成部分用索引派生，保证跨进程稳定且唯一。

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

`cargo test --locked` 共 24 项通过，其中 21 项在 GPUI 测试平台渲染真实 `Shell` 视图：

- 启动 Home、fixture 至少 100 条仓库、侧栏与 island 关键节点已绘制；
- 1400×900 → 900×600 缩放后 Home/Settings 位置与高度不变、只有仓库组变矮、底部入口贴住侧栏底部、island 不越界、侧栏宽度不变、Home 两列等宽且不越出 island；
- 滚轮在侧栏只滚仓库组，island 页面偏移不变；
- 点击 Home/Settings/四个底部入口/仓库行/账户行得到正确路由，New 禁用不改变路由；
- 搜索入口打开浮层、过滤、Enter 运行、Esc 关闭不导航、关闭后仍有焦点目标；
- 上下移动循环、Enter 运行的是高亮行而不是首行；
- 组合态（marked range）期间 Enter 不运行任何行，Up/Down 不移动高亮、Esc 不关闭浮层（这三者属于输入法候选）；
- Ctrl+K 在任意页面打开浮层；Tab 能到侧栏行并用 Enter 激活；
- 中文与超长仓库名保持在侧栏列内；
- 空白页面只保留 island 与会话条，不画占位卡片。

另有 5 项 fixture / 排序 / 列表布局单元测试：fixture 稳定且覆盖中文与超长名、分组连续不分叉、空查询覆盖全部命令、评分大小写与间隙规则、以及滚动子项索引包含分组标题。

**真实 Windows 窗口（本机 3840×2160，Windows 缩放 150%）已核验（合并 main 前后的同一代码路径均确认搜索入口与布局一致）：**

- 启动即 Home，无多余控制台；标题栏显示 Githux、搜索入口与三个窗口按钮；
- 拖动标题栏空白移动窗口（40,40 → 160,122 物理像素）；按在搜索入口上拖 120px 窗口不动；
- 双击标题栏空白最大化；最大化按钮切换最大化；Win+← 贴靠左半屏后布局正常（侧栏固定、仓库组滚动、island 在位）；
- Ctrl+K 打开浮层并显示分组结果；Esc 关闭；
- 关闭应用后本 worktree 的 `githux.exe` 进程退出，未残留。

**未覆盖 / 待人工验收（不据此宣布通过）：**

- **中文 IME 真实候选确认**：只在测试平台构造 marked range 验证了逻辑分支。Windows 上微软拼音候选框的 Enter 是否先被 IME 消费，必须在真实窗口用真实输入法确认；这是本轮明确留下的验收项。
- **多 DPI 跨屏**：仅在本机 150% 缩放下观察，未在 100/125/175/200% 与跨屏移动时逐项核对。
- **按钮逐个真实验收**：最大化与贴靠已实测，最小化与关闭按钮只通过代码路径与进程退出间接确认，未逐个点击核对恢复后的状态。
- 截图不能证明帧率、内存或输入行为；macOS/Linux 未做任何原生验收，仍使用系统标题栏。

## 首轮审查修复

首轮 Codex 审查（review `5610488856`，针对 `e236997`）提出两条 P2，均可直接修复：

1. **组合态只保护了 Enter。** 原实现只在 Enter 时检查 marked 区间，Up/Down 与 Esc 仍会移动高亮或关闭浮层，从而抢走输入法的候选导航与取消。现在拦截器在这些按键前统一判断组合态：存在 marked 区间时全部放行。测试新增 `a_composing_input_method_keeps_up_down_and_escape_for_its_candidate_list`。
2. **滚动索引与列表子项错位。** `scroll_to_item` 数的是滚动容器的子项，而分组标题也占一个子项；原来直接传结果序号，靠近底部时高亮会滚出视野。现在 `palette::layout` 同时给出标题与行的子项序列，`child_index` 把结果序号换算成子项序号，绘制与滚动共用同一份布局。测试新增 `the_scroll_child_index_counts_the_headings_in_between`，并在真实窗口用 Ctrl+K 加 40 次 Down 确认高亮仍在可视区内。

两条都属于既有需求内的正确性修复，不涉及产品取舍，因此本 PR 全程无待裁决事项。

## 与 main 的 CI 变更组合

推送修复后，api-capabilities 的 PR #6、#7 和 #9 先合入 main，其中 #7 将 `code-check` 从 `windows-2022` 移到 Ubuntu 24.04。本分支此前的 `search-entry` 选择器只在 `Platform::Windows` 下绘制，导致 #7 之后合入的基线上 10 项测试在 Linux 上失败；这不是 Linux 原生验收问题，而是外壳把“搜索入口必须在标题栏”这个已确认行为绑在了错误的平台分支上。

已按任务要求与 main 组合核验：合并 `origin/main`（合并基线提交见 PR 历史），同时把标题栏改为所有平台绘制同一条品牌 + 搜索入口，只有窗口按钮保留 Windows 专属的 `cfg!` 分支（其他平台继续用系统标题栏）。新增两项用例固定这条边界：`the_title_bar_always_offers_the_search_entry_and_brand`（品牌与搜索入口始终绘制，Windows 额外验证三个 caption 按钮）与 `the_shell_body_sits_below_the_title_bar_on_every_platform`（外壳主体不受平台分支影响）。合并后的本地检查与 26 项测试全部通过，并在真实窗口确认搜索入口与布局与合并前一致。

## 后续

1. 负责人/主 Agent 在真实窗口用微软拼音验证：输入候选期间按 Enter 只确认候选、不导航；随后确认 Esc 关闭浮层并把焦点还给搜索入口。
2. 多 DPI（100/125/175/200%）与跨屏移动时的 island 间距、圆角与标题栏高度复核；几何参数集中在 `src/ui/geometry.rs`。
3. 一次会话中逐个点击最小化/最大化还原/关闭，确认窗口状态与贴靠边界。
4. 下一份 R1 任务实现页面会话与标签：`Route` 已可承载会话身份，island 顶部的 `session-strip` 已预留组合边界；打开/复用/关闭与状态隔离、以及 Home/Settings 是否单例仍需先确定规则。
5. 仓库组目前是全量 fixture 的有界列表；分页/虚拟化属于接口接入时的实现手段，本轮不新增排序或分组产品规则。
